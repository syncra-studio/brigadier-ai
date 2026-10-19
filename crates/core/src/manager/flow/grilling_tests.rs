//! Grilling on cards (Q78): asked to be grilled on a plan, the thread looks the facts up first
//! (a research worker, in a Chat too), then asks in numbered, titled rounds on the same
//! question cards, each question with a recommended option, a researched fact shaping the
//! question; nothing is acted on until the user confirms the summary on a last card.

use std::sync::Arc;

use serde_json::json;

use super::chat_thread_tests::{answered, new_chat, shown};
use super::{Flow, Options, Reply, Script, Turn};
use crate::model::ConversationId;
use crate::work::TaskKind;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const DONE: &str = "Agreed: a small shop near the market, PETG and PLA first. Next: I'll draft the budget when you ask.";

/// The thread's script: research first, then round 1 with what it found, then the confirm
/// round, then the closing line. Workers report the rent they found.
fn grilling() -> Script {
    script(|turn| async move {
        if turn.chat().is_none() && !turn.is_orchestrator() {
            turn.call(
                "submit_report",
                json!({"summary": "Shop rent in Chisinau: about 9 EUR per m² near the central market, 15 in the centre."}),
            )
            .await;
            return Reply::text("Reported.");
        }
        if turn.input.contains("[report task-1") {
            assert!(turn.input.contains("9 EUR per m²"), "{}", turn.input);
            let asked = turn
                .call(
                    "ask_user",
                    json!({"title": "Round 1 · Location", "questions": [
                        {"question": "Rent near the central market is about 9 EUR/m², the centre 15. Where should the shop be?",
                         "options": [
                            {"label": "Near the market", "description": "Recommended: lowest rent, foot traffic from makers"},
                            {"label": "City centre", "description": "More walk-ins, nearly twice the rent"}],
                         "recommended": 0},
                        {"question": "Which filaments do you sell first?",
                         "options": [{"label": "PETG and PLA", "description": "Recommended: what most people print"},
                                     {"label": "Everything"}],
                         "recommended": 0}]}),
                )
                .await;
            assert!(!asked.is_error, "{}", asked.text);
            return Reply::text("[quiet]");
        }
        if turn.input.contains("[answer]") && turn.input.contains("Yes, that's it") {
            return Reply::text(DONE);
        }
        if turn.input.contains("[answer]") && turn.input.contains("Near the market") {
            let asked = turn
                .call(
                    "ask_user",
                    json!({"title": "Round 2 · Confirm", "questions": [
                        {"question": "So: a small shop near the market, PETG and PLA first. Is that right?",
                         "options": [{"label": "Yes, that's it"}, {"label": "Not quite"}],
                         "recommended": 0}]}),
                )
                .await;
            assert!(!asked.is_error, "{}", asked.text);
            return Reply::text("[quiet]");
        }
        let started = turn
            .call(
                "delegate_task",
                json!({"effort": "low", "title": "Shop rent", "kind": "research",
                       "spec": "Find current shop rent per m² in Chisinau by area."}),
            )
            .await;
        assert!(!started.is_error, "{}", started.text);
        Reply::text("[quiet]")
    })
}

async fn the_open_card(flow: &Flow, id: &ConversationId, what: &str) -> crate::work::Question {
    let board = flow
        .until_in(id, what, |board| {
            board.questions.values().any(|question| question.is_open())
        })
        .await;
    board
        .questions
        .values()
        .find(|question| question.is_open())
        .unwrap()
        .clone()
}

async fn a_grill_round_on_cards(chat: bool) {
    let flow = Flow::start(
        if chat { "grill-chat" } else { "grill-session" },
        Options::default(),
        grilling(),
    )
    .await;
    let id = if chat {
        new_chat(&flow).await
    } else {
        flow.conversation.clone()
    };
    flow.say_to(
        &id,
        "Grill me on my plan to open a filament shop in Chisinau.",
    )
    .await;
    let round = the_open_card(&flow, &id, "round 1").await;
    let board = flow.core.board(&id).await.unwrap();
    // The fact was looked up first, and shapes the question.
    let task = board.tasks.values().next().unwrap();
    assert_eq!(task.kind, TaskKind::Research);
    assert!(
        task.report.is_some(),
        "the research came back before the card"
    );
    assert_eq!(round.title.as_deref(), Some("Round 1 · Location"));
    assert_eq!(round.items.len(), 2);
    assert!(
        round.items[0].text.contains("9 EUR/m²"),
        "{}",
        round.items[0].text
    );
    assert!(round.items.iter().all(|item| item.recommended == Some(0)));
    flow.manager
        .answer_question(
            id.clone(),
            round.id.clone(),
            vec!["Near the market".into(), "PETG and PLA".into()],
            None,
        )
        .await
        .unwrap();
    let confirm = the_open_card(&flow, &id, "the confirm round").await;
    assert_eq!(confirm.title.as_deref(), Some("Round 2 · Confirm"));
    // Nothing acted on before the confirm: the one worker is the research.
    assert_eq!(flow.core.board(&id).await.unwrap().tasks.len(), 1);
    flow.manager
        .answer_question(
            id.clone(),
            confirm.id.clone(),
            vec!["Yes, that's it".into()],
            None,
        )
        .await
        .unwrap();
    let board = answered(&flow, &id).await;
    assert_eq!(board.tasks.len(), 1);
    assert_eq!(board.questions.len(), 2);
    assert_eq!(
        shown(&flow, &id).await.last().map(String::as_str),
        Some(DONE)
    );
    let rule = "map the decisions it needs as a tree";
    assert!(
        flow.specs()
            .into_iter()
            .filter_map(|(_, spec)| spec.append_system_prompt)
            .any(|prompt| prompt.contains(rule)),
        "the instructions hold the interview"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_chat_grills_on_cards_in_numbered_rounds_after_looking_facts_up() {
    a_grill_round_on_cards(true).await;
}

#[tokio::test]
async fn a_session_grills_on_cards_in_numbered_rounds_after_looking_facts_up() {
    a_grill_round_on_cards(false).await;
}
