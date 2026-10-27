//! `brigadier context` (D13): everything a thread or worker needs about its job, on pull. Nothing
//! of it is put into a conversation; the caller runs it when it starts and after a compaction.
//! It is a front door: each part is the code of the command it stands for (`ledger show`,
//! `transcript`, `checkpoint show`, `models`, the registry's catalog), as the caller's own.
//!
//! A worker reads its session's ledger and checkpoint, read-only, and then its own ledger. Its
//! transcript is its own records only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::plugins::resolve::{Resolved, Role as PluginRole};
use crate::accounts::AccountRef;
use brigadier_cli::protocol::{
    CliReply, ContextEntry, ContextFile, ContextItem, ContextOutput, ContextPart, ContextPartSize,
    ErrorCode,
};
use brigadier_providers::{McpStatus, McpTransport};

use super::SessionManager;
use super::cli::CliCaller;
use super::cli_ledger::{Entry, Kind, Ledger, LedgerFile};
use super::long_sessions::checkpoint;
use super::records::{RecordRead, RecordSearch};
use crate::model::{ConversationId, MessageRole, Setup};
use crate::tools::Role;
use crate::work::{Approval, ApprovalSubject, CardState, Question, Task};
use crate::{Error, Result};

/// The transcript items `context transcript` gives when not told how many.
const LAST: u32 = 20;

/// The repository's own instruction files a CLI reads at its root (D17).
const INSTRUCTION_FILES: [&str; 2] = ["CLAUDE.md", "AGENTS.md"];

/// Who calls, resolved once for every part.
struct Whose {
    conversation: ConversationId,
    /// A worker's task.
    task: Option<Task>,
    /// The session's `record/`: a worker's session's.
    session_record: PathBuf,
    /// A worker's own `record/`.
    own_record: Option<PathBuf>,
    /// How the records name its own transcript.
    key: String,
    account: AccountRef,
    /// Where its CLI works, when it has a checkout.
    checkout: Option<PathBuf>,
    /// The project's repository, while the user trusts it: the setup its CLI loads.
    repo: Option<PathBuf>,
}

impl Whose {
    fn worker(&self) -> bool {
        self.task.is_some()
    }
}

impl SessionManager {
    /// `brigadier context [<part>]`, for `caller`.
    pub(super) async fn cli_context(
        &self,
        caller: &CliCaller,
        part: Option<ContextPart>,
        name: Option<String>,
        grep: Option<String>,
        last: Option<u32>,
        item: Option<String>,
    ) -> Result<CliReply> {
        if !matches!(
            self.grants.resolve(&caller.grant),
            Some(Role::Orchestrator { .. } | Role::Worker { .. } | Role::Chat { .. })
        ) {
            return Ok(CliReply::error(
                ErrorCode::Refused,
                "refused: `brigadier context` is for a session's thread, a Chat and their \
                 workers",
            ));
        }
        let whose = self.context_caller(caller).await?;
        let output = match part {
            None => self.context_overview(&whose).await?,
            Some(ContextPart::Transcript) => {
                self.context_transcript(&whose, grep, last, item).await?
            }
            Some(ContextPart::Skills) => self.context_skills(&whose, name.as_deref()).await?,
            Some(part) => self.context_part(&whose, part).await?,
        };
        Ok(CliReply::ok(&output))
    }

