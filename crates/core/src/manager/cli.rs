//! The daemon's side of the `brigadier` command (STRIP-DOWN-PLAN S13): who calls, and the
//! answers to its read commands.
//!
//! A call names its session (`BRIGADIER_SESSION`) and carries that session's grant: the same
//! grants the MCP tools are checked by, so a session that ended can't call any more. A thread
//! or a Chat is its conversation's id, a worker its task's id. Everything it reads is in its
//! own project: the project's sidebar sessions and their workers.
//!
//! Ids shown are the last 8 characters of the full id (random in a v7 uuid); an id given is the
//! full id, or the start of a shown one.

use std::collections::HashMap;

use brigadier_cli::protocol::{
    self as cli, Agent, Changes, CliCall, CliReply, CliRequest, ErrorCode, Item, ItemKind, Kind,
    ReadOutput, ReadView, SessionRow, SessionState, SessionsOutput, ShowOutput, WorkerRow,
    WorkerState, WorkersOutput,
};
use brigadier_providers::{ProviderEvent, ProviderKind};

use super::SessionManager;
use crate::model::{
    Conversation, ConversationId, Environment, Lifecycle, MessageRole, Setup, TaskId,
};
use crate::tools::Role;
use crate::work::{ConversationActivity, DiffStat, RunState, Task, TaskState};

/// The shortest id start a call may give.
const MIN_ID: usize = 4;

/// What `read` looks back through for the last message or the screen.
const LOOK_BACK: u32 = 200;

/// A command's or tool's output is cut to its last this many bytes.
const OUTPUT_TAIL: usize = 4000;

/// Who calls: a session's thread or Chat, or one of its workers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliCaller {
    pub conversation_id: ConversationId,
    /// Set when a worker calls.
    pub task_id: Option<TaskId>,
}

/// What an id names.
enum Target {
    Session(Box<Conversation>),
    Worker(Box<Task>),
}

/// The shown id: the last 8 characters.
fn short(id: &str) -> String {
    id[id.len().saturating_sub(8)..].to_owned()
}

fn agent(provider: ProviderKind) -> Agent {
    match provider {
        ProviderKind::Claude => Agent::Claude,
        ProviderKind::Codex => Agent::Codex,
    }
}

fn changes(stat: &DiffStat) -> Changes {
    Changes {
        files: u32::try_from(stat.files.len()).unwrap_or(u32::MAX),
        insertions: stat.insertions,
        deletions: stat.deletions,
    }
}

/// A session's state as its sidebar row shows it.
fn session_state(
    conversation: &Conversation,
    activity: Option<&ConversationActivity>,
) -> SessionState {
    let Some(activity) = activity else {
        return match conversation.lifecycle {
            Lifecycle::Hibernated => SessionState::Asleep,
            _ => SessionState::Ready,
        };
    };
    if !activity.questions.is_empty() || !activity.approvals.is_empty() {
        return SessionState::NeedsYou;
    }
    match activity.run {
        RunState::Running => SessionState::Working,
        RunState::Starting => SessionState::Starting,
        RunState::Failed => SessionState::Failed,
        _ if activity.failed => SessionState::Failed,
        _ if activity.stopped => SessionState::Stopped,
        _ if !activity.requests.is_empty() => SessionState::Working,
        RunState::Hibernated => SessionState::Asleep,
        RunState::Idle if conversation.lifecycle == Lifecycle::Hibernated => SessionState::Asleep,
        RunState::Idle => SessionState::Ready,
    }
}

fn worker_state(state: TaskState) -> WorkerState {
    match state {
        TaskState::Queued => WorkerState::Queued,
        TaskState::Starting => WorkerState::Starting,
        TaskState::Running => WorkerState::Working,
        TaskState::Blocked => WorkerState::Asking,
        TaskState::Paused => WorkerState::Paused,
        TaskState::Reported => WorkerState::Reported,
        TaskState::TakenOver => WorkerState::TakenOver,
        TaskState::Landing => WorkerState::Landing,
        TaskState::ReadyToLand => WorkerState::ReadyToLand,
        TaskState::Landed => WorkerState::Landed,
        TaskState::Done => WorkerState::Done,
        TaskState::Rejected => WorkerState::Rejected,
        TaskState::Stopped => WorkerState::Stopped,
        TaskState::Failed => WorkerState::Failed,
    }
}

