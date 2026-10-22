//! Facts need evidence (BUILD-PLAN E1, Q38).
//!
//! A worker's `learned` lines and the thread's `remember` claims carry their evidence: lines of
//! a file or a check's output, each with the words they show. Whenever a claim is used (a later
//! worker's brief, a `query_brain` answer, a rebirth briefing) its source is re-read: words
//! still there make the *source* verified, and the line says so beside the claim, which stays
//! unverified. A false claim with a real but unrelated quote is never briefed as a verified
//! fact; a quote its source doesn't hold says so; a line with no evidence stays a bare claim;
//! and a claim with evidence that contradicts another supersedes it.

use std::sync::{Arc, Mutex};

use brigadier_brain::{NodeFilter, source_label};
use brigadier_providers::ProviderKind;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::context_pack::BRAIN_HEADING;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// What the file says, and the check's words.
const FILE_TEXT: &str = "# retry settings\nMAX_RETRIES = 3\n";
const FILE_WORDS: &str = "MAX_RETRIES = 3";
const CHECK_WORDS: &str = "retry_stops ok";

/// True, with the file's lines.
const BY_LINES: &str = "Retries stop after three tries";
/// True, with the check's output.
const BY_CHECK: &str = "The retry check passes";
/// False, with a real quote that doesn't bear it out.
const UNRELATED: &str = "Retries never stop";
/// False, with words the file doesn't hold.
const MISQUOTED: &str = "Retries stop after five tries";
/// False, with no evidence at all.
const BARE: &str = "Retries are kept in the database";
/// What a claim with evidence says instead of [`BARE`].
const CONTRADICTION: &str = "Retries are set in retry.txt, not in the database";

#[derive(Default)]
struct Seen {
    /// The stored output of the worker's check, its evidence reference.
    output: Option<String>,
    /// The remember replies.
    kept: Vec<String>,
    /// The thread's query_brain answer.
    answer: Option<String>,
    /// The scout's brief.
    brief: Option<String>,
    /// Every input the thread got.
    thread: Vec<String>,
    /// The bare claim's node, which a contradicting claim replaces.
    bare: Option<String>,
}

fn learned(output: &str) -> Vec<String> {
    vec![
        format!("{BY_LINES} [evidence: retry.txt:2 `{FILE_WORDS}`]"),
        format!("{BY_CHECK} [evidence: {output} `{CHECK_WORDS}`]"),
        format!("{UNRELATED} [evidence: retry.txt:2 `{FILE_WORDS}`]"),
        format!("{MISQUOTED} [evidence: retry.txt:2 `MAX_RETRIES = 5`]"),
        BARE.to_owned(),
    ]
}

