//! The pinned card's Git actions: the user's own commit and push in a session's
//! checkout. They are never automatic (PLAN §4); a commit waits for any landing on the same
//! repository (the landing lock). A blank commit message is written in a throwaway CLI session
//! that is disposed of right after, by the model and style the user's Git settings name.

use std::path::{Path, PathBuf};
use std::time::Duration;

use brigadier_git::{Oid, Repo, SourceEntry, SourceKind};
use brigadier_providers::{
    Access, Origin, ProviderEvent, ProviderKind, Role, SessionSpec, Started, ToolSet, TurnInput,
};

use brigadier_router::{Decision, Exclusion, Needs, Pin, Routed, TaskCategory};

use super::landing::diff_stat_of;
use super::routing::{Ask, Trial};
use super::{SessionManager, blocking, git_error};
use crate::accounts::AccountRef;
use crate::model::{ConversationId, Environment, GitStyle, ModelChoice, Settings, Setup};
use crate::work::{CommitOutcome, GitState, SourceFile, SourceScope, SourceState, SourceStatus};
use crate::{Error, Result};

/// How much of the patch the message writer reads.
const PATCH_CHARS: usize = 12_000;
/// How long the message writer may take before the commit gives up.
const WRITE_TIMEOUT: Duration = Duration::from_secs(90);

/// How many models the message writer tries: one more for each usage limit it runs into.
const WRITE_ATTEMPTS: usize = 3;
/// How many of the latest commit subjects show the writer the repository's style.
const STYLE_SUBJECTS: usize = 15;

const WRITER_PROMPT: &str = "You write git commit messages. Reply with the message only: a \
subject line in the imperative mood of at most 72 characters, then, only when the change needs \
it, a blank line and a short body wrapped at 72 characters. No quotes, no code fences, no \
trailers.";

impl SessionManager {
    /// The session's checkout: its path and the provider its orchestrator runs on.
    fn checkout(&self, id: &ConversationId) -> Result<(PathBuf, ProviderKind)> {
        let Some(Setup::Session {
            repo,
            environment,
            orchestrator,
            ..
        }) = self.core.conversation(id)?.setup
        else {
            return Err(Error::Invalid("only a session has a checkout".into()));
        };
        let path = match environment {
            Environment::LocalCheckout { .. } => repo,
            Environment::NewWorktree { path, .. } => path
                .ok_or_else(|| Error::Invalid("the session's worktree doesn't exist yet".into()))?,
        };
        Ok((PathBuf::from(path), orchestrator.provider))
    }

