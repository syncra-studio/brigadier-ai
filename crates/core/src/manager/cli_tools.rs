//! The tools a session keeps, behind `brigadier` (STRIP-DOWN-PLAN S13, S16c): `preview`,
//! `browser`, `app`, `page`, `ask`, and what the thread, Chats and side chats had as MCP tools
//! (`brain`, `transcript`, `code`, `report`, `feedback`, `session`, `approval`). Each runs as the
//! session's own call of the tool it was, with the grant it called with, so what a session or
//! worker may do is what it could do before.
//! `brigadier ask` waits for the user's answer on its card, and the answer goes back to it
//! alone.

use std::collections::HashSet;
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine as _;
use brigadier_cli::protocol::{
    AskAnswer, AskItem, AskOutput, CliReply, ErrorCode, LedgerOutput, Tool, ToolOutput,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::SessionManager;
use super::cards::CardAnswer;
use super::cli::CliCaller;
use crate::Result;
use crate::tools::{
    AppToolCall, AskOption, AskQuestion, AskUser, BrowserToolCall, ChatCall, OrchestratorCall,
    Role, TabChatCall, TabTools, ToolCall, ToolHost, ToolReply, WorkerCall,
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

    pub(super) fn hold(&self, card: CardId) -> AskHold {
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
pub(super) struct AskHold {
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

/// A tool its caller doesn't have, as the tools refuse one.
fn not_theirs() -> CliReply {
    CliReply::error(
        ErrorCode::Refused,
        "This tool is not available to this session.",
    )
}

/// The call a subcommand's action makes, as its caller's (`role`) own: the thread's, a Chat's,
/// a side chat's or a worker's. A call of a tool the caller doesn't have is refused where the
/// tools refuse it.
fn tool_call(
    tool: Tool,
    action: &str,
    given: Value,
    role: Option<&Role>,
) -> std::result::Result<ToolCall, CliReply> {
    use AppToolCall as A;
    use BrowserToolCall as B;
    use OrchestratorCall as O;
    use WorkerCall as W;
    let worker = matches!(role, Some(Role::Worker { .. }));
    let chat = matches!(role, Some(Role::Chat { .. }));
    let tab = match role {
        Some(Role::TabChat { tools, .. }) => Some(*tools),
        _ => None,
    };
    // A side chat has its tab's page or file, and nothing else.
    match (tab, tool, action) {
        (None, _, _) => {}
        (Some(TabTools::Page), Tool::Page, "read" | "write")
        | (Some(TabTools::File), Tool::Page, "file-read" | "file-write") => {}
        (Some(_), _, _) => return Err(not_theirs()),
    }
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
        (Tool::Page, "read") if tab.is_some() => {
            ToolCall::TabChat(TabChatCall::ReadPage(args(given)?))
        }
        (Tool::Page, "write") if tab.is_some() => {
            ToolCall::TabChat(TabChatCall::WritePage(args(given)?))
        }
        (Tool::Page, "read") => ToolCall::Orchestrator(O::ReadPage(args(given)?)),
        (Tool::Page, "write") => ToolCall::Orchestrator(O::WritePage(args(given)?)),
        (Tool::Page, "file-read") if tab.is_some() => ToolCall::TabChat(TabChatCall::ReadFile),
        (Tool::Page, "file-write") if tab.is_some() => {
            ToolCall::TabChat(TabChatCall::WriteFile(args(given)?))
        }
        (Tool::Page, "file-read" | "file-write") => return Err(not_theirs()),
        (Tool::Code, "search") => ToolCall::Orchestrator(O::CodeSearch(args(given)?)),
        (Tool::Code, "refs") => ToolCall::Orchestrator(O::CodeRefs(args(given)?)),
        (Tool::Code, "map") => ToolCall::Orchestrator(O::ProjectMap),
        (Tool::Report, "save") if chat => ToolCall::Chat(ChatCall::SaveReport(args(given)?)),
        (Tool::Report, "save") => ToolCall::Orchestrator(O::SaveReport(args(given)?)),
        (Tool::Feedback, "offer") => ToolCall::Orchestrator(O::OfferFeedback(args(given)?)),
        (Tool::Session, "propose") => ToolCall::Orchestrator(O::OfferSession(args(given)?)),
        (Tool::Approval, "request") => ToolCall::Orchestrator(O::RequestApproval(args(given)?)),
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
    /// `brigadier <tool> <action>`: the tool's call, as the caller.
    pub(super) async fn cli_tool(
        &self,
        caller: &CliCaller,
        tool: Tool,
        action: &str,
        given: Value,
    ) -> CliReply {
        let role = self.grants.resolve(&caller.grant);
        if matches!(tool, Tool::Pr | Tool::Waiting) {
            return self.cli_tracked_work(caller, tool, action, given).await;
        }
        if tool == Tool::Ledger {
            return match self.cli_ledger(caller, action, given).await {
                Ok((text, check)) => CliReply::ok(&LedgerOutput {
                    action: action.to_owned(),
                    text,
                    check,
                }),
                Err(err) => CliReply::error(ErrorCode::Failed, err.to_string()),
            };
        }
        if tool == Tool::Checkpoint {
            return match self.cli_checkpoint(caller, action, given).await {
                Ok(text) => CliReply::ok(&ToolOutput {
                    action: action.to_owned(),
                    text,
                    images: vec![],
                    saved: vec![],
                }),
                Err(err) => CliReply::error(ErrorCode::Failed, err.to_string()),
            };
        }
        if tool == Tool::Library {
            return self.cli_library(caller, action, given).await;
        }
        if matches!(tool, Tool::Brain | Tool::Transcript) {
            return crate::heap(|| self.cli_records(caller, role.as_ref(), tool, action, given))
                .await;
        }
        let call = match tool_call(tool, action, given, role.as_ref()) {
            Ok(call) => call,
            Err(reply) => return reply,
        };
        let reply = crate::heap(|| ToolHost::call(self, &caller.grant, call)).await;
        tool_reply(action, reply)
    }

    /// `brigadier brain search|read` and `transcript search|read` (S18): the records, as the
    /// thread's own. A worker has `transcript` within its own records, so it can follow its
    /// own search hits (D13); Chats and tab chats have neither.
    async fn cli_records(
        &self,
        caller: &CliCaller,
        role: Option<&Role>,
        tool: Tool,
        action: &str,
        given: Value,
    ) -> CliReply {
        let id = &caller.conversation_id;
        let own = match (role, &caller.task_id, tool) {
            (Some(Role::Orchestrator { .. }), None, Tool::Brain) => None,
            (Some(Role::Orchestrator { .. }), None, _) => Some(id.short().to_owned()),
            (Some(Role::Worker { .. }), Some(task), Tool::Transcript) => {
                match self.worker_records_key(id, task).await {
                    Ok(key) => Some(key),
                    Err(err) => return CliReply::error(ErrorCode::Failed, err.to_string()),
                }
            }
            _ => return not_theirs(),
        };
        let answer = match action {
            "search" => match args(given) {
                Ok(search) => self.records_search(id, own, search).await,
                Err(reply) => return reply,
            },
            "read" => match args(given) {
                Ok(read) => self.records_read(id, own, read).await,
                Err(reply) => return reply,
            },
            other => return usage(format!("no action {other:?}")),
        };
        match answer {
            Ok(text) => tool_reply(action, ToolReply::ok(text)),
            Err(err) => CliReply::error(ErrorCode::Failed, err.to_string()),
        }
    }

    /// `brigadier ask`: one card for the user with every question, and their answers once they
    /// give them.
    pub(super) async fn cli_ask(
        &self,
        caller: &CliCaller,
        questions: Vec<AskItem>,
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
            questions: questions
                .iter()
                .map(|item| {
                    // The recommended option is the one labelled "… (Recommended)", the mark
                    // left off its label, as the CLI's own question tool is read (harness D28);
                    // none marked, none is recommended (D58).
                    let marked = brigadier_providers::CliQuestion::new(
                        None,
                        item.question.clone(),
                        item.options.iter().map(|label| (label.clone(), None)),
                        false,
                    );
                    AskQuestion {
                        question: item.question.clone(),
                        recommended: marked.recommended,
                        options: marked
                            .options
                            .into_iter()
                            .map(|option| AskOption {
                                label: option.label,
                                description: None,
                            })
                            .collect(),
                    }
                })
                .collect(),
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
        // Held before the card opens: an answer as quick as the card reaches this ask, never
        // the thread.
        let id = CardId::generate();
        let _hold = self.cli_asks.hold(id.clone());
        let (card, answered) = self
            .open_round(
                id,
                &caller.conversation_id,
                None,
                QuestionKind::Orchestrator,
                items,
                ask.title,
            )
            .await?;
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
        let answered = board.questions.get(&card.id);
        let answers = card
            .items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                let answer = answered
                    .and_then(|card| {
                        card.answers.get(index).cloned().or_else(|| {
                            // A card stored without per-question answers has its one answer.
                            (index == 0).then(|| card.answer.clone()).flatten()
                        })
                    })
                    .unwrap_or_default();
                let option = item
                    .options
                    .iter()
                    .position(|option| option.label == answer)
                    .map(|index| index as u32);
                AskAnswer {
                    question: item.text.clone(),
                    answer,
                    option,
                }
            })
            .collect();
        Ok(CliReply::ok(&AskOutput::of(answers)))
    }
}