fn worker_row(task: &Task, diffs: &HashMap<TaskId, Changes>) -> WorkerRow {
    WorkerRow {
        id: short(&task.id.0),
        number: task.number,
        title: task.title.clone(),
        agent: agent(task.route.choice.provider),
        model: task.route.choice.model.clone(),
        effort: task.route.choice.effort.clone(),
        state: worker_state(task.state),
        branch: task
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.branch.clone()),
        changes: diffs.get(&task.id).copied(),
    }
}

/// A session's branch, where it works and the branch it started from.
fn session_place(
    conversation: &Conversation,
) -> (Option<String>, Option<cli::Checkout>, Option<String>) {
    match &conversation.setup {
        Some(Setup::Session {
            environment: Environment::NewWorktree { base, branch, .. },
            ..
        }) => (
            Some(branch.clone()),
            Some(cli::Checkout::Worktree),
            Some(base.clone()),
        ),
        Some(Setup::Session {
            environment: Environment::LocalCheckout { branch },
            ..
        }) => (Some(branch.clone()), Some(cli::Checkout::Local), None),
        _ => (None, None, None),
    }
}

/// The last `max` bytes of `text`, on a character boundary.
fn tail(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut start = text.len() - max;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("…{}", &text[start..])
}

fn item_status(status: brigadier_providers::model::ItemStatus) -> cli::ItemStatus {
    use brigadier_providers::model::ItemStatus as Status;
    match status {
        Status::InProgress => cli::ItemStatus::Running,
        Status::Completed => cli::ItemStatus::Done,
        Status::Failed | Status::Declined => cli::ItemStatus::Failed,
    }
}

/// A CLI event as a thread item; `None` for events that aren't one (deltas, usage, …).
fn event_item(event: &ProviderEvent, at_ms: i64) -> Option<Item> {
    let item = |kind, text: String| Item {
        kind,
        role: None,
        text,
        output: None,
        status: None,
        exit: None,
        at_ms,
    };
    Some(match event {
        ProviderEvent::Message { role, text, .. } => Item {
            role: Some(match role {
                brigadier_providers::model::Role::User => cli::Role::User,
                brigadier_providers::model::Role::Assistant => cli::Role::Assistant,
            }),
            ..item(ItemKind::Message, text.clone())
        },
        ProviderEvent::Reasoning { text, .. } => item(ItemKind::Reasoning, text.clone()),
        ProviderEvent::ToolCall {
            name,
            input,
            status,
            output,
            ..
        } => {
            let text = match input {
                Some(input) => format!("{name} {}", tail(input, OUTPUT_TAIL)),
                None => name.clone(),
            };
            Item {
                output: output.as_deref().map(|output| tail(output, OUTPUT_TAIL)),
                status: Some(item_status(*status)),
                ..item(ItemKind::Tool, text)
            }
        }
        ProviderEvent::Command {
            command,
            status,
            exit_code,
            output,
            ..
        } => Item {
            output: output.as_deref().map(|output| tail(output, OUTPUT_TAIL)),
            status: Some(item_status(*status)),
            exit: *exit_code,
            ..item(ItemKind::Command, command.clone())
        },
        ProviderEvent::TurnCompleted { status, .. } => {
            use brigadier_providers::model::TurnStatus;
            let word = match status {
                TurnStatus::Completed => "completed",
                TurnStatus::Interrupted => "interrupted",
                TurnStatus::Failed => "failed",
            };
            item(ItemKind::TurnEnd, word.into())
        }
        ProviderEvent::Error { error } => item(ItemKind::Error, error.message.clone()),
        _ => return None,
    })
}

