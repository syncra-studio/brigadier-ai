//! Fusion offers start nothing until a user clicks; only top-level Chats can make them.

use std::sync::Arc;

use brigadier_cli::protocol::{CliCall, CliReply, CliRequest, ErrorCode, PROTOCOL, Tool};
use brigadier_providers::ProviderKind;
use serde_json::json;

use super::chat_thread_tests::{answered, new_chat};
use super::cli_tests::brigadier;
use super::{Flow, Options, Reply, Script};
use crate::model::{
    ConversationId, ConversationKind, DomainEvent, ModelChoice, PermissionLevel, SetupRequest,
    TabTarget,
};
use crate::tools::{OfferFusion, OrchestratorCall, Role, TabTools, ToolCall, ToolHost};
use crate::work::{OrchestratorStep, OrchestratorStepKind, TaskId};

fn offer(question: &str) -> OfferFusion {
    OfferFusion {
        question: question.into(),
    }
}
fn quiet() -> Script {
    Arc::new(|_| Box::pin(async { Reply::text("Noted.") }))
}
async fn offered(flow: &Flow, chat: &ConversationId) -> String {
    flow.core
        .board(chat)
        .await
        .unwrap()
        .orchestrator_steps
        .iter()
        .find_map(|step| match &step.kind {
            OrchestratorStepKind::FusionOffered { id, .. } => Some(id.clone()),
            _ => None,
        })
        .expect("an offer")
}

#[tokio::test]
async fn fusion_offer_waits_for_a_click_and_concurrent_clicks_send_once() {
    let script: Script = Arc::new(|turn| {
        Box::pin(async move {
            if turn.input.contains("Compare approaches.") && !turn.input.contains("/fusion") {
                let reply = brigadier(
                    &turn,
                    CliRequest::Tool {
                        tool: Tool::Fusion,
                        action: "offer".into(),
                        args: json!({"question": "Compare approaches."}),
                    },
                )
                .await;
                assert_eq!(reply["action"], "offer", "{reply}");
            }
            Reply::text("Here is my answer.")
        })
    });
    let flow = Flow::start("fusion-offer-click", Options::default(), script).await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Compare approaches.").await;
    let before = answered(&flow, &chat).await;
    assert!(before.tasks.is_empty());
    let messages = flow.core.all_messages(&chat).await.unwrap().len();
    let id = offered(&flow, &chat).await;
    assert!(
        flow.manager
            .offer_fusion(&chat, offer("A different question."))
            .await
            .unwrap_err()
            .to_string()
            .contains("already offered")
    );
    let (first, second) = tokio::join!(
        flow.manager.start_offered_fusion(&chat, &id),
        flow.manager.start_offered_fusion(&chat, &id)
    );
    assert_ne!(first.is_ok(), second.is_ok());
    answered(&flow, &chat).await;
    let board = flow.core.board(&chat).await.unwrap();
    assert_eq!(board.orchestrator_steps.iter().filter(|step| matches!(&step.kind, OrchestratorStepKind::FusionOfferTaken { id: taken } if taken == &id)).count(), 1);
    let sent = flow.core.all_messages(&chat).await.unwrap();
    assert!(sent.len() > messages);
    assert_eq!(
        sent.iter()
            .filter(|message| message.text == "/fusion Compare approaches.")
            .count(),
        1
    );
    assert!(
        flow.manager
            .start_offered_fusion(&chat, &id)
            .await
            .unwrap_err()
            .to_string()
            .contains("already asked")
    );
    flow.stop().await;
}

#[tokio::test]
async fn fusion_invocations_use_the_selected_cli_and_keep_the_normal_queue() {
    let flow = Flow::start("fusion-invocations", Options::default(), quiet()).await;
    for provider in ProviderKind::ALL {
        let chat = flow
            .manager
            .create_conversation(
                ConversationKind::Chat,
                None,
                None,
                Some(SetupRequest::Chat {
                    model: ModelChoice {
                        provider,
                        model: None,
                        effort: None,
                        fast: None,
                        account: None,
                    },
                    permission: PermissionLevel::Full,
                }),
            )
            .await
            .unwrap()
            .id;
        let expected = match provider {
            ProviderKind::Claude => "/fusion Which approach?",
            ProviderKind::Codex => "$fusion Which approach?",
        };
        assert_eq!(
            flow.manager
                .fusion_message(&chat, "Which approach?".into())
                .unwrap(),
            expected
        );
        assert_eq!(
            flow.manager
                .fusion_message(&chat, "/fusion Which approach?".into())
                .unwrap(),
            expected
        );
        assert_eq!(
            flow.manager
                .fusion_message(&chat, "$fusion Which approach?".into())
                .unwrap(),
            expected
        );
        flow.manager
            .offer_fusion(&chat, offer("Which approach?"))
            .await
            .unwrap();
        flow.core.set_queue_paused(&chat, true).await.unwrap();
        // A queue slot while paused keeps a typed invocation in the same paused lane.
        flow.manager
            .send_message(
                chat.clone(),
                "/fusion Which approach?".into(),
                vec![],
                vec![],
                false,
                Some(0),
            )
            .await
            .unwrap();
        let board = flow.core.board(&chat).await.unwrap();
        assert_eq!(board.queue.items.len(), 1);
        assert_eq!(board.queue.items[0].text, expected);
        assert!(flow.core.all_messages(&chat).await.unwrap().is_empty());
    }
    flow.stop().await;
}

