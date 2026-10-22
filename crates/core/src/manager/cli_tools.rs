//! The tools a session keeps, behind `brigadier` (STRIP-DOWN-PLAN S13): `preview`, `browser`,
//! `app`, `page` and `ask`. Each runs as the session's own call of the tool it was, with the
//! grant it called with, so what a session or worker may do is what it could do before.
//! `brigadier ask` waits for the user's answer on its card, and the answer goes back to it
//! alone.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine as _;
use brigadier_cli::protocol::{AskOutput, CliReply, ErrorCode, Tool, ToolOutput};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::SessionManager;
use super::cards::CardAnswer;
use super::cli::CliCaller;
use crate::Result;
use crate::tools::{
    AppToolCall, AskOption, AskQuestion, AskUser, BrowserToolCall, OrchestratorCall, ToolCall,
    ToolHost, ToolReply, WorkerCall,
};
use crate::work::{CardId, QuestionKind};

/// The cards a `brigadier ask` waits on.
#[derive(Default, Clone)]
pub(crate) struct CliAsks(Arc<Mutex<HashSet<CardId>>>);

impl CliAsks {
    /// Whether `card`'s answer goes back to a waiting `brigadier ask`.
    pub(crate) fn waits(&self, card: &CardId) -> bool {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains(card)
    }

    fn hold(&self, card: CardId) -> AskHold {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(card.clone());
        AskHold {
            asks: self.clone(),
            card,
        }
    }
}

/// While it lives, its card's answer goes to the waiting `brigadier ask`. A caller that hung
/// up drops it, and the answer reaches the thread as any answer does.
struct AskHold {
    asks: CliAsks,
    card: CardId,
}

impl Drop for AskHold {
    fn drop(&mut self) {
        self.asks
            .0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.card);
    }
}

fn usage(message: impl Into<String>) -> CliReply {
    CliReply::error(ErrorCode::Usage, message)
}

/// The tool's arguments, as its subcommand built them.
fn args<T: DeserializeOwned>(args: Value) -> std::result::Result<T, CliReply> {
    serde_json::from_value(args).map_err(|err| usage(format!("bad arguments: {err}")))
}

/// The call a subcommand's action makes, for the thread or (`worker`) one of its workers.
fn tool_call(
    tool: Tool,
    action: &str,
    given: Value,
    worker: bool,
) -> std::result::Result<ToolCall, CliReply> {
    use AppToolCall as A;
    use BrowserToolCall as B;
    use OrchestratorCall as O;
    use WorkerCall as W;
    Ok(match (tool, action) {
        (Tool::Preview, "start") if worker => ToolCall::Worker(W::StartPreview(args(given)?)),
        (Tool::Preview, "start") => ToolCall::Orchestrator(O::StartPreview(args(given)?)),
        (Tool::Preview, "stop") if worker => ToolCall::Worker(W::StopPreview(args(given)?)),
        (Tool::Preview, "stop") => ToolCall::Orchestrator(O::StopPreview(args(given)?)),
        (Tool::Preview, "log") if worker => ToolCall::Worker(W::PreviewLog(args(given)?)),
        (Tool::Preview, "log") => ToolCall::Orchestrator(O::PreviewLog(args(given)?)),
        (Tool::Browser, "open") => ToolCall::Browser(B::Open(args(given)?)),
        (Tool::Browser, "navigate") => ToolCall::Browser(B::Navigate(args(given)?)),
        (Tool::Browser, "read") => ToolCall::Browser(B::Read(args(given)?)),
        (Tool::Browser, "click") => ToolCall::Browser(B::Click(args(given)?)),
        (Tool::Browser, "type") => ToolCall::Browser(B::Type(args(given)?)),
        (Tool::Browser, "screenshot") => ToolCall::Browser(B::Screenshot(args(given)?)),
        (Tool::Browser, "close") => ToolCall::Browser(B::Close(args(given)?)),
        (Tool::Browser, "show") => ToolCall::Browser(B::Show(args(given)?)),
        (Tool::App, "state") => ToolCall::App(A::State),
        (Tool::App, "read-terminal") => ToolCall::App(A::ReadTerminal(args(given)?)),
        (Tool::App, "open") => ToolCall::App(A::Open(args(given)?)),
        (Tool::App, "terminal-run") => ToolCall::App(A::TerminalRun(args(given)?)),
        (Tool::App, "show-session") => ToolCall::App(A::ShowSession(args(given)?)),
        (Tool::App, "close-tab") => ToolCall::App(A::CloseTab(args(given)?)),
        (Tool::Page, "read") => ToolCall::Orchestrator(O::ReadPage(args(given)?)),
        (Tool::Page, "write") => ToolCall::Orchestrator(O::WritePage(args(given)?)),
        (_, other) => return Err(usage(format!("no action {other:?}"))),
    })
}