/// The items of a run of CLI events: a tool call or command once, as its latest event has it
/// (a call is stored when it starts and again when it ends).
fn items(entries: &[crate::model::RawEntry]) -> Vec<Item> {
    let item_id = |event: &ProviderEvent| match event {
        ProviderEvent::ToolCall { item_id, .. } | ProviderEvent::Command { item_id, .. } => {
            Some(item_id.clone())
        }
        _ => None,
    };
    let mut latest: HashMap<String, usize> = HashMap::new();
    for (at, entry) in entries.iter().enumerate() {
        if let Some(id) = item_id(&entry.event) {
            latest.insert(id, at);
        }
    }
    entries
        .iter()
        .enumerate()
        .filter(|(at, entry)| item_id(&entry.event).is_none_or(|id| latest.get(&id) == Some(at)))
        .filter_map(|(_, entry)| event_item(&entry.event, entry.at_ms))
        .collect()
}

fn message_item(message: &crate::model::Message) -> Item {
    Item {
        kind: ItemKind::Message,
        role: Some(match message.role {
            MessageRole::User => cli::Role::User,
            MessageRole::Assistant => cli::Role::Assistant,
            MessageRole::System => cli::Role::System,
            MessageRole::Peer => cli::Role::Peer,
        }),
        text: message.text.clone(),
        output: None,
        status: None,
        exit: None,
        at_ms: message.created_at_ms,
    }
}

fn failed(err: impl std::fmt::Display) -> CliReply {
    CliReply::error(ErrorCode::Failed, err.to_string())
}

impl SessionManager {
    /// What a thread's, Chat's or worker's CLI gets so `brigadier` (and every command it runs)
    /// knows who calls and finds the daemon: the session's id and live `grant`, the data folder
    /// when it isn't the installed app's, and the folder `brigadier` ships in first on `PATH`.
    /// The grant stays in the environment, never on a command line.
    pub(crate) fn cli_env(&self, session: &str, grant: &str) -> Vec<(String, String)> {
        let mut env = vec![
            (cli::SESSION_ENV.to_owned(), session.to_owned()),
            (cli::GRANT_ENV.to_owned(), grant.to_owned()),
        ];
        if brigadier_sandbox::default_data_dir().ok().as_deref() != Some(self.data_dir.as_path()) {
            env.push((
                brigadier_sandbox::DATA_DIR_ENV.to_owned(),
                self.data_dir.to_string_lossy().into_owned(),
            ));
        }
        if let Some(bin) = self.config.daemon_exe.parent() {
            let rest = self.runtime.cli_env().var("PATH");
            let dirs = std::iter::once(bin.to_owned())
                .chain(rest.into_iter().flat_map(std::env::split_paths));
            if let Ok(path) = std::env::join_paths(dirs) {
                env.push(("PATH".to_owned(), path.to_string_lossy().into_owned()));
            }
        }
        env
    }

    /// Who a `brigadier` call comes from, when `grant` is the live grant of the session it
    /// names: a thread's or Chat's (`session` is its conversation's id) or a worker's
    /// (`session` is its task's id). `None` refuses the call.
    pub fn cli_caller(&self, session: &str, grant: &str) -> Option<CliCaller> {
        match self.grants.resolve(grant)? {
            Role::Orchestrator {
                conversation_id, ..
            }
            | Role::Chat { conversation_id }
                if conversation_id.0 == session =>
            {
                Some(CliCaller {
                    conversation_id,
                    task_id: None,
                })
            }
            Role::Worker {
                conversation_id,
                task_id,
                ..
            } if task_id.0 == session => Some(CliCaller {
                conversation_id,
                task_id: Some(task_id),
            }),
            _ => None,
        }
    }

