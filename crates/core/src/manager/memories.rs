//! A Chat's own tools: `save_memory` keeps a preference in the Personal Brain, shown in the Chat
//! as a Memory chip, and the user's memories go into the instructions of Chats and threads.

use brigadier_brain::{NewNode, NodeKind, Origin, Provenance};

use super::brains::{blake_key, brain_error, one_line};
use super::{SessionManager, blocking};
use crate::knowledge::MemoryChange;
use crate::model::{ConversationId, DomainEvent};
use crate::tools::{ChatCall, SaveMemory, ToolReply};
use crate::{Error, Result, now_ms};

/// The user's memories a Chat's or orchestrator's instructions carry, in bytes.
pub(super) const MEMORY_BYTES: usize = 6_000;

impl SessionManager {
    /// A Chat's tool call.
    pub(super) async fn chat_call(&self, id: ConversationId, call: ChatCall) -> ToolReply {
        let result = match call {
            ChatCall::SaveMemory(args) => self.save_memory(&id, args).await,
            ChatCall::SaveReport(args) => self.save_report(args).await,
        };
        match result {
            Ok(text) => ToolReply::ok(text),
            Err(err) => ToolReply::error(err.to_string()),
        }
    }

    /// `save_memory`: a preference in the Personal Brain, and a Memory chip in the Chat.
    ///
    /// Keyed per Chat: saving it again in the same Chat updates it in place, while the same
    /// memory saved in another Chat is a node of its own, so forgetting one Chat's never
    /// removes what another Chat saved.
    async fn save_memory(&self, id: &ConversationId, args: SaveMemory) -> Result<String> {
        let text = one_line(&args.memory, 240);
        if text.is_empty() {
            return Err(Error::Invalid("say what to remember in `memory`".into()));
        }
        if let Some(reason) = super::memory_guard::not_for_memory(&text) {
            return Err(Error::Invalid(format!(
                "not kept: {reason}. Never keep passwords, keys, card or account numbers; say nothing about it."
            )));
        }
        let brain = self.personal_brain().await?;
        let node = NewNode {
            kind: NodeKind::Preference,
            key: Some(format!(
                "preference:{}:{}",
                id.0,
                blake_key(&text.to_lowercase())
            )),
            title: text.clone(),
            body: String::new(),
            provenance: Provenance {
                origin: Origin::User,
                session_id: Some(id.0.clone()),
                task_id: None,
                job_id: None,
                worker: None,
                commit: None,
                recorded_at_ms: now_ms(),
                claim: None,
            },
            files: Vec::new(),
            expires_at_ms: None,
        };
        let node_id = blocking(move || brain.record(node).map_err(brain_error)).await?;
        let request_id = match self.conv(id) {
            Ok(conv) => conv.running_request().await,
            Err(_) => None,
        };
        self.core
            .record_conversation(
                id,
                vec![DomainEvent::MemoryUpdated {
                    conversation_id: id.clone(),
                    memory: MemoryChange {
                        node_id,
                        text,
                        forgotten: false,
                        request_id,
                        at_ms: now_ms(),
                    },
                }],
            )
            .await?;
        Ok("Saved. The user sees it and can remove it.".into())
    }

    /// The user's memories, newest first, within `max_bytes`, as lines for instructions.
    /// The same memory saved in several Chats is one line.
    pub(super) async fn memory_lines(&self, max_bytes: usize) -> Vec<String> {
        let Ok(memories) = self.list_memories().await else {
            return Vec::new();
        };
        let mut seen = std::collections::HashSet::new();
        let mut used = 0;
        memories
            .into_iter()
            .map(|node| match node.body.trim() {
                "" => node.title,
                body => format!("{}: {}", node.title, one_line(body, 300)),
            })
            .filter(|line| seen.insert(line.to_lowercase()))
            .take_while(|line| {
                used += line.len() + 3;
                used <= max_bytes
            })
            .collect()
    }
}
