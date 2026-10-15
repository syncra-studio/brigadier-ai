//! The thread is the worker (BUILD-PLAN B1-B3): it does normal work itself, and starts workers
//! only when they help, after one line saying so, or after asking with the user's "Start
//! workers on its own" off. Rule R: a session from before the change hears it once and does a
//! tiny edit and a three-file feature itself, with no worker and no merge card.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use brigadier_providers::{ProviderEvent, Role};
use serde_json::json;

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::manager::prompts::{THREAD_WORKS_REVISION, revision_note};
use crate::model::{DomainEvent, MessageRole};
use crate::work::{QuestionKind, RequestState, ReviewState, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const ANNOUNCED: &str = "Starting 3 workers: the CSV, JSON and XML exporters; about 10 min.";

/// A T4-shaped job: three exporters that can be written at once. The thread says so in one
/// line, starts the three in one batch, and answers once all three reported. With "Start
/// workers on its own" off it asks first on one card and starts them only on the yes.
async fn parts_that_run_at_once(start_workers_on_own: bool) {
    let reports = Arc::new(AtomicUsize::new(0));
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let (count, log) = (reports.clone(), inputs.clone());
    let flow = Flow::start(
        if start_workers_on_own {
            "works-t4-on"
        } else {
            "works-t4-off"
        },
        Options::default(),
        script(move |turn| {
            let (count, log) = (count.clone(), log.clone());
            async move {
                if !turn.is_orchestrator() {
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "The exporter is written."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("[report task-") {
                    // Reports that arrive together come in one message.
                    let new = turn.input.matches("[report task-").count();
                    let seen = count.fetch_add(new, Ordering::SeqCst) + new;
                    return Reply::text(if seen == 3 {
                        "The three exporters are written."
                    } else {
                        "[quiet]"
                    });
                }
                let asked_first = turn.input.contains("Add CSV, JSON and XML exporters.")
                    && !start_workers_on_own;
                if asked_first {
                    let asked = turn
                        .call(
                            "ask_user",
                            json!({ "questions": [{
                                "question": "Start 3 workers for the exporters (about 10 min)?",
                                "options": [{ "label": "Yes" }, { "label": "No, do it yourself" }],
                                "recommended": 0
                            }]}),
                        )
                        .await;
                    assert!(!asked.is_error, "{}", asked.text);
                    return Reply::text("[quiet]");
                }
                if start_workers_on_own {
                    turn.emit(ProviderEvent::Message {
                        item_id: uuid::Uuid::new_v4().to_string(),
                        role: Role::Assistant,
                        text: ANNOUNCED.into(),
                    })
                    .await;
                }
                for format in ["CSV", "JSON", "XML"] {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": format!("Write {format} export"),
                                   "kind": "research",
                                   "spec": format!("Write the {format} exporter.")}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    let mut settings = flow.core.settings();
    settings.start_workers_on_own = start_workers_on_own;
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Add CSV, JSON and XML exporters.").await;
    if !start_workers_on_own {
        let board = flow
            .until("the card", |board| open_card(board).is_some())
            .await;
        assert!(board.tasks.is_empty(), "no worker before the yes");
        let card = open_card(&board).unwrap();
        flow.manager
            .answer_question(
                flow.conversation.clone(),
                card.id.clone(),
                vec!["Yes".into()],
                None,
            )
            .await
            .unwrap();
    }
    flow.until("the three reports", |board| {
        board.tasks.len() == 3
            && board
                .tasks
                .values()
                .all(|task| task.state == TaskState::Done)
            && board
                .latest_request()
                .is_some_and(|request| request.state == RequestState::Done)
    })
    .await;
    let board = flow.settled().await;
    assert_eq!(board.tasks.len(), 3);
    assert_eq!(board.requests.len(), 1, "{:#?}", board.requests);
    let rule = flow.thread_specs()[0]
        .1
        .append_system_prompt
        .clone()
        .unwrap();
    let shown: Vec<String> = flow
        .core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| message.text)
        .collect();
    if start_workers_on_own {
        assert!(rule.contains("say so in one line first"), "{rule}");
        assert_eq!(
            shown.first().map(String::as_str),
            Some(ANNOUNCED),
            "{shown:#?}"
        );
    } else {
        assert!(rule.contains("ask first with"), "{rule}");
        assert!(!shown.iter().any(|text| text == ANNOUNCED), "{shown:#?}");
        let heard = inputs.lock().unwrap().clone();
        assert!(heard[1].contains("The user answered: Yes"), "{heard:#?}");
    }
    assert_eq!(
        shown.last().map(String::as_str),
        Some("The three exporters are written."),
        "{shown:#?} {:#?}",
        inputs.lock().unwrap()
    );
    flow.stop().await;
}

fn open_card(board: &Board) -> Option<crate::work::Question> {
    board
        .questions
        .values()
        .find(|question| question.is_open() && !matches!(question.kind, QuestionKind::Merge { .. }))
        .cloned()
}

#[tokio::test]
async fn parts_that_run_at_once_start_workers_after_one_line() {
    parts_that_run_at_once(true).await;
}

#[tokio::test]
async fn parts_that_run_at_once_are_asked_first_with_start_workers_off() {
    parts_that_run_at_once(false).await;
}

/// Rule R for B1-B3: a session told the instructions of revision 31 resumes after the change
/// and hears revision 32 once. A tiny edit it makes itself gets no review; a three-file feature
/// with no part worth running at once is done by the thread too, in one commit, with no worker:
/// `run_check` says its review runs, and the answer comes after the review's word. Neither
/// request offers a merge card.
#[tokio::test]
async fn a_session_from_before_hears_the_change_once_and_does_the_work_itself() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let checks: Arc<Mutex<Vec<String>>> = Arc::default();
    let (heard, said) = (inputs.clone(), checks.clone());
    let mut flow = Flow::start(
        "works-rule-r",
        Options::default(),
        script(move |turn| {
            let (heard, said) = (heard.clone(), said.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("");
                }
                heard.lock().unwrap().push(turn.input.clone());
                let check = |turn: Turn| async move {
                    let reply = turn.call("run_check", json!({"command": "true"})).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    reply.text
                };
                if turn.input.contains("Fix the typo in the README.") {
                    commit_in_workspace(&turn, "README.md", "fixed\n", "Fix the README typo");
                    let reply = check(turn).await;
                    said.lock().unwrap().push(reply.clone());
                    assert!(!reply.contains("[review running]"), "{reply}");
                    return Reply::text("Fixed the typo in README.md.");
                }
                if turn.input.contains("Add a greeting module.") {
                    let workspace = turn.add_dirs[0].clone();
                    for (file, text) in [
                        ("greet.rs", "pub fn greet() -> &'static str { \"hi\" }\n"),
                        ("lib.rs", "pub mod greet;\n"),
                        ("main.rs", "fn main() { greet::greet(); }\n"),
                    ] {
                        std::fs::write(workspace.join(file), text).unwrap();
                    }
                    super::git(&workspace, &["add", "-A"]);
                    super::git(&workspace, &["commit", "-q", "-m", "Add a greeting module"]);
                    let reply = check(turn).await;
                    said.lock().unwrap().push(reply.clone());
                    return Reply::text(if reply.contains("[review running]") {
                        "[quiet]"
                    } else {
                        "Added the greeting module."
                    });
                }
                if turn.input.contains("found nothing") {
                    return Reply::text("Added the greeting module; the review found nothing.");
                }
                Reply::text("Fine.")
            }
        }),
    )
    .await;
    flow.say("Look around.").await;
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
                            revision: Some(THREAD_WORKS_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;

    flow.say("Fix the typo in the README.").await;
    let board = flow
        .until("the typo's answer", |board| {
            board.requests.len() == 2
                && board
                    .latest_request()
                    .is_some_and(|request| request.state == RequestState::Done)
        })
        .await;
    assert!(board.reviews.is_empty(), "{:#?}", board.reviews);

    flow.say("Add a greeting module.").await;
    flow.until("the feature's answer", |board| {
        board.requests.len() == 3
            && board
                .latest_request()
                .is_some_and(|request| request.state == RequestState::Done)
    })
    .await;
    let board = flow.settled().await;
    // The thread did both itself: no worker, no merge card.
    assert!(board.tasks.is_empty(), "{:#?}", board.tasks);
    assert!(
        !board
            .questions
            .values()
            .any(|question| matches!(question.kind, QuestionKind::Merge { .. })),
        "{:#?}",
        board.questions
    );
    // One review, of the feature's commit, before its answer.
    let reviews: Vec<_> = board.reviews.values().collect();
    assert_eq!(reviews.len(), 1, "{reviews:#?}");
    assert!(reviews[0].task_id.is_none());
    assert_eq!(reviews[0].state, ReviewState::Clean);
    let said = checks.lock().unwrap().clone();
    assert!(said[1].contains("[review running]"), "{said:#?}");
    // The note came once, at the first turn after the change, in plain words.
    let heard = inputs.lock().unwrap().clone();
    let note = revision_note(THREAD_WORKS_REVISION - 1, false).unwrap();
    assert!(note.contains("From now on, do the work yourself"), "{note}");
    let told: Vec<usize> = heard
        .iter()
        .enumerate()
        .filter(|(_, input)| input.contains("From now on, do the work yourself"))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(told.len(), 1, "{heard:#?}");
    assert!(
        heard[told[0]].contains("Fix the typo in the README."),
        "{heard:#?}"
    );
    let shown: Vec<String> = flow
        .core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| message.text)
        .collect();
    assert_eq!(
        shown.last().map(String::as_str),
        Some("Added the greeting module; the review found nothing."),
        "{shown:#?}"
    );
    assert!(!shown.iter().any(|text| text == "[quiet]"), "{shown:#?}");
    flow.stop().await;
}