    /// Answers one `brigadier` call of `caller`.
    pub async fn cli_call(&self, caller: &CliCaller, call: CliCall) -> CliReply {
        if call.protocol != cli::PROTOCOL {
            return CliReply::error(
                ErrorCode::Version,
                format!(
                    "this brigadier speaks protocol {}, and Brigadier speaks {}: use the \
                     brigadier that came with the app",
                    call.protocol,
                    cli::PROTOCOL
                ),
            );
        }
        let reply = match call.request {
            CliRequest::Sessions => self.cli_sessions(caller).await.map(|o| CliReply::ok(&o)),
            CliRequest::Workers => self.cli_workers(caller).await.map(|o| CliReply::ok(&o)),
            CliRequest::Show { id } => self.cli_show(caller, &id).await,
            CliRequest::Read { id, last, screen } => self.cli_read(caller, &id, last, screen).await,
        };
        reply.unwrap_or_else(failed)
    }

    /// The caller's project's sidebar sessions, the caller's own when it is outside one.
    fn cli_project_sessions(
        &self,
        caller: &CliCaller,
    ) -> crate::Result<(Option<String>, Vec<Conversation>)> {
        let catalog = self.core.visible_catalog();
        let me = self.core.conversation(&caller.conversation_id)?;
        let Some(project_id) = me.project_id.clone() else {
            return Ok((None, vec![me]));
        };
        let project = catalog
            .projects
            .iter()
            .find(|project| project.id == project_id)
            .map(|project| project.name.clone());
        let mut sessions: Vec<Conversation> = catalog
            .conversations
            .into_iter()
            .filter(|conversation| {
                conversation.is_tab()
                    && conversation.project_id.as_ref() == Some(&project_id)
                    && conversation.lifecycle != Lifecycle::Archived
            })
            .collect();
        // Pinned first, then the latest active first, as the sidebar lists them.
        sessions.sort_by(|a, b| {
            b.pinned_at_ms
                .is_some()
                .cmp(&a.pinned_at_ms.is_some())
                .then(b.pinned_at_ms.cmp(&a.pinned_at_ms))
                .then(b.updated_at_ms.cmp(&a.updated_at_ms))
        });
        Ok((project, sessions))
    }

    async fn cli_activity(&self) -> HashMap<ConversationId, ConversationActivity> {
        self.core
            .activity()
            .await
            .into_iter()
            .map(|activity| (activity.conversation_id.clone(), activity))
            .collect()
    }

    async fn cli_session_changes(&self, id: &ConversationId) -> Option<Changes> {
        match self.session_diff_stat(id).await {
            Ok(stat) => stat.map(|(stat, _)| changes(&stat)),
            Err(err) => {
                tracing::debug!(conversation = %id, error = %err, "no diff for brigadier");
                None
            }
        }
    }

    async fn cli_worker_changes(&self, id: &ConversationId) -> HashMap<TaskId, Changes> {
        match self.worker_diffs(id).await {
            Ok(diffs) => diffs
                .into_iter()
                .map(|diff| (diff.task_id, changes(&diff.stat)))
                .collect(),
            Err(err) => {
                tracing::debug!(conversation = %id, error = %err, "no worker diffs for brigadier");
                HashMap::new()
            }
        }
    }

    async fn cli_sessions(&self, caller: &CliCaller) -> crate::Result<SessionsOutput> {
        let (project, conversations) = self.cli_project_sessions(caller)?;
        let activity = self.cli_activity().await;
        let mut sessions = Vec::with_capacity(conversations.len());
        for conversation in conversations {
            let (branch, _, _) = session_place(&conversation);
            let workers = self.core.tasks(&conversation.id).await?.len();
            sessions.push(SessionRow {
                id: short(&conversation.id.0),
                title: conversation.title.clone(),
                state: session_state(&conversation, activity.get(&conversation.id)),
                branch,
                changes: self.cli_session_changes(&conversation.id).await,
                workers: u32::try_from(workers).unwrap_or(u32::MAX),
                caller: conversation.id == caller.conversation_id && caller.task_id.is_none(),
            });
        }
        Ok(SessionsOutput { project, sessions })
    }

