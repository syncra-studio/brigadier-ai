//! `brigadier context` (D13): everything a thread or worker needs about its job, on pull. Nothing
//! of it is put into a conversation; the caller runs it when it starts and after a compaction.
//! It is a front door: each part is the code of the command it stands for (`ledger show`,
//! `transcript`, `checkpoint show`, `models`, the registry's catalog), as the caller's own.
//!
//! A worker reads its session's ledger and checkpoint, read-only, and then its own ledger. Its
//! transcript is its own records only.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use brigadier_cli::protocol::{
    CliReply, ContextEntry, ContextFile, ContextItem, ContextOutput, ContextPart, ContextPartSize,
    ErrorCode,
};
use brigadier_providers::{McpStatus, ProviderKind};
use brigadier_registry::{Catalog, Session, Skill, SkillRoot};

use super::SessionManager;
use super::cli::CliCaller;
use super::cli_ledger::{Entry, Kind, Ledger, LedgerFile};
use super::integrations::{RESERVED, agent};
use super::long_sessions::checkpoint;
use super::records::{RecordRead, RecordSearch};
use crate::model::{ConversationId, MessageRole, Setup};
use crate::tools::Role;
use crate::work::{Approval, ApprovalSubject, CardState, PlainWorker, Question, Task};
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
    provider: ProviderKind,
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

/// What a worker was given of its agent's setup: `None` for its agent's whole setup, else only
/// the names listed. The one place a worker's grants are read (D18).
pub(crate) struct WorkerGrants {
    pub skills: Option<Vec<String>>,
    pub mcps: Option<Vec<String>>,
}