    async fn context_caller(&self, caller: &CliCaller) -> Result<Whose> {
        let conversation = self.core.conversation(&caller.conversation_id)?;
        let folder = self.session_dir(&conversation.id);
        let repo = self.trusted_repo(&conversation.id);
        let choice = match &conversation.setup {
            Some(Setup::Session { orchestrator, .. }) => orchestrator.clone(),
            Some(Setup::Chat { model, .. }) => model.clone(),
            None => self.default_chat_choice(),
        };
        let thread_checkout = self
            .recorded_workspace(&conversation.id)
            .map(|workspace| workspace.path);
        let Some(task_id) = &caller.task_id else {
            let live = self.convs_lock().get(&conversation.id).cloned();
            let account = match live {
                Some(live) => live.cli().await.map(|cli| cli.account.clone()),
                None => None,
            }
            .unwrap_or_else(|| self.runtime.account_for(&choice));
            return Ok(Whose {
                key: conversation.id.short().to_owned(),
                conversation: conversation.id,
                task: None,
                session_record: folder.record(),
                own_record: None,
                account,
                checkout: thread_checkout,
                repo,
            });
        };
        let task = self.task_by_id(&conversation.id, task_id).await?;
        let worker = folder.worker(&task);
        let account = match self.existing_task_live(task_id) {
            Some(live) => live.cli().await.map(|cli| cli.account.clone()),
            None => None,
        }
        .unwrap_or_else(|| self.runtime.account_for(&task.route.choice));
        Ok(Whose {
            key: self.worker_records_key(&conversation.id, task_id).await?,
            conversation: conversation.id,
            session_record: folder.record(),
            own_record: Some(worker.join("record")),
            account,
            checkout: task
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.worktree.as_ref())
                .map(PathBuf::from),
            repo,
            task: Some(task),
        })
    }

    /// How the records name worker `task`'s own transcript: `<short>/<n>-<task8>`.
    pub(super) async fn worker_records_key(
        &self,
        conversation: &ConversationId,
        task: &crate::model::TaskId,
    ) -> Result<String> {
        let task = self.task_by_id(conversation, task).await?;
        let folder = self.session_dir(conversation).worker(&task);
        let name = folder
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        Ok(format!("{}/{name}", conversation.short()))
    }

    /// The parts, one line each, with their sizes and the commands that open them.
    async fn context_overview(&self, whose: &Whose) -> Result<ContextOutput> {
        let mut parts = Vec::new();
        for part in ContextPart::ALL {
            let (entries, lines, tokens) = match part {
                // Read whole only when asked: its size is its item count.
                ContextPart::Transcript => {
                    let items = self.records_len(&whose.conversation, &whose.key).await;
                    (Some(count(items)), 0, 0)
                }
                ContextPart::Skills => {
                    let output = self.context_skills(whose, None).await?;
                    (
                        Some(count(output.items.len())),
                        lines(&output.text),
                        tokens(&output.text),
                    )
                }
                part => {
                    let output = self.context_part(whose, part).await?;
                    let entries = match part {
                        ContextPart::Decisions | ContextPart::Open | ContextPart::Ledger => {
                            Some(count(output.entries.len()))
                        }
                        ContextPart::Mcps => Some(count(output.items.len())),
                        ContextPart::Project => Some(count(output.files.len())),
                        _ => None,
                    };
                    (entries, lines(&output.text), tokens(&output.text))
                }
            };
            parts.push(ContextPartSize {
                part: part.word().to_owned(),
                summary: summary(part, whose.worker()).to_owned(),
                entries,
                lines,
                tokens,
                command: format!("brigadier context {}", part.word()),
            });
        }
        let mut text = String::from(
            "Everything about your job is here, on pull: open what you need. Run \
             `brigadier context` again after any compaction.\n\n",
        );
        let width = parts.iter().map(|part| part.part.len()).max().unwrap_or(0);
        for part in &parts {
            let size = match (part.part.as_str(), part.entries) {
                ("transcript", Some(items)) => plural(items, "item"),
                (name, Some(entries)) => {
                    let noun = match name {
                        "skills" => "skill",
                        "mcps" => "server",
                        "project" => "file",
                        _ => "entry",
                    };
                    format!("{} · ~{} tokens", plural(entries, noun), part.tokens)
                }
                (_, None) => format!("{} · ~{} tokens", plural(part.lines, "line"), part.tokens),
            };
            text.push_str(&format!(
                "  {:width$}  {} ({size})\n",
                part.part, part.summary
            ));
        }
        text.push_str(
            "\nOpen one with `brigadier context <part>`; each takes --json. `brigadier context \
             --help` says more.\n",
        );
        Ok(ContextOutput {
            part: "overview".into(),
            text,
            parts,
            entries: Vec::new(),
            items: Vec::new(),
            files: Vec::new(),
            models: None,
        })
    }

    /// Every part but `transcript` and `skills`, which take arguments.
    async fn context_part(&self, whose: &Whose, part: ContextPart) -> Result<ContextOutput> {
        let mut output = ContextOutput {
            part: part.word().to_owned(),
            text: String::new(),
            parts: Vec::new(),
            entries: Vec::new(),
            items: Vec::new(),
            files: Vec::new(),
            models: None,
        };
        match part {
            ContextPart::Waiting => {
                let work = self.tracked_work(&whose.conversation).await?;
                output.text = work
                    .waiting_on
                    .iter()
                    .filter(|w| w.done_at_ms.is_none())
                    .map(|w| {
                        format!(
                            "{} · {}{}",
                            w.id,
                            w.text,
                            w.by.as_ref()
                                .map(|d| format!(" · by {d} ({})", w.timezone))
                                .unwrap_or_default()
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                if output.text.is_empty() {
                    output.text = "No explicit waits.\n".into();
                } else {
                    output.text.push('\n');
                }
            }
            ContextPart::Brief => output.text = self.context_brief(whose).await?,
            ContextPart::Decisions | ContextPart::Open | ContextPart::Ledger => {
                let (session, own) = self.context_ledgers(whose).await?;
                let cards = match part {
                    ContextPart::Open => self.waiting_cards(&whose.conversation).await,
                    _ => Vec::new(),
                };
                let (text, entries) =
                    ledger_part(part, whose.worker(), &session, own.as_ref(), cards);
                output.text = text;
                output.entries = entries;
            }
            ContextPart::Checkpoint => {
                let record = whose.session_record.clone();
                let text = super::blocking(move || Ok(checkpoint::read(&record))).await?;
                output.text = match (text, whose.worker()) {
                    (Some(text), false) => format!("{text}\n"),
                    (Some(text), true) => format!("Your session's checkpoint:\n\n{text}\n"),
                    (None, _) => "No checkpoint set.\n".into(),
                };
            }
            ContextPart::Mcps => {
                output.items = self.context_mcps(whose).await?;
                output.text = items_text(
                    &output.items,
                    "MCP servers",
                    "No MCP servers in this setup.",
                    whose.worker(),
                );
            }
            ContextPart::Project => {
                let (text, files) = self.context_project(whose).await;
                output.text = text;
                output.files = files;
            }
            ContextPart::Routing => {
                let caller = CliCaller {
                    conversation_id: whose.conversation.clone(),
                    task_id: whose.task.as_ref().map(|task| task.id.clone()),
                    grant: String::new(),
                };
                let models = self.cli_models(&caller).await?;
                output.text = brigadier_cli::render::models(&models);
                output.models = Some(models);
            }
            ContextPart::Transcript | ContextPart::Skills => {
                return Err(Error::Invalid(format!(
                    "{} takes its own call",
                    part.word()
                )));
            }
        }
        Ok(output)
    }

    /// A worker's spec and what its session sent it since; a session's first request.
    async fn context_brief(&self, whose: &Whose) -> Result<String> {
        if let Some(task) = &whose.task {
            let mut text = format!("{}\n", task.spec.trim_end());
            if !task.messages.is_empty() {
                text.push_str("\nSent to you since, oldest first:\n");
                for message in &task.messages {
                    text.push_str(&format!("\n{}\n", message.trim_end()));
                }
            }
            return Ok(text);
        }
        let messages = self.core.all_messages(&whose.conversation).await?;
        match messages
            .iter()
            .find(|message| message.role == MessageRole::User)
        {
            Some(first) => Ok(format!("{}\n", self.full_text(first).await.trim_end())),
            None => Ok("No request yet.\n".into()),
        }
    }

    /// The session's ledger and, for a worker, its own.
    async fn context_ledgers(&self, whose: &Whose) -> Result<(Ledger, Option<Ledger>)> {
        let (session, own) = (whose.session_record.clone(), whose.own_record.clone());
        super::blocking(move || {
            let own = match own {
                Some(own) => Some(Ledger::read(&own)?),
                None => None,
            };
            Ok((Ledger::read(&session)?, own))
        })
        .await
    }

    /// The conversation's question and approval cards still waiting on the user.
    async fn waiting_cards(&self, conversation: &ConversationId) -> Vec<ContextEntry> {
        let Ok(board) = self.core.board(conversation).await else {
            return Vec::new();
        };
        let mut questions: Vec<&Question> =
            board.questions.values().filter(|q| q.is_open()).collect();
        questions.sort_by_key(|question| question.position);
        let mut approvals: Vec<&Approval> = board
            .approvals
            .values()
            .filter(|approval| approval.state == CardState::Pending)
            .collect();
        approvals.sort_by_key(|approval| approval.position);
        let questions = questions.into_iter().map(|question| ContextEntry {
            id: Some(question.id.0.clone()),
            kind: "question".into(),
            source: "card".into(),
            text: question.text.clone(),
        });
        let approvals = approvals.into_iter().map(|approval| ContextEntry {
            id: Some(approval.id.0.clone()),
            kind: "approval".into(),
            source: "card".into(),
            text: approval_text(&approval.subject),
        });
        questions.chain(approvals).collect()
    }

    /// `context transcript`: the caller's own records, by the code `brigadier transcript` runs.
    async fn context_transcript(
        &self,
        whose: &Whose,
        grep: Option<String>,
        last: Option<u32>,
        item: Option<String>,
    ) -> Result<ContextOutput> {
        let text = match (grep, item) {
            (Some(query), _) => {
                self.records_search(
                    &whose.conversation,
                    Some(whose.key.clone()),
                    RecordSearch {
                        query,
                        session: None,
                        limit: None,
                        role: None,
                        tool: None,
                    },
                )
                .await?
            }
            (None, Some(item)) => {
                self.records_read(
                    &whose.conversation,
                    Some(whose.key.clone()),
                    RecordRead {
                        session: whose.key.clone(),
                        item: Some(item),
                        ..RecordRead::default()
                    },
                )
                .await?
            }
            (None, None) => {
                self.records_last(&whose.conversation, &whose.key, last.unwrap_or(LAST))
                    .await?
            }
        };
        let text = if text.ends_with('\n') {
            text
        } else {
            format!("{text}\n")
        };
        Ok(ContextOutput {
            part: "transcript".into(),
            text,
            parts: Vec::new(),
            entries: Vec::new(),
            items: Vec::new(),
            files: Vec::new(),
            models: None,
        })
    }

    /// Resolve exactly as launch does, including grants-only workers and effective homes.
    async fn context_resolved(&self, whose: &Whose) -> Result<Resolved> {
        let cwd = self.setup_folder(whose);
        let trusted = self.repo_trust(&whose.conversation) == Some(true);
        let all = self
            .resolve_plugins(
                &whose.account,
                whose.repo.as_deref(),
                &cwd,
                trusted,
                PluginRole::Thread,
            )
            .await?;
        if let Some(plain) = whose.task.as_ref().and_then(|task| task.plain.as_ref()) {
            let grants: Vec<_> = all
                .servers
                .into_iter()
                .filter(|server| plain.mcps.contains(&server.name))
                .collect();
            self.resolve_plugins(
                &whose.account,
                whose.repo.as_deref(),
                &cwd,
                trusted,
                PluginRole::Worker { grants: &grants },
            )
            .await
        } else {
            Ok(all)
        }
    }

    /// `context skills [<name>]`: only selected skills; read the selected version by its
    /// display name, row identity, or exact provider invocation.
    async fn context_skills(&self, whose: &Whose, name: Option<&str>) -> Result<ContextOutput> {
        let resolved = self.context_resolved(whose).await?;
        let (name, worker) = (name.map(str::to_owned), whose.worker());
        super::blocking(move || skills_output(&resolved, name.as_deref(), worker)).await
    }

    /// Status annotates the resolved set; stale CLI reports never add or remove items.
    async fn context_mcps(&self, whose: &Whose) -> Result<Vec<ContextItem>> {
        let resolved = self.context_resolved(whose).await?;
        let reported: Option<HashMap<String, McpStatus>> = self
            .core
            .board(&whose.conversation)
            .await
            .ok()
            .and_then(|board| {
                let task = whose.task.as_ref().map(|task| task.id.0.as_str());
                board
                    .mcp_servers_of(task)
                    .filter(|agent| agent.provider == whose.account.provider)
                    .map(|agent| {
                        agent
                            .servers
                            .iter()
                            .map(|server| (server.name.clone(), server.status))
                            .collect()
                    })
            });
        Ok(mcp_items(&resolved, reported.as_ref()))
    }

    /// `context project`: the checkout's own instruction files, whole (D17), and the project's
    /// facts.
    async fn context_project(&self, whose: &Whose) -> (String, Vec<ContextFile>) {
        let conversation = self.core.conversation(&whose.conversation).ok();
        let project = conversation
            .as_ref()
            .and_then(|conversation| conversation.project_id.as_ref())
            .and_then(|id| self.core.project(id).ok());
        let mut text = String::new();
        if let Some(project) = &project {
            text.push_str(&format!("Project: {}\n", project.name));
            for repo in &project.repos {
                text.push_str(&format!("Repository: {}\n", repo.path));
            }
        }
        if let Some(checkout) = &whose.checkout {
            text.push_str(&format!("Your checkout: {}\n", checkout.display()));
        }
        match whose.task.as_ref().and_then(|task| task.workspace.as_ref()) {
            Some(workspace) => {
                if let Some(branch) = &workspace.branch {
                    text.push_str(&format!("Your branch: {branch}\n"));
                }
                if let Some(target) = &workspace.target {
                    text.push_str(&format!("It merges into: {target}\n"));
                }
            }
            None => {
                if let Some(workspace) = self.recorded_workspace(&whose.conversation) {
                    text.push_str(&format!("Branch: {}\n", workspace.branch));
                }
            }
        }
        let checkout = whose.checkout.clone();
        let files = super::blocking(move || Ok(instruction_files(checkout.as_deref())))
            .await
            .unwrap_or_default();
        if text.is_empty() && files.is_empty() {
            return (
                "No project: this conversation has no checkout.\n".into(),
                files,
            );
        }
        if files.is_empty() {
            text.push_str("\nThe checkout has no CLAUDE.md or AGENTS.md.\n");
        }
        for file in &files {
            text.push_str(&format!(
                "\n── {} ({} lines) ──\n{}",
                file.path, file.lines, file.text
            ));
            if !file.text.ends_with('\n') {
                text.push('\n');
            }
        }
        (text, files)
    }

    /// The folder whose setup the caller's CLI loads.
    fn setup_folder(&self, whose: &Whose) -> PathBuf {
        whose
            .checkout
            .clone()
            .or_else(|| {
                whose
                    .task
                    .as_ref()
                    .and_then(|task| task.workspace.as_ref())
                    .map(|workspace| PathBuf::from(&workspace.scratch))
            })
            .unwrap_or_else(|| self.session_dir(&whose.conversation).work())
    }
}

/// Context is a view of the current resolver result, not a second catalog policy.
fn skills_output(resolved: &Resolved, name: Option<&str>, worker: bool) -> Result<ContextOutput> {
    let mut output = ContextOutput {
        part: "skills".into(),
        text: String::new(),
        parts: Vec::new(),
        entries: Vec::new(),
        items: Vec::new(),
        files: Vec::new(),
        models: None,
    };
    if let Some(name) = name {
        let skill = resolved
            .skills
            .iter()
            .find(|skill| skill.display == name || skill.invocation == name || skill.row_id == name)
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "no skill {name:?} here; `brigadier context skills` lists them"
                ))
            })?;
        let text = std::fs::read_to_string(&skill.path)
            .map_err(|err| Error::Invalid(format!("couldn't read the skill {name:?}: {err}")))?;
        output.text = if text.ends_with('\n') {
            text
        } else {
            format!("{text}\n")
        };
        return Ok(output);
    }
    output.items = resolved
        .skills
        .iter()
        .map(|skill| ContextItem {
            name: skill.display.clone(),
            kind: "skill".into(),
            has: true,
            status: None,
            detail: if skill.invocation == skill.display {
                skill.path.display().to_string()
            } else {
                format!("{}; invoke as {}", skill.path.display(), skill.invocation)
            },
        })
        .collect();
    output.text = items_text(&output.items, "Skills", "No skills in this setup.", worker);
    Ok(output)
}

