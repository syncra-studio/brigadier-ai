//! Memory safeguards (BUILD-PLAN A11, Q38): what a worker reports under `learned` and what
//! the thread keeps with `remember` on its own are claims, unverified, labelled as such in a
//! later worker's brief, a `query_brain` answer and a rebirth briefing, never as settled
//! decisions; only a cited user message makes a user decision. Rule R: a session from before
//! the change hears it once.

use std::sync::{Arc, Mutex};

use brigadier_brain::{NodeFilter, NodeKind, Origin, source_label};
use brigadier_providers::ProviderKind;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::context_pack::BRAIN_HEADING;
use crate::manager::prompts::{MEMORY_CLAIMS_REVISION, revision_note};
use crate::model::{DomainEvent, MessageRole};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The worker's false `learned` line: the notes are in NOTES.md, not a database.
const FALSE_LINE: &str = "Notes are stored in the notes database.";
/// What the thread keeps with `remember` on its own, with no user message and no evidence.
const THREAD_LINE: &str = "Note files are named in kebab case.";

#[derive(Default)]
struct Seen {
    /// Every input the thread got.
    thread: Vec<String>,
    /// The scout's first input: its brief.
    brief: Option<String>,
    /// The thread's `query_brain` answer.
    answer: Option<String>,
}

