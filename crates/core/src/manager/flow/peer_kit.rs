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
    inputs: Mutex<Vec<(ConversationId, String)>>,
}

impl Peers {
    pub(super) fn new() -> Arc<Self> {
        Arc::default()
    }

    /// The script every CLI runs: a thread's turn goes to its session's handler (else it
    /// answers "Done."), after its input is logged.
    pub(super) fn script(self: &Arc<Self>) -> Script {
        let peers = self.clone();
        Arc::new(move |turn: Turn| {
            let peers = peers.clone();
            Box::pin(async move {
                let Some(id) = turn.conversation() else {
                    return Reply::text("Done.");
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
