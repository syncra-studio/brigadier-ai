//! Standing constraints, word for word (BUILD-PLAN D1.1): the user's rule and its exception,
//! kept with `remember` as constraints citing their messages, survive two size rebirths and
//! reach a worker brief verbatim, with the briefing's constraints section measured; a
//! constraint is superseded only by one that names it, a request's own ends with the request,
//! and one the user's message doesn't hold is refused. Rule R: a session told the instructions
//! before this change hears the note once.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::knowledge::{RebirthRecord, RebirthTrigger};
use crate::manager::constraints::{BRIEF_HEADING, REBIRTH_HEADING};
use crate::manager::prompts::{CONSTRAINTS_REVISION, revision_note};
use crate::model::DomainEvent;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const RULE: &str = "A standing rule for this session: never edit src/money.ts by hand.";
const RULE_WORDS: &str = "never edit src/money.ts by hand";
const EXCEPTION: &str =
    "A correction to that rule: you may add new tests for src/money.ts in test/money.test.ts.";
const EXCEPTION_WORDS: &str = "you may add new tests for src/money.ts in test/money.test.ts";
/// Past the 300k prepare and 375k swap lines of a 1M window.
const FULL: i64 = 460_000;
const NOTE: &str = "Goal and open threads: the fillers.\nWhat to do next: wait.";

#[derive(Default, Clone)]
struct Seen {
    /// Every input of the thread, in order.
    thread: Vec<String>,
    /// The worker's first input: its brief.
    brief: Option<String>,
    /// What remember answered.
    remembered: Vec<(bool, String)>,
}

fn rebirths(entries: Vec<crate::model::OrchestratorLogEntry>) -> Vec<RebirthRecord> {
    entries
        .into_iter()
        .filter_map(|entry| match entry.entry {
            crate::work::OrchestratorEntry::Rebirth { record } => Some(*record),
            _ => None,
        })
        .collect()
}

