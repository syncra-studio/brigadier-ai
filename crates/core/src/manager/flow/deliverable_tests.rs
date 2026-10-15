//! Deliverables survive their task (BUILD-PLAN A1): a scout's output linked by its scratch
//! path, in its report, in what it wrote after the report and in the thread's answer, still
//! opens Brigadier's stored copy once the task ended and its scratch folder went, and the
//! addendum says the folder is gone. Rule R: a session from before the change hears it once.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::prompts::{DELIVERABLES_REVISION, QUIET, revision_note};
use crate::model::DomainEvent;
use crate::model::MessageRole;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const LIST: &str = "# Rounding\n\n- round_cents (src/money.ts:12)\n- to_minor (src/money.ts:30)\n";

/// The worker's outputs folder, as its brief names it.
fn outputs_folder(turn: &Turn) -> PathBuf {
    const SAYS: &str = "Your outputs folder (files for the orchestrator and the user): ";
    let text = format!("{}\n{}", turn.prompt, turn.input);
    let rest = &text[text.find(SAYS).expect("the brief names the outputs folder") + SAYS.len()..];
    PathBuf::from(rest.lines().next().unwrap().trim())
}

/// The first path ending in `/rounding.md` that `text` links, as a model writes it.
fn link_in(text: &str) -> String {
    let end = text.find("/rounding.md").expect("a link to rounding.md") + "/rounding.md".len();
    let start = text[..end]
        .rfind(|c: char| c.is_whitespace() || matches!(c, '(' | '`' | '['))
        .map_or(0, |i| i + 1);
    text[start..end].to_owned()
}

#[tokio::test]
async fn a_scouts_output_linked_by_its_scratch_path_opens_the_stored_copy_after_the_task_ended() {
    let thread_inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let worker_link: Arc<Mutex<Option<String>>> = Arc::default();
    let (heard, written) = (thread_inputs.clone(), worker_link.clone());
    let flow = Flow::start(
        "deliverable-links",
        Options::default(),
        script(move |turn| {
            let (heard, written) = (heard.clone(), written.clone());
            async move {
                if turn.is_orchestrator() {
                    heard.lock().unwrap().push(turn.input.clone());
                    if turn.input.contains("List the rounding functions") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "low", "title": "List rounding functions",
                                       "kind": "scout", "spec": "List them in outputs/rounding.md."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                        return Reply::text(QUIET);
                    }
                    if turn.input.contains("[report task-1") && !turn.input.contains("· addendum]") {
                        let link = link_in(&turn.input);
                        return Reply::text(format!("The list is in [rounding.md]({link})."));
                    }
                    return Reply::text(QUIET);
                }
                let outputs = outputs_folder(&turn);
                std::fs::create_dir_all(&outputs).unwrap();
                let file = outputs.join("rounding.md");
                std::fs::write(&file, LIST).unwrap();
                let link = file.display().to_string();
                *written.lock().unwrap() = Some(link.clone());
                let reply = turn
                    .call(
                        "submit_report",
                        json!({"summary": format!("Listed the rounding functions in {link}; the details are below.")}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text(format!(
                    "Details: {link} lists two functions. round_cents rounds half up; to_minor truncates."
                ))
            }
        }),
    )
    .await;
    flow.say("List the rounding functions.").await;
    let board = flow
        .until("the scout to end", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 1 && task.state.is_final())
        })
        .await;
    let task = Flow::task(&board, 1).clone();
    let scratch = PathBuf::from(&task.workspace.as_ref().unwrap().scratch);
    flow.settled().await;
    flow.until("the scratch folder to go", |_| !scratch.exists())
        .await;

    let worker_link = worker_link.lock().unwrap().clone().unwrap();
    let inputs = thread_inputs.lock().unwrap().clone();
    let addendum = inputs
        .iter()
        .find_map(|input| {
            input
                .split_once("· addendum]")
                .map(|(_, rest)| rest.to_owned())
        })
        .expect("the thread got the addendum");
    assert!(
        addendum.contains("Its scratch folder is gone, so paths in it no longer open"),
        "{addendum}"
    );
    // The report tells the thread how to link an output without its scratch path.
    let report = inputs
        .iter()
        .find(|input| input.contains("[report task-1") && !input.contains("· addendum]"))
        .unwrap();
    assert!(
        report.contains(
            "link one as [notes.md](outputs/notes.md), which keeps opening Brigadier's copy after its scratch folder goes, never by its scratch path"
        ),
        "{report}"
    );
    let answer = flow
        .core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .rev()
        .find(|message| {
            message.role == MessageRole::Assistant && message.text.contains("rounding.md")
        })
        .expect("the thread's answer links the list")
        .text;
    for (whose, link) in [
        ("the worker's report", worker_link.clone()),
        ("the addendum", link_in(&addendum)),
        ("the thread's answer", link_in(&answer)),
    ] {
        assert!(
            link.contains(&*scratch.display().to_string()),
            "{whose}: {link}"
        );
        assert!(!PathBuf::from(&link).exists(), "{whose}: {link} is gone");
        let stored = flow
            .manager
            .stored_output(&flow.conversation, &link, None)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{whose}: {link} has no stored copy"));
        assert_eq!(stored.task_id, task.id, "{whose}");
        let copy = flow
            .manager
            .artifact_copy(stored.artifact.id.clone(), "rounding.md".into())
            .await
            .unwrap();
        assert_eq!(std::fs::read_to_string(copy).unwrap(), LIST, "{whose}");
    }
    // The link form the report teaches opens the same copy.
    let stored = flow
        .manager
        .stored_output(&flow.conversation, "outputs/rounding.md", None)
        .await
        .unwrap()
        .expect("outputs/rounding.md has a stored copy");
    assert_eq!(stored.task_id, task.id);
    flow.stop().await;
}

/// Rule R: a session told the instructions before this change hears revision 30 once, at its
/// next message, and its workers hear theirs. (Links are resolved when opened, so the test
/// above covers messages written before the change too.)
#[tokio::test]
async fn a_session_from_before_hears_the_deliverables_note_once() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let heard = inputs.clone();
    let mut flow = Flow::start(
        "deliverables-revision",
        Options::default(),
        script(move |turn| {
            let heard = heard.clone();
            async move {
                if turn.is_orchestrator() {
                    heard.lock().unwrap().push(turn.input.clone());
                }
                Reply::text("Fine.")
            }
        }),
    )
    .await;
    flow.say("Look around.").await;
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
                            revision: Some(DELIVERABLES_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.say("Where is the list?").await;
    flow.settled().await;
    flow.say("Thanks.").await;
    flow.settled().await;
    let note = revision_note(DELIVERABLES_REVISION - 1, false).unwrap();
    assert!(
        note.contains("never give the user a scratch path"),
        "{note}"
    );
    assert!(
        note.contains("\"To check: …\" is only for what nobody here can check or fix"),
        "{note}"
    );
    let worker_note = revision_note(DELIVERABLES_REVISION - 1, true).unwrap();
    assert!(worker_note.contains("never link its path"), "{worker_note}");
    let inputs = inputs.lock().unwrap().clone();
    let told: usize = inputs
        .iter()
        .map(|input| input.matches(&note).count())
        .sum();
    assert_eq!(told, 1, "{inputs:?}");
    flow.stop().await;
}
