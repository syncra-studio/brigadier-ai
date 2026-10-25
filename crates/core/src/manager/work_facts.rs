//! The facts a session reads of its workers (harness P1, D1; STRIP-DOWN-PLAN §9.1 O2): what git
//! shows of each one's work, and the shell commands its CLI ran, whole and as its CLI reported
//! them. Brigadier reads them; the worker writes nothing for them (G2), and nothing is judged:
//! a command shows how it ended, never "tests passed".
//!
//! - Git is read on demand: as a turn ends (for its `brigadier wait` event and the net's
//!   message) and when `brigadier workers` or `show` asks. Nothing polls.
//! - Commands are followed from the worker's CLI events into its `record/commands.jsonl`, a
//!   line per change, so a restart keeps them: the turn each started in, the HEAD Brigadier
//!   observed as it started (a compound command may move HEAD after), how it ended, and a
//!   background command's later status, matched by the call that started it, in whatever turn
//!   that comes.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use brigadier_cli::protocol::{
    CommandFact, CommandStatus, CommitFact, GitFacts, MergeWord, TurnCommands, WaitEvent,
};
use brigadier_git::{Git, MergeStanding, Oid};
use brigadier_providers::{BackgroundTaskKind, BackgroundTaskStatus, ItemStatus, ProviderEvent};
use serde::{Deserialize, Serialize};

use super::{SessionManager, blocking};
use crate::model::{ConversationId, Setup};
use crate::work::{Task, TaskId};

/// A worker's commands, in its `record/`.
pub(crate) const FILE: &str = "commands.jsonl";

/// The most commits a worker's facts list (`ahead` counts them all).
const COMMITS_SHOWN: usize = 20;
/// The most commands a turn's facts list (`total` counts them all).
const COMMANDS_SHOWN: usize = 30;
/// How Claude answers a `Bash` it moved to the background: the command still runs.
const BACKGROUND_START: &str = "Command running in background with ID:";
/// The most of a command the net's message shows of each.
const NET_COMMAND_CHARS: usize = 80;

/// One line of a worker's `commands.jsonl`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "line", rename_all = "kebab-case")]
enum Line {
    /// Its CLI started its `n`th turn.
    Turn { n: u32 },
    /// A command its CLI reported, as it started in `turn` (again when its CLI said its whole
    /// invocation only later).
    Ran {
        item: String,
        turn: u32,
        command: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        head: Option<String>,
    },
    /// How a command stands now, as its CLI said in `turn`.
    Status {
        item: String,
        turn: u32,
        status: CommandStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit: Option<i32>,
    },
}

/// A command a worker's CLI ran.
#[derive(Debug, Clone, PartialEq)]
struct Ran {
    item: String,
    command: String,
    turn: u32,
    head: Option<String>,
    status: CommandStatus,
    exit: Option<i32>,
    /// The latest turn it changed in.
    changed: u32,
    /// Its CLI moved it to the background: its task tells how it goes on.
    background: bool,
}

/// A worker's commands, read back.
#[derive(Debug, Default)]
pub(crate) struct Log {
    /// Its CLI's turns so far.
    turn: u32,
    /// A turn of its CLI is open, as seen in this daemon.
    open: bool,
    ran: Vec<Ran>,
}

/// How a command event of a CLI stands: its status and the exit code its CLI reported.
fn standing(
    status: ItemStatus,
    exit: Option<i32>,
    output: Option<&str>,
) -> (CommandStatus, Option<i32>) {
    match status {
        ItemStatus::InProgress => (CommandStatus::Running, None),
        // Its CLI answered at once: the command still runs, and how it ends is unknown yet.
        ItemStatus::Completed
            if output.is_some_and(|output| output.trim_start().starts_with(BACKGROUND_START)) =>
        {
            (CommandStatus::Background, None)
        }
        ItemStatus::Completed => (CommandStatus::Done, exit),
        ItemStatus::Failed => (CommandStatus::Failed, exit),
        ItemStatus::Declined => (CommandStatus::Declined, None),
    }
}