#[tokio::test]
async fn claims_carry_their_evidence() {
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        "claim-evidence",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    log.lock().unwrap().thread.push(turn.input.clone());
                    if turn.input.contains("Add retries.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Add retries",
                                       "kind": "implement", "spec": "Write retry.txt.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("");
                    }
                    if turn.input.contains("[report task-1") {
                        let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Landed.");
                    }
                    if turn.input.contains("Keep the claims.") {
                        for args in [
                            json!({"kind": "convention", "title": "Retry limit by its lines",
                                   "files": ["retry.txt:2"], "quote": FILE_WORDS}),
                            // A claim that contradicts the bare one, with its evidence.
                            json!({"kind": "convention", "title": CONTRADICTION,
                                   "files": ["retry.txt:2"], "quote": FILE_WORDS,
                                   "replaces": log.lock().unwrap().bare.clone().unwrap()}),
                        ] {
                            let reply = turn.call("remember", args).await;
                            assert!(!reply.is_error, "{}", reply.text);
                            log.lock().unwrap().kept.push(reply.text);
                        }
                        return Reply::text("Kept.");
                    }
                    if turn.input.contains("How do retries work?") {
                        // Both pages: five claims are more than a page's facts.
                        let mut answer = String::new();
                        for page in [1, 2] {
                            let reply = turn
                                .call(
                                    "query_brain",
                                    json!({"query": "retries retry stop", "page": page}),
                                )
                                .await;
                            assert!(!reply.is_error, "{}", reply.text);
                            answer.push_str(&reply.text);
                            answer.push('\n');
                        }
                        log.lock().unwrap().answer = Some(answer);
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Look at retries",
                                       "kind": "scout", "spec": "Say how retries stop."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("");
                    }
                    return Reply::text("Noted.");
                }
                match turn.task_number() {
                    Some(1) => {
                        turn.write("retry.txt", FILE_TEXT);
                        turn.git(&["add", "retry.txt"]);
                        turn.git(&["commit", "-qm", "Add retries"]);
                        let output = log.lock().unwrap().output.clone().unwrap();
                        let reply = turn
                            .call(
                                "submit_report",
                                json!({"summary": "Wrote retry.txt.", "changes": ["retry.txt"],
                                       "verification": [format!("echo: passed [evidence: {output}]")],
                                       "learned": learned(&output)}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        Reply::text("Reported.")
                    }
                    _ => {
                        log.lock().unwrap().brief.get_or_insert(turn.input.clone());
                        let reply = turn
                            .call("submit_report", json!({"summary": "They stop at three."}))
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        Reply::text("Reported.")
                    }
                }
            }
        }),
    )
    .await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap();
    let brain = flow
        .manager
        .project_brain(&project)
        .await
        .unwrap()
        .brain
        .clone();
    // A check's output the session kept, as a long command's is.
    let (stored, _) = flow
        .manager
        .store_output(
            &flow.conversation,
            crate::work::OutputSource::Bash,
            "exit 0",
            format!("{CHECK_WORDS}\n").into_bytes(),
            false,
        )
        .await
        .unwrap();
    seen.lock().unwrap().output = Some(stored.alias);

    flow.say("Add retries.").await;
    flow.until("task-1 to land", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && task.landed.is_some())
    })
    .await;
    super::eventually("the worker's claims to be kept", || {
        brain
            .nodes(&NodeFilter::default())
            .unwrap()
            .iter()
            .any(|node| node.title == BARE)
    })
    .await;
    flow.settled().await;
    // Every claim is stored unverified, with the words of its evidence.
    let nodes = brain.nodes(&NodeFilter::default()).unwrap();
    for title in [BY_LINES, BY_CHECK, UNRELATED, MISQUOTED, BARE] {
        let node = nodes
            .iter()
            .find(|node| node.title == title)
            .unwrap_or_else(|| panic!("{title} kept"));
        assert_eq!(
            source_label(node.kind, &node.provenance),
            "worker claim, unverified",
            "{title}"
        );
        let sources = &node.provenance.claim.as_ref().unwrap().sources;
        assert_eq!(sources.len(), usize::from(title != BARE), "{title}");
    }

    let output = seen.lock().unwrap().output.clone().unwrap();
    flow.say("How do retries work?").await;
    flow.until("the scout to end", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 2 && task.state.is_final())
    })
    .await;
    flow.settled().await;

    let verified = |text: &str, line: &str, cite: &str| {
        assert!(
            text.contains(&format!(
                "worker claim, unverified · verified source: {cite} `"
            )) && text.contains(line),
            "{line}: {text}"
        );
    };
    // The brief of a later worker.
    let brief = seen
        .lock()
        .unwrap()
        .brief
        .clone()
        .expect("the scout's brief");
    let section = &brief[brief.find(BRAIN_HEADING).expect("a Brain section")..];
    // The query_brain answer.
    let answer = seen.lock().unwrap().answer.clone().unwrap();
    for text in [section, answer.as_str()] {
        verified(text, BY_LINES, "retry.txt:2");
        verified(text, BY_CHECK, &output);
        // A real quote that doesn't bear the claim out: its source is verified, never the claim.
        verified(text, UNRELATED, "retry.txt:2");
        assert!(!text.contains("worker claim, verified"), "{text}");
        assert!(
            text.contains("worker claim, unverified · its source no longer shows it: retry.txt:2"),
            "{text}"
        );
        assert!(text.contains(BARE), "{text}");
    }
    for line in section.lines().filter(|line| line.contains(UNRELATED)) {
        assert!(
            line.contains("[worker claim, unverified · verified source"),
            "{line}"
        );
    }
    for line in section.lines().filter(|line| line.contains(BARE)) {
        assert!(line.contains("[worker claim, unverified]"), "{line}");
    }

    // The thread's own claims with their lines.
    seen.lock().unwrap().bare = brain
        .nodes(&NodeFilter::default())
        .unwrap()
        .into_iter()
        .find(|node| node.title == BARE)
        .map(|node| node.id);
    flow.say("Keep the claims.").await;
    flow.settled().await;
    let kept = seen.lock().unwrap().kept.clone();
    assert!(
        kept[0].contains("its source retry.txt:2 shows your words"),
        "{}",
        kept[0]
    );
    // A contradicting claim with evidence supersedes the bare one.
    assert!(
        kept[1].contains("its source retry.txt:2 shows your words"),
        "{}",
        kept[1]
    );
    let current: Vec<String> = brain
        .nodes(&NodeFilter {
            current_only: true,
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .map(|node| node.title)
        .collect();
    assert!(
        current.iter().any(|title| title == CONTRADICTION),
        "{current:?}"
    );
    assert!(!current.iter().any(|title| title == BARE), "{current:?}");

    // A forced rebirth briefing.
    let native = flow
        .manager
        .read_native_id(&flow.conversation, ProviderKind::Claude)
        .await
        .flatten()
        .expect("a logged session start");
    let now = crate::now_ms();
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    conv.set_cache_mark(crate::manager::cold::CacheMark {
        native_id: native.clone(),
        provider: ProviderKind::Claude,
        at_ms: now - 2 * 3_600_000,
        context: 200_000,
        window: Some(1_000_000),
    })
    .await;
    conv.set_checkpoint(crate::manager::rebirth::RebirthPrep::written(
        "Goal and open threads: retries.\nWhat to do next: wait.",
        &native,
        now - 2 * 3_600_000 + 60_000,
    ))
    .await;
    conv.close_cli().await;
    let before = seen.lock().unwrap().thread.len();
    flow.say("Next.").await;
    flow.until("the reborn turn", |_| {
        seen.lock().unwrap().thread.len() > before
    })
    .await;
    flow.settled().await;
    let briefing = seen.lock().unwrap().thread[before].clone();
    assert!(briefing.contains("[Brigadier briefing"), "{briefing}");
    for (line, words) in [
        (BY_LINES, "unverified · verified source: retry.txt:2"),
        (UNRELATED, "unverified · verified source: retry.txt:2"),
        (MISQUOTED, "unverified · its source no longer shows it"),
    ] {
        assert!(
            briefing
                .lines()
                .any(|text| text.contains(line) && text.contains(words)),
            "{line}: {briefing}"
        );
    }
    assert!(!briefing.contains("claim, verified"), "{briefing}");
    // A superseded claim is not briefed: the decision index (D1.2) names it only as replaced,
    // with its label.
    for line in briefing.lines().filter(|line| line.contains(BARE)) {
        assert!(
            line.contains("· worker claim, unverified, replaced by "),
            "a superseded claim is not briefed: {briefing}"
        );
    }

    // The source is re-read at each use: once the file changes, it no longer holds.
    let workspace = flow
        .manager
        .recorded_workspace(&flow.conversation)
        .expect("the session's workspace")
        .path;
    std::fs::write(
        workspace.join("retry.txt"),
        "# retry settings\nMAX_RETRIES = 4\n",
    )
    .unwrap();
    let reply = flow
        .manager
        .query_brain_tool(
            &flow.conversation,
            None,
            "retries retry stop".into(),
            false,
            None,
        )
        .await
        .unwrap();
    assert!(
        !reply.contains("verified source: retry.txt:2"),
        "the edited file no longer shows the words: {reply}"
    );
    flow.stop().await;
}

/// A worker's report after a restart goes on from its report's version before: the version is
/// kept on the task, and the task's live state after a restart hands out the next one, so two
/// reports of the same task never carry the same version (BUILD-PLAN A11).
#[tokio::test]
async fn a_report_after_a_restart_goes_on_from_the_version_before() {
    const FIRST: &str = "Retries stop after three tries";
    let mut flow = Flow::start(
        "report-version",
        Options::default(),
        script(|turn| async move {
            if turn.is_orchestrator() {
                if turn.input.contains("Look at retries.") {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Look at retries",
                                   "kind": "scout", "spec": "Say how retries stop."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("");
                }
                return Reply::text("Noted.");
            }
            let reply = turn
                .call(
                    "submit_report",
                    json!({"summary": "Looked.", "learned": [FIRST]}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("Reported.")
        }),
    )
    .await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap();
    flow.say("Look at retries.").await;
    flow.settled().await;
    let mut version = None;
    for _ in 0..100 {
        let brain = flow
            .manager
            .project_brain(&project)
            .await
            .unwrap()
            .brain
            .clone();
        version = brain
            .nodes(&NodeFilter::default())
            .unwrap()
            .into_iter()
            .find(|node| node.title == FIRST)
            .and_then(|node| node.provenance.claim)
            .and_then(|claim| claim.report_version);
        if version.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(version, Some(1));
    flow.restart().await;
    let tasks = flow.core.tasks(&flow.conversation).await.unwrap();
    let task = tasks
        .iter()
        .find(|task| task.title == "Look at retries")
        .unwrap();
    assert_eq!(task.report_version, Some(1));
    assert_eq!(flow.manager.task_live(task).learning.next(), 2);
    flow.stop().await;
}
