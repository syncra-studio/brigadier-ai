//! Drawing scope and recovery from the CLIs' own files, without live output.

use std::sync::Arc;

use brigadier_providers::{ProviderEvent, ProviderKind, Role};
use serde_json::json;

use super::chat_thread_tests::new_chat;
use super::{Flow, Options, Reply};
use crate::model::{ConversationId, ConversationKind, TabTarget};

#[tokio::test]
async fn the_visuals_guide_is_only_available_in_top_level_chats() {
    let flow = Flow::start(
        "visuals-guide-scope",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("Noted.") })),
    )
    .await;
    let chat = new_chat(&flow).await;
    assert!(
        flow.manager
            .visuals_guide(&chat)
            .unwrap()
            .contains("brigadier-html")
    );
    let side = flow
        .manager
        .open_side_chat(&flow.conversation, None)
        .await
        .unwrap()
        .id;
    flow.manager.keep_tab_chats(vec![]).await.unwrap();
    let tab = flow
        .manager
        .open_tab_chat(
            &flow.conversation,
            "browser:visuals".into(),
            TabTarget::Browser,
            ConversationId(uuid::Uuid::new_v4().to_string()),
        )
        .await
        .unwrap()
        .id;
    for id in [&flow.conversation, &side, &tab] {
        assert!(
            flow.manager
                .visuals_guide(id)
                .unwrap_err()
                .to_string()
                .contains("top-level Chats only")
        );
    }
    flow.stop().await;
}

#[tokio::test]
async fn visual_fences_rebuild_from_both_cli_session_files_after_restart() {
    let mut flow = Flow::start(
        "visuals-file-rebuild",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("Noted.") })),
    )
    .await;
    let reply = "```brigadier-table\n{\"title\":\"Time\",\"columns\":[\"Route\"],\"rows\":[[\"Train\"]],\"source\":\"Illustrative\"}\n```\n\n```brigadier-chart\n{\"kind\":\"bar\",\"title\":\"Time\",\"description\":\"Minutes\",\"data\":[{\"label\":\"Train\",\"value\":35}]}\n```\n\n```brigadier-html\n<p>Recovered visual</p><script>document.querySelector('p').textContent='Ready';</script>\n```";
    let mut chats = Vec::new();
    for provider in ProviderKind::ALL {
        let chat = flow
            .manager
            .create_conversation(ConversationKind::Chat, None, None, None)
            .await
            .unwrap()
            .id;
        let record = flow.manager.session_dir(&chat).record();
        std::fs::create_dir_all(&record).unwrap();
        let file = flow.dir.join(format!("visuals-{provider:?}.jsonl"));
        let line = match provider {
            ProviderKind::Claude => {
                json!({"type":"assistant","uuid":"a1","timestamp":"2026-10-26T10:00:00Z",
                "message":{"id":"m1","role":"assistant","content":[{"type":"text","text":reply}]}})
            }
            ProviderKind::Codex => json!({"type":"event_msg","timestamp":"2026-10-26T10:00:00Z",
                "payload":{"type":"item_completed","item":{"type":"AgentMessage","id":"a1","phase":"final_answer","content":[{"type":"Text","text":reply}]}}}),
        };
        std::fs::write(&file, format!("{line}\n")).unwrap();
        std::fs::write(
            record.join("lineage.json"),
            json!([{"provider":provider,"sessionId":"visuals","file":file,"startedAt":1}])
                .to_string(),
        )
        .unwrap();
        let before = flow.manager.thread_view(&chat).unwrap();
        assert!(before.entries.iter().any(|entry| matches!(&entry.event, ProviderEvent::Message { role: Role::Assistant, text, .. } if text == reply)), "{provider:?}: {before:?}");
        chats.push((chat, before));
    }
    flow.restart().await;
    for (chat, before) in chats {
        assert_eq!(flow.manager.thread_view(&chat).unwrap(), before);
    }
    flow.stop().await;
}