    async fn cli_workers(&self, caller: &CliCaller) -> crate::Result<WorkersOutput> {
        let session = match &caller.task_id {
            Some(task_id) => short(&task_id.0),
            None => short(&caller.conversation_id.0),
        };
        // A worker has no workers of its own.
        if caller.task_id.is_some() {
            return Ok(WorkersOutput {
                session,
                workers: Vec::new(),
            });
        }
        let diffs = self.cli_worker_changes(&caller.conversation_id).await;
        let workers = self
            .core
            .tasks(&caller.conversation_id)
            .await?
            .iter()
            .map(|task| worker_row(task, &diffs))
            .collect();
        Ok(WorkersOutput { session, workers })
    }

    /// The session or worker of the caller's project that `id` names.
    async fn cli_target(&self, caller: &CliCaller, id: &str) -> Result<Target, CliReply> {
        let not_found = || {
            CliReply::error(
                ErrorCode::NotFound,
                format!(
                    "no session or worker of this project has the id {id:?}; see `brigadier \
                     sessions` and `brigadier workers`"
                ),
            )
        };
        let id = id.trim();
        if id.is_empty() {
            return Err(not_found());
        }
        let matches =
            |full: &str| full == id || (id.len() >= MIN_ID && short(full).starts_with(id));
        let (_, sessions) = self.cli_project_sessions(caller).map_err(failed)?;
        let mut found: Vec<Target> = Vec::new();
        for session in sessions {
            for task in self.core.tasks(&session.id).await.map_err(failed)? {
                if matches(&task.id.0) {
                    found.push(Target::Worker(Box::new(task)));
                }
            }
            if matches(&session.id.0) {
                found.push(Target::Session(Box::new(session)));
            }
        }
        match found.len() {
            0 => Err(not_found()),
            1 => Ok(found.remove(0)),
            _ => {
                let ids: Vec<String> = found
                    .iter()
                    .map(|target| match target {
                        Target::Session(session) => short(&session.id.0),
                        Target::Worker(task) => short(&task.id.0),
                    })
                    .collect();
                Err(CliReply::error(
                    ErrorCode::Ambiguous,
                    format!("{id:?} could be {}; give more of it", ids.join(", ")),
                ))
            }
        }
    }

    async fn cli_show(&self, caller: &CliCaller, id: &str) -> crate::Result<CliReply> {
        let target = match self.cli_target(caller, id).await {
            Ok(target) => target,
            Err(reply) => return Ok(reply),
        };
        let output = match target {
            Target::Session(conversation) => {
                let activity = self.cli_activity().await;
                let activity = activity.get(&conversation.id);
                let (branch, checkout, base) = session_place(&conversation);
                let choice = match &conversation.setup {
                    Some(Setup::Session { orchestrator, .. }) => Some(orchestrator.clone()),
                    Some(Setup::Chat { model }) => Some(model.clone()),
                    None => None,
                };
                let diffs = self.cli_worker_changes(&conversation.id).await;
                let workers = self
                    .core
                    .tasks(&conversation.id)
                    .await?
                    .iter()
                    .map(|task| worker_row(task, &diffs))
                    .collect();
                ShowOutput {
                    kind: Kind::Session,
                    id: short(&conversation.id.0),
                    title: conversation.title.clone(),
                    state: session_state(&conversation, activity).word().into(),
                    agent: agent(choice.as_ref().map_or(ProviderKind::Claude, |c| c.provider)),
                    model: choice.as_ref().and_then(|choice| choice.model.clone()),
                    effort: choice.as_ref().and_then(|choice| choice.effort.clone()),
                    branch,
                    changes: self.cli_session_changes(&conversation.id).await,
                    created_at_ms: conversation.created_at_ms,
                    checkout,
                    base,
                    waiting: Some(cli::Waiting {
                        questions: activity
                            .map_or(0, |a| u32::try_from(a.questions.len()).unwrap_or(0)),
                        approvals: activity
                            .map_or(0, |a| u32::try_from(a.approvals.len()).unwrap_or(0)),
                    }),
                    workers: Some(workers),
                    parent: None,
                    number: None,
                    task: None,
                    report: None,
                    error: None,
                }
            }
            Target::Worker(task) => {
                let diffs = self.cli_worker_changes(&task.conversation_id).await;
                let row = worker_row(&task, &diffs);
                ShowOutput {
                    kind: Kind::Worker,
                    id: row.id,
                    title: row.title,
                    state: row.state.word().into(),
                    agent: row.agent,
                    model: row.model,
                    effort: row.effort,
                    branch: row.branch,
                    changes: row.changes,
                    created_at_ms: task.created_at_ms,
                    checkout: None,
                    base: None,
                    waiting: None,
                    workers: None,
                    parent: Some(short(&task.conversation_id.0)),
                    number: Some(task.number),
                    task: Some(task.spec.clone()),
                    report: task.report.as_ref().map(|report| report.summary.clone()),
                    error: task.error.clone().or_else(|| task.blocked_reason.clone()),
                }
            }
        };
        Ok(CliReply::ok(&output))
    }

