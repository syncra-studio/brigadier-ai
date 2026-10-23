//! A Chat's flow helpers: a new Chat, what it showed, and its answer. A Chat is a thread
//! without a project (Q76): its CLI has its own shell at its level, and it may start workers
//! (S28).

use brigadier_providers::ProviderKind;

use super::Flow;
use crate::model::{
    ConversationId, ConversationKind, MessageRole, ModelChoice, PermissionLevel, SetupRequest,
};
use crate::work::RequestState;

/// A new Chat on Claude.
pub(super) async fn new_chat(flow: &Flow) -> ConversationId {
    new_chat_at(flow, PermissionLevel::default()).await
}

/// A new Chat on Claude at `permission`.
pub(super) async fn new_chat_at(flow: &Flow, permission: PermissionLevel) -> ConversationId {
    flow.manager
        .create_conversation(
            ConversationKind::Chat,
            None,
            Some("Chat".into()),
            Some(SetupRequest::Chat {
                model: ModelChoice {
                    provider: ProviderKind::Claude,
                    model: Some("claude-opus-5-5".into()),
                    effort: None,
                    fast: None,
                    account: None,
                },
                permission,
            }),
        )
        .await
        .unwrap()
        .id
}

/// What the Chat showed the user, in order.
pub(super) async fn shown(flow: &Flow, chat: &ConversationId) -> Vec<String> {
    flow.core
        .all_messages(chat)
        .await
        .unwrap()
        .into_iter()
        .filter(|message| message.role == MessageRole::Assistant)
        .map(|message| message.text)
        .collect()
}

/// Waits until every request of `chat` is done.
pub(super) async fn answered(flow: &Flow, chat: &ConversationId) -> crate::board::Board {
    flow.until_in(chat, "the Chat to answer", |board| {
        !board.requests.is_empty()
            && board
                .requests
                .values()
                .all(|request| request.state == RequestState::Done)
    })
    .await
}