/// A background command's status, as its CLI says it; how it exited stays unknown, as no CLI
/// reports it there.
fn background_standing(status: BackgroundTaskStatus) -> CommandStatus {
    match status {
        BackgroundTaskStatus::Running => CommandStatus::Background,
        BackgroundTaskStatus::Completed => CommandStatus::Done,
        BackgroundTaskStatus::Failed => CommandStatus::Failed,
        BackgroundTaskStatus::Stopped => CommandStatus::Stopped,
    }
}

/// Whether `event` changes what a worker's commands say.
fn concerns(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::TurnStarted { .. }
            | ProviderEvent::TurnCompleted { .. }
            | ProviderEvent::Exited { .. }
            | ProviderEvent::Command { .. }
            | ProviderEvent::BackgroundTask { .. }
    )
}

impl Log {
    fn apply(&mut self, line: Line) {
        match line {
            Line::Turn { n } => {
                self.turn = self.turn.max(n);
                self.open = true;
            }
            Line::Ran {
                item,
                turn,
                command,
                head,
            } => match self.ran.iter_mut().find(|ran| ran.item == item) {
                Some(ran) => ran.command = command,
                None => self.ran.push(Ran {
                    item,
                    command,
                    turn,
                    head,
                    status: CommandStatus::Running,
                    exit: None,
                    changed: turn,
                    background: false,
                }),
            },
            Line::Status {
                item,
                turn,
                status,
                exit,
            } => {
                if let Some(ran) = self.ran.iter_mut().find(|ran| ran.item == item) {
                    ran.background |= status == CommandStatus::Background;
                    ran.status = status;
                    ran.exit = exit;
                    ran.changed = turn;
                }
            }
        }
    }

