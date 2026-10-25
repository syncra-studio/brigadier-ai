//! The pinned card's Git actions: the user's own commit and push in a session's checkout,
//! from its buttons, which never run on their own (PLAN §4). The thread commits, pushes and
//! opens pull requests itself as it judges (Q61). A commit waits for any landing on the same
//! repository (the landing lock). A blank commit message is written in a throwaway CLI session
//! that is disposed of right after, by the model and style the user's Git settings name.
//!
//! A session not started yet has no checkout of its own: its Source pane reads, stages and
//! commits in its project's folder (the `project_*` actions), which is where a session that
//! works in the project folder will work. One that will make its own copy shows it read-only;
//! the window keeps to that, as only it knows how the session will be set up.

use std::path::{Path, PathBuf};
use std::time::Duration;

use brigadier_git::{Oid, Repo, SourceEntry, SourceKind};
use brigadier_providers::{
    Access, ModelInfo, Origin, ProviderEvent, ProviderKind, Role, SessionSpec, Started, ToolSet,
    TurnInput,
};

use brigadier_router::{MergedModel, QualityTier};

use super::diff_stat_of;
use super::source_actions::MergedPr;
use super::{SessionManager, blocking, git_error};
use crate::accounts::AccountRef;
use crate::model::{
    ConversationId, Environment, GitStyle, ModelChoice, ProjectId, Settings, Setup,
};
use crate::work::{
    CommitOutcome, GitState, SourceChange, SourceFile, SourceMerge, SourceRebase, SourceScope,
    SourceState, SourceStatus,
};
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
    pub(super) fn checkout(&self, id: &ConversationId) -> Result<(PathBuf, ProviderKind)> {
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

    /// A project's folder: its first repository, the user's own checkout. What a session not
    /// started yet shows in its Files and Source panes.
    pub(crate) fn project_checkout(&self, project: &ProjectId) -> Result<PathBuf> {
        self.core
            .project(project)?
            .repos
            .first()
            .map(|repo| PathBuf::from(&repo.path))
            .ok_or_else(|| Error::Invalid("the project has no repository".into()))
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
        self.source_state_at(path).await
    }

    /// What the Source pane lists before a session starts: its project folder's changes.
    pub async fn project_source_state(&self, project: &ProjectId) -> Result<SourceState> {
        let path = self.project_checkout(project)?;
        self.source_state_at(path).await
    }

    async fn source_state_at(&self, path: PathBuf) -> Result<SourceState> {
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

    /// The user's staging, unstaging or discarding in a project's folder, from the Source pane
    /// of a session not started yet.
    pub async fn change_project_source(
        &self,
        project: &ProjectId,
        change: SourceChange,
    ) -> Result<SourceState> {
        self.admit()?;
        let path = self.project_checkout(project)?;
        self.change_source_at(path, move |repo| match change {
            SourceChange::Stage { paths } => repo.stage(paths.as_deref()),
            SourceChange::Unstage { paths } => repo.unstage(paths.as_deref()),
            SourceChange::Discard { scope, paths } => {
                repo.discard(scope == SourceScope::Staged, paths.as_deref())
            }
        })
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
        self.change_source_at(path, change).await
    }

    async fn change_source_at(
        &self,
        path: PathBuf,
        change: impl FnOnce(&Repo) -> brigadier_git::Result<()> + Send + 'static,
    ) -> Result<SourceState> {
        let (git, log) = self.logged_git();
        let dir = path.clone();
        let done = blocking(move || {
            let repo = git.open(&dir).map_err(git_error)?;
            change(&repo).map_err(git_error)?;
            source_state_of(&repo)
        })
        .await;
        self.file_output(&path, &log);
        done
    }

    /// The user's commit of the checkout's changes (with `include_unstaged`, every change),
    /// then, with `push`, a push of the branch. A blank `message` is written for them; during
    /// a merge it is git's merge message, and with `amend` the last commit's own.
    pub async fn commit_changes(
        &self,
        id: &ConversationId,
        message: Option<String>,
        include_unstaged: bool,
        push: bool,
        amend: bool,
    ) -> Result<CommitOutcome> {
        self.admit()?;
        let _fence = self.enter(id)?;
        let (path, _) = self.checkout(id)?;
        let merged = if push { self.merged_pr(id).await } else { None };
        self.commit_at(path, merged, message, include_unstaged, push, amend)
            .await
    }

    /// The user's commit in a project's folder, from the Source pane of a session not started
    /// yet; as [`Self::commit_changes`] otherwise.
    pub async fn commit_project_changes(
        &self,
        project: &ProjectId,
        message: Option<String>,
        include_unstaged: bool,
        push: bool,
        amend: bool,
    ) -> Result<CommitOutcome> {
        self.admit()?;
        let path = self.project_checkout(project)?;
        self.commit_at(path, None, message, include_unstaged, push, amend)
            .await
    }

    /// `merged`: the session's pull request GitHub merged, whose kept branch a push replaces.
    #[allow(clippy::too_many_arguments)]
    async fn commit_at(
        &self,
        path: PathBuf,
        merged: Option<MergedPr>,
        message: Option<String>,
        include_unstaged: bool,
        push: bool,
        amend: bool,
    ) -> Result<CommitOutcome> {
        let merging = {
            let git = self.git.clone();
            let path = path.clone();
            blocking(move || {
                git.open(&path)
                    .and_then(|repo| repo.merging())
                    .map_err(git_error)
            })
            .await?
        };
        let (message, note) = match message.map(|text| text.trim().to_owned()) {
            Some(text) if !text.is_empty() => (text, None),
            // A merge commits with git's own message, an amend keeps the last commit's.
            _ if merging.is_some() => {
                (merging.map(|merge| merge.message).unwrap_or_default(), None)
            }
            _ if amend => {
                let git = self.git.clone();
                let path = path.clone();
                let last = blocking(move || {
                    git.open(&path)
                        .and_then(|repo| repo.read_only(&["log", "-1", "--format=%B", "HEAD"]))
                        .map_err(git_error)
                })
                .await?;
                (last.trim_end().to_owned(), None)
            }
            _ => {
                let patch = self.pending_patch(&path, include_unstaged).await?;
                self.write_commit_message(&path, &patch).await?
            }
        };
        let message = if self.core.settings().omit_ai_coauthors {
            brigadier_git::strip_ai_coauthors(&message)
        } else {
            message
        };
        let (git, log) = self.logged_git();
        let text = message.clone();
        let dir = path.clone();
        let done = blocking(move || {
            let repo = git.open(&dir).map_err(git_error)?;
            // A rebase has HEAD detached: its commits are made by Continue.
            if repo.rebased_branch().map_err(git_error)?.is_some() {
                return Err(Error::Invalid(
                    "continue or abort the rebase in progress first".into(),
                ));
            }
            let branch = repo
                .state()
                .map_err(git_error)?
                .current_branch
                .ok_or_else(|| Error::Invalid("the checkout is not on a branch".into()))?;
            let commit = repo
                .commit_with(&text, include_unstaged, amend)
                .map_err(git_error)?;
            if push {
                MergedPr::push(merged.as_ref(), &repo, &branch).map_err(git_error)?;
            }
            Ok(CommitOutcome {
                commit: commit.0,
                message: text,
                branch,
                pushed: push,
                note,
            })
        })
        .await;
        self.file_output(&path, &log);
        done
    }

    /// The user's push of the checkout's branch.
    pub async fn push_changes(&self, id: &ConversationId) -> Result<String> {
        self.admit()?;
        let _fence = self.enter(id)?;
        let (path, _) = self.checkout(id)?;
        let merged = self.merged_pr(id).await;
        let git = self.git.clone();
        blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            let branch = repo
                .state()
                .map_err(git_error)?
                .current_branch
                .ok_or_else(|| Error::Invalid("the checkout is not on a branch".into()))?;
            MergedPr::push(merged.as_ref(), &repo, &branch).map_err(git_error)?;
            Ok(branch)
        })
        .await
    }

    /// The patch a commit would record, for the message writer.
    pub(super) async fn pending_patch(
        &self,
        path: &Path,
        include_unstaged: bool,
    ) -> Result<String> {
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
    pub(super) async fn write_commit_message(
        &self,
        path: &Path,
        patch: &str,
    ) -> Result<(String, Option<String>)> {
        let _turn = self.background_turn();
        let settings = self.core.settings();
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
        let (message, why) = self
            .write_text(
                settings.git_writer.as_ref(),
                &system,
                &prompt,
                "the message",
            )
            .await?;
        let message = clean_message(&message);
        if message.is_empty() {
            tracing::warn!(checkout = %path.display(), "the message writer's reply was empty");
            return Err(Error::Provider("the message writer replied nothing".into()));
        }
        Ok((message, why))
    }

    /// A short text (`what`, for errors: "the message") written in a throwaway session with no
    /// tools, by `chosen` (else Automatic: a ready CLI's cheapest model for chores): its reply
    /// as written. A model at its usage limit hands it to another account of its provider,
    /// then to the next model. With it, why the `chosen` model didn't write it, when it didn't.
    pub(super) async fn write_text(
        &self,
        chosen: Option<&ModelChoice>,
        system: &str,
        prompt: &str,
        what: &str,
    ) -> Result<(String, Option<String>)> {
        let mut exclude: Vec<(ProviderKind, String)> = Vec::new();
        let mut limited: Vec<(AccountRef, String)> = Vec::new();
        let mut runs = 0;
        // Each pass either runs the writer or rules a model out, and the models run out; the
        // cap only guards against a pick that ignores its exclusions.
        for _ in 0..WRITE_ATTEMPTS * 4 {
            if runs == WRITE_ATTEMPTS {
                break;
            }
            let (choice, mut account, why) = self.writer_route(chosen, &exclude, what).await?;
            let model = choice.model.clone().unwrap_or_default();
            // The user's model writes it as they set it, Fast included.
            let fast = why.is_none() && chosen.is_some_and(|chosen| chosen.fast == Some(true));
            if limited.contains(&(account.clone(), model.clone())) {
                // Another account of the same model that still has room, else that model is
                // out until its reset.
                match self
                    .runtime
                    .switch_target(&account, choice.model.as_deref())
                    .filter(|next| !limited.contains(&(next.clone(), model.clone())))
                {
                    Some(next) => account = next,
                    None => {
                        exclude.push((choice.provider, model));
                        continue;
                    }
                }
            }
            runs += 1;
            match self
                .run_writer(&choice, fast, &account, system, prompt)
                .await?
            {
                // Why the user's model didn't write it, of the pass that did.
                Some(text) => return Ok((text, why)),
                None => {
                    tracing::info!(model = %model, "the writer hit its usage limit");
                    limited.push((account, model));
                }
            }
        }
        Err(Error::Provider(format!(
            "every model that could write {what} is at its usage limit"
        )))
    }

    /// The model, effort and account the next message is written with, and why it isn't the
    /// user's `chosen` one when it isn't. Automatic: the first ready CLI's cheapest model the
    /// user left on ([`Self::writer_models`]), leaving out the `exclude`d ones.
    async fn writer_route(
        &self,
        chosen: Option<&ModelChoice>,
        exclude: &[(ProviderKind, String)],
        what: &str,
    ) -> Result<(ModelChoice, AccountRef, Option<String>)> {
        let excluded = |provider: ProviderKind, model: Option<&str>| {
            exclude
                .iter()
                .any(|(p, m)| *p == provider && Some(m.as_str()) == model)
        };
        let settings = self.core.settings();
        let switched_on = |choice: &ModelChoice| {
            choice.model.as_deref().is_none_or(|id| {
                crate::routing::availability::model_available(&settings, choice.provider, id)
            })
        };
        if let Some(chosen) = chosen
            && self.provider_ready(chosen.provider)
            && switched_on(chosen)
            && !excluded(chosen.provider, chosen.model.as_deref())
            && self.choice_available(chosen).await
        {
            let account = self.runtime.account_for(chosen);
            return Ok((chosen.clone(), account, None));
        }
        let ready = ProviderKind::ALL
            .into_iter()
            .filter(|provider| self.provider_ready(*provider));
        for (provider, (model, effort)) in ready.flat_map(|provider| {
            self.writer_models(provider)
                .into_iter()
                .map(move |m| (provider, m))
        }) {
            let choice = ModelChoice {
                provider,
                model: Some(model),
                effort,
                fast: None,
                account: None,
            };
            if excluded(provider, choice.model.as_deref()) || !self.choice_available(&choice).await
            {
                continue;
            }
            let why = chosen.map(|chosen| {
                format!(
                    "{} isn't available now, so Automatic wrote the message with {}.",
                    choice_name(chosen.provider, chosen.model.as_deref()),
                    choice_name(provider, choice.model.as_deref()),
                )
            });
            let account = self
                .runtime
                .launch_account(provider, choice.model.as_deref());
            return Ok((choice, account, why));
        }
        Err(Error::Invalid(format!("no model can write {what} now")))
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

    /// One try of the message writer on `choice`'s model (on its fast tier when `fast`) under
    /// `account`: its reply, or `None` when it hit its usage limit (noted, so the account
    /// choice moves on).
    async fn run_writer(
        &self,
        choice: &ModelChoice,
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
            model: choice.model.clone(),
            effort: choice.effort.clone(),
            fast,
            origin: Origin::New,
            ephemeral: true,
            access: Access::ReadOnly,
            append_system_prompt: Some(system.to_owned()),
            mcp_servers: Vec::new(),
            tools: ToolSet::None,
            skill_plugins: Vec::new(),
            installed_skills: Vec::new(),
            switched_off: Default::default(),
            add_dirs: Vec::new(),
            project: None,
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            sweep_dir: Some(dir.clone()),
            keeps_detached: false,
            auto_compact: true,
            allowed_models: None,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            lean_worker: false,
            native_compaction: None,
            temp_tag: None,
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

    /// The models Automatic writes git text with on `provider`, in order: its cheapest model for
    /// chores ([`Self::cheapest`]), then the models of its list the user left on, so a limit on
    /// one leaves the others.
    fn writer_models(&self, provider: ProviderKind) -> Vec<(String, Option<String>)> {
        let settings = self.core.settings();
        let listed = self
            .runtime
            .overview(provider)
            .and_then(|overview| overview.models)
            .map(|catalog| catalog.models)
            .unwrap_or_default();
        let mut models: Vec<_> = self.cheapest(provider).ok().into_iter().collect();
        for model in &listed {
            if !model.legacy
                && crate::routing::availability::model_available(&settings, provider, &model.id)
                && models.iter().all(|(id, _)| *id != model.id)
            {
                models.push((model.id.clone(), None));
            }
        }
        models
    }

    /// The provider's cheapest model for a one-off chore, at low effort where it has efforts:
    /// see [`chore_model`]. When its list has none, the chore isn't done.
    pub(super) fn cheapest(&self, provider: ProviderKind) -> Result<(String, Option<String>)> {
        let family = match provider {
            ProviderKind::Claude => "haiku",
            ProviderKind::Codex => "luna",
        };
        let settings = self.core.settings();
        let available =
            |id: &str| crate::routing::availability::model_available(&settings, provider, id);
        let listed = self
            .runtime
            .overview(provider)
            .and_then(|overview| overview.models)
            .map(|catalog| catalog.models)
            .unwrap_or_default();
        let registry = self.runtime.registry().current();
        // Before its list is read, the family's alias stands in for it.
        let model = if listed.is_empty() {
            available(family).then(|| family.to_owned())
        } else {
            let merged = brigadier_router::merge(&registry, &[(provider, &listed)], &[], &[]);
            let model = chore_model(&listed, &merged, family, available);
            if model.is_none() {
                tracing::debug!(provider = %provider, "no light or standard model available for chores");
            }
            model
        };
        let model = model.ok_or_else(|| {
            Error::Invalid(format!(
                "{} has no small model on for this. Turn one on in Settings › Providers.",
                provider.label()
            ))
        })?;
        let effort = (provider == ProviderKind::Codex).then(|| "low".to_owned());
        Ok((model, effort))
    }
}

/// The model a chore runs on from a provider's `listed` models (with their `merged` entries):
/// the lowest registry tier the user left available, light then standard, never strong or
/// frontier, nor an unrated model or one the registry keeps for one kind of work (Haiku 5.5,
/// for operating apps). Within a tier, the `family` word first, then current models, in the
/// CLI's order.
fn chore_model(
    listed: &[ModelInfo],
    merged: &[MergedModel],
    family: &str,
    available: impl Fn(&str) -> bool,
) -> Option<String> {
    listed
        .iter()
        .zip(merged)
        .filter(|(model, merged)| {
            merged.only.is_none()
                && !merged.excluded
                && matches!(merged.tier, QualityTier::Light | QualityTier::Standard)
                && available(&model.id)
        })
        .min_by_key(|(model, merged)| (merged.tier, !model.id.contains(family), model.legacy))
        .map(|(model, _)| model.id.clone())
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
pub(super) fn source_state_of(repo: &Repo) -> Result<SourceState> {
    let (changes, branch) = repo.source_status().map_err(git_error)?;
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
        merging: repo.merging().map_err(git_error)?.map(|merge| SourceMerge {
            incoming: merge.incoming,
            message: merge.message,
        }),
        rebasing: source_rebase(repo)?,
    })
}

/// The checkout's rebase in progress, for the Source panel.
pub(super) fn source_rebase(repo: &Repo) -> Result<Option<SourceRebase>> {
    Ok(repo
        .rebasing()
        .map_err(git_error)?
        .map(|rebase| SourceRebase {
            branch: rebase.branch,
            onto: rebase.onto,
            step: rebase.step,
            total: rebase.total,
            commit: rebase.commit,
            message: rebase.message,
        }))
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

impl SessionManager {
    /// The user clicked "Merge into main" (BUILD-PLAN A4): the click is recorded with where it
    /// came from, and the thread is told to merge.
    pub async fn merge_session(
        &self,
        id: &ConversationId,
        origin: crate::work::AnswerOrigin,
    ) -> Result<String> {
        let Some(Setup::Session {
            repo,
            environment: Environment::NewWorktree { base, branch, .. },
            ..
        }) = self.core.conversation(id)?.setup
        else {
            return Err(Error::Invalid(
                "This session works in your checkout: its commits are already on the branch."
                    .into(),
            ));
        };
        let (git, repo_path, base_name, branch_name) = (
            self.git.clone(),
            PathBuf::from(&repo),
            base.clone(),
            branch.clone(),
        );
        let ahead = blocking(move || {
            let repo = git.open(&repo_path).map_err(git_error)?;
            let (Some(tip), Some(base_tip)) = (
                repo.branch_tip(&branch_name).map_err(git_error)?,
                repo.branch_tip(&base_name).map_err(git_error)?,
            ) else {
                return Ok(0);
            };
            repo.count_commits(&base_tip, &tip).map_err(git_error)
        })
        .await?;
        if ahead == 0 {
            return Err(Error::Invalid(format!(
                "Nothing to merge: `{branch}` has no commits that `{base}` lacks."
            )));
        }
        let asked = format!("merge-click:{}", crate::model::CardId::generate());
        self.orchestrator_step(
            id,
            crate::work::OrchestratorStepKind::MergeRequested {
                id: asked,
                branch: branch.clone(),
                base: base.clone(),
                origin,
            },
        )
        .await;
        self.deliver(
            id,
            super::conversation::Envelope {
                kind: crate::work::InjectionKind::Decision,
                label: "merge".into(),
                task_id: None,
                text: format!(
                    "[merge] The user clicked \"Merge into main\" for `{branch}` into `{base}`. Merge it now."
                ),
            },
        )
        .await;
        Ok(format!("Merging `{branch}` into `{base}`."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, resolved: Option<&str>) -> ModelInfo {
        ModelInfo {
            id: id.to_owned(),
            display_name: id.to_owned(),
            description: String::new(),
            resolved: resolved.map(str::to_owned),
            efforts: Vec::new(),
            default_effort: None,
            is_default: false,
            input_modalities: vec!["text".into()],
            fast: None,
            legacy: false,
        }
    }

    fn chore(provider: ProviderKind, listed: &[ModelInfo], hidden: &[&str]) -> Option<String> {
        let registry = brigadier_router::Registry::bundled();
        let merged = brigadier_router::merge(&registry, &[(provider, listed)], &[], &[]);
        let family = match provider {
            ProviderKind::Claude => "haiku",
            ProviderKind::Codex => "luna",
        };
        chore_model(listed, &merged, family, |id| !hidden.contains(&id))
    }

    /// The `haiku` alias resolving to Haiku 5.5 (kept for operating apps) leaves no small
    /// Claude model: the chore is skipped, not run on the CLI's first model, Opus.
    #[test]
    fn a_chore_never_falls_back_to_a_strong_or_frontier_model() {
        let claude = [
            info("opus", Some("claude-opus-5-5")),
            info("sonnet", Some("claude-sonnet-5-5")),
            info("haiku", Some("claude-haiku-5-5")),
        ];
        assert_eq!(chore(ProviderKind::Claude, &claude, &[]), None);

        let mut with_standard = claude.to_vec();
        with_standard.push(info("claude-sonnet-4-6", None));
        assert_eq!(
            chore(ProviderKind::Claude, &with_standard, &[]).as_deref(),
            Some("claude-sonnet-4-6")
        );
    }

    #[test]
    fn a_chore_takes_the_lowest_tier_the_user_left_on() {
        let claude = [
            info("opus", Some("claude-opus-5-5")),
            info("claude-sonnet-4-6", None),
            info("haiku", Some("claude-haiku-4-5-20251001")),
        ];
        assert_eq!(
            chore(ProviderKind::Claude, &claude, &[]).as_deref(),
            Some("haiku")
        );
        assert_eq!(
            chore(ProviderKind::Claude, &claude, &["haiku"]).as_deref(),
            Some("claude-sonnet-4-6")
        );

        let codex = [
            info("gpt-6-astra", None),
            info("gpt-6.1-sol", None),
            info("gpt-6-luna", None),
            info("gpt-5.6-luna", None),
        ];
        assert_eq!(
            chore(ProviderKind::Codex, &codex, &[]).as_deref(),
            Some("gpt-5.6-luna")
        );
        assert_eq!(
            chore(ProviderKind::Codex, &codex, &["gpt-5.6-luna"]).as_deref(),
            Some("gpt-6-luna")
        );
        assert_eq!(
            chore(ProviderKind::Codex, &codex, &["gpt-5.6-luna", "gpt-6-luna"]),
            None
        );
    }
}
