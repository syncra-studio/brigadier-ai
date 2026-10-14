//! What the flows of sessions that message each other share: one script for every session
//! of a manager, each session's turns answered by a handler of its own, and every input each
//! took.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use brigadier_providers::BoxFuture;
use serde_json::{Value, json};

use super::{Reply, Script, Turn};
use crate::model::ConversationId;

pub(super) type Handler = Arc<dyn Fn(Turn) -> BoxFuture<'static, Reply> + Send + Sync>;

/// The sessions' handlers and the inputs their turns took, in order.
#[derive(Default)]
pub(super) struct Peers {
    handlers: Mutex<HashMap<ConversationId, Handler>>,
    /// Answers every worker's turns, when set.
    worker: Mutex<Option<Handler>>,
    inputs: Mutex<Vec<(ConversationId, String)>>,
}

impl Peers {
    pub(super) fn new() -> Arc<Self> {
        Arc::default()
    }

    /// The script every CLI runs: a thread's turn goes to its session's handler (else it
    /// answers "Done."), after its input is logged; a worker's to the worker handler.
    pub(super) fn script(self: &Arc<Self>) -> Script {
        let peers = self.clone();
        Arc::new(move |turn: Turn| {
            let peers = peers.clone();
            Box::pin(async move {
                let Some(id) = turn.conversation() else {
                    let worker = peers.worker.lock().unwrap().clone();
                    return match worker {
                        Some(worker) => worker(turn).await,
                        None => Reply::text("Done."),
                    };
                };
                peers
                    .inputs
                    .lock()
                    .unwrap()
                    .push((id.clone(), turn.input.clone()));
                let handler = peers.handlers.lock().unwrap().get(&id).cloned();
                match handler {
                    Some(handler) => handler(turn).await,
                    None => Reply::text("Done."),
                }
            })
        })
    }

    /// Answers the turns of session `id` with `f`.
    pub(super) fn on<F, Fut>(&self, id: &ConversationId, f: F)
    where
        F: Fn(Turn) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Reply> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |turn| Box::pin(f(turn)));
        self.handlers.lock().unwrap().insert(id.clone(), handler);
    }

    /// Answers every worker's turns with `f`.
    pub(super) fn worker<F, Fut>(&self, f: F)
    where
        F: Fn(Turn) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = Reply> + Send + 'static,
    {
        let handler: Handler = Arc::new(move |turn| Box::pin(f(turn)));
        *self.worker.lock().unwrap() = Some(handler);
    }

    /// The inputs session `id`'s turns took, in order.
    pub(super) fn inputs_of(&self, id: &ConversationId) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap()
            .iter()
            .filter(|(of, _)| of == id)
            .map(|(_, input)| input.clone())
            .collect()
    }

    /// How many of session `id`'s inputs carry `needle`.
    pub(super) fn count_in(&self, id: &ConversationId, needle: &str) -> usize {
        self.inputs_of(id)
            .iter()
            .filter(|input| input.contains(needle))
            .count()
    }
}

/// The `message_session` replies of a session's turns: each text, and whether it failed.
pub(super) type Sent = Arc<Mutex<Vec<(String, bool)>>>;

impl Peers {
    /// Has each turn of session `from` send `text` to `to` and answer "Sent.", keeping each
    /// reply of the tool; a turn a reply started only notes it.
    pub(super) fn sends(&self, from: &ConversationId, to: &ConversationId, text: &str) -> Sent {
        let sent = Sent::default();
        let (kept, to, text) = (sent.clone(), to.clone(), text.to_owned());
        self.on(from, move |turn| {
            let (kept, to, text) = (kept.clone(), to.clone(), text.clone());
            async move {
                if is_reply(&turn.input) {
                    return Reply::text("Noted.");
                }
                let reply = send(&turn, &to, &text, None).await;
                kept.lock().unwrap().push(reply);
                Reply::text("Sent.")
            }
        });
        sent
    }
}

