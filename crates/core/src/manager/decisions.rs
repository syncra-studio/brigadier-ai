//! "Decided for you": what the session's summary tells the user about
//! the work Brigadier runs on their behalf (built in, always on).
//!
//! - **Decided for you.** Under "Accept edits" and "Full access", each decision Brigadier
//!   takes in the user's place is logged with why: a plan approved, sent back for revision or
//!   turned down; a change landed after its checks, sent back with their findings, held
//!   because it couldn't be verified, or handed to the orchestrator; a permission a worker
//!   asked for declined. The orchestrator notes its own judgement calls (`note_for_user`).

use super::SessionManager;
use crate::board::Board;
use crate::model::{ConversationId, DomainEvent};
use crate::now_ms;
use crate::sessions::one_line;
use crate::work::{Decision, DecisionKind, DecisionSource, DecisionWords, Task};

/// The longest line a decision or a waiting item keeps.
const LINE_CHARS: usize = 300;
/// The longest reason a decision keeps.
const WHY_CHARS: usize = 600;

impl SessionManager {
    /// Records a decision, whatever the permission level (the orchestrator's own notes).
    pub(crate) async fn record_decision(
        &self,
        conversation_id: &ConversationId,
        request_id: Option<String>,
        source: DecisionSource,
        kind: DecisionKind,
        what: String,
        why: String,
    ) {
        let what = self.core.display_quote(conversation_id, &what).await;
        let why = self.core.display_quote(conversation_id, &why).await;
        // What the user reads names a worker as the app shows it, never as "task-N".
        let (what, why) = match self.core.board(conversation_id).await {
            Ok(board) => (named(&what, &board), named(&why, &board)),
            Err(_) => (what, why),
        };
        let decision = Decision {
            id: uuid::Uuid::now_v7().to_string(),
            request_id,
            source,
            kind,
            what: one_line(&what, LINE_CHARS),
            why: one_line(&why, WHY_CHARS),
            at_ms: now_ms(),
            position: 0,
            short: None,
        };
        if let Err(err) = self
            .core
            .record_conversation(
                conversation_id,
                vec![DomainEvent::DecidedForYou { decision }],
            )
            .await
        {
            tracing::warn!(conversation = %conversation_id, error = %err, "could not record a decision");
        }
    }
}

/// The longest worker name in a line the user reads, in characters.
const NAME_CHARS: usize = 40;

/// A worker as the user knows it: its title, as on its row, in quotes and cut short at a word
/// ("“Fix the A/B breakdown note after…”").
pub(crate) fn worker_name(task: &Task) -> String {
    let title = task.title.split_whitespace().collect::<Vec<_>>().join(" ");
    if title.chars().count() <= NAME_CHARS {
        return format!("\u{201c}{title}\u{201d}");
    }
    let cut: String = title.chars().take(NAME_CHARS).collect();
    let cut = match cut.rfind(' ') {
        Some(at) if at > NAME_CHARS / 2 => &cut[..at],
        _ => cut.as_str(),
    };
    format!(
        "\u{201c}{}\u{2026}\u{201d}",
        cut.trim_end_matches([' ', ',', ':', ';', '-', '·'])
    )
}

/// `text` with each "task-N" of the conversation's tasks as the worker's name. A title quoted
/// right after it ("task-3 “Add the flag”") becomes that name; "task-N" inside a branch, path,
/// file name or longer word stays as it is.
pub(crate) fn named(text: &str, board: &Board) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find("task-") {
        let before = rest[..at].chars().next_back();
        let digits: String = rest[at + 5..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect();
        let mut tail = rest[at + 5 + digits.len()..].chars();
        let after = tail.next();
        // A file name's extension ("task-3.diff") makes it a longer word too.
        let extension = after == Some('.') && tail.next().is_some_and(char::is_alphanumeric);
        let word = before.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '/' | '-' | '_')))
            && !digits.is_empty()
            && !extension
            && after.is_none_or(|c| !(c.is_alphanumeric() || matches!(c, '-' | '_' | '/')));
        let task = word
            .then(|| digits.parse::<u32>().ok())
            .flatten()
            .and_then(|number| board.tasks.values().find(|task| task.number == number));
        let Some(task) = task else {
            let end = at + 5;
            out.push_str(&rest[..end]);
            rest = &rest[end..];
            continue;
        };
        out.push_str(&rest[..at]);
        out.push_str(&worker_name(task));
        rest = &rest[at + 5 + digits.len()..];
        // Its title quoted after it is the name already.
        if let Some(quoted) = rest.strip_prefix(" \u{201c}")
            && let Some(close) = quoted.find('\u{201d}')
        {
            rest = &quoted[close + '\u{201d}'.len_utf8()..];
        }
    }
    out.push_str(rest);
    out
}

