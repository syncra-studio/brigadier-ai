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

/// A 1×1 PNG: only a real image is kept and read back.
pub(super) const PNG: &[u8] = &[
    137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0,
    0, 0, 31, 21, 196, 137, 0, 0, 0, 13, 73, 68, 65, 84, 120, 218, 99, 100, 248, 207, 80, 15, 0, 3,
    134, 1, 128, 90, 52, 125, 107, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
];

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

/// The Chat's image row, image and kept copy: the "Created an image" row in the turn, its copy
/// kept in the Chat's own `thread-images/<id>`, read back once Codex's own file is gone.
/// Returns the kept copy.
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
    let own = flow.dir.join("data/thread-images").join(&chat.0);
    let kept = std::fs::read_dir(&own)
        .expect("the Chat keeps its images")
        .flatten()
        .map(|entry| entry.path())
        .next()
        .expect("the image is kept");
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

/// An image the thread views and one its answer names are copied as they are recorded: closing
/// the worker that made them, which removes its temp folder, doesn't take them away. A path that
/// is no image is refused, and an image of this thread is not one for another conversation.
#[tokio::test]
async fn images_a_thread_views_and_answers_with_outlast_their_files() {
    let folder = std::env::temp_dir().join(format!("worker-temp-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&folder).unwrap();
    let (viewed, named, notes) = (
        folder.join("viewed.png"),
        folder.join("named one.png"),
        folder.join("notes.png"),
    );
    for file in [&viewed, &named] {
        std::fs::write(file, PNG).unwrap();
    }
    std::fs::write(&notes, "text").unwrap();
    let (input, answer) = (
        serde_json::json!({ "path": viewed }).to_string(),
        format!(
            "Here it is:\n\n![Named](<{}>)\n\n![Notes]({})",
            named.display(),
            notes.display()
        ),
    );
    let flow = Flow::start(
        "thread-images-outlast",
        Options::default(),
        script(move |turn| {
            let (input, answer) = (input.clone(), answer.clone());
            async move {
                if turn.chat().is_none() {
                    return Reply::text("ok");
                }
                turn.emit(ProviderEvent::ToolCall {
                    item_id: "view_1".into(),
                    name: "view_image".into(),
                    input: Some(input),
                    status: ItemStatus::Completed,
                    output: None,
                    kept: None,
                })
                .await;
                Reply::text(&answer)
            }
        }),
    )
    .await;
    let chat = codex_chat(&flow).await;
    flow.say_to(&chat, "Show me.").await;
    answered(&flow, &chat).await;
    std::fs::remove_dir_all(&folder).unwrap();
    let (data, mime) = flow
        .manager
        .read_thread_image(&chat, "view_1")
        .await
        .unwrap();
    assert_eq!(
        (data.as_str(), mime.as_str()),
        (brigadier_providers::image_base64(PNG).as_str(), "image/png")
    );
    let (data, _) = flow
        .manager
        .read_local_image(&chat, &named.to_string_lossy())
        .await
        .unwrap();
    assert_eq!(data, brigadier_providers::image_base64(PNG));
    // Nothing was kept of the text, and a relative path is refused.
    assert!(
        flow.manager
            .read_local_image(&chat, &notes.to_string_lossy())
            .await
            .is_err()
    );
    assert!(
        flow.manager
            .read_local_image(&chat, "picture.png")
            .await
            .is_err()
    );
    // Another conversation has no copy, and the original has gone.
    assert!(
        flow.manager
            .read_local_image(&flow.conversation, &named.to_string_lossy())
            .await
            .is_err()
    );
    // The copies go with the thread.
    let own = flow.dir.join("data/thread-images").join(&chat.0);
    assert_eq!(std::fs::read_dir(&own).unwrap().count(), 2);
    images_go_with(&flow, &chat, &own).await;
}
