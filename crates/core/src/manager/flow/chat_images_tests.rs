//! A Chat makes images (Q24, issue #391): on Codex with Codex's own image generation, shown,
//! kept and removed as a session thread's are; on Claude by handing the work to a Codex worker,
//! whose image reaches the Chat as the task's output.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind, ToolSet};

use super::chat_thread_tests::answered;
use super::{Flow, Options, Reply, Script, Turn};
use crate::model::{ConversationId, ConversationKind, ModelChoice, SetupRequest, TabTarget};
use crate::work::{OrchestratorStepKind, ThreadItemKind};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const PNG: &[u8] = b"a sunset";

/// Codex's image generation in a turn: the item starts, then completes with the file Codex
/// saved, as the app-server reports it.
async fn generate(turn: &Turn, item: &str, file: &Path) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, PNG).unwrap();
    for (status, path) in [
        (ItemStatus::InProgress, None),
        (ItemStatus::Completed, Some(file.display().to_string())),
    ] {
        turn.emit(ProviderEvent::Image {
            item_id: item.into(),
            status,
            path,
            prompt: Some("A sunset over the sea".into()),
        })
        .await;
    }
}

/// A worker's outputs folder, as its brief names it.
fn outputs_folder(turn: &Turn) -> PathBuf {
    const SAYS: &str = "Your outputs folder (files for the thread and the user): ";
    let text = format!("{}\n{}", turn.prompt, turn.input);
    let rest = &text[text.find(SAYS).expect("the brief names the outputs folder") + SAYS.len()..];
    PathBuf::from(rest.lines().next().unwrap().trim())
}

async fn codex_chat(flow: &Flow) -> ConversationId {
    flow.manager
        .create_conversation(
            ConversationKind::Chat,
            None,
            Some("Pictures".into()),
            Some(SetupRequest::Chat {
                model: ModelChoice {
                    provider: ProviderKind::Codex,
                    model: None,
                    effort: None,
                    fast: None,
                    account: None,
                },
                permission: Default::default(),
            }),
        )
        .await
        .unwrap()
        .id
}

/// The Chat's image row, image and kept copy: the "Created an image" row in the turn, the
/// logged event pointing at the Chat's own `thread-images/<id>`, read back once Codex's own
/// file is gone. Returns the kept copy.
async fn image_is_shown(flow: &Flow, chat: &ConversationId, source: &Path) -> PathBuf {
    let board = answered(flow, chat).await;
    let request = board.latest_request().unwrap();
    assert!(
        board.orchestrator_steps.iter().any(|step| {
            step.request_id.as_deref() == Some(&request.id)
                && matches!(&step.kind, OrchestratorStepKind::Tool {
                    name, detail, status: ItemStatus::Completed, item: Some(ThreadItemKind::Image),
                    turn: Some(_), ..
                } if name == "imagegen" && detail.as_deref() == Some("A sunset over the sea"))
        }),
        "{:#?}",
        board.orchestrator_steps
    );
    let items = flow
        .manager
        .thread_items(chat, &request.id, None, None, 100)
        .await
        .unwrap();
    let kept = items
        .entries
        .iter()
        .find_map(|entry| match &entry.event {
            ProviderEvent::Image {
                status: ItemStatus::Completed,
                path,
                ..
            } => path.clone(),
            _ => None,
        })
        .map(PathBuf::from)
        .expect("the image's event is logged");
    let own = flow.dir.join("data/thread-images").join(&chat.0);
    assert!(kept.starts_with(&own), "{}", kept.display());
    std::fs::remove_file(source).unwrap();
    let (data, mime) = flow.manager.read_thread_image(chat, "ig_1").await.unwrap();
    assert_eq!(data, brigadier_providers::image_base64(PNG));
    assert_eq!(mime, "image/png");
    // Another conversation can't open it.
    assert!(
        flow.manager
            .read_thread_image(&flow.conversation, "ig_1")
            .await
            .is_err()
    );
    kept
}

/// The Chat is deleted: its kept images go with it.
async fn images_go_with(flow: &Flow, chat: &ConversationId, kept: &Path) {
    flow.manager.delete(chat.clone()).await.unwrap();
    flow.manager.cleanup_finished(chat).await;
    super::eventually("the Chat to go", || flow.core.conversation(chat).is_err()).await;
    assert!(!kept.exists(), "{}", kept.display());
    assert!(!flow.dir.join("data/thread-images").join(&chat.0).exists());
}

/// A Chat on Codex runs with Codex's image generation on (its other tools as before), and its
/// image shows, is kept and goes with it as a session thread's does.
#[tokio::test]
async fn a_codex_chat_makes_an_image_shown_kept_and_removed_like_a_threads() {
    let flow = Flow::start(
        "chat-images-codex",
        Options::default(),
        script(|turn| async move {
            if turn.chat().is_none() {
                return Reply::text("ok");
            }
            let source = turn.cwd.join("generated_images/ig_1.png");
            generate(&turn, "ig_1", &source).await;
            Reply::text("Here is your sunset.")
        }),
    )
    .await;
    let chat = codex_chat(&flow).await;
    flow.say_to(&chat, "Draw me a sunset.").await;
    answered(&flow, &chat).await;
    let source = flow
        .specs()
        .into_iter()
        .find(|(provider, spec)| {
            *provider == ProviderKind::Codex && spec.cwd == flow.manager.session_dir(&chat).work()
        })
        .map(|(_, spec)| spec.cwd.join("generated_images/ig_1.png"))
        .expect("the Chat runs on Codex in its own folder");
    let kept = image_is_shown(&flow, &chat, &source).await;
    images_go_with(&flow, &chat, &kept).await;
    flow.stop().await;
}

/// A session's tab chat on Codex makes an image too, shown and removed with it.
#[tokio::test]
async fn a_codex_tab_chat_makes_an_image_and_it_goes_with_the_chat() {
    let flow = Flow::start(
        "chat-images-tab",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(|turn| async move {
            if !turn.prompt.contains("You are a chat beside") {
                return Reply::text("ok");
            }
            let source = turn.cwd.join("generated_images/ig_1.png");
            generate(&turn, "ig_1", &source).await;
            Reply::text("Here is your sunset.")
        }),
    )
    .await;
    flow.manager.keep_tab_chats(Vec::new()).await.unwrap();
    std::fs::write(flow.repo.join("notes.txt"), "one\n").unwrap();
    let chat = flow
        .manager
        .open_tab_chat(
            &flow.conversation,
            "file:notes.txt".into(),
            TabTarget::File {
                path: "notes.txt".into(),
            },
            ConversationId(uuid::Uuid::new_v4().to_string()),
        )
        .await
        .unwrap()
        .id;
    flow.say_to(&chat, "Draw me a sunset.").await;
    answered(&flow, &chat).await;
    let source = flow
        .specs()
        .into_iter()
        .find(|(provider, spec)| *provider == ProviderKind::Codex && spec.tools == ToolSet::TabChat)
        .map(|(_, spec)| spec.cwd.join("generated_images/ig_1.png"))
        .expect("the tab chat runs on Codex with a tab chat's tools");
    let kept = image_is_shown(&flow, &chat, &source).await;
    images_go_with(&flow, &chat, &kept).await;
    flow.stop().await;
}
