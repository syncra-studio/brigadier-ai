//! Personal memory (Q78): a stable fact the user states in a Chat is kept, shown in the
//! Chat, and used by every later Chat; the user's edit in Settings changes it everywhere,
//! with no earlier text left; their delete removes it. A password, key, card or account
//! number is never kept, whoever saves or edits it.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::chat_thread_tests::{answered, new_chat};
use super::{Flow, Options, Reply, Script, Turn};
use crate::Error;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// The Chats' script: it keeps what the user says of themselves, and tries to keep a password
/// (refused). `heard`: each turn's input.
fn keeping(heard: Arc<Mutex<Vec<String>>>) -> Script {
    script(move |turn| {
        let heard = heard.clone();
        async move {
            heard.lock().unwrap().push(turn.input.clone());
            if turn.input.contains("I live in Chisinau") {
                let saved = turn
                    .call(
                        "save_memory",
                        json!({"memory": "Lives in Chisinau, Moldova"}),
                    )
                    .await;
                assert!(!saved.is_error, "{}", saved.text);
                return Reply::text("Noted.");
            }
            if turn.input.contains("my password") {
                let saved = turn
                    .call(
                        "save_memory",
                        json!({"memory": "Their password is hunter2"}),
                    )
                    .await;
                assert!(saved.is_error, "{}", saved.text);
                assert!(saved.text.contains("not kept"), "{}", saved.text);
                return Reply::text("I won't keep that.");
            }
            Reply::text("Hello.")
        }
    })
}

/// The newest Chat's instructions.
fn latest_chat_prompt(flow: &Flow) -> String {
    flow.specs()
        .into_iter()
        .filter_map(|(_, spec)| spec.append_system_prompt)
        .rfind(|prompt| prompt.contains("This Chat"))
        .expect("a Chat's instructions")
}

#[tokio::test]
async fn a_memory_is_kept_edited_and_deleted_and_later_chats_use_it() {
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let flow = Flow::start(
        "personal-memory",
        Options::default(),
        keeping(heard.clone()),
    )
    .await;
    let first = new_chat(&flow).await;
    flow.say_to(&first, "I live in Chisinau and print PETG.")
        .await;
    let board = answered(&flow, &first).await;
    let memories = flow.manager.list_memories().await.unwrap();
    assert_eq!(memories.len(), 1);
    let memory = memories[0].clone();
    assert_eq!(memory.title, "Lives in Chisinau, Moldova");
    assert_eq!(board.memories.len(), 1, "the Chat shows its Memory chip");

    // A later Chat knows it from its first message.
    let second = new_chat(&flow).await;
    flow.say_to(&second, "Where can I buy filament?").await;
    answered(&flow, &second).await;
    assert!(latest_chat_prompt(&flow).contains("Lives in Chisinau, Moldova"));

    // Edited in Settings: the same memory, its Chat's chip and where it came from kept, and
    // the next Chat and the open one go by the new text.
    let edited = flow
        .manager
        .edit_memory(memory.id.clone(), "Lives in Iasi, Romania".into())
        .await
        .unwrap();
    assert_eq!(edited.id, memory.id);
    assert_eq!(edited.title, "Lives in Iasi, Romania");
    assert_eq!(edited.provenance.session_id, memory.provenance.session_id);
    let memories = flow.manager.list_memories().await.unwrap();
    assert_eq!(memories.len(), 1);
    assert_eq!(memories[0].title, "Lives in Iasi, Romania");
    let board = flow.core.board(&first).await.unwrap();
    assert_eq!(board.memories.len(), 1);
    assert_eq!(board.memories[0].text, "Lives in Iasi, Romania");
    let third = new_chat(&flow).await;
    flow.say_to(&third, "Any filament shops near me?").await;
    answered(&flow, &third).await;
    let prompt = latest_chat_prompt(&flow);
    assert!(prompt.contains("Lives in Iasi, Romania"), "{prompt}");
    assert!(!prompt.contains("Chisinau"), "{prompt}");
    flow.say_to(&second, "And for PLA?").await;
    answered(&flow, &second).await;
    let said = heard.lock().unwrap().last().cloned().unwrap();
    assert!(said.contains("Lives in Iasi, Romania"), "{said}");

    // An edit can't hold a secret, nor be empty.
    for text in ["Card 4111 1111 1111 1111", "  "] {
        let refused = flow
            .manager
            .edit_memory(memory.id.clone(), text.into())
            .await;
        assert!(matches!(refused, Err(Error::Invalid(_))), "{refused:?}");
    }
    assert_eq!(
        flow.manager.list_memories().await.unwrap()[0].title,
        "Lives in Iasi, Romania"
    );

    // Deleted: gone, the earlier text with it, and the next Chat knows nothing of it.
    flow.manager.forget_memory(memory.id.clone()).await.unwrap();
    assert!(flow.manager.list_memories().await.unwrap().is_empty());
    let board = flow.core.board(&first).await.unwrap();
    assert!(board.memories[0].forgotten);
    let fourth = new_chat(&flow).await;
    flow.say_to(&fourth, "Hello again.").await;
    answered(&flow, &fourth).await;
    let prompt = latest_chat_prompt(&flow);
    assert!(
        !prompt.contains("Iasi") && !prompt.contains("Chisinau"),
        "{prompt}"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_password_is_never_kept() {
    let heard: Arc<Mutex<Vec<String>>> = Arc::default();
    let flow = Flow::start("personal-memory-secret", Options::default(), keeping(heard)).await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Remember my password: hunter2").await;
    let board = answered(&flow, &chat).await;
    assert!(flow.manager.list_memories().await.unwrap().is_empty());
    assert!(board.memories.is_empty());
    let prompt = latest_chat_prompt(&flow);
    assert!(
        prompt.contains(
            "Never keep passwords, keys, card or account numbers, health or money details"
        )
    );
    flow.stop().await;
}