/// A decision as the app and the morning report show it. A decision recorded in an earlier
/// version's words (which carried the checks' findings) says the same in the current short
/// ones; the findings stay on the task's checks. One recorded now is returned as it is.
pub(crate) fn short_words(what: &str, why: &str) -> DecisionWords {
    let (what, why) = (what.trim(), why.trim());
    let words = |what: String, why: &str| DecisionWords {
        what,
        why: why.to_owned(),
    };
    const ON_ITS_CHECKS: &str = "The findings are on its checks.";
    if let Some(rest) = what.strip_prefix("Sent task-")
        && let Some((number, tail)) = rest.split_once(" back to fix what its checks found")
    {
        let from = match (
            why.contains("From the review"),
            why.contains("From the verification"),
        ) {
            (true, true) => "review and verification",
            (true, false) => "review",
            (false, true) => "verification",
            (false, false) => "its checks",
        };
        return words(
            format!("Sent task-{number} back after {from}{}", tail.trim_end()),
            ON_ITS_CHECKS,
        );
    }
    if let Some(rest) = what.strip_prefix("Did not land task-") {
        if let Some(number) = rest.strip_suffix(": the orchestrator decides what happens next") {
            let reason = if let Some(rounds) = why
                .strip_prefix("The problems were still there after ")
                .and_then(|rest| rest.split_once('.'))
                .map(|(rounds, _)| rounds)
            {
                format!("problems left after {rounds}")
            } else if why.starts_with("Its change is the same one") {
                "the fix changed nothing".into()
            } else {
                "its checks found problems".into()
            };
            return words(
                format!("Didn't land task-{number}: {reason}"),
                "The thread decides what happens next; the findings are on its checks.",
            );
        }
        if let Some(number) = rest.strip_suffix(": its checks could not finish") {
            return words(
                format!("Didn't land task-{number}: its checks couldn't finish"),
                &reason_head(why),
            );
        }
    }
    if let Some(rest) = what.strip_prefix("Held task-")
        && let Some(held) = rest.strip_suffix(": its change could not be verified")
    {
        let number = held.split_once(' ').map_or(held, |(number, _)| number);
        return words(
            format!("Held task-{number}: its change couldn't be verified"),
            &reason_head(why),
        );
    }
    if what.starts_with("Landed ") {
        if let Some(rest) = why.strip_prefix("Its change passed independent checks: ") {
            let reviewed = if rest.starts_with("reviewed by another vendor") {
                "Reviewed by another vendor"
            } else if rest.starts_with("reviewed by another model of the same vendor") {
                "Reviewed by the same vendor (the only one available)"
            } else {
                "Reviewed"
            };
            let rounds = if rest.contains(", after one round of fixes") {
                ", after 1 fix round".to_owned()
            } else {
                rest.split_once(", after ")
                    .and_then(|(_, after)| after.split_once(" rounds of fixes"))
                    .map(|(rounds, _)| format!(", after {rounds} fix rounds"))
                    .unwrap_or_default()
            };
            return words(
                what.to_owned(),
                &format!("{reviewed} and verified{rounds}."),
            );
        }
        if why.starts_with("Landed on the user's word despite") {
            return words(what.to_owned(), "Landed despite its checks' findings.");
        }
    }
    if let Some(plan) = what
        .strip_prefix("Sent the plan ")
        .and_then(|rest| rest.strip_suffix(" back for revision"))
    {
        return words(
            format!("Sent the plan {plan} back after review"),
            "The findings are on the plan card.",
        );
    }
    if what.starts_with("Did not approve the plan ") {
        let what = what.replacen("Did not approve", "Didn't approve", 1);
        if let Some(rounds) = why
            .strip_prefix("It still had problems after ")
            .and_then(|rest| rest.split_once(';'))
            .map(|(rounds, _)| rounds)
        {
            return words(
                what,
                &format!("Problems left after {rounds}. The thread asks you or makes it smaller."),
            );
        }
        if why.starts_with("Its independent review could not run") {
            return words(what, "Its review couldn't run.");
        }
    }
    if let Some(number) = what
        .strip_prefix("Sent phase ")
        .and_then(|rest| rest.strip_suffix(" back to its lead to fix what its checks found"))
    {
        let round = why
            .strip_prefix("Fix round ")
            .map(|round| format!(" (fix {})", round.trim_end_matches('.')))
            .unwrap_or_default();
        return words(
            format!("Sent phase {number} back to its lead{round}"),
            "The findings are on the phase's checks.",
        );
    }
    if what.starts_with("Verified phase ")
        && let Some(count) = why
            .strip_prefix("A fresh verifier showed each of its ")
            .and_then(|rest| rest.split_once(' '))
            .map(|(count, _)| count)
    {
        return words(
            what.to_owned(),
            &format!(
                "All {count} criteria met. A fresh verifier, another vendor's reviewer and a judge agreed."
            ),
        );
    }
    if what.starts_with("Settled phase ")
        && let Some((met, _)) = why.split_once(" Still missing: ")
    {
        return words(
            what.to_owned(),
            &format!("{met} What's missing is in the phase's checks."),
        );
    }
    if what.starts_with("Approved the overnight plan ")
        && let Some(count) = why
            .strip_prefix("Another vendor reviewed its ")
            .and_then(|rest| rest.split_once(" phases and a fresh judge"))
            .map(|(count, _)| count)
    {
        return words(
            what.to_owned(),
            &format!(
                "Another vendor reviewed its {count} phases; a judge found they follow the goal."
            ),
        );
    }
    words(what.to_owned(), why)
}