impl Peers {
    /// Has each turn of session `from` that took a message from `to` answer it with a reply
    /// saying `text`, once `gate` (if any) lets it; other turns answer "Done.".
    pub(super) fn replies(
        &self,
        from: &ConversationId,
        to: &ConversationId,
        text: &str,
        gate: Option<Arc<tokio::sync::Notify>>,
    ) -> Sent {
        let sent = Sent::default();
        let (kept, to, text) = (sent.clone(), to.clone(), text.to_owned());
        self.on(from, move |turn| {
            let (kept, to, text, gate) = (kept.clone(), to.clone(), text.clone(), gate.clone());
            async move {
                let Some(original) = header_id(&turn.input) else {
                    return Reply::text("Done.");
                };
                if let Some(gate) = gate {
                    gate.notified().await;
                }
                let reply = send(&turn, &to, &text, Some(&original)).await;
                kept.lock().unwrap().push(reply);
                Reply::text("Replied.")
            }
        });
        sent
    }
}

/// The one reply `sent` holds.
pub(super) fn only(sent: &Sent) -> (String, bool) {
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 1, "{sent:?}");
    sent[0].clone()
}

/// Sends `text` to `to` from the turn's session; the tool's reply text, and whether it failed.
pub(super) async fn send(
    turn: &Turn,
    to: &ConversationId,
    text: &str,
    reply_to: Option<&str>,
) -> (String, bool) {
    let mut args = json!({ "to": to.0, "text": text });
    if let Some(reply_to) = reply_to {
        args["reply_to"] = Value::from(reply_to);
    }
    let reply = turn.call("message_session", args).await;
    (reply.text, reply.is_error)
}

/// Waits in the turn for replies (`wait_for_replies` with `args`), shown as a tool call
/// that starts and then finishes with its result, as a CLI shows it; the result's text, and
/// whether it failed.
pub(super) async fn wait(turn: &Turn, args: Value) -> (String, bool) {
    wait_as(turn, args, "mcp__brigadier__wait_for_replies").await
}

/// [`wait`], with the call shown under `name`, as the CLI names the tool (Claude:
/// `mcp__brigadier__wait_for_replies`, Codex: `brigadier/wait_for_replies`).
pub(super) async fn wait_as(turn: &Turn, args: Value, name: &str) -> (String, bool) {
    use brigadier_providers::{ItemStatus, ProviderEvent};
    let item_id = format!("wait-{}", uuid::Uuid::now_v7());
    let event = |status, output| ProviderEvent::ToolCall {
        item_id: item_id.clone(),
        name: name.into(),
        input: Some(args.to_string()),
        status,
        output,
    };
    turn.emit(event(ItemStatus::InProgress, None)).await;
    let reply = turn.call("wait_for_replies", args.clone()).await;
    let status = if reply.is_error {
        ItemStatus::Failed
    } else {
        ItemStatus::Completed
    };
    turn.emit(event(status, Some(reply.text.clone()))).await;
    (reply.text, reply.is_error)
}

/// The message id a `message_session` reply names.
pub(super) fn message_id(reply: &str) -> String {
    let value: Value = serde_json::from_str(reply).unwrap_or_else(|_| panic!("{reply}"));
    value["message_id"].as_str().unwrap().to_owned()
}

/// The id a received message's header names: `… · id ‹id›`.
pub(super) fn header_id(input: &str) -> Option<String> {
    let header = input
        .lines()
        .find(|line| line.starts_with("[message from session"))?;
    let rest = header.split(" · id ").nth(1)?;
    Some(rest.split([' ', ']']).next()?.to_owned())
}

/// The sending session a received message's header names: `… · session ‹id› · …`.
pub(super) fn header_session(input: &str) -> Option<ConversationId> {
    let header = input
        .lines()
        .find(|line| line.starts_with("[message from session"))?;
    let rest = header.split(" · session ").nth(1)?;
    Some(ConversationId(rest.split([' ', ']']).next()?.to_owned()))
}

/// Whether a turn's input is a reply from another session: its header says what it replies
/// to.
pub(super) fn is_reply(input: &str) -> bool {
    input
        .lines()
        .any(|line| line.starts_with("[message from session") && line.contains(" · reply to "))
}