    async fn cli_read(
        &self,
        caller: &CliCaller,
        id: &str,
        last: Option<u32>,
        screen: bool,
    ) -> crate::Result<CliReply> {
        let target = match self.cli_target(caller, id).await {
            Ok(target) => target,
            Err(reply) => return Ok(reply),
        };
        let view = match (last, screen) {
            (Some(_), _) => ReadView::Items,
            (None, true) => ReadView::Screen,
            (None, false) => ReadView::Last,
        };
        let output = match target {
            Target::Session(conversation) => {
                let activity = self.cli_activity().await;
                let state = session_state(&conversation, activity.get(&conversation.id));
                let (items, has_more) =
                    self.cli_session_items(&conversation.id, view, last).await?;
                ReadOutput {
                    kind: Kind::Session,
                    id: short(&conversation.id.0),
                    state: state.word().into(),
                    view,
                    items,
                    has_more,
                }
            }
            Target::Worker(task) => {
                let (items, has_more) = self.cli_worker_items(&task.id, view, last).await?;
                ReadOutput {
                    kind: Kind::Worker,
                    id: short(&task.id.0),
                    state: worker_state(task.state).word().into(),
                    view,
                    items,
                    has_more,
                }
            }
        };
        Ok(CliReply::ok(&output))
    }

    /// A session's messages: its last answer, its last `last`, or its latest request with the
    /// thread's own commands and tools in it.
    async fn cli_session_items(
        &self,
        id: &ConversationId,
        view: ReadView,
        last: Option<u32>,
    ) -> crate::Result<(Vec<Item>, bool)> {
        let limit = last.unwrap_or(LOOK_BACK);
        let page = self.core.list_messages(id.clone(), None, limit).await?;
        let messages = page.messages;
        Ok(match view {
            ReadView::Items => (messages.iter().map(message_item).collect(), page.has_more),
            ReadView::Last => match messages
                .iter()
                .rposition(|message| message.role == MessageRole::Assistant)
            {
                Some(at) => (vec![message_item(&messages[at])], at > 0 || page.has_more),
                None => (Vec::new(), page.has_more),
            },
            ReadView::Screen => {
                let from = messages
                    .iter()
                    .rposition(|message| message.role == MessageRole::User)
                    .unwrap_or(0);
                let mut items: Vec<Item> = messages[from..].iter().map(message_item).collect();
                if let Some(request) = messages
                    .get(from)
                    .and_then(|message| message.request_id.clone())
                {
                    let steps = self
                        .thread_items(id, &request, None, None, LOOK_BACK)
                        .await
                        .map(|page| page.entries)
                        .unwrap_or_default();
                    items.extend(
                        self::items(&steps)
                            .into_iter()
                            .filter(|item| item.kind != ItemKind::Message),
                    );
                    items.sort_by_key(|item| item.at_ms);
                }
                (items, from > 0 || page.has_more)
            }
        })
    }