/// Brigadier's own words at the head of a reason, without the check's text after them:
/// "Nothing lands unverified. The project's checks could not run." from "… could not run:
/// [not run] CI desktop: …".
fn reason_head(why: &str) -> String {
    let head = why.split_once(": ").map_or(why, |(head, _)| head).trim();
    let head = head.trim_start_matches("- ").trim_end_matches('.');
    if head.is_empty() {
        String::new()
    } else {
        format!("{head}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::work::TaskState;

    fn task(id: &str, state: TaskState) -> Task {
        let mut task: Task = serde_json::from_value(serde_json::json!({
            "id": id,
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "Add the flag",
            "kind": "implement",
            "spec": "Add the flag.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reported",
            "requestId": "r1",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        task.state = state;
        task
    }

    #[test]
    fn a_decision_in_old_words_reads_as_one_recorded_now() {
        for (what, why, short_what, short_why) in [
            (
                "Sent task-5 back to fix what its checks found (fix 1 of 2)",
                "From the review (task-7, Codex gpt-6.1-sol): F1: docs/x.md:132 must say …",
                "Sent task-5 back after review (fix 1 of 2)",
                "The findings are on its checks.",
            ),
            (
                "Did not land task-1: the orchestrator decides what happens next",
                "The problems were still there after 1 fix round. From the review (task-2): P2 …",
                "Didn't land task-1: problems left after 1 fix round",
                "The thread decides what happens next; the findings are on its checks.",
            ),
            (
                "Did not land task-4: its checks could not finish",
                "The project's checks could not run: [not run] CI desktop: …",
                "Didn't land task-4: its checks couldn't finish",
                "The project's checks could not run.",
            ),
            (
                "Held task-13 “Make cause 1's fix obey §10.6”: its change could not be verified",
                "Nothing lands unverified. The project's checks could not run: [not run] CI: …",
                "Held task-13: its change couldn't be verified",
                "Nothing lands unverified. The project's checks could not run.",
            ),
            (
                "Landed task-32 “Landing: read entries” on `overnight/x`",
                "Its change passed independent checks: reviewed by another vendor, and verified against each \"done when\" criterion, after one round of fixes.",
                "Landed task-32 “Landing: read entries” on `overnight/x`",
                "Reviewed by another vendor and verified, after 1 fix round.",
            ),
            (
                "Landed task-9 “X” on `b`",
                "Landed on the user's word despite: F1: …",
                "Landed task-9 “X” on `b`",
                "Landed despite its checks' findings.",
            ),
            (
                "Sent the plan “Phase 2 · Fix the causes” back for revision",
                "Its independent review asked for changes. F1: Replace tools/full-checks.sh …",
                "Sent the plan “Phase 2 · Fix the causes” back after review",
                "The findings are on the plan card.",
            ),
            (
                "Did not approve the plan “Y”",
                "It still had problems after 3 review rounds; the orchestrator asks you or makes it smaller. F1: …",
                "Didn't approve the plan “Y”",
                "Problems left after 3 review rounds. The thread asks you or makes it smaller.",
            ),
            (
                "Sent phase 2 back to its lead to fix what its checks found",
                "Fix round 1 of 2.",
                "Sent phase 2 back to its lead (fix 1 of 2)",
                "The findings are on the phase's checks.",
            ),
            (
                "Verified phase 1 “Measure”",
                "A fresh verifier showed each of its 1 criteria met, a reviewer from another vendor approved the whole diff, and a fresh judge agreed.",
                "Verified phase 1 “Measure”",
                "All 1 criteria met. A fresh verifier, another vendor's reviewer and a judge agreed.",
            ),
            (
                "Settled phase 3 “Ship” as partial",
                "2 of 3 criteria met. Still missing: p3-c2: the smoke …",
                "Settled phase 3 “Ship” as partial",
                "2 of 3 criteria met. What's missing is in the phase's checks.",
            ),
            (
                "Approved the overnight plan “Speed”",
                "Another vendor reviewed its 3 phases and a fresh judge found they follow the goal without invented scope.",
                "Approved the overnight plan “Speed”",
                "Another vendor reviewed its 3 phases; a judge found they follow the goal.",
            ),
            (
                "Kept the old API",
                "The orchestrator's own words stay as they are.",
                "Kept the old API",
                "The orchestrator's own words stay as they are.",
            ),
        ] {
            let short = short_words(what, why);
            assert_eq!(
                (short.what.as_str(), short.why.as_str()),
                (short_what, short_why)
            );
            // The current words read as they are.
            assert_eq!(short_words(&short.what, &short.why), short);
        }
    }

    #[test]
    fn a_worker_is_named_as_the_app_shows_it() {
        let mut board = Board::default();
        let mut short = task("t1", TaskState::Landed);
        short.number = 3;
        let mut long = task("t2", TaskState::Stopped);
        long.number = 37;
        long.title = "Gate members take run slots in a fixed priority order".into();
        board.tasks.insert(short.id.clone(), short);
        board.tasks.insert(long.id.clone(), long);
        assert_eq!(
            named(
                "Landed task-3 \u{201c}Add the flag\u{201d} on `main`",
                &board
            ),
            "Landed \u{201c}Add the flag\u{201d} on `main`"
        );
        assert_eq!(
            named("Landed task-3.", &board),
            "Landed \u{201c}Add the flag\u{201d}."
        );
        assert_eq!(
            named("Sent task-37 back (task-37's fix 1 of 2).", &board),
            "Sent \u{201c}Gate members take run slots in a fixed\u{2026}\u{201d} back (\u{201c}Gate members take run slots in a fixed\u{2026}\u{201d}'s fix 1 of 2)."
        );
        // Branches, paths and unknown numbers stay as they are.
        for kept in [
            "on `brigadier/4158464b/task-3-add-the-flag`",
            "see task-30 and subtask-3 and task-3x",
            "read task-3.diff and docs/task-3.md",
            "task-",
        ] {
            assert_eq!(named(kept, &board), kept);
        }
    }

    #[test]
    fn the_board_keeps_every_decision() {
        let mut board = Board::default();
        board.apply(
            &DomainEvent::DecidedForYou {
                decision: Decision {
                    id: "d1".into(),
                    request_id: None,
                    source: DecisionSource::Orchestrator,
                    kind: DecisionKind::Routine,
                    what: "Kept the old API".into(),
                    why: String::new(),
                    at_ms: 2,
                    position: 0,
                    short: None,
                },
            },
            5,
        );
        assert_eq!(board.decisions.len(), 1);
        assert_eq!(board.decisions[0].position, 5);
    }

    #[test]
    fn stored_events_read_back() {
        let event: DomainEvent = serde_json::from_value(serde_json::json!({
            "type": "decidedForYou",
            "decision": {
                "id": "d1",
                "source": { "type": "task", "taskId": "t1" },
                "what": "Landed task-1",
                "atMs": 1,
            },
        }))
        .expect("a decision");
        let DomainEvent::DecidedForYou { decision } = event else {
            panic!("not a decision");
        };
        assert!(decision.why.is_empty() && decision.request_id.is_none());
        let event: DomainEvent = serde_json::from_value(serde_json::json!({
            "type": "waitingResolved",
            "id": "w1",
            "by": "user",
        }))
        .expect("a resolution of an older version's list");
        assert!(matches!(event, DomainEvent::Retired));
    }
}