#[tokio::test]
async fn fusion_is_refused_in_sessions_side_chats_tab_chats_and_workers() {
    let flow = Flow::start("fusion-scope", Options::default(), quiet()).await;
    let chat = new_chat(&flow).await;
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
            "browser:fusion".into(),
            TabTarget::Browser,
            ConversationId(uuid::Uuid::new_v4().to_string()),
        )
        .await
        .unwrap()
        .id;
    for id in [&flow.conversation, &side, &tab] {
        assert!(
            flow.manager
                .offer_fusion(id, offer("Which approach?"))
                .await
                .unwrap_err()
                .to_string()
                .contains("not available")
        );
        assert!(flow.manager.fusion_message(id, "Question".into()).is_err());
        assert!(
            flow.manager
                .start_offered_fusion(id, "unknown")
                .await
                .is_err()
        );
        assert!(
            flow.manager
                .send_message(
                    id.clone(),
                    "/fusion Question".into(),
                    vec![],
                    vec![],
                    false,
                    None
                )
                .await
                .is_err()
        );
    }
    let grant = flow.manager.grants.issue(
        "test:fusion-worker",
        Role::Worker {
            conversation_id: chat.clone(),
            task_id: TaskId("worker".into()),
        },
    );
    let reply = ToolHost::call(
        &*flow.manager,
        &grant,
        ToolCall::Orchestrator(OrchestratorCall::OfferFusion(offer("Question"))),
    )
    .await;
    assert!(reply.is_error && reply.text.contains("not available"));
    for (id, role) in [
        (
            &flow.conversation,
            Role::Orchestrator {
                conversation_id: flow.conversation.clone(),
            },
        ),
        (
            &side,
            Role::Chat {
                conversation_id: side.clone(),
            },
        ),
        (
            &tab,
            Role::TabChat {
                conversation_id: tab.clone(),
                tools: TabTools::Page,
            },
        ),
        (
            &chat,
            Role::Worker {
                conversation_id: chat.clone(),
                task_id: TaskId("worker".into()),
            },
        ),
    ] {
        let caller_id = if matches!(role, Role::Worker { .. }) {
            "worker"
        } else {
            &id.0
        };
        let grant = flow.manager.grants.issue("test:fusion-scope", role);
        let caller = flow.manager.cli_caller(caller_id, &grant).unwrap();
        let reply = flow
            .manager
            .cli_call(
                &caller,
                CliCall {
                    protocol: PROTOCOL,
                    request: CliRequest::Tool {
                        tool: Tool::Fusion,
                        action: "offer".into(),
                        args: json!({"question": "Question"}),
                    },
                },
            )
            .await;
        assert!(
            matches!(reply, CliReply::Error { ref error } if error.code == ErrorCode::Refused),
            "{reply:?}"
        );
    }
    assert!(flow.manager.offer_fusion(&chat, offer("  ")).await.is_err());
    assert!(
        flow.manager
            .start_offered_fusion(&chat, "unknown")
            .await
            .is_err()
    );
    flow.manager.archive(chat.clone()).await.unwrap();
    assert!(
        flow.manager
            .offer_fusion(&chat, offer("Question"))
            .await
            .is_err()
    );
    assert!(
        flow.manager
            .fusion_message(&chat, "Question".into())
            .is_err()
    );
    flow.stop().await;
}

#[tokio::test]
async fn fusion_stale_or_edited_away_offers_start_nothing() {
    let flow = Flow::start("fusion-stale", Options::default(), quiet()).await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Compare approaches.").await;
    let board = answered(&flow, &chat).await;
    let request = board.latest_request().unwrap().id.clone();
    for (id, request, at_ms) in [
        ("stale", request, 0),
        ("edited", "edited-away".into(), crate::now_ms()),
    ] {
        flow.core
            .record_conversation(
                &chat,
                vec![DomainEvent::OrchestratorStepped {
                    step: OrchestratorStep {
                        request_id: Some(request),
                        kind: OrchestratorStepKind::FusionOffered {
                            id: id.into(),
                            question: "Which approach?".into(),
                        },
                        at_ms,
                        position: 0,
                    },
                }],
            )
            .await
            .unwrap();
        assert!(
            flow.manager
                .start_offered_fusion(&chat, id)
                .await
                .unwrap_err()
                .to_string()
                .contains("earlier answer")
        );
    }
    assert!(
        !flow
            .core
            .board(&chat)
            .await
            .unwrap()
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, OrchestratorStepKind::FusionOfferTaken { .. }))
    );
    flow.stop().await;
}