fn mcp_items(
    resolved: &Resolved,
    reported: Option<&HashMap<String, McpStatus>>,
) -> Vec<ContextItem> {
    resolved
        .servers
        .iter()
        .map(|server| ContextItem {
            name: server.name.clone(),
            kind: "server".into(),
            has: true,
            detail: match &server.transport {
                McpTransport::Http { .. } => "HTTP".into(),
                McpTransport::Stdio { .. } => "stdio".into(),
            },
            status: reported
                .and_then(|reported| reported.get(&server.name))
                .map(|status| status_word(*status).to_owned()),
        })
        .collect()
}

/// `decisions`, `open` or `ledger`, as text and as entries.
fn ledger_part(
    part: ContextPart,
    worker: bool,
    session: &Ledger,
    own: Option<&Ledger>,
    cards: Vec<ContextEntry>,
) -> (String, Vec<ContextEntry>) {
    let pick = |ledger: &Ledger| -> Vec<Entry> {
        match part {
            ContextPart::Decisions => ledger.rules().into_iter().cloned().collect(),
            ContextPart::Open => ledger.open().into_iter().cloned().collect(),
            _ => ledger
                .entries
                .iter()
                .filter(|entry| entry.file == LedgerFile::Ledger)
                .cloned()
                .collect(),
        }
    };
    let (title, of) = match part {
        ContextPart::Decisions => ("Rules and decisions still standing", " in "),
        ContextPart::Open => ("Open items", " in "),
        _ => ("", ""),
    };
    let mut text = String::new();
    let mut entries = Vec::new();
    let mut section = |heading: String, picked: Vec<Entry>, source: &str| {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("{heading} ({}):\n", picked.len()));
        if picked.is_empty() {
            text.push_str("None.\n");
        }
        for entry in picked {
            text.push_str(&entry.text);
            text.push('\n');
            entries.push(ContextEntry {
                id: entry.id.map(|id| format!("L{id}")),
                kind: kind_word(&entry.kind).into(),
                source: source.into(),
                text: entry.text,
            });
        }
    };
    if worker {
        let heading = format!("{title}{of}your session's ledger, read-only");
        section(capitalized(&heading), pick(session), "session");
        if let Some(own) = own.filter(|own| !own.entries.is_empty()) {
            section(
                capitalized(&format!("{title}{of}your own ledger")),
                pick(own),
                "own",
            );
        }
    } else if title.is_empty() {
        section("Ledger".into(), pick(session), "session");
    } else {
        section(title.to_owned(), pick(session), "session");
    }
    if part == ContextPart::Open {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(&format!("Waiting on the user ({}):\n", cards.len()));
        if cards.is_empty() {
            text.push_str("None.\n");
        }
        for card in cards {
            text.push_str(&format!(
                "- {} {}: {}\n",
                card.kind,
                card.id.as_deref().unwrap_or_default(),
                card.text.lines().collect::<Vec<_>>().join(" / ")
            ));
            entries.push(card);
        }
    }
    if worker && part == ContextPart::Decisions {
        text.push_str("\nYour brief may carry more: `brigadier context brief`.\n");
    }
    (text, entries)
}

