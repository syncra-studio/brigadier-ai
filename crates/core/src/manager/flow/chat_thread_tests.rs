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

/// Q78: before researching, the thread asks the one thing the answer depends on, on one card,
/// then researches with the answer: in a Chat (`chat`) or a session's thread alike.
async fn one_clarifying_card_then_research(chat: bool) {
    let specs: Arc<Mutex<Vec<String>>> = Arc::default();
    let seen = specs.clone();
    let flow = Flow::start(
        if chat {
            "chat-thread-clarify"
        } else {
            "session-clarify"
        },
        Options::default(),
        script(move |turn| {
            let seen = seen.clone();
            async move {
                let thread = turn.chat().is_some() || turn.is_orchestrator();
                if !thread {
                    seen.lock().unwrap().push(turn.input.clone());
                    turn.call("submit_report", json!({"summary": "Two shops in Chisinau stock it."}))
                        .await;
                    return Reply::text("Reported.");
                }
                if turn.input.contains("[report task-1") {
                    return Reply::text("Buy it at the shop on Stefan cel Mare street.");
                }
                if turn.input.contains("[answer]") {
                    assert!(turn.input.contains("Chisinau"), "{}", turn.input);
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "low", "title": "Shops in Chisinau", "kind": "research",
                                   "spec": "Find shops in Chisinau that stock PETG."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("[quiet]");
                }
                let asked = turn
                    .call(
                        "ask_user",
                        json!({"questions": [{
                            "question": "Which city do you live in?",
                            "options": [{"label": "Chisinau"}, {"label": "Bucharest"}],
                            "recommended": 0
                        }]}),
                    )
                    .await;
                assert!(!asked.is_error, "{}", asked.text);
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    let id = if chat {
        new_chat(&flow).await
    } else {
        flow.conversation.clone()
    };
    flow.say_to(&id, "Where can I buy PETG filament?").await;
    let board = flow
        .until_in(&id, "the card", |board| {
            board.questions.values().any(|question| question.is_open())
        })
        .await;
    assert!(board.tasks.is_empty(), "no research before the answer");
    let card = board.questions.values().next().unwrap().clone();
    flow.manager
        .answer_question(id.clone(), card.id.clone(), vec!["Chisinau".into()], None)
        .await
        .unwrap();
    let board = answered(&flow, &id).await;
    assert_eq!(board.questions.len(), 1);
    assert_eq!(board.requests.len(), 1, "{:#?}", board.requests);
    assert_eq!(board.tasks.len(), 1);
    assert!(specs.lock().unwrap()[0].contains("Chisinau"));
    assert_eq!(
        shown(&flow, &id).await,
        vec!["Buy it at the shop on Stefan cel Mare street.".to_owned()]
    );
    let rule = "at most one card before researching, holding only what changes the answer";
    let prompts: Vec<String> = flow
        .specs()
        .into_iter()
        .filter_map(|(_, spec)| spec.append_system_prompt)
        .collect();
    let told = if chat {
        prompts
            .iter()
            .any(|prompt| prompt.contains(rule) && prompt.contains("This Chat"))
    } else {
        prompts.iter().any(|prompt| {
            prompt.contains(rule) && prompt.contains(crate::manager::prompts::THREAD_OPENING)
        })
    };
    assert!(told, "{prompts:#?}");
    flow.stop().await;
}

#[tokio::test]
async fn a_chat_asks_one_clarifying_card_then_researches() {
    one_clarifying_card_then_research(true).await;
}

#[tokio::test]
async fn a_session_asks_one_clarifying_card_then_researches() {
    one_clarifying_card_then_research(false).await;
}

/// A file a Chat hands over is made by a worker in its outputs folder (a Chat writes none
/// itself): the answer links it as `outputs/<name>`, which opens the stored copy, and the
/// answer's Save lists it (answerFiles, the app's).
#[tokio::test]
async fn a_chats_file_comes_from_a_worker_and_opens_from_the_answer() {
    let flow = Flow::start(
        "chat-thread-file",
        Options::default(),
        script(|turn| async move {
            if turn.chat().is_none() {
                std::fs::create_dir_all(turn.cwd.join("outputs")).unwrap();
                std::fs::write(turn.cwd.join("outputs/list.csv"), "shop,price\nA,10\n").unwrap();
                turn.call(
                    "submit_report",
                    json!({"summary": "The list is in outputs/list.csv."}),
                )
                .await;
                return Reply::text("Reported.");
            }
            if turn.input.contains("[report task-1") {
                return Reply::text("Here is the list: [list.csv](outputs/list.csv).");
            }
            turn.call(
                "delegate_task",
                json!({"effort": "low", "title": "Price list", "kind": "research",
                       "spec": "Write a CSV of PETG prices."}),
            )
            .await;
            Reply::text("[quiet]")
        }),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Make me a price list file.").await;
    let board = answered(&flow, &chat).await;
    let task = board.tasks.values().next().unwrap();
    assert!(
        task.outputs
            .iter()
            .any(|output| output.file_name.as_deref() == Some("list.csv")
                || output.title.ends_with("list.csv")),
        "{:#?}",
        task.outputs
    );
    let stored = flow
        .manager
        .stored_output(&chat, "outputs/list.csv", None)
        .await
        .unwrap()
        .expect("the answer's link opens the stored copy");
    assert_eq!(stored.task_id, task.id);
    flow.stop().await;
}

/// A worker's report that comes after the user wrote again is answered under their newest
/// request, at the end of the thread where they look; the earlier request keeps what it said
/// and still names what the report is for (#440).
#[tokio::test]
async fn a_late_report_is_answered_at_the_end_of_the_chat() {
    let gate = Arc::new(tokio::sync::Notify::new());
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let (open, seen) = (gate.clone(), inputs.clone());
    let flow = Flow::start(
        "chat-late-report",
        Options::default(),
        script(move |turn| {
            let (open, seen) = (open.clone(), seen.clone());
            async move {
                if turn.chat().is_none() {
                    open.notified().await;
                    turn.call(
                        "submit_report",
                        json!({"summary": "The 0.4 mm brass nozzle prints PETG cleanly."}),
                    )
                    .await;
                    return Reply::text("Reported.");
                }
                seen.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("[report task-1") {
                    return Reply::text(ANSWER);
                }
                if turn.input.contains("any news?") {
                    return Reply::text("Still on it; I'll send it here.");
                }
                turn.call(
                    "delegate_task",
                    json!({"effort": "medium", "title": "Compare nozzles", "kind": "research",
                           "spec": "Compare 3D printer nozzles for PETG."}),
                )
                .await;
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Which nozzle should I buy for PETG?")
        .await;
    flow.until_in(&chat, "the worker to run", |board| {
        board
            .tasks
            .values()
            .any(|task| task.state == TaskState::Running)
    })
    .await;
    flow.say_to(&chat, "any news?").await;
    flow.until_in(&chat, "the follow-up to be answered", |board| {
        board
            .requests
            .values()
            .any(|request| request.preview == "any news?" && request.state == RequestState::Done)
    })
    .await;
    gate.notify_one();
    let board = answered(&flow, &chat).await;
    let newest = board.latest_request().unwrap().id.clone();
    let messages = flow.core.all_messages(&chat).await.unwrap();
    let answer = messages
        .iter()
        .find(|message| message.role == MessageRole::Assistant && message.text == ANSWER)
        .expect("the answer");
    assert_eq!(answer.request_id.as_deref(), Some(newest.as_str()));
    assert_eq!(
        messages.last().map(|message| message.text.as_str()),
        Some(ANSWER)
    );
    let input = inputs.lock().unwrap().join("\n");
    assert!(
        input.contains(
            "[for the user's earlier request: \"Which nozzle should I buy for PETG?\"]\n[report task-1"
        ),
        "{input}"
    );
    flow.stop().await;
}