/// The thread of a session whose user sets a rule and its exception, then talks on: it keeps
/// both as constraints, grows past its size twice, and later starts a scout.
fn thread_script(seen: Arc<Mutex<Seen>>) -> Script {
    script(move |turn| {
        let seen = seen.clone();
        async move {
            if !turn.is_orchestrator() {
                seen.lock().unwrap().brief.get_or_insert(turn.input.clone());
                let reply = turn
                    .call("submit_report", json!({"summary": "Looked."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Reported.");
            }
            // The note fork of a rebirth.
            if turn.input.contains("Write the handoff note") {
                return Reply::text(NOTE);
            }
            seen.lock().unwrap().thread.push(turn.input.clone());
            let remember = async |words: &str| {
                let reply = turn
                    .call(
                        "remember",
                        json!({"kind": "constraint", "title": "money.ts rule",
                               "user_message": "latest", "quote": words, "scope": "session"}),
                    )
                    .await;
                seen.lock()
                    .unwrap()
                    .remembered
                    .push((reply.is_error, reply.text.clone()));
            };
            if turn.input.contains(RULE) {
                remember(RULE_WORDS).await;
                return Reply::text("Kept.");
            }
            if turn.input.contains(EXCEPTION) {
                remember(EXCEPTION_WORDS).await;
                return Reply::text("Kept.");
            }
            // A briefing quotes earlier messages: only the user's own ask grows the context.
            if turn.input.contains("Grow.") && !turn.input.contains("[Brigadier briefing") {
                return Reply {
                    text: "Grown.".into(),
                    context_tokens: Some(FULL),
                    ..Reply::default()
                };
            }
            if turn.input.contains("Look around.") {
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "medium", "title": "Look around", "kind": "scout",
                               "spec": "List the files."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            Reply::text("Noted.")
        }
    })
}

/// The rule and its exception reach two size rebirths' briefings and a worker's brief in the
/// user's own words, after both left the verbatim tail; each briefing reports the constraints
/// section's size.
#[tokio::test]
async fn a_rule_and_its_exception_survive_two_size_rebirths_and_reach_a_worker_verbatim() {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let flow = Flow::start(
        "constraints-rebirths",
        Options::default(),
        thread_script(seen.clone()),
    )
    .await;
    flow.say(RULE).await;
    flow.settled().await;
    flow.say(EXCEPTION).await;
    flow.settled().await;
    // Far out of the verbatim tail: exchanges this big fill the briefing's budget with the
    // 6 it keeps at least.
    let filler = |label: String| format!("{label} {}", "words of a long paste ".repeat(700));
    for n in 0..7 {
        flow.say(&filler(format!("Filler {n}."))).await;
        flow.settled().await;
    }
    for round in 1..=2 {
        flow.say("Grow.").await;
        flow.settled().await;
        flow.say(&format!("After rebirth {round}.")).await;
        flow.settled().await;
        // Out of the tail again before the next one.
        for n in 0..7 {
            flow.say(&filler(format!("More {round}.{n}."))).await;
            flow.settled().await;
        }
    }
    flow.say("Look around.").await;
    flow.until("the scout's brief", |_| {
        seen.lock().unwrap().brief.is_some()
    })
    .await;
    flow.settled().await;

    let records = rebirths(
        flow.core
            .list_orchestrator_log(&flow.conversation, None, brigadier_store::MAX_PAGE)
            .await
            .unwrap()
            .entries,
    );
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.remembered.len(), 2, "{:?}", seen.remembered);
    for (error, text) in &seen.remembered {
        assert!(!error, "{text}");
        assert!(text.starts_with("Kept constraint "), "{text}");
    }
    assert_eq!(records.len(), 2, "two size rebirths");
    for record in &records {
        assert_eq!(record.trigger, RebirthTrigger::Threshold);
        let section = record
            .sections
            .iter()
            .find(|section| section.name == "constraints")
            .expect("a constraints section");
        assert_eq!(section.items, 2);
        assert!(section.tokens > 0, "its size is reported");
    }
    // Each reborn session's first input is its briefing.
    let briefings: Vec<&String> = seen
        .thread
        .iter()
        .filter(|input| input.contains("[Brigadier briefing"))
        .collect();
    assert_eq!(briefings.len(), 2, "{:#?}", seen.thread);
    for briefing in briefings {
        let section = briefing
            .split(REBIRTH_HEADING)
            .nth(1)
            .expect("the constraints section");
        assert!(section.contains(&format!("\"{RULE_WORDS}\"")), "{briefing}");
        assert!(
            section.contains(&format!("\"{EXCEPTION_WORDS}\"")),
            "{briefing}"
        );
        // Not from the verbatim tail: neither message is carried there.
        let recent = briefing
            .split("[The conversation's latest messages, verbatim]")
            .nth(1)
            .unwrap_or_default();
        assert!(
            !recent.contains(RULE) && !recent.contains(EXCEPTION),
            "{briefing}"
        );
        assert!(recent.contains("aren't carried here; search_transcript finds them"));
    }
    let brief = seen.brief.as_deref().unwrap();
    let section = brief.split(BRIEF_HEADING).nth(1).expect("{brief}");
    assert!(section.contains(&format!("\"{RULE_WORDS}\"")), "{brief}");
    assert!(
        section.contains(&format!("\"{EXCEPTION_WORDS}\"")),
        "{brief}"
    );
    flow.stop().await;
}

/// A constraint that names another replaces it, and only then: the old one leaves the brief.
/// A request's own constraint holds while its request is open. A quote the message doesn't
/// hold, a constraint without the user's message, and a replacement of a non-constraint are
/// refused, with nothing kept.
#[tokio::test]
async fn constraints_are_replaced_only_by_name_and_a_requests_own_ends_with_it() {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let ids: Arc<Mutex<Vec<String>>> = Arc::default();
    let (log, kept) = (seen.clone(), ids.clone());
    let flow = Flow::start(
        "constraints-replace",
        Options::default(),
        script(move |turn| {
            let (log, kept) = (log.clone(), ids_of(&kept));
            async move {
                if !turn.is_orchestrator() {
                    log.lock().unwrap().brief = Some(turn.input.clone());
                    let reply = turn
                        .call("submit_report", json!({"summary": "Looked."}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                let call = async |args: serde_json::Value| {
                    let reply = turn.call("remember", args).await;
                    log.lock()
                        .unwrap()
                        .remembered
                        .push((reply.is_error, reply.text.clone()));
                    if let Some(id) = reply
                        .text
                        .strip_prefix("Kept constraint ")
                        .and_then(|rest| rest.split(' ').next())
                    {
                        kept.lock().unwrap().push(id.to_owned());
                    }
                };
                if turn.input.contains("Never use tabs.") {
                    call(json!({"kind": "constraint", "title": "No tabs",
                                "user_message": "latest", "scope": "project"}))
                    .await;
                    call(json!({"kind": "constraint", "title": "Wrong words",
                                "user_message": "latest", "quote": "always use tabs"}))
                    .await;
                    call(json!({"kind": "constraint", "title": "No message"})).await;
                    let decision = turn
                        .call(
                            "remember",
                            json!({"kind": "decision", "title": "Ship Friday",
                                   "user_message": "latest", "quote": "Never use tabs"}),
                        )
                        .await;
                    let decision = decision
                        .text
                        .split(' ')
                        .nth(2)
                        .unwrap()
                        .trim_end_matches('.');
                    call(json!({"kind": "constraint", "title": "Replaces a decision",
                                "user_message": "latest", "replaces": decision}))
                    .await;
                    return Reply::text("Kept.");
                }
                if turn.input.contains("Use two spaces, not four.") {
                    let first = kept.lock().unwrap()[0].clone();
                    call(json!({"kind": "constraint", "title": "Two spaces",
                                "user_message": "latest", "scope": "project",
                                "replaces": first}))
                    .await;
                    call(json!({"kind": "constraint", "title": "This request only",
                                "user_message": "latest", "quote": "not four",
                                "scope": "request"}))
                    .await;
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Look", "kind": "scout",
                                   "spec": "Look."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    flow.say("Never use tabs.").await;
    flow.settled().await;
    {
        let seen = seen.lock().unwrap();
        let replies = &seen.remembered;
        assert_eq!(replies.len(), 4, "{replies:?}");
        assert!(
            !replies[0].0 && replies[0].1.contains("\"Never use tabs.\""),
            "{replies:?}"
        );
        assert!(
            replies[1].0 && replies[1].1.contains("doesn't say"),
            "{replies:?}"
        );
        assert!(
            replies[2].0 && replies[2].1.contains("cite their message"),
            "{replies:?}"
        );
        assert!(
            replies[3].0 && replies[3].1.contains("not a constraint"),
            "{replies:?}"
        );
    }
    assert_eq!(ids.lock().unwrap().len(), 1);
    flow.say("Use two spaces, not four.").await;
    flow.until("the scout's brief", |_| {
        seen.lock().unwrap().brief.is_some()
    })
    .await;
    let brief = seen.lock().unwrap().brief.clone().unwrap();
    let section = brief.split(BRIEF_HEADING).nth(1).expect("{brief}");
    let first = ids.lock().unwrap()[0].clone();
    assert!(
        !section.contains("\"Never use tabs.\""),
        "replaced: {brief}"
    );
    assert!(section.contains(&format!("replaces {first}")), "{brief}");
    assert!(section.contains("\"Use two spaces, not four.\""), "{brief}");
    assert!(section.contains("(for this request;"), "{brief}");
    flow.settled().await;
    // The request is over: its own constraint no longer holds; the project's does.
    let active = flow.manager.active_constraints(&flow.conversation).await;
    assert_eq!(active.len(), 1, "{active:?}");
    assert_eq!(active[0].words, "Use two spaces, not four.");
    assert_eq!(active[0].replaces.as_deref(), Some(first.as_str()));
    flow.stop().await;
}

fn ids_of(ids: &Arc<Mutex<Vec<String>>>) -> Arc<Mutex<Vec<String>>> {
    ids.clone()
}

/// Rule R: a session told the instructions before this change hears the note once, and then
/// keeps the user's rule as a constraint.
#[tokio::test]
async fn an_open_session_hears_of_constraints_once_and_then_keeps_one() {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let mut flow = Flow::start(
        "constraints-rule-r",
        Options::default(),
        thread_script(seen.clone()),
    )
    .await;
    flow.say("Hello.").await;
    flow.settled().await;
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&flow.conversation),
            DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: crate::work::OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            revision: Some(CONSTRAINTS_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.say(RULE).await;
    flow.settled().await;
    flow.say("Thanks.").await;
    flow.settled().await;
    let note = revision_note(CONSTRAINTS_REVISION - 1, false).unwrap();
    assert!(note.contains("kind constraint"), "{note}");
    let active = flow.manager.active_constraints(&flow.conversation).await;
    let seen = seen.lock().unwrap().clone();
    let told: usize = seen
        .thread
        .iter()
        .map(|input| input.matches(&note).count())
        .sum();
    assert_eq!(told, 1, "{:?}", seen.thread);
    assert_eq!(seen.remembered.len(), 1);
    assert!(!seen.remembered[0].0, "{:?}", seen.remembered);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].words, RULE_WORDS);
    flow.stop().await;
}