/// Skills or servers as text: what the caller has first.
fn items_text(items: &[ContextItem], title: &str, none: &str, worker: bool) -> String {
    if items.is_empty() {
        return format!("{none}\n");
    }
    let mut text = String::new();
    let (has, not): (Vec<_>, Vec<_>) = items.iter().partition(|item| item.has);
    let line = |item: &ContextItem| {
        let status = item
            .status
            .as_ref()
            .map(|status| format!(" [{status}]"))
            .unwrap_or_default();
        if item.detail.is_empty() {
            format!("- {}{status}\n", item.name)
        } else {
            format!("- {}{status}: {}\n", item.name, item.detail)
        }
    };
    text.push_str(&format!("{title} you have ({}):\n", has.len()));
    for item in &has {
        text.push_str(&line(item));
    }
    if !not.is_empty() {
        text.push_str(&format!("\n{title} you don't have ({}):\n", not.len()));
        for item in &not {
            text.push_str(&line(item));
        }
    }
    if title == "Skills" {
        text.push_str("\n`brigadier context skills <name>` prints one whole.\n");
    } else if worker {
        text.push_str(
            "\nA worker has the servers it was started with (`brigadier start --mcp`); ask your \
             session in your last message for one you need.\n",
        );
    }
    text
}

/// The checkout's root instruction files, whole.
fn instruction_files(checkout: Option<&Path>) -> Vec<ContextFile> {
    let Some(checkout) = checkout else {
        return Vec::new();
    };
    INSTRUCTION_FILES
        .iter()
        .filter_map(|name| {
            let text = std::fs::read_to_string(checkout.join(name)).ok()?;
            Some(ContextFile {
                path: (*name).to_owned(),
                lines: lines(&text),
                text,
            })
        })
        .collect()
}

