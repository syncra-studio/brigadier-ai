//! A Chat is a thread without a project (Q76): it starts workers that look things up or operate
//! apps, in folders of their own with no repository, answers their questions and writes its
//! answer from their reports. Work on a repository is refused in plain words.

use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::{ConversationId, ConversationKind, MessageRole, ModelChoice, SetupRequest};
use crate::work::{RequestState, TaskKind, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A new Chat on Claude.
pub(super) async fn new_chat(flow: &Flow) -> ConversationId {
    flow.manager
        .create_conversation(
            ConversationKind::Chat,
            None,
            Some("Chat".into()),
            Some(SetupRequest::Chat {
                model: ModelChoice {
                    provider: ProviderKind::Claude,
                    model: Some("claude-opus-5-5".into()),
                    effort: None,
                    fast: None,
                    account: None,
                },
            }),
        )
        .await
        .unwrap()
        .id
}

/// What the Chat showed the user, in order.
pub(super) async fn shown(flow: &Flow, chat: &ConversationId) -> Vec<String> {
    flow.core
        .all_messages(chat)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| message.text)
        .collect()
}

/// Waits until every request of `chat` is done.
pub(super) async fn answered(flow: &Flow, chat: &ConversationId) -> crate::board::Board {
    flow.until_in(chat, "the Chat to answer", |board| {
        !board.requests.is_empty()
            && board
                .requests
                .values()
                .all(|request| request.state == RequestState::Done)
    })
    .await
}

const ANSWER: &str = "Pick the 0.4 mm brass nozzle: it prints PETG cleanly and costs least.";

/// The Chat starts a research worker; the worker works in a scratch folder with no repository,
/// and the Chat answers from its report once, with nothing of `[quiet]` shown.
#[tokio::test]
async fn a_chat_starts_a_research_worker_and_answers_from_its_report() {
    let worker_dirs: Arc<Mutex<Vec<std::path::PathBuf>>> = Arc::default();
    let dirs = worker_dirs.clone();
    let flow = Flow::start(
        "chat-thread-research",
        Options::default(),
        script(move |turn| {
            let dirs = dirs.clone();
            async move {
                if turn.chat().is_none() {
                    dirs.lock().unwrap().push(turn.cwd.clone());
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "The 0.4 mm brass nozzle prints PETG cleanly."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if turn.input.contains("[report task-1") {
                    return Reply::text(ANSWER);
                }
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "medium", "title": "Compare nozzles", "kind": "research",
                               "spec": "Compare 3D printer nozzles for PETG."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Which nozzle should I buy for PETG?")
        .await;
    let board = answered(&flow, &chat).await;
    let task = board.tasks.values().next().expect("a task");
    assert_eq!(task.kind, TaskKind::Research);
    assert_eq!(task.state, TaskState::Done);
    let workspace = task.workspace.as_ref().unwrap();
    assert!(workspace.worktree.is_none() && workspace.branch.is_none());
    let dirs = worker_dirs.lock().unwrap().clone();
    assert_eq!(dirs.len(), 1);
    assert!(!dirs[0].starts_with(&flow.repo), "{}", dirs[0].display());
    assert!(!dirs[0].join(".git").exists());
    assert_eq!(shown(&flow, &chat).await, vec![ANSWER.to_owned()]);
    // The session's own board got nothing.
    assert!(flow.board().await.tasks.is_empty());
    flow.stop().await;
}

/// A Chat has no repository: work on code is refused in words that say what it can start.
#[tokio::test]
async fn a_chat_cannot_start_work_on_a_repository() {
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = replies.clone();
    let flow = Flow::start(
        "chat-thread-no-repo",
        Options::default(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                if turn.chat().is_none() {
                    return Reply::text("ok");
                }
                for kind in ["implement", "scout"] {
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Fix it", "kind": kind,
                                   "spec": "Fix the bug."}),
                        )
                        .await;
                    assert!(reply.is_error, "{}", reply.text);
                    seen.lock().unwrap().push(reply.text);
                }
                Reply::text("I can't change code from a Chat.")
            }
        }),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Fix my app.").await;
    let board = answered(&flow, &chat).await;
    assert!(board.tasks.is_empty());
    for text in replies.lock().unwrap().iter() {
        assert!(text.contains("only research or operate apps"), "{text}");
    }
    flow.stop().await;
}

/// A Chat's worker asks it a question; the Chat answers with answer_worker and the worker
/// reports with the answer.
#[tokio::test]
async fn a_chat_answers_its_workers_question() {
    let flow = Flow::start(
        "chat-thread-question",
        Options::default(),
        script(|turn| async move {
            if turn.chat().is_none() {
                let asked = turn
                    .call("ask_thread", json!({"question": "Which printer is it?"}))
                    .await;
                assert!(!asked.is_error, "{}", asked.text);
                assert!(asked.text.contains("Prusa MK4"), "{}", asked.text);
                turn.call("submit_report", json!({"summary": "For the Prusa MK4: brass."}))
                    .await;
                return Reply::text("Reported.");
            }
            if turn.input.contains("[question from task-1") {
                let reply = turn
                    .call(
                        "answer_worker",
                        json!({"task": "task-1", "answer": "A Prusa MK4.", "why": "The user said so."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            if turn.input.contains("[report task-1") {
                return Reply::text("Brass suits your Prusa MK4.");
            }
            let reply = turn
                .call(
                    "delegate_task",
                    json!({"effort": "low", "title": "Nozzle for my printer", "kind": "research",
                           "spec": "Find the right nozzle."}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            Reply::text("[quiet]")
        }),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Which nozzle fits my printer? It's a Prusa MK4.")
        .await;
    answered(&flow, &chat).await;
    assert_eq!(
        shown(&flow, &chat).await,
        vec!["Brass suits your Prusa MK4.".to_owned()]
    );
    flow.stop().await;
}
