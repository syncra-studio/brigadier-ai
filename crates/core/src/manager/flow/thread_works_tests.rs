//! The thread is the worker (BUILD-PLAN B1-B3): it does normal work itself, and starts workers
//! only when they help, after one line saying so, or after asking with the user's "Start
//! workers on its own" off.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use brigadier_providers::{ProviderEvent, Role};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::model::MessageRole;
use crate::work::{QuestionKind, RequestState, TaskState};

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
                        ""
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
                    return Reply::text("");
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
                Reply::text("")
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
        assert!(
            rule.contains("ask first on one `brigadier ask` card"),
            "{rule}"
        );
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
    // How workers are handled is in the instructions the CLI starts with, never a note.
    assert!(rule.contains("How you handle workers:"), "{rule}");
    let heard = inputs.lock().unwrap().clone();
    assert!(
        heard.iter().all(|input| !input.contains("[instructions]")),
        "{heard:#?}"
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