/// A false `learned` line from a worker whose check passed and whose review was clean, and a
/// convention the thread kept on its own, reach a later worker's brief, a `query_brain`
/// answer and a forced rebirth briefing as unverified claims with their source; a handoff
/// line that quotes the user is a user decision, one that doesn't is the thread's claim.
#[tokio::test]
async fn claims_reach_briefs_answers_and_rebirths_labelled_unverified() {
    if std::env::var_os("BRIGADIER_COLD_REBIRTH_TOKENS").is_some() {
        return;
    }
    let seen: Arc<Mutex<Seen>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        "memory-claims",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    log.lock().unwrap().thread.push(turn.input.clone());
                    if turn.input.contains("Add notes.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Write the notes",
                                       "kind": "implement", "spec": "Write NOTES.md.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    if turn.input.contains("[report task-1") {
                        // The thread sees what the Brain keeps.
                        assert!(
                            turn.input.contains(&format!("Learned:\n- {FALSE_LINE}")),
                            "{}",
                            turn.input
                        );
                        let reply = turn.call("land_phase", json!({"task": "task-1"})).await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("Landed.");
                    }
                    if turn.input.contains("Name them well.") {
                        let reply = turn
                            .call(
                                "remember",
                                json!({"kind": "convention", "title": THREAD_LINE}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        assert!(reply.text.contains("(your claim, unverified)"), "{}", reply.text);
                        return Reply::text("Kept.");
                    }
                    if turn.input.contains("Where do notes go?") {
                        let reply = turn
                            .call("query_brain", json!({"query": "where are notes stored"}))
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        log.lock().unwrap().answer = Some(reply.text);
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Find where notes are kept",
                                       "kind": "scout", "spec": "Find where the notes are kept and how they are named."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text("[quiet]");
                    }
                    return Reply::text("Noted.");
                }
                match turn.task_number() {
                    Some(1) => {
                        turn.write("NOTES.md", "notes\n");
                        turn.git(&["add", "NOTES.md"]);
                        turn.git(&["commit", "-qm", "Write the notes"]);
                        let check = turn
                            .call("run_check", json!({"command": "echo checked"}))
                            .await;
                        assert!(!check.is_error, "{}", check.text);
                        let review = turn.call("review_code", json!({})).await;
                        assert!(!review.is_error, "{}", review.text);
                        let reply = turn
                            .call(
                                "submit_report",
                                json!({"summary": "Wrote NOTES.md.", "changes": ["NOTES.md"],
                                       "verification": ["echo checked: passed"],
                                       "learned": FALSE_LINE}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        Reply::text("Reported.")
                    }
                    _ => {
                        log.lock().unwrap().brief.get_or_insert(turn.input.clone());
                        let reply = turn
                            .call("submit_report", json!({"summary": "Notes go in NOTES.md."}))
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

    flow.say("Use tabs in the Makefile, always.").await;
    flow.settled().await;
    flow.say("Add notes.").await;
    flow.until("task-1 to land", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && task.landed.is_some())
    })
    .await;
    // The landed worker's line is kept as its claim, never as a fact.
    super::eventually("the worker's claim to be kept", || {
        brain
            .nodes(&NodeFilter {
                kinds: vec![NodeKind::Convention],
                ..Default::default()
            })
            .unwrap()
            .iter()
            .any(|node| node.title == FALSE_LINE)
    })
    .await;
    let claim = brain
        .nodes(&NodeFilter::default())
        .unwrap()
        .into_iter()
        .find(|node| node.title == FALSE_LINE)
        .unwrap();
    assert_eq!(
        source_label(claim.kind, &claim.provenance),
        "worker claim, unverified"
    );
    let task_1 = Flow::task(&flow.board().await, 1).id.clone();
    assert_eq!(claim.provenance.task_id.as_deref(), Some(task_1.0.as_str()));
    flow.settled().await;
    flow.say("Name them well.").await;
    flow.settled().await;

    flow.say("Where do notes go?").await;
    flow.until("the scout to end", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 2 && task.state.is_final())
    })
    .await;
    flow.settled().await;

    // A later worker's brief.
    let brief = seen
        .lock()
        .unwrap()
        .brief
        .clone()
        .expect("the scout's brief");
    let section = &brief[brief.find(BRAIN_HEADING).expect("a Brain section")..];
    assert!(
        section.contains(&format!(
            "- convention [worker claim, unverified]: {}",
            FALSE_LINE.trim_end_matches('.')
        )),
        "{section}"
    );
    assert!(
        section.contains(&format!(
            "- convention [thread claim, unverified]: {}",
            THREAD_LINE.trim_end_matches('.')
        )),
        "{section}"
    );
    // The thread's query_brain answer.
    let answer = seen.lock().unwrap().answer.clone().unwrap();
    assert!(
        answer.contains(&format!("· worker claim, unverified] {FALSE_LINE}")),
        "{answer}"
    );

    // A forced rebirth: the cache expired over a big context, with a checkpoint whose note
    // lists one decision the user's words back and one they don't.
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
        "Goal and open threads: notes.\nDecisions not kept yet:\n- Tabs in the Makefile (\"tabs in the Makefile, always\")\n- Keep notes short.\nWhat to do next: wait.",
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
    let claims_at = briefing
        .find("[Claims kept in this conversation")
        .expect("a claims section");
    let settled_at = briefing
        .find("[Decisions settled in this conversation")
        .expect("a settled section");
    assert!(settled_at < claims_at);
    let settled = &briefing[settled_at..claims_at];
    let claims = &briefing[claims_at..];
    for (line, label) in [
        (FALSE_LINE, "worker claim, unverified"),
        (THREAD_LINE, "thread claim, unverified"),
        ("Keep notes short.", "thread claim, unverified"),
    ] {
        assert!(
            claims.contains(&format!("[{label}]: {line}")),
            "{line}: {briefing}"
        );
        assert!(!settled.contains(line), "settled: {line}: {briefing}");
    }
    assert!(
        settled.contains("Decision [user decision]: Tabs in the Makefile"),
        "{briefing}"
    );
    flow.stop().await;
}

/// Rule R: a session told the instructions before this change hears the note once; then a
/// `remember` that cites the user's real message is a user decision, and one citing a
/// message that isn't the user's, or words it doesn't hold, is refused, as is a preference
/// with no message.
#[tokio::test]
async fn remember_cites_the_users_message_or_is_refused() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let refs: Arc<Mutex<(i64, i64)>> = Arc::default();
    let (log, cited) = (seen.clone(), refs.clone());
    let mut flow = Flow::start(
        "memory-remember",
        Options::default(),
        script(move |turn| {
            let (log, cited) = (log.clone(), cited.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                log.lock().unwrap().push(turn.input.clone());
                if !turn.input.contains("Keep it.") {
                    return Reply::text("Noted.");
                }
                let (user, mine) = *cited.lock().unwrap();
                let calls = [
                    json!({"kind": "decision", "title": "Use tabs in the Makefile",
                           "user_message": format!("#{user}"), "quote": "tabs in the Makefile, always"}),
                    json!({"kind": "decision", "title": "Use spaces in the Makefile",
                           "user_message": format!("#{mine}")}),
                    json!({"kind": "decision", "title": "Use spaces in the Makefile",
                           "user_message": format!("#{user}"), "quote": "spaces in the Makefile"}),
                    json!({"kind": "preference", "title": "Likes long answers", "personal": true}),
                ];
                for args in calls {
                    let reply = turn.call("remember", args).await;
                    log.lock()
                        .unwrap()
                        .push(format!("remember: {} {}", reply.is_error, reply.text));
                }
                Reply::text("Kept.")
            }
        }),
    )
    .await;
    flow.say("Use tabs in the Makefile, always.").await;
    flow.settled().await;
    // It was told the instructions of the revision before.
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
                            revision: Some(MEMORY_CLAIMS_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    let messages = flow
        .core
        .list_messages(flow.conversation.clone(), None, 50)
        .await
        .unwrap()
        .messages;
    let seq_of = |role: MessageRole| {
        messages
            .iter()
            .find(|message| message.role == role)
            .unwrap()
            .seq
    };
    *refs.lock().unwrap() = (seq_of(MessageRole::User), seq_of(MessageRole::Assistant));
    flow.say("Keep it.").await;
    flow.settled().await;
    flow.say("Thanks.").await;
    flow.settled().await;

    let note = revision_note(MEMORY_CLAIMS_REVISION - 1, false).unwrap();
    assert!(
        note.contains("the Brain keeps only the user's decisions as settled"),
        "{note}"
    );
    let seen = seen.lock().unwrap().clone();
    let told: usize = seen.iter().map(|input| input.matches(&note).count()).sum();
    assert_eq!(told, 1, "{seen:?}");
    let replies: Vec<&String> = seen
        .iter()
        .filter(|line| line.starts_with("remember: "))
        .collect();
    assert_eq!(replies.len(), 4, "{replies:?}");
    assert!(
        replies[0].starts_with("remember: false ") && replies[0].contains("(a user decision)"),
        "{}",
        replies[0]
    );
    assert!(
        replies[1].starts_with("remember: true ") && replies[1].contains("isn't the user's"),
        "{}",
        replies[1]
    );
    assert!(
        replies[2].starts_with("remember: true ") && replies[2].contains("doesn't say"),
        "{}",
        replies[2]
    );
    assert!(
        replies[3].starts_with("remember: true ") && replies[3].contains("needs `user_message`"),
        "{}",
        replies[3]
    );
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
    let decisions = brain
        .nodes(&NodeFilter {
            kinds: vec![NodeKind::Decision],
            ..Default::default()
        })
        .unwrap();
    let kept: Vec<_> = decisions
        .iter()
        .filter(|node| node.title.contains("Makefile"))
        .collect();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_eq!(kept[0].title, "Use tabs in the Makefile");
    assert_eq!(kept[0].provenance.origin, Origin::User);
    assert_eq!(
        source_label(kept[0].kind, &kept[0].provenance),
        "user decision"
    );
    assert!(
        kept[0].body.contains("\"tabs in the Makefile, always\""),
        "{}",
        kept[0].body
    );
    flow.stop().await;
}
