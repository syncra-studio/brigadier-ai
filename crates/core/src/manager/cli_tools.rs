//! The tools a session keeps, behind `brigadier` (STRIP-DOWN-PLAN S13, S16c): `preview`,
//! `browser`, `app`, `page`, `ask`, and what the thread, Chats and side chats had as MCP tools
//! (`brain`, `transcript`, `code`, `report`, `feedback`, `session`, `approval`). Each runs as the
//! session's own call of the tool it was, with the grant it called with, so what a session or
//! worker may do is what it could do before.
//! `brigadier ask` waits for the user's answer on its card, and the answer goes back to it
//! alone.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use base64::Engine as _;
use brigadier_cli::protocol::{
    AskAnswer, AskItem, AskOutput, CliReply, ErrorCode, LedgerOutput, Tool, ToolOutput,
};
use brigadier_providers::{ApprovalKind, ApprovalRequest, policy};
use serde::de::DeserializeOwned;
use serde_json::Value;

use super::SessionManager;
use super::cards::CardAnswer;
use super::cli::CliCaller;
use crate::Result;
use crate::model::PermissionLevel;
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
        (Tool::Browser, "events") => ToolCall::Browser(B::Events(args(given)?)),
        (Tool::App, "visuals") => ToolCall::App(A::Visuals),
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
        (Tool::Fusion, _) if !chat => return Err(not_theirs()),
        (Tool::Fusion, "offer") => ToolCall::Orchestrator(O::OfferFusion(args(given)?)),
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

/// Whether `--out` may save to `out` in `folder`: where a file change at Accept edits goes
/// unasked ([`policy::workspace_grant`]). Its names as given, then as the file system resolves
/// them (`..`, symlinks, junctions, case), lie in `folder` and outside Git's metadata and the
/// agents' settings (`.git`, `.claude`, `.codex`, `.agents`, `.vscode`, `.mcp.json`), so a
/// `.claude` that is a junction to a plain folder still refuses `.claude/settings.json`. The
/// save is written beside `out` and renamed over it, replacing a link there rather than
/// following it, so `out`'s own folder must lie in `folder` too (not a link outside leading
/// in, nor `folder` itself). A relative path, another drive, a UNC path and a name Windows
/// reads as another are refused: one with `:` (an NTFS stream, as in
/// `.git::$INDEX_ALLOCATION`), one ending in a dot or a space, which it drops (`.mcp.json.`),
/// and a short 8.3 name of a protected one (`CLAUDE~1`). With no folder, everything is.
pub(super) fn out_allowed(out: &Path, folder: Option<&Path>) -> bool {
    let (Some(folder), Some(text), Some(parent)) =
        (folder, out.to_str(), out.parent().and_then(Path::to_str))
    else {
        return false;
    };
    let mut named = PathBuf::new();
    for part in out.components() {
        named.push(part);
        let Component::Normal(name) = part else {
            continue;
        };
        let name = name.to_string_lossy();
        if name.contains(':')
            || name.ends_with(['.', ' '])
            || (name.contains('~') && short_name_of_protected(&named))
        {
            return false;
        }
    }
    let request = ApprovalRequest {
        id: String::new(),
        kind: ApprovalKind::FileChange,
        tool: "brigadier --out".into(),
        command: None,
        cwd: None,
        paths: vec![text.to_owned(), parent.to_owned()],
        reason: None,
        escalation: false,
        input: None,
        grant: None,
        network: false,
        agent: None,
    };
    policy::workspace_grant(&request, folder).is_some()
}

/// Whether `path` is another name of a [`policy::PROTECTED_NAMES`] entry beside it, as
/// Windows' short 8.3 names are (`CLAUDE~1` for `.claude`): the given names would pass for
/// plain ones, and where `.claude` is a link to a plain folder so would the resolved ones.
fn short_name_of_protected(path: &Path) -> bool {
    let (Some(parent), Ok(real)) = (path.parent(), path.canonicalize()) else {
        return false;
    };
    policy::PROTECTED_NAMES.iter().any(|name| {
        parent
            .join(name)
            .canonicalize()
            .is_ok_and(|other| other == real)
    })
}

