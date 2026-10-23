//! The thread asks the user only on cards (THREAD-PARITY-PLAN.md §5): a round of questions
//! on one card, answered at once, inside the same request; a question asked in text is asked
//! again on a card. Merges are never asked on a card (merge_by_user_tests).

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::work::{Question, QuestionKind, RequestState};

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
