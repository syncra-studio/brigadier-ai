//! The thread asks the user only on cards (THREAD-PARITY-PLAN.md §5): a round of questions
//! on one card, answered at once, inside the same request; a question asked in text is asked
//! again on a card; the merge is asked once on a card, whose Merge is the user's consent.

use std::sync::{Arc, Mutex};

use brigadier_providers::{ItemStatus, ProviderEvent, Role};
use serde_json::{Value, json};

use super::thread_tests::{commit_in_workspace, on_main};
use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::model::MessageRole;
use crate::work::{OrchestratorStepKind, Question, QuestionKind, RequestState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The question cards of the session, by kind.
fn cards(board: &Board, merge: bool) -> Vec<Question> {
    let mut cards: Vec<Question> = board
        .questions
        .values()
        .filter(|q| matches!(q.kind, QuestionKind::Merge { .. }) == merge)
        .cloned()
        .collect();
    cards.sort_by_key(|q| q.created_at_ms);
    cards
}

fn open_card(board: &Board, merge: bool) -> Option<Question> {
    cards(board, merge).into_iter().find(Question::is_open)
}

fn round() -> Value {
    json!({ "questions": [
        { "question": "Which format should the export use?",
          "options": [
              { "label": "CSV", "description": "Opens in any spreadsheet." },
              { "label": "JSON" }
          ],
          "recommended": 0 },
        { "question": "Who may export?",
          "options": [{ "label": "Everyone" }, { "label": "Admins only" }],
          "recommended": 1 },
        { "question": "What should the file be called?" }
    ]})
}

/// One ask_user call puts a round of three questions on one card. The request waits on it with
/// its time running, the user answers all three at once, and the thread gets one [answer] in
/// the same request, which then ends: one request, one block.
#[tokio::test]
async fn a_round_of_questions_is_one_card_and_its_answers_continue_the_same_request() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "card-round",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("[answer]") {
                    return Reply::text("Decided: a CSV export for admins, named export.csv.");
                }
                // A round the card can't hold is refused, with what to change.
                let seven: Vec<Value> = (0..7)
                    .map(|n| json!({ "question": format!("Question {n}?") }))
                    .collect();
                let refused = turn.call("ask_user", json!({ "questions": seven })).await;
                assert!(refused.is_error && refused.text.contains("1 to 6"), "{}", refused.text);
                let one = turn
                    .call(
                        "ask_user",
                        json!({ "questions": [{ "question": "CSV?", "options": [{ "label": "Yes" }], "recommended": 0 }] }),
                    )
                    .await;
                assert!(one.is_error && one.text.contains("2 to 4"), "{}", one.text);
                let unsaid = turn
                    .call(
                        "ask_user",
                        json!({ "questions": [{ "question": "CSV?", "options": [{ "label": "Yes" }, { "label": "No" }] }] }),
                    )
                    .await;
                assert!(unsaid.is_error && unsaid.text.contains("recommend"), "{}", unsaid.text);
                let asked = turn.call("ask_user", round()).await;
                assert!(!asked.is_error, "{}", asked.text);
                assert!(asked.text.contains("3 questions"), "{}", asked.text);
                assert!(asked.text.contains("[quiet]"), "{}", asked.text);
                Reply::text("[quiet]")
            }
        }),
    )
    .await;
    flow.say("Plan the export.").await;
    let board = flow
        .until("the card", |board| open_card(board, false).is_some())
        .await;
    let card = open_card(&board, false).unwrap();
    assert_eq!(card.round().len(), 3);
    assert_eq!(
        card.items[0].options[0].description.as_deref(),
        Some("Opens in any spreadsheet.")
    );
    assert_eq!(card.items[1].recommended, Some(1));
    let board = flow
        .until("the request to wait on the card", |board| {
            board
                .latest_request()
                .is_some_and(|request| request.state == RequestState::Waiting)
        })
        .await;
    // Waiting on its card, the request's time runs on.
    let request = board.latest_request().unwrap().clone();
    assert!(
        request
            .worked
            .last()
            .is_some_and(|span| span.to_ms.is_none()),
        "{request:#?}"
    );
    // Every question needs its answer.
    let short = flow
        .manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["CSV".into()],
        )
        .await;
    assert!(short.is_err());
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["CSV".into(), "Admins only".into(), "export.csv".into()],
        )
        .await
        .unwrap();
    flow.until("the answers' turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    let board = flow.settled().await;
    let answered = &board.questions[&card.id];
    assert_eq!(answered.answers, ["CSV", "Admins only", "export.csv"]);
    let heard = inputs.lock().unwrap().clone();
    assert_eq!(heard.len(), 2, "{heard:#?}");
    assert!(
        heard[1].contains(
            "[answer] You asked the user:\n1. Which format should the export use?\n   → CSV\n2. Who may export?\n   → Admins only\n3. What should the file be called?\n   → export.csv"
        ),
        "{}",
        heard[1]
    );
    assert_eq!(board.requests.len(), 1, "{:#?}", board.requests);
    let request = board.latest_request().unwrap();
    assert_eq!(request.state, RequestState::Done);
    // One span, from the request's start to its end: the wait on the card counted.
    assert_eq!(request.worked.len(), 1, "{:#?}", request.worked);
    flow.stop().await;
}