    fn read(file: &Path) -> Self {
        let mut log = Self::default();
        let Ok(text) = std::fs::read_to_string(file) else {
            return log;
        };
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            // A line a crash cut short: what it said never happened.
            if let Ok(line) = serde_json::from_str(line) {
                log.apply(line);
            }
        }
        // Its CLI started again since: no turn of it is open.
        log.open = false;
        log
    }

    /// The turn what happens now belongs to: the open one, else the next, whose end tells it
    /// (a background command that ends while its worker waits).
    fn now(&self) -> u32 {
        if self.open { self.turn } else { self.turn + 1 }
    }

    fn known(&self, item: &str) -> Option<&Ran> {
        self.ran.iter().find(|ran| ran.item == item)
    }

    /// Whether `event` reports a command not seen yet: the HEAD it starts at is to be read.
    fn is_new(&self, event: &ProviderEvent) -> bool {
        matches!(event, ProviderEvent::Command { item_id, .. } if self.known(item_id).is_none())
    }

    /// The lines `event` adds; `head`, for a command first seen, is the HEAD observed now.
    fn lines(&self, event: &ProviderEvent, head: Option<String>) -> Vec<Line> {
        let mut lines = Vec::new();
        match event {
            ProviderEvent::TurnStarted { .. } => lines.push(Line::Turn { n: self.turn + 1 }),
            // Not a line: a restart closes every turn anyway.
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. } => {}
            ProviderEvent::Command {
                item_id,
                command,
                status,
                exit_code,
                output,
                ..
            } => {
                let known = self.known(item_id);
                match known {
                    None => lines.push(Line::Ran {
                        item: item_id.clone(),
                        turn: self.now(),
                        command: command.clone(),
                        head,
                    }),
                    // Claude may say a call's input only after it named the call.
                    Some(ran) if !command.is_empty() && *command != ran.command => {
                        lines.push(Line::Ran {
                            item: item_id.clone(),
                            turn: ran.turn,
                            command: command.clone(),
                            head: ran.head.clone(),
                        });
                    }
                    Some(_) => {}
                }
                // Once in the background, a command's word comes from its task, not its call.
                if known.is_some_and(|ran| ran.background) {
                    return lines;
                }
                let (status, exit) = standing(*status, *exit_code, output.as_deref());
                let was =
                    known.map_or((CommandStatus::Running, None), |ran| (ran.status, ran.exit));
                if (status, exit) != was {
                    lines.push(Line::Status {
                        item: item_id.clone(),
                        turn: self.now(),
                        status,
                        exit,
                    });
                }
            }
            ProviderEvent::BackgroundTask { task }
                if matches!(
                    task.kind,
                    BackgroundTaskKind::Shell | BackgroundTaskKind::Terminal
                ) =>
            {
                let Some(ran) = task.item_id.as_deref().and_then(|item| self.known(item)) else {
                    return lines;
                };
                let status = background_standing(task.status);
                if status != ran.status {
                    lines.push(Line::Status {
                        item: ran.item.clone(),
                        turn: self.now(),
                        status,
                        exit: ran.exit.filter(|_| status != CommandStatus::Background),
                    });
                }
            }
            _ => {}
        }
        lines
    }

    /// The commands of turn `turn`: those it started and those that changed in it (a
    /// background command started earlier). With `None`, the latest turn's, and what changed
    /// since it ended. `None` before any turn.
    pub(crate) fn commands(&self, turn: Option<u32>) -> Option<TurnCommands> {
        let latest = turn.is_none();
        let turn = turn.unwrap_or(self.turn);
        let of: Vec<&Ran> = self
            .ran
            .iter()
            .filter(|ran| ran.turn == turn || ran.changed == turn || (latest && ran.changed > turn))
            .collect();
        if turn == 0 && of.is_empty() {
            return None;
        }
        let skip = of.len().saturating_sub(COMMANDS_SHOWN);
        Some(TurnCommands {
            turn: (turn > 0).then_some(turn),
            total: u32::try_from(of.len()).unwrap_or(u32::MAX),
            commands: of[skip..]
                .iter()
                .map(|ran| CommandFact {
                    command: ran.command.clone(),
                    turn: (ran.turn > 0).then_some(ran.turn),
                    status: ran.status,
                    exit: ran.exit,
                    head: ran.head.clone(),
                })
                .collect(),
        })
    }
}

struct Held {
    file: PathBuf,
    log: Log,
}

/// Every worker's commands, each read from its file at its first use.
#[derive(Default)]
pub(crate) struct CommandLogs {
    logs: Mutex<HashMap<TaskId, Arc<Mutex<Held>>>>,
}

impl CommandLogs {
    /// `task`'s log, read from `record` (its `record/` folder) at the first use; none when
    /// it was not read and no `record` is given (an empty path: a worker forgotten meanwhile).
    fn held(&self, task: &TaskId, record: &Path) -> Option<Arc<Mutex<Held>>> {
        let mut logs = self.logs.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(held) = logs.get(task) {
            return Some(held.clone());
        }
        if record.as_os_str().is_empty() {
            return None;
        }
        let file = record.join(FILE);
        let log = Log::read(&file);
        let held = Arc::new(Mutex::new(Held { file, log }));
        logs.insert(task.clone(), held.clone());
        Some(held)
    }

    /// Whether `task`'s log was read in this daemon.
    fn record_known(&self, task: &TaskId) -> bool {
        self.logs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(task)
    }

    fn is_new(&self, task: &TaskId, record: &Path, event: &ProviderEvent) -> bool {
        let Some(held) = self.held(task, record) else {
            return false;
        };
        let held = held.lock().unwrap_or_else(PoisonError::into_inner);
        held.log.is_new(event)
    }

