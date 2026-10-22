//! The thread asks the user only on cards (THREAD-PARITY-PLAN.md §5): a round of questions
//! on one card, answered at once, inside the same request; a question asked in text is asked
//! again on a card. Merges are never asked on a card (merge_by_user_tests).

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::work::{AnswerInput, AnswerOrigin, Question, QuestionKind, RequestState};

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
                assert!(asked.text.contains("The answers arrive as an [answer] message"), "{}", asked.text);
                Reply::text("")
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
    // Waiting on its card, the request's work stops and the wait for the user runs on
    // (BUILD-PLAN A9).
    let request = board.latest_request().unwrap().clone();
    assert!(
        request.worked.iter().all(|span| span.to_ms.is_some()),
        "{request:#?}"
    );
    assert!(
        request
            .waited
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
            None,
        )
        .await;
    assert!(short.is_err());
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["CSV".into(), "Admins only".into(), "export.csv".into()],
            Some(AnswerOrigin {
                input: Some(AnswerInput::Click),
                client: "Brigadier".into(),
                pid: 4242,
            }),
        )
        .await
        .unwrap();
    flow.until("the answers' turn", |_| inputs.lock().unwrap().len() >= 2)
        .await;
    let board = flow.settled().await;
    let answered = &board.questions[&card.id];
    assert_eq!(answered.answers, ["CSV", "Admins only", "export.csv"]);
    // Where the answer came from is stored with it (BUILD-PLAN A5).
    assert_eq!(
        answered.answered_by,
        Some(AnswerOrigin {
            input: Some(AnswerInput::Click),
            client: "Brigadier".into(),
            pid: 4242,
        })
    );
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
    // Two work spans around one closed wait on the card: the wait isn't counted as work.
    assert_eq!(request.worked.len(), 2, "{:#?}", request.worked);
    assert_eq!(request.waited.len(), 1, "{:#?}", request.waited);
    assert!(request.waited[0].to_ms.is_some(), "{:#?}", request.waited);
    flow.stop().await;
}

/// A reply that asks the user something in text ends its turn like any other (G9, #517): no
/// note asks it again on a card, and the request is done, as an idle plain CLI is.
#[tokio::test]
async fn a_question_asked_in_text_ends_the_turn_with_no_note() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "card-guard",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                Reply::text("Which theme should be the default: dark or light?")
            }
        }),
    )
    .await;
    flow.say("Add a dark mode.").await;
    let board = flow.settled().await;
    assert_eq!(board.latest_request().unwrap().state, RequestState::Done);
    assert!(open_card(&board, false).is_none());
    let heard = inputs.lock().unwrap().clone();
    assert_eq!(heard.len(), 1, "{heard:#?}");
    flow.stop().await;
}
