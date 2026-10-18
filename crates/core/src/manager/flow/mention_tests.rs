//! What a message's mentions of files and folders tell the thread.

use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::Mention;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

#[tokio::test]
async fn a_mentioned_folder_is_named_as_a_folder_and_a_file_as_a_file() {
    let seen: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = seen.clone();
    let flow = Flow::start(
        "mention-folder",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    log.lock().unwrap().push(turn.input.clone());
                }
                Reply::text("ok")
            }
        }),
    )
    .await;
    flow.manager
        .send_message(
            flow.conversation.clone(),
            "look at @src/lib and @src/lib/a.rs".into(),
            Vec::new(),
            vec![
                Mention::Folder {
                    path: "src/lib".into(),
                },
                Mention::File {
                    path: "src/lib/a.rs".into(),
                },
            ],
            false,
            None,
        )
        .await
        .unwrap();
    flow.settled().await;
    let seen = seen.lock().unwrap().clone();
    let input = seen.first().expect("the thread had a turn");
    assert!(input.contains("[mentions the folder src/lib]"), "{input}");
    assert!(
        input.contains("[mentions the file src/lib/a.rs]"),
        "{input}"
    );
    flow.stop().await;
}