impl SessionManager {
    /// `brigadier <tool> <action>`: the tool's call, as the caller. A file its `--out` saves to
    /// is checked first ([`Self::cli_out`]): refused, the tool doesn't run.
    pub(super) async fn cli_tool(
        &self,
        caller: &CliCaller,
        tool: Tool,
        action: &str,
        given: Value,
        out: Option<&str>,
    ) -> CliReply {
        let role = self.grants.resolve(&caller.grant);
        if let Some(out) = out
            && let Err(refused) = self.cli_out(caller, role.as_ref(), Path::new(out)).await
        {
            return refused;
        }
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
        if tool == Tool::Mcp {
            return self.cli_mcp(caller, action, given).await;
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

    /// Whether the caller's `--out` may save to `out`, by the level the session (or the
    /// worker) runs at now: at Full access anywhere, as any of its file writes; below it, and
    /// in a side chat, only [`out_allowed`] in its working folder: the thread's checkout, or
    /// the worker's worktree. A Chat, or a session whose worktree isn't made yet, has none,
    /// and its `--out` is refused. The `brigadier` that saves writes later: this can't stop a
    /// process that swaps a checked folder for a link in between.
    async fn cli_out(
        &self,
        caller: &CliCaller,
        role: Option<&Role>,
        out: &Path,
    ) -> std::result::Result<(), CliReply> {
        let conversation = &caller.conversation_id;
        let (level, folder) = match &caller.task_id {
            None => (
                self.permission(conversation),
                self.recorded_workspace(conversation)
                    .map(|workspace| workspace.path),
            ),
            Some(task) => {
                let task = self
                    .task_by_id(conversation, task)
                    .await
                    .map_err(|err| CliReply::error(ErrorCode::Failed, err.to_string()))?;
                let worktree = task
                    .workspace
                    .as_ref()
                    .and_then(|workspace| workspace.worktree.as_ref())
                    .map(PathBuf::from);
                (self.worker_level(&task), worktree)
            }
        };
        let full = level == PermissionLevel::Full
            && matches!(
                role,
                Some(Role::Orchestrator { .. } | Role::Worker { .. } | Role::Chat { .. })
            );
        if full || out_allowed(out, folder.as_deref()) {
            return Ok(());
        }
        let message = match folder {
            None => {
                "refused: --out saves only into this session's working folder, and it has \
                 none; leave out --out and the screenshot goes to the temporary folder"
            }
            Some(_) => {
                "refused: --out may name only a file in this session's working folder, \
                 outside .git and the agents' settings (.claude, .codex, .agents, .vscode, \
                 .mcp.json), unless the session runs at Full access; leave out --out and the \
                 screenshot goes to the temporary folder"
            }
        };
        Err(CliReply::error(ErrorCode::Refused, message))
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
            // Its earlier sessions' transcripts are in its own records (core-goals D121).
            (Some(Role::Worker { .. }), Some(_), Tool::Brain) => {
                return CliReply::error(
                    ErrorCode::Refused,
                    "This tool is not available to this session: a worker reads its own \
                     records, its earlier sessions' transcripts included, with `brigadier \
                     transcript search|read`.",
                );
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

#[cfg(test)]
mod out_tests {
    use super::out_allowed;
    use std::path::{Path, PathBuf};

    /// A folder of its own under the temporary folder, removed when dropped.
    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// The scratch folder, written as `brigadier` sends paths: `C:\…`, not `\\?\C:\…`.
    fn scratch(name: &str) -> Scratch {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "brigadier-out-{name}-{}-{stamp}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.canonicalize().unwrap().display().to_string();
        Scratch(PathBuf::from(real.trim_start_matches(r"\\?\")))
    }

    /// A folder link: a junction on Windows (no privilege needed), a symlink elsewhere.
    fn link_dir(target: &Path, link: &Path) {
        brigadier_providers::accounts::link_to(target, link, true).unwrap();
    }

    /// A file symlink; Windows makes one only with Developer Mode or as an administrator.
    fn link_file(target: &Path, link: &Path) -> std::io::Result<()> {
        #[cfg(windows)]
        return std::os::windows::fs::symlink_file(target, link);
        #[cfg(unix)]
        return std::os::unix::fs::symlink(target, link);
    }

    /// `brigadier browser screenshot --out` writes only where a file change at Accept edits
    /// goes unasked: in the working folder, outside Git's metadata and the agents' settings,
    /// judged by the names as given and again as the file system resolves them. Every other
    /// form of a path elsewhere is refused.
    #[test]
    fn out_names_only_a_file_in_the_working_folder() {
        let dir = scratch("paths");
        let folder = dir.0.join("checkout");
        let outside = dir.0.join("outside");
        for made in ["src", ".git/hooks", "config-data", "Upper"] {
            std::fs::create_dir_all(folder.join(made)).unwrap();
        }
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("profile.ps1"), "keep").unwrap();
        std::fs::write(folder.join("src/existing.png"), "old").unwrap();
        // A junction (Windows) inside leading out, and `.claude` as a junction to a plain
        // folder: its name is still `.claude`.
        link_dir(&outside, &folder.join("link"));
        link_dir(&folder.join("config-data"), &folder.join(".claude"));
        let allowed = |path: &Path| out_allowed(path, Some(&folder));

        for path in [
            folder.join("shot.png"),
            folder.join("src/shot.png"),
            // Overwriting a file of the folder.
            folder.join("src/existing.png"),
            // Folders not made yet.
            folder.join("new/deeper/shot.png"),
            folder.join("config-data/settings.json"),
            folder.join("Upper/shot.png"),
        ] {
            assert!(allowed(&path), "{}", path.display());
        }
        if cfg!(windows) {
            // The same folder in another case, and in its verbatim form.
            let upper = PathBuf::from(folder.display().to_string().to_uppercase());
            assert!(allowed(&upper.join("shot.png")), "{}", upper.display());
            let lower = PathBuf::from(folder.display().to_string().to_lowercase());
            assert!(allowed(&lower.join("upper/shot.png")));
            let verbatim = PathBuf::from(format!(r"\\?\{}", folder.display()));
            assert!(
                allowed(&verbatim.join("shot.png")),
                "{}",
                verbatim.display()
            );
        }

        let mut refused = vec![
            // Out with `..`, as given and as Windows reads it.
            folder.join("../outside/profile.ps1"),
            folder.join("src/../../outside/new.png"),
            std::path::absolute(folder.join("../outside/profile.ps1")).unwrap(),
            // Absolute elsewhere.
            outside.join("profile.ps1"),
            outside.join("new.png"),
            dir.0.join("beside.png"),
            // Relative, which the daemon can't place.
            PathBuf::from("shot.png"),
            PathBuf::from("src/shot.png"),
            // Through the junction leading out.
            folder.join("link/profile.ps1"),
            folder.join("link/new.png"),
            // Git's metadata and the agents' settings, any case, any depth.
            folder.join(".git/hooks/post-checkout"),
            folder.join(".GIT/hooks/pre-commit"),
            folder.join(".git/config"),
            folder.join(".claude/settings.json"),
            folder.join(".Claude/settings.local.json"),
            folder.join(".codex/config.toml"),
            folder.join(".agents/skills/x/SKILL.md"),
            folder.join("src/.vscode/tasks.json"),
            folder.join(".mcp.json"),
            // NTFS streams: one of a file, and `.git` named as its index.
            folder.join("shot.png:hidden"),
            folder.join(".git::$INDEX_ALLOCATION/hooks/post-checkout"),
            folder.join("src::$INDEX_ALLOCATION/shot.png"),
        ];
        if cfg!(windows) {
            let text = folder.display().to_string();
            let (drive, rest) = text.split_at(2);
            let other = if drive.eq_ignore_ascii_case("C:") {
                "D:"
            } else {
                "C:"
            };
            refused.extend([
                // Another drive, and a drive-relative path.
                PathBuf::from(format!(r"{other}\shot.png")),
                PathBuf::from(format!(r"{other}{rest}\shot.png")),
                PathBuf::from(format!("{drive}shot.png")),
                // UNC, even when it names the folder itself through the admin share.
                PathBuf::from(format!(r"\\localhost\{}${rest}\shot.png", &drive[..1])),
                PathBuf::from(format!(
                    r"\\?\UNC\localhost\{}${rest}\shot.png",
                    &drive[..1]
                )),
                PathBuf::from(r"\\127.0.0.1\share\shot.png"),
                // A device.
                PathBuf::from(r"\\.\PhysicalDrive0"),
            ]);
        } else {
            refused.push(PathBuf::from("/shot.png"));
        }
        for path in &refused {
            assert!(!allowed(path), "{}", path.display());
        }

        // A link outside leading in: the save replaces the link, in the folder outside. And
        // the folder itself, whose save would be written beside it, outside.
        link_dir(&folder.join("src"), &outside.join("in"));
        for path in [outside.join("in"), folder.clone(), folder.join(".")] {
            assert!(!allowed(&path), "{}", path.display());
        }

        // Names Windows reads as other names: a trailing dot or space, which it drops
        // (`.mcp.json.` is `.mcp.json`), and a short 8.3 name, which hides the name of a
        // `.claude` that is a junction to a plain folder. Refused wherever they'd be read so.
        for path in [
            folder.join(".mcp.json."),
            folder.join(".mcp.json "),
            folder.join(".claude./settings.json"),
            folder.join(".claude /settings.json"),
            folder.join("src/shot.png."),
        ] {
            assert!(!allowed(&path), "{}", path.display());
        }
        if folder.join("CLAUDE~1").exists() {
            for path in [
                folder.join("CLAUDE~1/settings.json"),
                folder.join("claude~1/settings.local.json"),
            ] {
                assert!(!allowed(&path), "{}", path.display());
            }
        } else {
            eprintln!("no 8.3 names here: the short-name case is left out");
        }
        // A name with `~` that is its own name is a plain name.
        std::fs::create_dir_all(folder.join("v~1")).unwrap();
        assert!(allowed(&folder.join("v~1/shot.png")));
        assert!(allowed(&folder.join("new~1.png")));

        // A file symlink inside leading out, and one leading nowhere; one outside leading in.
        match link_file(&outside.join("profile.ps1"), &folder.join("evil.png")) {
            Ok(()) => {
                link_file(&outside.join("gone.png"), &folder.join("dangling.png")).unwrap();
                assert!(!allowed(&folder.join("evil.png")));
                assert!(!allowed(&folder.join("dangling.png")));
                link_file(&folder.join("src/existing.png"), &outside.join("in.png")).unwrap();
                assert!(!allowed(&outside.join("in.png")));
            }
            // The junction above covers a link leading out on such a machine.
            Err(err) => eprintln!("no file symlink here ({err}); the junction case covers it"),
        }

        // No working folder: nothing is allowed.
        assert!(!out_allowed(&folder.join("shot.png"), None));
    }
}