    /// Follows `event` of `task`'s CLI; `head`: the HEAD observed now, for a new command.
    /// `record` is read only when the log was not yet.
    pub(crate) fn follow(
        &self,
        task: &TaskId,
        record: &Path,
        event: &ProviderEvent,
        head: Option<String>,
    ) {
        let Some(held) = self.held(task, record) else {
            return;
        };
        let mut held = held.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(
            event,
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
        ) {
            held.log.open = false;
            return;
        }
        let lines = held.log.lines(event, head);
        if lines.is_empty() {
            return;
        }
        let mut text = String::new();
        for line in &lines {
            if let Ok(json) = serde_json::to_string(line) {
                text.push_str(&json);
                text.push('\n');
            }
        }
        let written = held
            .file
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&held.file)
            })
            .and_then(|mut file| file.write_all(text.as_bytes()));
        if let Err(err) = written {
            tracing::warn!(task = %task, error = %err, "could not keep a worker's command");
        }
        for line in lines {
            held.log.apply(line);
        }
    }

    /// `task`'s commands of turn `turn`, the latest when `None`.
    pub(crate) fn commands(
        &self,
        task: &TaskId,
        record: &Path,
        turn: Option<u32>,
    ) -> Option<TurnCommands> {
        let held = self.held(task, record)?;
        let held = held.lock().unwrap_or_else(PoisonError::into_inner);
        held.log.commands(turn)
    }

    pub(crate) fn forget(&self, task: &TaskId) {
        self.logs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(task);
    }
}

/// Where a worker's git facts are read.
#[derive(Debug, Clone, Default)]
struct GitPlace {
    /// Its worktree, or with `local` the checkout it works in.
    checkout: Option<PathBuf>,
    /// Where its branch is read once its worktree is gone: its repository's checkout.
    fallback: Option<PathBuf>,
    branch: Option<String>,
    /// The branch it merges into.
    base: Option<String>,
    /// The commit it started from.
    start: Option<String>,
    /// It works in its session's checkout, on its session's branch.
    local: bool,
}