/// A reply that asks the user something in text, with no card open, gets one note to ask it on
/// a card instead; the request goes on working, and the thread's card is what waits.
#[tokio::test]
async fn a_question_asked_in_text_is_asked_again_on_a_card() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "card-guard",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                let again = log
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|input| input.contains("high-contrast"));
                if turn.input.contains("asked the user a question in text") && !again {
                    let asked = turn
                        .call(
                            "ask_user",
                            json!({ "questions": [{
                                "question": "Which theme should be the default?",
                                "options": [{ "label": "Dark" }, { "label": "Light" }],
                                "recommended": 0
                            }]}),
                        )
                        .await;
                    assert!(!asked.is_error, "{}", asked.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("[answer]") {
                    return Reply::text("Dark it is.");
                }
                Reply::text("Which theme should be the default: dark or light?")
            }
        }),
    )
    .await;
    flow.say("Add a dark mode.").await;
    let board = flow
        .until("the card", |board| open_card(board, false).is_some())
        .await;
    let card = open_card(&board, false).unwrap();
    assert_eq!(board.requests.len(), 1);
    assert_eq!(
        card.request_id.as_deref(),
        Some(board.latest_request().unwrap().id.as_str())
    );
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["Dark".into()],
        )
        .await
        .unwrap();
    flow.until("the answer's turn", |_| inputs.lock().unwrap().len() >= 3)
        .await;
    let board = flow.settled().await;
    assert_eq!(board.requests.len(), 1, "{:#?}", board.requests);
    assert_eq!(board.latest_request().unwrap().state, RequestState::Done);
    let heard = inputs.lock().unwrap().clone();
    assert_eq!(heard.len(), 3, "{heard:#?}");
    assert!(
        heard[1].contains("Ask it on a card instead"),
        "{}",
        heard[1]
    );
    assert!(heard[2].contains("The user answered: Dark"), "{}", heard[2]);

    // Told once per request: a thread that asks in text again ends the request as before.
    flow.say("Also add a high-contrast theme.").await;
    flow.until("its turns", |_| inputs.lock().unwrap().len() >= 5)
        .await;
    let board = flow.settled().await;
    let heard = inputs.lock().unwrap().clone();
    assert_eq!(heard.len(), 5, "{heard:#?}");
    let latest = board.latest_request().unwrap();
    assert_eq!(latest.state, RequestState::Done, "{latest:#?}");
    assert!(open_card(&board, false).is_none());
    flow.stop().await;
}

/// What the thread was told by its merge calls, in order.
type Calls = Arc<Mutex<Vec<(String, String, bool)>>>;

async fn call(turn: &Turn, calls: &Calls, name: &str, args: Value) -> bool {
    let reply = turn.call(name, args).await;
    calls
        .lock()
        .unwrap()
        .push((name.to_owned(), reply.text.clone(), reply.is_error));
    !reply.is_error
}