    /// What the commit popover shows: the checkout's branch, what is staged and uncommitted,
    /// and where it pushes.
    pub async fn git_state(&self, id: &ConversationId) -> Result<GitState> {
        let (path, _) = self.checkout(id)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let branch = repo.state().map_err(git_error)?.current_branch;
            let trees = repo.checkout_trees().map_err(git_error)?;
            let staged = repo
                .diff_stat(&trees.head_tree, &trees.staged)
                .map_err(git_error)?;
            let uncommitted = repo
                .diff_stat(&trees.head_tree, &trees.files)
                .map_err(git_error)?;
            let remote = match &branch {
                Some(branch) => Some(repo.remote_state(branch).map_err(git_error)?),
                None => None,
            };
            Ok(GitState {
                branch,
                staged: diff_stat_of(&staged),
                uncommitted: diff_stat_of(&uncommitted),
                remote: remote.as_ref().and_then(|state| state.remote.clone()),
                upstream: remote.as_ref().and_then(|state| state.upstream.clone()),
                ahead: remote.map_or(0, |state| state.ahead),
            })
        })
        .await
    }

    /// What the Source panel lists: the checkout's staged and unstaged files, and where its
    /// branch pushes.
    pub async fn source_state(&self, id: &ConversationId) -> Result<SourceState> {
        let (path, _) = self.checkout(id)?;
        let git = self.git.clone();
        blocking(move || source_state_of(&git.open(&path).map_err(git_error)?)).await
    }

    /// The user's staging of the listed changes (every change with `None`).
    pub async fn stage_files(
        &self,
        id: &ConversationId,
        paths: Option<Vec<String>>,
    ) -> Result<SourceState> {
        self.change_source(id, move |repo| repo.stage(paths.as_deref()))
            .await
    }

    /// The user's unstaging of the listed staged files (every one with `None`).
    pub async fn unstage_files(
        &self,
        id: &ConversationId,
        paths: Option<Vec<String>>,
    ) -> Result<SourceState> {
        self.change_source(id, move |repo| repo.unstage(paths.as_deref()))
            .await
    }

    /// The user's Discard of the listed files on one side (all of that side with `None`).
    pub async fn discard_files(
        &self,
        id: &ConversationId,
        scope: SourceScope,
        paths: Option<Vec<String>>,
    ) -> Result<SourceState> {
        let staged = scope == SourceScope::Staged;
        self.change_source(id, move |repo| repo.discard(staged, paths.as_deref()))
            .await
    }

    /// Runs one of the Source panel's changes in the checkout, then reads it again.
    async fn change_source(
        &self,
        id: &ConversationId,
        change: impl FnOnce(&Repo) -> brigadier_git::Result<()> + Send + 'static,
    ) -> Result<SourceState> {
        self.admit()?;
        let (path, _) = self.checkout(id)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            change(&repo).map_err(git_error)?;
            source_state_of(&repo)
        })
        .await
    }

    /// The user's commit of the checkout's changes (with `include_unstaged`, every change),
    /// then, with `push`, a push of the branch. A blank `message` is written for them.
    pub async fn commit_changes(
        &self,
        id: &ConversationId,
        message: Option<String>,
        include_unstaged: bool,
        push: bool,
    ) -> Result<CommitOutcome> {
        self.admit()?;
        let _fence = self.enter(id)?;
        let (path, _) = self.checkout(id)?;
        let (message, note) = match message.map(|text| text.trim().to_owned()) {
            Some(text) if !text.is_empty() => (text, None),
            _ => {
                let patch = self.pending_patch(&path, include_unstaged).await?;
                self.write_commit_message(id, &path, &patch).await?
            }
        };
        let message = if self.core.settings().omit_ai_coauthors {
            brigadier_git::strip_ai_coauthors(&message)
        } else {
            message
        };
        let git = self.git.clone();
        let text = message.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let branch = repo
                .state()
                .map_err(git_error)?
                .current_branch
                .ok_or_else(|| Error::Invalid("the checkout is not on a branch".into()))?;
            let commit = repo
                .commit_changes(&text, include_unstaged)
                .map_err(git_error)?;
            if push {
                repo.push(&branch).map_err(git_error)?;
            }
            Ok(CommitOutcome {
                commit: commit.0,
                message: text,
                branch,
                pushed: push,
                note,
            })
        })
        .await
    }

    /// The user's push of the checkout's branch.
    pub async fn push_changes(&self, id: &ConversationId) -> Result<String> {
        self.admit()?;
        let _fence = self.enter(id)?;
        let (path, _) = self.checkout(id)?;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let branch = repo
                .state()
                .map_err(git_error)?
                .current_branch
                .ok_or_else(|| Error::Invalid("the checkout is not on a branch".into()))?;
            repo.push(&branch).map_err(git_error)?;
            Ok(branch)
        })
        .await
    }

    /// The patch a commit would record, for the message writer.
    async fn pending_patch(&self, path: &Path, include_unstaged: bool) -> Result<String> {
        let git = self.git.clone();
        let path = path.to_owned();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let trees = repo.checkout_trees().map_err(git_error)?;
            let to: Oid = if include_unstaged {
                trees.files
            } else {
                trees.staged
            };
            if to == trees.head_tree {
                return Err(Error::Invalid("there are no changes to commit".into()));
            }
            repo.review_patch(&trees.head_tree, &to, 3, false)
                .map_err(git_error)
        })
        .await
    }

    /// A commit message for `patch` in the checkout at `path`, in a throwaway session with no
    /// tools, by the user's Git settings: their writer model (else Automatic, routing's pick
    /// for git text) and style. A model at its usage limit hands the message to routing's next
    /// pick (another account, then the other provider). With it, why the user's model didn't
    /// write it, when it didn't.
    async fn write_commit_message(
        &self,
        id: &ConversationId,
        path: &Path,
        patch: &str,
    ) -> Result<(String, Option<String>)> {
        let _turn = self.background_turn();
        let settings = self.core.settings();
        let project = self.core.conversation(id)?.project_id;
        let subjects = match settings.commit_style {
            GitStyle::Repository => self.recent_subjects(path).await,
            _ => Vec::new(),
        };
        let system = writer_prompt(&settings, &subjects);
        let shown: String = patch.chars().take(PATCH_CHARS).collect();
        let cut = if shown.len() < patch.len() {
            "\n[the rest of the diff is cut]"
        } else {
            ""
        };
        let prompt = format!("Write the commit message for this diff:\n\n{shown}{cut}");
        let mut exclude: Vec<Exclusion> = Vec::new();
        let mut limited: Vec<(AccountRef, String)> = Vec::new();
        let mut runs = 0;
        // Each pass either runs the writer or rules a model out, and routing runs out of
        // models; the cap only guards against a pick that ignores its exclusions.
        for _ in 0..WRITE_ATTEMPTS * 4 {
            if runs == WRITE_ATTEMPTS {
                break;
            }
            let (routed, mut account, why) = self
                .writer_route(settings.git_writer.as_ref(), project.as_ref(), &exclude)
                .await?;
            // The user's model writes it as they set it, Fast included.
            let fast = why.is_none()
                && settings
                    .git_writer
                    .as_ref()
                    .is_some_and(|chosen| chosen.fast == Some(true));
            if limited.contains(&(account.clone(), routed.model.clone())) {
                // Another account of the same model that still has room, else that model is
                // out until its reset.
                match self
                    .runtime
                    .switch_target(&account, Some(&routed.model))
                    .filter(|next| !limited.contains(&(next.clone(), routed.model.clone())))
                {
                    Some(next) => account = next,
                    None => {
                        exclude.push(Exclusion {
                            provider: routed.provider,
                            model: Some(routed.model.clone()),
                        });
                        continue;
                    }
                }
            }
            runs += 1;
            match self
                .run_writer(&routed, fast, &account, &system, &prompt)
                .await?
            {
                Some(message) => {
                    let message = clean_message(&message);
                    if message.is_empty() {
                        tracing::warn!(conversation = %id, "the message writer's reply was empty");
                        return Err(Error::Provider("the message writer replied nothing".into()));
                    }
                    // Why the user's model didn't write it, of the pass that did.
                    return Ok((message, why));
                }
                None => {
                    tracing::info!(conversation = %id, model = %routed.model, "the message writer hit its usage limit");
                    limited.push((account, routed.model));
                }
            }
        }
        Err(Error::Provider(
            "every model that could write the message is at its usage limit".into(),
        ))
    }

    /// The model, effort and account the next message is written with, and why it isn't the
    /// user's `chosen` one when it isn't. Automatic: routing's pick for git text.
    async fn writer_route(
        &self,
        chosen: Option<&ModelChoice>,
        project: Option<&crate::model::ProjectId>,
        exclude: &[Exclusion],
    ) -> Result<(Routed, AccountRef, Option<String>)> {
        let ask = |pin: Option<Pin>| Ask {
            category: TaskCategory::GitText,
            areas: &[],
            floor: brigadier_router::default_floor(TaskCategory::GitText),
            needs: Needs::default(),
            pin,
            hold_pin: false,
            avoid: None,
            distinct_from: Vec::new(),
            exclude,
            project_id: project,
            trial: Trial::Never,
        };
        if let Some(chosen) = chosen {
            let pin = Pin {
                provider: Some(chosen.provider),
                model: chosen.model.clone(),
                effort: chosen.effort.clone(),
            };
            if let Decision::Run(routed) = self.decide(&ask(Some(pin))).await
                && routed.provider == chosen.provider
                && chosen
                    .model
                    .as_deref()
                    .is_none_or(|model| model == routed.model)
            {
                let account = self.runtime.account_for(chosen);
                return Ok((routed, account, None));
            }
        }
        let routed = match self.decide(&ask(None)).await {
            Decision::Run(routed) => routed,
            Decision::Wait(waiting) => {
                return Err(Error::Invalid(format!(
                    "no model can write the message now: {}",
                    waiting.reason
                )));
            }
        };
        let why = chosen.map(|chosen| {
            format!(
                "{} isn't available now, so Automatic wrote the message with {}.",
                choice_name(chosen.provider, chosen.model.as_deref()),
                choice_name(routed.provider, Some(&routed.model)),
            )
        });
        let account = self
            .runtime
            .launch_account(routed.provider, Some(&routed.model));
        Ok((routed, account, why))
    }

    /// The latest commit subjects of the checkout's branch, for the writer to match; none
    /// before its first commit.
    async fn recent_subjects(&self, path: &Path) -> Vec<String> {
        let git = self.git.clone();
        let path = path.to_owned();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let head = repo.resolve("HEAD").map_err(git_error)?;
            let log = repo.log(&head, STYLE_SUBJECTS).map_err(git_error)?;
            Ok(log.into_iter().map(|commit| commit.subject).collect())
        })
        .await
        .unwrap_or_default()
    }

    /// One try of the message writer on `routed`'s model (on its fast tier when `fast`) under
    /// `account`: its reply, or `None` when it hit its usage limit (noted, so routing and the
    /// account choice move on).
    async fn run_writer(
        &self,
        routed: &Routed,
        fast: bool,
        account: &AccountRef,
        system: &str,
        prompt: &str,
    ) -> Result<Option<String>> {
        let run = uuid::Uuid::now_v7().to_string();
        let owner = format!("gen:{run}");
        let dir = self.owned_dir("scratch", &run);
        tokio::fs::create_dir_all(&dir).await.map_err(|err| {
            Error::Invalid(format!("could not prepare the message writer: {err}"))
        })?;
        let spec = SessionSpec {
            cwd: dir.clone(),
            model: Some(routed.model.clone()),
            effort: routed.effort.clone(),
            fast,
            origin: Origin::New,
            ephemeral: true,
            access: Access::ReadOnly,
            append_system_prompt: Some(system.to_owned()),
            mcp_servers: Vec::new(),
            tools: ToolSet::None,
            add_dirs: Vec::new(),
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            auto_compact: true,
            allowed_models: None,
            auto_review: false,
            omit_ai_coauthors: false,
            output_hook: None,
        };
        let written = async {
            let Started {
                session,
                mut events,
            } = self.runtime.start_hosted(&owner, account, spec).await?;
            let reply = async {
                session
                    .send(TurnInput::text(prompt.to_owned()))
                    .await
                    .map_err(|err| {
                        Error::Provider(format!("the message writer didn't start: {err}"))
                    })?;
                let mut text = None;
                let mut limit = None;
                while let Some(event) = events.recv().await {
                    match event {
                        ProviderEvent::Message {
                            role: Role::Assistant,
                            text: reply,
                            ..
                        } => text = Some(reply),
                        ProviderEvent::RateLimits { quota } => {
                            self.runtime.note_quota_snapshot(account, quota).await;
                        }
                        ProviderEvent::Error { error } if !error.will_retry => {
                            limit = error.limit.or(limit);
                        }
                        ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. } => break,
                        _ => {}
                    }
                }
                if let Some(limit) = limit {
                    self.runtime.note_limit(account, limit).await;
                    return Ok(None);
                }
                text.map(Some)
                    .ok_or_else(|| Error::Provider("the message writer replied nothing".into()))
            };
            let reply = tokio::time::timeout(WRITE_TIMEOUT, reply)
                .await
                .unwrap_or_else(|_| {
                    Err(Error::Provider("the message writer took too long".into()))
                });
            session.close().await;
            reply
        }
        .await;
        let _ = self.runtime.ledger().dispose(&owner).await;
        let _ = tokio::fs::remove_dir_all(&dir).await;
        written
    }

    /// The provider's cheapest model for a one-off chore, at low effort where it has efforts.
    /// Only a model the user left available: when its list has none, the chore isn't done.
    pub(super) fn cheapest(&self, provider: ProviderKind) -> Result<(String, Option<String>)> {
        let family = match provider {
            ProviderKind::Claude => "haiku",
            ProviderKind::Codex => "luna",
        };
        let settings = self.core.settings();
        let available =
            |id: &str| crate::routing::availability::model_available(&settings, provider, id);
        let models = self
            .runtime
            .overview(provider)
            .and_then(|overview| overview.models)
            .map(|catalog| catalog.models)
            .unwrap_or_default();
        // Before its list is read, the family's alias stands in for it.
        let model = if models.is_empty() {
            available(family).then(|| family.to_owned())
        } else {
            models
                .iter()
                .find(|model| model.id.contains(family) && available(&model.id))
                .or_else(|| {
                    models
                        .iter()
                        .find(|model| !model.legacy && available(&model.id))
                })
                .map(|model| model.id.clone())
        };
        let model = model.ok_or_else(|| {
            Error::Invalid(format!(
                "{} has no model available. Turn one on in Settings › Providers.",
                provider.label()
            ))
        })?;
        let effort = (provider == ProviderKind::Codex).then(|| "low".to_owned());
        Ok((model, effort))
    }
}