fn merge_word(merge: MergeStanding) -> MergeWord {
    match merge {
        MergeStanding::Nothing => MergeWord::Nothing,
        MergeStanding::Merged => MergeWord::Merged,
        MergeStanding::Clean => MergeWord::Clean,
        MergeStanding::Behind => MergeWord::Behind,
        MergeStanding::Conflicts => MergeWord::Conflicts,
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

/// What git shows at `place`; `None` without git, or when its branch can't be read.
fn read_git(git: &Git, place: &GitPlace) -> Option<GitFacts> {
    let own = place
        .checkout
        .as_ref()
        .filter(|path| path.exists())
        .and_then(|path| git.open(path).ok());
    if place.local {
        let repo = own?;
        return Some(GitFacts {
            branch: repo.current_branch().ok().flatten(),
            base: None,
            base_commit: None,
            base_checkout_clean: None,
            commits: Vec::new(),
            ahead: 0,
            behind: 0,
            uncommitted: repo.dirty_files().ok().map(|files| count(files.len())),
            merge: None,
            conflicts: Vec::new(),
        });
    }
    let uncommitted = own
        .as_ref()
        .and_then(|repo| repo.dirty_files().ok())
        .map(|files| count(files.len()));
    let repo = match own {
        Some(repo) => repo,
        None => git.open(place.fallback.as_ref()?).ok()?,
    };
    let (branch, base) = (place.branch.as_deref()?, place.base.as_deref()?);
    let start = place.start.clone().map(Oid);
    let facts = match repo.branch_facts(branch, base, start.as_ref(), COMMITS_SHOWN) {
        Ok(facts) => facts,
        Err(err) => {
            tracing::debug!(branch, base, error = %err, "no git facts for a worker");
            return None;
        }
    };
    Some(GitFacts {
        branch: Some(branch.to_owned()),
        base: Some(base.to_owned()),
        base_commit: place.start.clone(),
        base_checkout_clean: repo
            .checkout_of_dirty(base)
            .ok()
            .flatten()
            .map(|files| files.is_empty()),
        commits: facts
            .commits
            .into_iter()
            .map(|commit| CommitFact {
                hash: commit.commit.0,
                subject: commit.subject,
            })
            .collect(),
        ahead: facts.ahead,
        behind: facts.behind,
        uncommitted,
        merge: Some(merge_word(facts.merge)),
        conflicts: facts.conflicts,
    })
}

/// A command for the net's line: its first line, cut short.
fn net_command(command: &str) -> String {
    let first = command.lines().next().unwrap_or_default().trim();
    let cut: String = first.chars().take(NET_COMMAND_CHARS).collect();
    if cut.len() < command.trim().len() {
        format!("`{cut}…`")
    } else {
        format!("`{cut}`")
    }
}

/// An event's facts for the net's one message (S29): git in a few words, every command that
/// failed, has no known exit or is still running, each with its turn and commit, and where the
/// whole of it is.
pub(crate) fn net_facts(event: &WaitEvent) -> String {
    let mut parts = Vec::new();
    if let Some(git) = &event.git {
        let base = git.base.as_deref().unwrap_or("its base");
        let mut said = Vec::new();
        match git.merge {
            Some(MergeWord::Nothing) => said.push("no commits".to_owned()),
            Some(MergeWord::Merged) => said.push(format!("merged into {base}")),
            Some(merge) => said.push(format!(
                "{} commits not in {base}, merge {}",
                git.ahead,
                merge.word()
            )),
            None => said.extend(git.branch.as_ref().map(|branch| format!("on {branch}"))),
        }
        if let Some(n) = git.uncommitted.filter(|n| *n > 0) {
            said.push(format!("{n} uncommitted"));
        }
        if git.base_checkout_clean == Some(false) {
            said.push(format!("the checkout of {base} has uncommitted files"));
        }
        parts.push(format!("git: {}", said.join(", ")));
    }
    if let Some(commands) = &event.commands {
        let mut said = vec![format!(
            "ran {} commands{}",
            commands.total,
            commands
                .turn
                .map_or(String::new(), |turn| format!(" in turn {turn}"))
        )];
        let at = |command: &CommandFact| {
            let turn = command
                .turn
                .map_or("turn ?".to_owned(), |turn| format!("turn {turn}"));
            let head = command
                .head
                .as_deref()
                .map_or("unknown commit".to_owned(), |head| {
                    head[..head.len().min(8)].to_owned()
                });
            format!("{turn}, at {head}")
        };
        for command in &commands.commands {
            let how = match (command.status, command.exit) {
                (CommandStatus::Failed, Some(exit)) => format!("failed, exit {exit}"),
                (CommandStatus::Failed, None) => "failed, exit unknown".to_owned(),
                (CommandStatus::Running, _) => "no end seen".to_owned(),
                (CommandStatus::Background, _) => "in the background, exit unknown".to_owned(),
                (CommandStatus::Declined, _) => "declined".to_owned(),
                (CommandStatus::Stopped, _) => "stopped".to_owned(),
                (CommandStatus::Done, None) => "exit unknown".to_owned(),
                (CommandStatus::Done, Some(_)) => continue,
            };
            said.push(format!(
                "{} {how} ({})",
                net_command(&command.command),
                at(command)
            ));
        }
        parts.push(said.join("; "));
    }
    if parts.is_empty() {
        return String::new();
    }
    format!(
        " · {} · full facts: `brigadier show {}`",
        parts.join(" · "),
        event.id
    )
}

impl SessionManager {
    /// Worker `task`'s `record/` folder.
    fn worker_record(&self, task: &Task) -> PathBuf {
        self.session_dir(&task.conversation_id)
            .worker(task)
            .join("record")
    }

    /// Follows an event of worker `task`'s CLI into its commands; a new command's starting
    /// HEAD is read in `cwd`, where its CLI runs.
    pub(super) async fn follow_worker_commands(
        &self,
        conversation: &ConversationId,
        task: &TaskId,
        cwd: Option<PathBuf>,
        event: &ProviderEvent,
    ) {
        if !concerns(event) {
            return;
        }
        let record = if self.command_logs.record_known(task) {
            None
        } else {
            match self.task_by_id(conversation, task).await {
                Ok(task) => Some(self.worker_record(&task)),
                Err(_) => return,
            }
        };
        // Known once read: its path is only needed to read it.
        let record = record.unwrap_or_default();
        let head = if self.command_logs.is_new(task, &record, event) {
            let git = self.git.clone();
            match cwd {
                Some(cwd) => {
                    blocking(move || Ok(git.open(&cwd).and_then(|repo| repo.resolve("HEAD")).ok()))
                        .await
                        .ok()
                        .flatten()
                        .map(|head| head.0)
                }
                None => None,
            }
        } else {
            None
        };
        self.command_logs.follow(task, &record, event, head);
    }

    /// Where `task`'s git facts are read; `None` for a worker without git.
    fn git_place(&self, task: &Task) -> Option<GitPlace> {
        let plain = task.plain.as_ref();
        let conversation = self.core.conversation(&task.conversation_id).ok()?;
        let repo = plain
            .and_then(|plain| plain.repo.clone())
            .map(PathBuf::from);
        // A Chat's worker without a repository works in a folder with no git (S28).
        if matches!(conversation.setup, Some(Setup::Chat { .. })) && repo.is_none() {
            return None;
        }
        let session = self.changes_checkout(&task.conversation_id);
        let checkout = repo.or_else(|| session.as_ref().map(|(path, _)| path.clone()));
        if plain.is_some_and(|plain| plain.local) {
            return Some(GitPlace {
                checkout,
                local: true,
                ..GitPlace::default()
            });
        }
        let workspace = task.workspace.as_ref()?;
        Some(GitPlace {
            checkout: workspace.worktree.clone().map(PathBuf::from),
            fallback: checkout,
            branch: workspace.branch.clone(),
            base: workspace
                .target
                .clone()
                .or_else(|| plain.and_then(|plain| plain.base.clone()))
                .or_else(|| session.and_then(|(_, branch)| branch)),
            start: workspace.base.clone(),
            local: false,
        })
    }

    /// What git shows of worker `task`'s work now.
    pub(super) async fn worker_git_facts(&self, task: &Task) -> Option<GitFacts> {
        let place = self.git_place(task)?;
        let git = self.git.clone();
        blocking(move || Ok(read_git(&git, &place)))
            .await
            .ok()
            .flatten()
    }

    /// Worker `task`'s facts now: its git, and the commands of its latest turn.
    pub(super) async fn worker_facts(
        &self,
        task: &Task,
    ) -> (Option<GitFacts>, Option<TurnCommands>) {
        let git = self.worker_git_facts(task).await;
        let commands = self
            .command_logs
            .commands(&task.id, &self.worker_record(task), None);
        (git, commands)
    }

    /// Adds worker `task`'s facts to an event of its turn's end.
    pub(super) async fn with_facts(&self, task: &Task, event: &mut WaitEvent) {
        let (git, commands) = self.worker_facts(task).await;
        event.git = git;
        event.commands = commands;
    }

    /// Worker `task` was closed: its commands are read no more.
    pub(super) fn forget_worker_commands(&self, task: &TaskId) {
        self.command_logs.forget(task);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_providers::{claude, codex};

    /// A folder of its own, removed after the test.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("brigadier-commands-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn claude_events(lines: &str) -> Vec<ProviderEvent> {
        let mut parser = claude::parse::Parser::live();
        lines
            .lines()
            .flat_map(|line| parser.feed(line))
            .filter_map(|output| match output {
                claude::parse::Output::Event(event) => Some(event),
                _ => None,
            })
            .collect()
    }

    fn codex_events(lines: &str) -> Vec<ProviderEvent> {
        let mut parser = codex::parse::Parser::live();
        lines
            .lines()
            .flat_map(|line| parser.feed(line))
            .filter_map(|output| match output {
                codex::parse::Output::Event(event) => Some(event),
                _ => None,
            })
            .collect()
    }

    /// Follows `turns` of events, each started first unless its CLI's own events start it;
    /// the heads read are `head-<n>`, the nth command seen.
    fn follow(logs: &CommandLogs, task: &TaskId, record: &Path, turns: &[Vec<ProviderEvent>]) {
        let mut seen = 0;
        for events in turns {
            if !events
                .iter()
                .any(|event| matches!(event, ProviderEvent::TurnStarted { .. }))
            {
                logs.follow(
                    task,
                    record,
                    &ProviderEvent::TurnStarted { turn_id: None },
                    None,
                );
            }
            for event in events {
                let head = logs.is_new(task, record, event).then(|| {
                    seen += 1;
                    format!("head-{seen}")
                });
                logs.follow(task, record, event, head);
            }
        }
    }

    fn fact(
        command: &str,
        turn: u32,
        status: CommandStatus,
        exit: Option<i32>,
        head: Option<&str>,
    ) -> CommandFact {
        CommandFact {
            command: command.into(),
            turn: Some(turn),
            status,
            exit,
            head: head.map(str::to_owned),
        }
    }

    #[test]
    fn claude_commands_keep_their_whole_invocation_exit_and_head_and_a_background_one_ends_later() {
        let dir = Scratch::new();
        let (logs, task) = (CommandLogs::default(), TaskId("t-claude".into()));
        // One CLI over two turns: the background command's end comes in the second.
        let events = claude_events(include_str!("work_facts/claude-commands.jsonl"));
        let end = events
            .iter()
            .position(|event| matches!(event, ProviderEvent::TurnCompleted { .. }))
            .unwrap();
        let (first, second) = events.split_at(end + 1);
        follow(&logs, &task, &dir.0, &[first.to_vec(), second.to_vec()]);
        let heredoc = "cat <<'EOF' > notes.md\nfirst line\nEOF\ngit add notes.md && git commit -qm 'Add notes'";
        let turn_one = logs.commands(&task, &dir.0, Some(1)).unwrap();
        assert_eq!(
            turn_one.commands,
            [
                fact(
                    "cargo test -p demo",
                    1,
                    CommandStatus::Failed,
                    Some(101),
                    Some("head-1")
                ),
                fact("pnpm dev", 1, CommandStatus::Failed, None, Some("head-2")),
                fact(heredoc, 1, CommandStatus::Done, Some(0), Some("head-3")),
            ],
            "turn 1, as it reads now"
        );
        // The background command started in turn 1 and ended in turn 2: it shows in both, its
        // exit unknown, as Claude reports none.
        let turn_two = logs.commands(&task, &dir.0, None).unwrap();
        assert_eq!(turn_two.turn, Some(2));
        assert_eq!(
            turn_two.commands,
            [
                fact("pnpm dev", 1, CommandStatus::Failed, None, Some("head-2")),
                fact("ls", 2, CommandStatus::Done, Some(0), Some("head-4")),
            ]
        );

        // A restart reads the same back, heads and all.
        let again = CommandLogs::default();
        assert_eq!(again.commands(&task, &dir.0, None), Some(turn_two));
        assert_eq!(again.commands(&task, &dir.0, Some(1)), Some(turn_one));
    }

    #[test]
    fn a_background_command_reads_running_in_the_background_until_its_task_ends() {
        let dir = Scratch::new();
        let (logs, task) = (CommandLogs::default(), TaskId("t-bg".into()));
        let fixture = include_str!("work_facts/claude-commands.jsonl");
        let (first, _) = fixture.split_once("\n\n").unwrap();
        follow(&logs, &task, &dir.0, &[claude_events(first)]);
        let commands = logs.commands(&task, &dir.0, None).unwrap();
        assert_eq!(
            commands.commands[1],
            fact(
                "pnpm dev",
                1,
                CommandStatus::Background,
                None,
                Some("head-2")
            ),
            "never `exit 0`: it still runs"
        );
    }

    #[test]
    fn codex_commands_keep_their_reported_exit_a_decline_and_one_never_ended() {
        let dir = Scratch::new();
        let (logs, task) = (CommandLogs::default(), TaskId("t-codex".into()));
        let events = codex_events(include_str!("work_facts/codex-commands.jsonl"));
        follow(&logs, &task, &dir.0, &[events]);
        let commands = logs.commands(&task, &dir.0, None).unwrap();
        assert_eq!((commands.turn, commands.total), (Some(1), 4));
        assert_eq!(
            commands.commands,
            [
                fact(
                    "/bin/zsh -c 'cargo test -p demo'",
                    1,
                    CommandStatus::Failed,
                    Some(101),
                    Some("head-1")
                ),
                fact(
                    "/bin/zsh -c 'cargo fmt --check && cargo clippy'",
                    1,
                    CommandStatus::Done,
                    Some(0),
                    Some("head-2")
                ),
                fact(
                    "/bin/zsh -c 'rm -rf target'",
                    1,
                    CommandStatus::Declined,
                    None,
                    Some("head-3")
                ),
                fact(
                    "/bin/zsh -c 'sleep 600'",
                    1,
                    CommandStatus::Running,
                    None,
                    Some("head-4")
                ),
            ]
        );
    }

    #[test]
    fn a_worker_with_no_turn_has_no_commands_and_one_that_ran_none_says_so() {
        let dir = Scratch::new();
        let (logs, task) = (CommandLogs::default(), TaskId("t-none".into()));
        assert_eq!(logs.commands(&task, &dir.0, None), None);
        follow(&logs, &task, &dir.0, &[Vec::new()]);
        let commands = logs.commands(&task, &dir.0, None).unwrap();
        assert_eq!((commands.turn, commands.total), (Some(1), 0));
    }

    fn wait_event() -> WaitEvent {
        WaitEvent {
            id: "a1b2c3d4".into(),
            event_id: "ev-1".into(),
            event: brigadier_cli::protocol::EventKind::TurnEnded,
            message: Some("All green.".into()),
            asks: None,
            resets_at_ms: None,
            at_ms: 1,
            git: Some(GitFacts {
                branch: Some("work".into()),
                base: Some("main".into()),
                base_commit: Some("1111".into()),
                base_checkout_clean: Some(false),
                commits: Vec::new(),
                ahead: 2,
                behind: 0,
                uncommitted: Some(3),
                merge: Some(MergeWord::Clean),
                conflicts: Vec::new(),
            }),
            commands: Some(TurnCommands {
                turn: Some(4),
                total: 5,
                commands: vec![
                    fact(
                        "cargo build",
                        4,
                        CommandStatus::Failed,
                        Some(101),
                        Some("abcdef0123"),
                    ),
                    fact("pnpm dev", 3, CommandStatus::Background, None, None),
                    fact(
                        "sleep 600",
                        4,
                        CommandStatus::Running,
                        None,
                        Some("abcdef0123"),
                    ),
                    fact(
                        "git status",
                        4,
                        CommandStatus::Done,
                        Some(0),
                        Some("abcdef0123"),
                    ),
                ],
            }),
        }
    }

    #[test]
    fn the_nets_facts_name_every_failed_unknown_and_running_command_and_where_the_rest_is() {
        let text = net_facts(&wait_event());
        assert_eq!(
            text,
            " · git: 2 commits not in main, merge clean, 3 uncommitted, the checkout of main has \
             uncommitted files · ran 5 commands in turn 4; `cargo build` failed, exit 101 (turn \
             4, at abcdef01); `pnpm dev` in the background, exit unknown (turn 3, at unknown \
             commit); `sleep 600` no end seen (turn 4, at abcdef01) · full facts: `brigadier \
             show a1b2c3d4`"
        );
        let mut bare = wait_event();
        bare.git = None;
        bare.commands = None;
        assert_eq!(net_facts(&bare), "");
    }
}