/// What an approval card asks, in a line.
fn approval_text(subject: &ApprovalSubject) -> String {
    match subject {
        ApprovalSubject::Action { action, .. } => action.clone(),
        ApprovalSubject::Browser { question, .. } => question.clone(),
        ApprovalSubject::PeerMessage { title, .. } => format!("Send a message to {title}?"),
        ApprovalSubject::Cli { .. } => "A worker's permission prompt".into(),
        ApprovalSubject::Landing { branch, .. } => format!("Land {branch}?"),
        ApprovalSubject::FinishSession { branch, base, .. } => {
            format!("Merge {branch} into {base}?")
        }
        ApprovalSubject::OutwardCommand { argv, .. } => format!("Run `{}`?", argv.join(" ")),
        ApprovalSubject::Outline { title, .. } => format!("Start {title}?"),
        ApprovalSubject::Plan { .. } => "Build the plan?".into(),
    }
}

fn kind_word(kind: &Kind) -> &'static str {
    match kind {
        Kind::Rule => "rule",
        Kind::Decision => "decision",
        Kind::Open => "open",
        Kind::Close(_) => "close",
        Kind::Other => "other",
    }
}

fn status_word(status: McpStatus) -> &'static str {
    match status {
        McpStatus::Connected => "connected",
        McpStatus::Connecting => "connecting",
        McpStatus::NeedsAuth => "needsAuth",
        McpStatus::Failed => "failed",
    }
}