/// What the tool answered, as `brigadier` prints it.
fn tool_reply(action: &str, reply: ToolReply) -> CliReply {
    if reply.is_error {
        // A tool its caller doesn't have is refused; anything else failed.
        let code = match reply.text.contains("not available to this session") {
            true => ErrorCode::Refused,
            false => ErrorCode::Failed,
        };
        return CliReply::error(code, reply.text);
    }
    let images = reply
        .images
        .iter()
        .map(|image| {
            format!(
                "data:{};base64,{}",
                image.mime,
                base64::engine::general_purpose::STANDARD.encode(&image.data)
            )
        })
        .collect();
    CliReply::ok(&ToolOutput {
        action: action.to_owned(),
        text: reply.text,
        images,
        saved: Vec::new(),
    })
}

impl SessionManager {
    /// `brigadier preview|browser|app|page <action>`: the tool's call, as the caller.
    pub(super) async fn cli_tool(
        &self,
        caller: &CliCaller,
        tool: Tool,
        action: &str,
        given: Value,
    ) -> CliReply {
        let call = match tool_call(tool, action, given, caller.task_id.is_some()) {
            Ok(call) => call,
            Err(reply) => return reply,
        };
        let reply = crate::heap(|| ToolHost::call(self, &caller.grant, call)).await;
        tool_reply(action, reply)
    }

    /// `brigadier ask`: a card for the user, and their answer once they give it.
    pub(super) async fn cli_ask(
        &self,
        caller: &CliCaller,
        question: String,
        options: Vec<String>,
        title: Option<String>,
    ) -> Result<CliReply> {
        // Workers are read-only to the user (G19.Q6): their session asks for them.
        if caller.task_id.is_some() {
            return Ok(CliReply::error(
                ErrorCode::Refused,
                "refused: a worker doesn't ask the user; say what you need in your last \
                 message, and your session asks",
            ));
        }
        let ask = AskUser {
            questions: vec![AskQuestion {
                question,
                recommended: (!options.is_empty()).then_some(0),
                options: options
                    .iter()
                    .map(|label| AskOption {
                        label: label.clone(),
                        description: None,
                    })
                    .collect(),
            }],
            task: None,
            title: title
                .as_deref()
                .map(|title| super::brains::one_line(title, 80))
                .filter(|title| !title.is_empty()),
        };
        let items = match super::tools::ask_round(ask.questions) {
            Ok(items) => items,
            Err(err) => return Ok(usage(err.to_string())),
        };
        let (card, answered) = self
            .open_round(
                &caller.conversation_id,
                None,
                QuestionKind::Orchestrator,
                items,
                ask.title,
            )
            .await?;
        let _hold = self.cli_asks.hold(card.id.clone());
        match answered.await {
            Ok(CardAnswer::Answered) => {}
            _ => {
                return Ok(CliReply::error(
                    ErrorCode::Failed,
                    "the card was closed without an answer",
                ));
            }
        }
        let board = self.core.board(&caller.conversation_id).await?;
        let answer = board
            .questions
            .get(&card.id)
            .and_then(|card| {
                card.answers
                    .first()
                    .cloned()
                    .or_else(|| card.answer.clone())
            })
            .unwrap_or_default();
        let option = options
            .iter()
            .position(|label| *label == answer)
            .map(|index| index as u32);
        Ok(CliReply::ok(&AskOutput { answer, option }))
    }
}