    /// A worker's transcript: its last message, its last `last` items, or its latest turn.
    async fn cli_worker_items(
        &self,
        id: &TaskId,
        view: ReadView,
        last: Option<u32>,
    ) -> crate::Result<(Vec<Item>, bool)> {
        // Most events aren't items (deltas, usage): read well past what is shown.
        let limit = last.map_or(LOOK_BACK * 5, |n| {
            n.saturating_mul(8).clamp(LOOK_BACK, 5000)
        });
        let page = self.core.list_worker_events(id, None, limit).await?;
        let entries = &page.entries;
        Ok(match view {
            ReadView::Items => {
                let items = items(entries);
                let n = last.unwrap_or(LOOK_BACK) as usize;
                let skip = items.len().saturating_sub(n);
                (items[skip..].to_vec(), skip > 0 || page.has_more)
            }
            ReadView::Last => {
                let found =
                    entries
                        .iter()
                        .enumerate()
                        .rev()
                        .find_map(|(at, entry)| match &entry.event {
                            ProviderEvent::Message {
                                role: brigadier_providers::model::Role::Assistant,
                                ..
                            } => event_item(&entry.event, entry.at_ms).map(|item| (at, item)),
                            _ => None,
                        });
                match found {
                    Some((at, item)) => (vec![item], at > 0 || page.has_more),
                    None => (Vec::new(), page.has_more),
                }
            }
            ReadView::Screen => {
                let from = entries
                    .iter()
                    .rposition(|entry| matches!(entry.event, ProviderEvent::TurnStarted { .. }))
                    .unwrap_or(0);
                (items(&entries[from..]), from > 0 || page.has_more)
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_providers::model::ItemStatus as Status;

    fn entry(at_ms: i64, event: ProviderEvent) -> crate::model::RawEntry {
        crate::model::RawEntry {
            stream_seq: at_ms,
            at_ms,
            event,
        }
    }

    fn tool(status: Status, output: Option<&str>) -> ProviderEvent {
        ProviderEvent::ToolCall {
            item_id: "t1".into(),
            name: "Read".into(),
            input: Some("{}".into()),
            status,
            output: output.map(Into::into),
            kept: None,
        }
    }

    /// A call is stored when it starts and again when it ends: it reads once, as it ended.
    #[test]
    fn a_tool_call_reads_once_as_its_latest_event() {
        let read = items(&[
            entry(1, tool(Status::InProgress, None)),
            entry(
                2,
                ProviderEvent::MessageDelta {
                    item_id: "m".into(),
                    text: "par".into(),
                    phase: None,
                },
            ),
            entry(3, tool(Status::Completed, Some("done"))),
            entry(
                4,
                ProviderEvent::Message {
                    item_id: "m".into(),
                    role: brigadier_providers::model::Role::Assistant,
                    text: "All read.".into(),
                },
            ),
        ]);
        assert_eq!(read.len(), 2, "{read:?}");
        assert_eq!(read[0].kind, ItemKind::Tool);
        assert_eq!(read[0].status, Some(cli::ItemStatus::Done));
        assert_eq!(read[0].output.as_deref(), Some("done"));
        assert_eq!(read[0].at_ms, 3);
        assert_eq!(read[1].text, "All read.");
    }

    #[test]
    fn long_output_keeps_its_end() {
        let text = format!("{}é tail", "x".repeat(OUTPUT_TAIL));
        let cut = tail(&text, OUTPUT_TAIL);
        assert!(cut.starts_with('…') && cut.ends_with("é tail"));
        assert!(cut.len() <= OUTPUT_TAIL + '…'.len_utf8());
        assert_eq!(short("0199f0aa-0000-7000-8000-00000000c0de"), "0000c0de");
    }
}