fn summary(part: ContextPart, worker: bool) -> &'static str {
    match (part, worker) {
        (ContextPart::Waiting, _) => "Explicit non-PR waits and durable IDs",
        (ContextPart::Brief, true) => "Your task as `brigadier start` gave it",
        (ContextPart::Brief, false) => "The session's first request",
        (ContextPart::Decisions, true) => "Your session's rules and decisions, quoted",
        (ContextPart::Decisions, false) => "Rules and decisions still standing, quoted",
        (ContextPart::Open, _) => "Open items and cards waiting on the user",
        (ContextPart::Ledger, true) => "Your session's whole ledger, then yours",
        (ContextPart::Ledger, false) => "The whole ledger",
        (ContextPart::Checkpoint, _) => "The session's checkpoint",
        (ContextPart::Transcript, _) => "Your own records: --last N, --grep, --item",
        (ContextPart::Skills, _) => "Skills of your setup; one whole by name",
        (ContextPart::Mcps, _) => "MCP servers, and which you have",
        (ContextPart::Project, _) => "CLAUDE.md, AGENTS.md and the project's facts",
        (ContextPart::Routing, _) => "Models, usage and routing guidance",
    }
}

/// `1 entry`, `2 entries`.
fn plural(n: u32, noun: &str) -> String {
    match (n, noun.strip_suffix('y')) {
        (1, _) => format!("1 {noun}"),
        (_, Some(stem)) => format!("{n} {stem}ies"),
        (_, None) => format!("{n} {noun}s"),
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

fn lines(text: &str) -> u32 {
    count(text.lines().count())
}

fn tokens(text: &str) -> u32 {
    count(text.chars().count().div_ceil(4))
}

#[cfg(test)]
mod tests {
    use super::super::plugins::{
        choices::Choices,
        resolve::{self, Context},
    };
    use brigadier_registry::{Agent, Catalog, Roots};

    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn skill(dir: &Path, name: &str, body: &str) {
        write(
            &dir.join("SKILL.md"),
            &format!("---\nname: {name}\ndescription: {name} things\n---\n{body}\n"),
        );
    }

    /// A home with a user skill, a Claude plugin of two skills, a project skill named as the
    /// user's, two Claude servers and a Codex one; removed when dropped.
    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Self {
            let home = std::env::temp_dir().join(format!(
                "20261025-1046-harness-research-w03-{name}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&home);
            std::fs::create_dir_all(&home).unwrap();
            let home = std::fs::canonicalize(&home).unwrap();
            skill(
                &home.join(".claude/skills/review"),
                "review",
                "Read it twice.",
            );
            let plugin = home.join(".claude/plugins/cache/market/tools/1.0.0");
            write(
                &plugin.join(".claude-plugin/plugin.json"),
                r#"{"name": "tools"}"#,
            );
            skill(&plugin.join("skills/lint"), "lint", "Run the linter.");
            skill(
                &plugin.join("skills/format"),
                "format",
                "Run the formatter.",
            );
            write(
                &home.join(".claude/plugins/installed_plugins.json"),
                &format!(
                    r#"{{"plugins": {{"tools@market": [{{"scope": "user", "installPath": "{}"}}]}}}}"#,
                    plugin.display()
                ),
            );
            skill(
                &home.join("repo/.claude/skills/review"),
                "review",
                "The project's way.",
            );
            write(
                &home.join(".claude.json"),
                r#"{"mcpServers": {"pencil": {"command": "p"}, "railway": {"command": "r"}}}"#,
            );
            write(
                &home.join(".codex/config.toml"),
                "[mcp_servers.calc]\ncommand = \"c\"\n",
            );
            Self(home)
        }

        fn resolved(&self, agent: Agent, choices: &Choices, role: PluginRole<'_>) -> Resolved {
            let roots =
                Roots::from_vars([("HOME".to_owned(), self.0.display().to_string())]).unwrap();
            let repo = self.0.join("repo");
            let rows = Catalog::read_rows(&roots, std::slice::from_ref(&repo));
            resolve::resolve(Context {
                agent,
                account_home: &roots,
                project_root: Some(&repo),
                cwd: &repo,
                trusted: true,
                role,
                rows: &rows,
                choices,
                gateway: None,
            })
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn names(items: &[ContextItem], has: bool) -> Vec<String> {
        let mut names: Vec<String> = items
            .iter()
            .filter(|item| item.has == has)
            .map(|item| item.name.clone())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn skills_report_selected_versions_with_plain_names_and_exact_invocations() {
        let home = Home::new("skills");
        let resolved = home.resolved(Agent::Claude, &Choices::default(), PluginRole::Thread);
        let listed = skills_output(&resolved, None, false).unwrap();
        assert_eq!(names(&listed.items, true), ["format", "lint", "review"]);
        assert!(listed.items.iter().all(|item| item.has));
        assert!(
            listed
                .items
                .iter()
                .find(|item| item.name == "lint")
                .unwrap()
                .detail
                .contains("tools:lint")
        );
        let review = skills_output(&resolved, Some("review"), false).unwrap();
        assert!(review.text.contains("The project's way."));
        assert!(!review.text.contains("Read it twice."));
        for skill in &resolved.skills {
            for name in [&skill.display, &skill.invocation, &skill.row_id] {
                assert_eq!(
                    skills_output(&resolved, Some(name), false)
                        .unwrap()
                        .text
                        .trim_end(),
                    std::fs::read_to_string(&skill.path).unwrap().trim_end()
                );
            }
        }
        assert!(skills_output(&resolved, Some("not-resolved"), false).is_err());
        let root = Roots::from_vars([("HOME".into(), home.0.display().to_string())]).unwrap();
        let rows = Catalog::read_rows(&root, &[home.0.join("repo")]);
        let lint = rows.iter().find(|row| row.name == "lint").unwrap();
        let mut choices = Choices::default();
        choices.0.insert(
            lint.id.clone(),
            super::super::plugins::choices::Choice {
                removed: None,
                on: false,
                scope: super::super::plugins::choices::Scope::Global(
                    super::super::plugins::choices::Global::Global,
                ),
            },
        );
        let off = home.resolved(Agent::Claude, &choices, PluginRole::Thread);
        assert!(
            !names(&skills_output(&off, None, false).unwrap().items, true)
                .contains(&"lint".to_owned())
        );
        assert!(skills_output(&off, Some("tools:lint"), false).is_err());
    }

    #[test]
    fn mcps_include_injected_servers_and_ignore_stale_report_membership() {
        let home = Home::new("mcps");
        for agent in [Agent::Claude, Agent::Codex] {
            let resolved = home.resolved(agent, &Choices::default(), PluginRole::Thread);
            let expected = vec!["calc".to_owned(), "pencil".to_owned(), "railway".to_owned()];
            assert_eq!(names(&mcp_items(&resolved, None), true), expected);
            let reported = HashMap::from([
                ("pencil".into(), McpStatus::NeedsAuth),
                ("stale-native".into(), McpStatus::Connected),
            ]);
            let listed = mcp_items(&resolved, Some(&reported));
            assert_eq!(names(&listed, true), expected);
            assert_eq!(
                listed
                    .iter()
                    .find(|item| item.name == "pencil")
                    .unwrap()
                    .status
                    .as_deref(),
                Some("needsAuth")
            );
            assert!(
                listed
                    .iter()
                    .find(|item| item.name == "calc")
                    .unwrap()
                    .status
                    .is_none()
            );
        }
    }

    #[test]
    fn worker_context_contains_only_grants_and_cannot_read_ungranted_skills() {
        let home = Home::new("worker");
        let thread = home.resolved(Agent::Claude, &Choices::default(), PluginRole::Thread);
        let grants: Vec<_> = thread
            .servers
            .into_iter()
            .filter(|server| server.name == "pencil")
            .collect();
        let worker = home.resolved(
            Agent::Claude,
            &Choices::default(),
            PluginRole::Worker { grants: &grants },
        );
        assert_eq!(names(&mcp_items(&worker, None), true), ["pencil"]);
        assert!(skills_output(&worker, None, true).unwrap().items.is_empty());
        assert!(skills_output(&worker, Some("tools:lint"), true).is_err());
        let none = home.resolved(
            Agent::Claude,
            &Choices::default(),
            PluginRole::Worker { grants: &[] },
        );
        assert!(mcp_items(&none, None).is_empty());
    }
}