/// The merge is asked once, on a card. A second card is refused while one is open, and after
/// "Not yet" until the user writes again; finish_session refuses without consent. The card's
/// "Merge into main" is consent: finish_session merges without the user's words, once.
#[tokio::test]
async fn the_merge_is_asked_on_a_card_once_and_its_merge_is_the_users_consent() {
    let calls: Calls = Arc::default();
    let log = calls.clone();
    let flow = Flow::start(
        "card-merge",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                let input = turn.input.clone();
                if input.contains("Add notes.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    assert!(
                        call(
                            &turn,
                            &log,
                            "propose_merge",
                            json!({ "note": "Adds NOTES.md; the review is clean." })
                        )
                        .await
                    );
                    assert!(!call(&turn, &log, "propose_merge", json!({})).await);
                    assert!(!call(&turn, &log, "finish_session", json!({})).await);
                    return Reply::text("[quiet]");
                }
                if input.contains("doesn't want") {
                    assert!(!call(&turn, &log, "propose_merge", json!({})).await);
                    assert!(!call(&turn, &log, "finish_session", json!({})).await);
                    return Reply::text("It stays on its branch.");
                }
                if input.contains("ask me again") {
                    assert!(call(&turn, &log, "propose_merge", json!({})).await);
                    return Reply::text("[quiet]");
                }
                if input.contains("chose to merge") {
                    assert!(call(&turn, &log, "finish_session", json!({})).await);
                    // One Merge, one merge.
                    assert!(!call(&turn, &log, "finish_session", json!({})).await);
                    return Reply::text("Merged.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    let board = flow
        .until("the merge card", |board| open_card(board, true).is_some())
        .await;
    let first = open_card(&board, true).unwrap();
    let item = &first.round()[0];
    assert_eq!(
        item.options
            .iter()
            .map(|o| o.label.as_str())
            .collect::<Vec<_>>(),
        ["Merge into main", "Not yet"]
    );
    assert_eq!(
        item.options[0].description.as_deref(),
        Some("Adds NOTES.md; the review is clean.")
    );
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            first.id.clone(),
            vec!["Not yet".into()],
        )
        .await
        .unwrap();
    flow.until("the Not yet's turn", |_| calls.lock().unwrap().len() >= 5)
        .await;
    flow.settled().await;
    assert!(!on_main(&flow, "NOTES.md"), "nothing merged on Not yet");
    flow.say("OK, ask me again about merging.").await;
    let board = flow
        .until("the second merge card", |board| {
            cards(board, true).len() == 2 && open_card(board, true).is_some()
        })
        .await;
    let second = open_card(&board, true).unwrap();
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            second.id.clone(),
            vec!["Merge into main".into()],
        )
        .await
        .unwrap();
    flow.until("the merge", |_| calls.lock().unwrap().len() >= 8)
        .await;
    let board = flow.settled().await;
    assert!(on_main(&flow, "NOTES.md"), "merged on the card's Merge");
    let calls = calls.lock().unwrap().clone();
    let said = |at: usize, text: &str| {
        assert!(calls[at].1.contains(text), "{at}: {calls:#?}");
    };
    said(1, "already open");
    said(2, "[not merged]");
    said(3, "Not yet");
    said(4, "[not merged]");
    said(6, "[finished]");
    said(7, "already used");
    let merged: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Merged { asked_in, .. } => Some(asked_in.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(merged, [Some(second.id.to_string())]);
    flow.stop().await;
}

/// The thread's one short opening line reaches the user; a later line that only announces
/// a lookup stays hidden, as before.
#[tokio::test]
async fn the_requests_opening_line_shows_and_later_narration_does_not() {
    let flow = Flow::start(
        "card-opening",
        Options::default(),
        script(|turn| async move {
            let say = |text: &str| ProviderEvent::Message {
                item_id: uuid::Uuid::new_v4().to_string(),
                role: Role::Assistant,
                text: text.into(),
            };
            let search = || ProviderEvent::ToolCall {
                item_id: uuid::Uuid::new_v4().to_string(),
                name: "mcp__brigadier__code_search".into(),
                input: Some("{}".into()),
                status: ItemStatus::InProgress,
                output: None,
            };
            turn.emit(say(
                "I'll check how the tabs work today, then ask you a few questions.",
            ))
            .await;
            turn.emit(search()).await;
            turn.emit(say("Let me check the tab bar next.")).await;
            turn.emit(search()).await;
            Reply::text("The tabs keep their order across restarts.")
        }),
    )
    .await;
    flow.say("How do the tabs work?").await;
    flow.settled().await;
    let shown: Vec<String> = flow
        .core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| message.text)
        .collect();
    assert!(
        shown
            .iter()
            .any(|text| text.starts_with("I'll check how the tabs work")),
        "{shown:#?}"
    );
    assert!(
        !shown.iter().any(|text| text.contains("tab bar next")),
        "{shown:#?}"
    );
    assert!(
        shown.iter().any(|text| text.contains("keep their order")),
        "{shown:#?}"
    );
    flow.stop().await;
}

/// A card left open for an earlier request doesn't spare a later one: its text question still
/// gets the note.
#[tokio::test]
async fn an_earlier_requests_open_card_doesnt_spare_a_later_text_question() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "card-guard-scope",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.input.contains("Plan the export.") {
                    let asked = turn.call("ask_user", round()).await;
                    assert!(!asked.is_error, "{}", asked.text);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("asked the user a question in text") {
                    return Reply::text("[quiet]");
                }
                Reply::text("Should the theme follow the system?")
            }
        }),
    )
    .await;
    flow.say("Plan the export.").await;
    flow.until("the card", |board| open_card(board, false).is_some())
        .await;
    flow.settled().await;
    flow.say("Add a dark mode.").await;
    flow.until("the note", |_| {
        inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("asked the user a question in text"))
    })
    .await;
    flow.stop().await;
}

/// A "Not yet" on the merge card after the user's message revokes what that message said:
/// finish_session refuses the words it quoted.
#[tokio::test]
async fn a_not_yet_on_the_card_revokes_the_words_before_it() {
    let calls: Calls = Arc::default();
    let log = calls.clone();
    let flow = Flow::start(
        "card-merge-revoked",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.input.contains("Add notes and merge it.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    assert!(call(&turn, &log, "propose_merge", json!({})).await);
                    return Reply::text("[quiet]");
                }
                if turn.input.contains("doesn't want") {
                    assert!(
                        !call(
                            &turn,
                            &log,
                            "finish_session",
                            json!({ "user_words": "merge it" })
                        )
                        .await
                    );
                    return Reply::text("It stays on its branch.");
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    flow.say("Add notes and merge it.").await;
    let board = flow
        .until("the merge card", |board| open_card(board, true).is_some())
        .await;
    let card = open_card(&board, true).unwrap();
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["Not yet".into()],
        )
        .await
        .unwrap();
    flow.until("the refusal", |_| calls.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    assert!(!on_main(&flow, "NOTES.md"), "nothing merged after Not yet");
    let calls = calls.lock().unwrap().clone();
    assert!(calls[1].1.contains("Not yet"), "{calls:#?}");
    flow.stop().await;
}