/// The writer's instructions, by the user's commit style: the repository's latest
/// `subjects` to match, Conventional Commits, or the user's own instructions.
fn writer_prompt(settings: &Settings, subjects: &[String]) -> String {
    match super::prompts::commit_rules(settings.commit_style, &settings.commit_instructions) {
        Some(rules) => format!("{WRITER_PROMPT}\n\nThese win over the rules above. {rules}"),
        None if subjects.is_empty() => WRITER_PROMPT.to_owned(),
        None => {
            let mut text = format!(
                "{WRITER_PROMPT}\n\nMatch this repository's style: its form, prefixes, tense, capitals and length. Its latest commit subjects:"
            );
            for subject in subjects {
                text.push_str("\n- ");
                text.push_str(subject);
            }
            text
        }
    }
}

/// A model as a note names it: "Claude Code haiku".
fn choice_name(provider: ProviderKind, model: Option<&str>) -> String {
    match model {
        Some(model) => format!("{} {model}", provider.label()),
        None => provider.label().to_owned(),
    }
}

/// A checkout's Source panel state.
fn source_state_of(repo: &Repo) -> Result<SourceState> {
    let changes = repo.source_changes().map_err(git_error)?;
    let branch = repo.state().map_err(git_error)?.current_branch;
    let remote = match &branch {
        Some(branch) => Some(repo.remote_state(branch).map_err(git_error)?),
        None => None,
    };
    let files = |entries: Vec<SourceEntry>| {
        entries
            .into_iter()
            .map(|entry| SourceFile {
                path: entry.path,
                old_path: entry.old_path,
                status: match entry.kind {
                    SourceKind::Modified => SourceStatus::Modified,
                    SourceKind::Added => SourceStatus::Added,
                    SourceKind::Deleted => SourceStatus::Deleted,
                    SourceKind::Renamed => SourceStatus::Renamed,
                    SourceKind::Copied => SourceStatus::Copied,
                    SourceKind::TypeChanged => SourceStatus::TypeChanged,
                    SourceKind::Untracked => SourceStatus::Untracked,
                    SourceKind::Conflicted => SourceStatus::Conflicted,
                },
            })
            .collect()
    };
    Ok(SourceState {
        branch,
        remote: remote.as_ref().and_then(|state| state.remote.clone()),
        upstream: remote.as_ref().and_then(|state| state.upstream.clone()),
        ahead: remote.map_or(0, |state| state.ahead),
        staged: files(changes.staged),
        changes: files(changes.changes),
    })
}

/// The writer's reply as a commit message: no code fences or surrounding quotes, and no
/// trailing blank lines.
fn clean_message(reply: &str) -> String {
    let lines: Vec<&str> = reply
        .trim()
        .lines()
        .filter(|line| !line.trim_start().starts_with("```"))
        .collect();
    lines
        .join("\n")
        .trim()
        .trim_matches(|c| c == '"' || c == '`')
        .trim()
        .to_owned()
}