/// The skills and MCP servers worker `plain` was started with: a `brigadier start` worker is
/// naked (D13), with no skills and only the servers it was granted (D18); any other its
/// agent's whole setup.
pub(crate) fn worker_grants(plain: Option<&PlainWorker>) -> WorkerGrants {
    match plain {
        Some(plain) => WorkerGrants {
            skills: Some(Vec::new()),
            mcps: Some(plain.mcps.clone()),
        },
        None => WorkerGrants {
            skills: None,
            mcps: None,
        },
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
        let thread_provider = match &conversation.setup {
            Some(Setup::Session { orchestrator, .. }) => orchestrator.provider,
            Some(Setup::Chat { model, .. }) => model.provider,
            None => ProviderKind::Claude,
        };
        let thread_checkout = self
            .recorded_workspace(&conversation.id)
            .map(|workspace| workspace.path);
        let Some(task_id) = &caller.task_id else {
            return Ok(Whose {
                key: conversation.id.short().to_owned(),
                conversation: conversation.id,
                task: None,
                session_record: folder.record(),
                own_record: None,
                provider: thread_provider,
                checkout: thread_checkout,
                repo,
            });
        };
        let task = self.task_by_id(&conversation.id, task_id).await?;
        let worker = folder.worker(&task);
        Ok(Whose {
            key: self.worker_records_key(&conversation.id, task_id).await?,
            conversation: conversation.id,
            session_record: folder.record(),
            own_record: Some(worker.join("record")),
            provider: task.route.choice.provider,
            checkout: task
                .workspace
                .as_ref()
                .and_then(|workspace| workspace.worktree.as_ref())
                .map(PathBuf::from)
                .or(thread_checkout),
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
                output.items = self.context_mcps(whose).await;
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

    /// `context skills [<name>]`: the skills of the caller's agent's setup, or one whole.
    async fn context_skills(&self, whose: &Whose, name: Option<&str>) -> Result<ContextOutput> {
        let registry = self.registry();
        let (repo, cwd) = (whose.repo.clone(), self.setup_folder(whose));
        let provider = whose.provider;
        let kept = whose
            .task
            .as_ref()
            .and_then(|task| worker_grants(task.plain.as_ref()).skills);
        let name = name.map(str::to_owned);
        let worker = whose.worker();
        super::blocking(move || {
            let catalog = registry.catalog(repo.as_deref());
            let session = Session {
                agent: agent(provider),
                cwd: &cwd,
                project: repo.as_deref(),
                in_checkout: true,
            };
            skills_output(&catalog, &session, kept.as_deref(), name.as_deref(), worker)
        })
        .await
    }

    /// `context mcps`: the MCP servers of the setup, and which the caller has.
    async fn context_mcps(&self, whose: &Whose) -> Vec<ContextItem> {
        let reported: Option<HashMap<String, McpStatus>> = self
            .core
            .board(&whose.conversation)
            .await
            .ok()
            .and_then(|board| {
                let task = whose.task.as_ref().map(|task| task.id.0.as_str());
                board.mcp_servers_of(task).map(|agent| {
                    agent
                        .servers
                        .iter()
                        .map(|server| (server.name.clone(), server.status))
                        .collect()
                })
            });
        let grants = whose
            .task
            .as_ref()
            .and_then(|task| worker_grants(task.plain.as_ref()).mcps);
        let registry = self.registry();
        let (repo, cwd) = (whose.repo.clone(), self.setup_folder(whose));
        let provider = whose.provider;
        super::blocking(move || {
            let catalog = registry.catalog(repo.as_deref());
            let session = Session {
                agent: agent(provider),
                cwd: &cwd,
                project: repo.as_deref(),
                in_checkout: true,
            };
            Ok(mcp_items(
                &catalog,
                &session,
                reported.as_ref(),
                grants.as_deref(),
            ))
        })
        .await
        .unwrap_or_default()
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
            .or_else(|| whose.repo.clone())
            .unwrap_or_else(|| self.session_dir(&whose.conversation).root().to_path_buf())
    }
}

/// The skills of `session`'s agent's setup, each by the name `context skills <name>` takes,
/// or skill `name`'s whole `SKILL.md`. `kept`: a worker's only skills, where it has only some.
fn skills_output(
    catalog: &Catalog,
    session: &Session<'_>,
    kept: Option<&[String]>,
    name: Option<&str>,
    worker: bool,
) -> Result<ContextOutput> {
    let named = skill_names(&catalog.skills, session, |skill| {
        catalog.skill_native(skill, session)
    });
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
        // Its printed name, its id, or a name only one skill of the catalog has.
        let unique = |skill: &Skill| {
            skill.name == name && catalog.skills.iter().filter(|s| s.name == name).count() == 1
        };
        let skill = named
            .iter()
            .find(|(_, shown)| shown == name)
            .map(|(skill, _)| *skill)
            .or_else(|| {
                catalog
                    .skills
                    .iter()
                    .find(|skill| skill.id == name || unique(skill))
            })
            .ok_or_else(|| {
                Error::Invalid(format!(
                    "no skill {name:?} here; `brigadier context skills` lists them"
                ))
            })?;
        let text = std::fs::read_to_string(skill.skill_md())
            .map_err(|err| Error::Invalid(format!("couldn't read the skill {name:?}: {err}")))?;
        output.text = if text.ends_with('\n') {
            text
        } else {
            format!("{text}\n")
        };
        return Ok(output);
    }
    output.items = named
        .into_iter()
        .map(|(skill, shown)| ContextItem {
            has: kept.is_none_or(|kept| {
                kept.iter()
                    .any(|kept| *kept == skill.name || *kept == skill.id || *kept == shown)
            }),
            name: shown,
            kind: "skill".into(),
            detail: skill.description.clone(),
            status: None,
        })
        .collect();
    output.text = items_text(&output.items, "Skills", "No skills in this setup.", worker);
    Ok(output)
}

/// The MCP servers of the catalog, `session`'s agent's first, and which the caller has: what
/// its CLI `reported` when it reported any, else what it was given, a worker its `grants`
/// (D18); a server of another agent never. What its CLI runs that no config names follows.
fn mcp_items(
    catalog: &Catalog,
    session: &Session<'_>,
    reported: Option<&HashMap<String, McpStatus>>,
    grants: Option<&[String]>,
) -> Vec<ContextItem> {
    let mut items: Vec<ContextItem> = Vec::new();
    let mut seen = HashSet::new();
    let mut servers: Vec<_> = catalog
        .servers
        .iter()
        .filter(|server| !RESERVED.contains(&server.name.as_str()))
        .map(|server| (catalog.server_native(server, session), server))
        .collect();
    servers.sort_by_key(|(native, _)| !native);
    for (native, server) in servers {
        let name = if native {
            server.native_name()
        } else {
            server.name.clone()
        };
        if !seen.insert(name.clone()) {
            continue;
        }
        let status = reported.and_then(|reported| reported.get(&name));
        let has = native
            && match (reported, grants) {
                (Some(_), _) => status.is_some(),
                (None, Some(grants)) => grants.contains(&name),
                (None, None) => true,
            };
        let detail = match &server.plugin {
            Some(plugin) => format!("{}, plugin {}", server.source.label(), plugin.name),
            None => server.source.label().to_owned(),
        };
        items.push(ContextItem {
            name,
            kind: "server".into(),
            detail: if native {
                detail
            } else {
                format!("{detail}; not your agent's")
            },
            has,
            status: status.map(|status| status_word(*status).to_owned()),
        });
    }
    let mut extra: Vec<_> = reported
        .into_iter()
        .flatten()
        .filter(|(name, _)| !seen.contains(*name))
        .collect();
    extra.sort_by(|a, b| a.0.cmp(b.0));
    for (name, status) in extra {
        items.push(ContextItem {
            name: name.clone(),
            kind: "server".into(),
            detail: "Given by Brigadier".into(),
            has: true,
            status: Some(status_word(*status).to_owned()),
        });
    }
    items
}

/// The skills `native` takes, each with the name printed for it: a plugin's as its agent shows
/// it (`<plugin>:<name>`), else its own; a name two skills share, their ids.
fn skill_names<'a>(
    skills: &'a [Skill],
    session: &Session<'_>,
    native: impl Fn(&Skill) -> bool,
) -> Vec<(&'a Skill, String)> {
    let named: Vec<_> = skills
        .iter()
        .filter(|skill| native(skill))
        .map(|skill| {
            let plugin = skill.roots.iter().find_map(|root| match root {
                SkillRoot::Plugin(plugin) if root.native(session) => Some(plugin),
                _ => None,
            });
            let name = match plugin {
                Some(plugin) => plugin.skill_name(&skill.name),
                None => skill.name.clone(),
            };
            (skill, name)
        })
        .collect();
    let mut uses: HashMap<String, usize> = HashMap::new();
    for (_, name) in &named {
        *uses.entry(name.clone()).or_default() += 1;
    }
    named
        .into_iter()
        .map(|(skill, name)| match uses.get(&name) {
            Some(1) => (skill, name),
            _ => (skill, skill.id.clone()),
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
    use brigadier_registry::{Agent, Roots};

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

        fn catalog(&self) -> Catalog {
            let roots =
                Roots::from_vars([("HOME".to_owned(), self.0.display().to_string())]).unwrap();
            Catalog::read(&roots, Some(&self.0.join("repo")))
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
    fn skills_list_each_skill_folder_by_a_name_that_prints_it() {
        let home = Home::new("skills");
        let catalog = home.catalog();
        let repo = home.0.join("repo");
        let session = Session {
            agent: Agent::Claude,
            cwd: &repo,
            project: Some(&repo),
            in_checkout: true,
        };
        let listed = skills_output(&catalog, &session, None, None, false).unwrap();
        let shown = names(&listed.items, true);
        // One plugin, two skills, each by its agent's name; the two `review`s by their ids.
        assert!(shown.contains(&"tools:lint".to_owned()), "{shown:?}");
        assert!(shown.contains(&"tools:format".to_owned()), "{shown:?}");
        assert!(!shown.contains(&"review".to_owned()), "{shown:?}");
        let reviews: Vec<_> = shown
            .iter()
            .filter(|name| name.contains("review"))
            .collect();
        assert_eq!(reviews.len(), 2, "{shown:?}");
        assert!(
            listed.text.contains("- tools:lint: lint things"),
            "{}",
            listed.text
        );
        for name in &shown {
            let one = skills_output(&catalog, &session, None, Some(name), false).unwrap();
            assert!(one.text.starts_with("---\nname: "), "{name}: {}", one.text);
        }
        let lint = skills_output(&catalog, &session, None, Some("tools:lint"), false).unwrap();
        assert!(lint.text.contains("Run the linter."), "{}", lint.text);
        assert!(lint.items.is_empty());
        // A name only one skill has prints it too.
        let format = skills_output(&catalog, &session, None, Some("format"), false).unwrap();
        assert!(format.text.contains("Run the formatter."));
        assert!(skills_output(&catalog, &session, None, Some("nope"), false).is_err());

        // A worker given only some skills has those, and can still read the others.
        let kept = ["tools:lint".to_owned()];
        let lean = skills_output(&catalog, &session, Some(&kept), None, true).unwrap();
        assert_eq!(names(&lean.items, true), ["tools:lint"]);
        assert!(names(&lean.items, false).contains(&"tools:format".to_owned()));
    }

    #[test]
    fn mcps_say_which_servers_the_caller_has() {
        let home = Home::new("mcps");
        let catalog = home.catalog();
        let repo = home.0.join("repo");
        let session = Session {
            agent: Agent::Claude,
            cwd: &repo,
            project: Some(&repo),
            in_checkout: true,
        };
        // The session's thread has its agent's whole setup; Codex's server is not its.
        let thread = mcp_items(&catalog, &session, None, None);
        assert_eq!(names(&thread, true), ["pencil", "railway"]);
        assert_eq!(names(&thread, false), ["calc"]);
        // A worker given none has none; one given pencil has only pencil, not its session's
        // railway (D18).
        let none = mcp_items(&catalog, &session, None, Some(&[]));
        assert!(names(&none, true).is_empty(), "{none:?}");
        assert_eq!(names(&none, false), ["calc", "pencil", "railway"]);
        let one = mcp_items(&catalog, &session, None, Some(&["pencil".to_owned()]));
        assert_eq!(names(&one, true), ["pencil"]);
        assert_eq!(names(&one, false), ["calc", "railway"]);
        // What its CLI reported wins, Brigadier's own server among it.
        let reported = HashMap::from([
            ("pencil".to_owned(), McpStatus::NeedsAuth),
            ("brigadier-computer".to_owned(), McpStatus::Connected),
        ]);
        let heard = mcp_items(&catalog, &session, Some(&reported), None);
        assert_eq!(names(&heard, true), ["brigadier-computer", "pencil"]);
        assert_eq!(names(&heard, false), ["calc", "railway"]);
        let pencil = heard.iter().find(|item| item.name == "pencil").unwrap();
        assert_eq!(pencil.status.as_deref(), Some("needsAuth"));
        assert_eq!(heard.last().unwrap().detail, "Given by Brigadier");
    }

    #[test]
    fn a_worker_has_no_skills_and_only_its_granted_servers() {
        let naked = PlainWorker {
            permission: crate::model::PermissionLevel::Ask,
            local: false,
            repo: None,
            base: None,
            mcps: vec!["pencil".into()],
            computer_use: false,
            closed: false,
            auto_resumed: false,
        };
        let grants = worker_grants(Some(&naked));
        assert_eq!(grants.mcps, Some(vec!["pencil".to_owned()]));
        assert_eq!(grants.skills, Some(Vec::new()));
        let grants = worker_grants(None);
        assert_eq!((grants.mcps, grants.skills), (None, None));
    }
}
