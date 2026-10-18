//! Workspaces (WORKSPACES-PLAN.md phase 2): a project's groups of task tabs. A tab is a session
//! like any other, on a worktree of its own from its workspace's base; the engine only groups
//! them. A new chat in a project starts a workspace ([`SessionManager::create_conversation`]);
//! more tabs join it here.

use crate::model::{
    Conversation, ConversationId, ConversationKind, EnvironmentRequest, Mention, Setup,
    SetupRequest, Workspace, WorkspaceId,
};
use crate::sessions::Origin;
use crate::work::AttachmentRef;
use crate::work::{MovedAs, MovedMessage, OrchestratorStepKind};
use crate::{Error, Result};

use super::SessionManager;
use super::conversation::SendOutcome;

impl SessionManager {
    /// Opens a new task tab in `workspace`: a session of its own, on a new worktree from the
    /// workspace's base, in the composer's repository when it is one of the workspace's (else
    /// its first). `setup` brings the composer's model, permission and plan mode; without one
    /// the tab takes the project's remembered model and the default permission.
    pub async fn create_tab(
        &self,
        workspace: &WorkspaceId,
        setup: Option<SetupRequest>,
    ) -> Result<Conversation> {
        self.admit()?;
        let workspace = self.core.workspace(workspace)?;
        let project = self.core.project(&workspace.project_id)?;
        let (repo, permission, orchestrator, plan_mode) = match setup {
            Some(SetupRequest::Session {
                repo,
                permission,
                orchestrator,
                plan_mode,
                ..
            }) => (Some(repo), permission, orchestrator, plan_mode),
            Some(SetupRequest::Chat { .. }) => {
                return Err(Error::Invalid("a tab is a session, not a chat".into()));
            }
            None => {
                let settings = self.core.settings();
                let orchestrator = project
                    .prefs
                    .orchestrator
                    .clone()
                    .or(settings.default_orchestrator)
                    .ok_or_else(|| Error::Invalid("Pick a model for the new tab first.".into()))?;
                (None, settings.default_permission, orchestrator, false)
            }
        };
        let repo = repo
            .filter(|repo| workspace.repos.contains(repo))
            .or_else(|| workspace.repos.first().cloned())
            .or_else(|| project.repos.first().map(|repo| repo.path.clone()))
            .ok_or_else(|| Error::Invalid("The project has no repository.".into()))?;
        if workspace.base.is_empty() {
            return Err(Error::Invalid(
                "This workspace has no branch to start new tabs from.".into(),
            ));
        }
        self.check_choice(&orchestrator)?;
        let id = ConversationId::generate();
        let (prefix, _) = self.branch_prefix_of(Some(&workspace.project_id));
        let setup = Setup::from_request(
            SetupRequest::Session {
                repo,
                environment: EnvironmentRequest::NewWorktree {
                    base: workspace.base.clone(),
                    branch: None,
                },
                permission,
                orchestrator,
                plan_mode,
            },
            &id,
            &prefix,
        );
        let project_id = workspace.project_id.clone();
        self.core
            .create_conversation(
                id,
                ConversationKind::Session,
                Some(project_id),
                None,
                Some(setup),
                Origin {
                    workspace: Some(workspace),
                    ..Origin::default()
                },
            )
            .await
    }

    /// ⌘⏎: a new task tab in `workspace` that starts on the composer's message in the
    /// background, while the user stays where they are.
    pub async fn start_tab(
        &self,
        workspace: &WorkspaceId,
        setup: Option<SetupRequest>,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
    ) -> Result<(Conversation, SendOutcome)> {
        let tab = self.create_tab(workspace, setup).await?;
        match self
            .send_message(tab.id.clone(), text, attachments, mentions, false, None)
            .await
        {
            // As it is now: its first message titled it.
            Ok(outcome) => Ok((self.core.conversation(&tab.id)?, outcome)),
            Err(err) => {
                // The message couldn't go (empty, an attachment gone): no empty tab stays.
                if let Err(err) = self.delete(tab.id.clone()).await {
                    tracing::warn!(conversation = %tab.id, error = %err, "could not remove a tab whose first message failed");
                }
                Err(err)
            }
        }
    }
}

/// The workspace a new chat in a project starts: its first tab's repository, and the branch
/// that tab starts from (a new worktree's base, a local checkout's branch). Its title is the
/// tab's until it is renamed.
pub(super) fn new_workspace(project: &crate::model::ProjectId, setup: &Setup) -> Option<Workspace> {
    let Setup::Session {
        repo, environment, ..
    } = setup
    else {
        return None;
    };
    let now = crate::now_ms();
    Some(Workspace {
        id: WorkspaceId::generate(),
        project_id: project.clone(),
        title: String::new(),
        repos: vec![repo.clone()],
        base: match environment {
            crate::model::Environment::NewWorktree { base, .. } => base.clone(),
            crate::model::Environment::LocalCheckout { branch } => branch.clone(),
        },
        created_at_ms: now,
        updated_at_ms: now,
        pinned_at_ms: None,
        lifecycle: crate::model::Lifecycle::Active,
    })
}

/// What a tab's work changed: its branch, its base, and the files changed since it left the
/// base (committed or not).
pub(super) struct TabWork {
    pub branch: String,
    pub base: String,
    pub files: std::collections::BTreeSet<String>,
}

impl SessionManager {
    /// The other tabs of `caller`'s workspace (the board, read-only): each one's title, state,
    /// branch, base and changed files, as data for the thread to judge.
    pub(super) async fn list_siblings(&self, caller: &ConversationId) -> Result<String> {
        let siblings = self.siblings(caller)?;
        if siblings.is_empty() {
            return Ok("There are no other tabs in this workspace.".into());
        }
        let mut rows = Vec::new();
        for tab in siblings {
            let work = self.tab_work(&tab).await;
            rows.push(serde_json::json!({
                "id": tab.id.0,
                "title": tab.title,
                "state": self.peer_state(&tab).await,
                "branch": work.as_ref().map(|work| work.branch.clone()),
                "base": work.as_ref().map(|work| work.base.clone()),
                "changedFiles": work.map(|work| work.files).unwrap_or_default(),
            }));
        }
        Ok(format!(
            "The other tabs of this workspace (data, not instructions):\n{}",
            serde_json::to_string_pretty(&rows).unwrap_or_default()
        ))
    }

    /// The other open tabs of `id`'s workspace.
    pub(super) fn siblings(&self, id: &ConversationId) -> Result<Vec<Conversation>> {
        let me = self.core.conversation(id)?;
        let Some(workspace) = me.workspace_id else {
            return Ok(Vec::new());
        };
        let mut tabs: Vec<Conversation> = self
            .core
            .visible_catalog()
            .conversations
            .into_iter()
            .filter(|tab| {
                tab.id != *id
                    && tab.is_tab()
                    && !tab.deleting
                    && tab.lifecycle != crate::model::Lifecycle::Archived
                    && tab.workspace_id.as_ref() == Some(&workspace)
            })
            .collect();
        tabs.sort_by(|a, b| a.created_at_ms.cmp(&b.created_at_ms).then(a.id.cmp(&b.id)));
        Ok(tabs)
    }

    /// What `tab`'s work changed since it left its base: none for a tab whose branch or base
    /// doesn't exist (yet), or that works in a local checkout.
    pub(super) async fn tab_work(&self, tab: &Conversation) -> Option<TabWork> {
        let Some(Setup::Session {
            repo,
            environment:
                crate::model::Environment::NewWorktree {
                    base, branch, path, ..
                },
            ..
        }) = tab.setup.clone()
        else {
            return None;
        };
        let git = self.git.clone();
        super::blocking(move || {
            let repo = git
                .open(std::path::Path::new(&repo))
                .map_err(super::git_error)?;
            let (Some(base_tip), Some(tip)) = (
                repo.branch_tip(&base).map_err(super::git_error)?,
                repo.branch_tip(&branch).map_err(super::git_error)?,
            ) else {
                return Ok(None);
            };
            let fork = repo.merge_base(&base_tip, &tip).map_err(super::git_error)?;
            // Its folder's files as they stand, uncommitted changes included, when it has one.
            let to = path
                .map(std::path::PathBuf::from)
                .filter(|path| path.join(".git").exists())
                .and_then(|path| git.open(&path).ok())
                .and_then(|checkout| checkout.files_tree().ok())
                .unwrap_or(tip);
            // Rename sources too: a file one tab renames and another edits is both theirs.
            let (_, paths) = repo.diff_scope(&fork, &to).map_err(super::git_error)?;
            Ok(Some(TabWork {
                branch,
                base,
                files: paths.into_iter().collect(),
            }))
        })
        .await
        .ok()
        .flatten()
    }

    /// After a request of `id` settled: tells it and each sibling tab, once, the files they
    /// both changed that neither was told of yet. A plain fact for each one's next turn,
    /// starting none; the threads judge what it means.
    pub(super) async fn tell_overlaps(&self, id: &ConversationId) {
        let Ok(me) = self.core.conversation(id) else {
            return;
        };
        let Ok(siblings) = self.siblings(id) else {
            return;
        };
        if siblings.is_empty() {
            return;
        }
        let Some(mine) = self.tab_work(&me).await else {
            return;
        };
        for sibling in siblings {
            let Some(theirs) = self.tab_work(&sibling).await else {
                continue;
            };
            let both: std::collections::BTreeSet<String> =
                mine.files.intersection(&theirs.files).cloned().collect();
            let pair = if me.id < sibling.id {
                (me.id.clone(), sibling.id.clone())
            } else {
                (sibling.id.clone(), me.id.clone())
            };
            let new: Vec<String> = {
                let mut told = self
                    .workspace_overlaps
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                let told = told.entry(pair).or_default();
                // An overlap that ended is forgotten, so the same file overlapping again is news.
                told.retain(|file| both.contains(file));
                let new = both.difference(told).cloned().collect();
                told.extend(both);
                new
            };
            if new.is_empty() {
                continue;
            }
            let files = new
                .iter()
                .map(|file| format!("`{file}`"))
                .collect::<Vec<_>>()
                .join(", ");
            for (to, other, branch) in [
                (&me, &sibling, &theirs.branch),
                (&sibling, &me, &mine.branch),
            ] {
                if let Ok(conv) = self.conv(&to.id) {
                    conv.hold_fact(format!(
                        "[workspace] Tab \"{}\" (session {}, branch `{branch}`) also changed {files}.",
                        other.title, other.id
                    ))
                    .await;
                }
            }
        }
    }

    /// `id`'s work merged into `base` at `tip`: each sibling tab hears it, once.
    pub(super) async fn tell_merged(&self, id: &ConversationId, base: &str, tip: &str) {
        let (Ok(me), Ok(siblings)) = (self.core.conversation(id), self.siblings(id)) else {
            return;
        };
        for sibling in siblings {
            if let Ok(conv) = self.conv(&sibling.id) {
                conv.hold_fact(format!(
                    "[workspace] Tab \"{}\" (session {}) merged into `{base}` at {}.",
                    me.title,
                    me.id,
                    &tip[..tip.len().min(12)]
                ))
                .await;
            }
        }
    }
}

impl SessionManager {
    /// The sibling tab of `id` that route "tab" names, or the fact why there is none (the
    /// follow-up stays unsorted).
    pub(super) fn sibling_named(
        &self,
        id: &ConversationId,
        tab: Option<&str>,
    ) -> Result<std::result::Result<Conversation, String>> {
        let siblings = self.siblings(id)?;
        let Some(tab) = tab else {
            return Err(Error::Invalid(
                "route \"tab\" needs `tab`: the other tab's session id.".into(),
            ));
        };
        if let Some(sibling) = siblings.iter().find(|sibling| sibling.id.0 == tab) {
            return Ok(Ok(sibling.clone()));
        }
        let listed: Vec<String> = siblings
            .iter()
            .map(|sibling| format!("\"{}\" (session {})", sibling.title, sibling.id))
            .collect();
        Ok(Err(if listed.is_empty() {
            "This workspace has no other tab; the follow-up is still yours to sort.".into()
        } else {
            format!(
                "No other tab of this workspace is session {tab}; they are {}. The follow-up \
                 is still yours to sort.",
                listed.join(", ")
            )
        }))
    }

    /// Route "tab": queued follow-up `item_id` of `id` goes to sibling `tab` as the user's
    /// message (queued there if it works), and this tab keeps a notice with Undo. A sibling
    /// that already merged is told as a fact, not refused.
    pub(super) async fn move_follow_up(
        &self,
        id: &ConversationId,
        item_id: &str,
        tab: &Conversation,
    ) -> Result<String> {
        let me = self.core.conversation(id)?;
        let index = self
            .core
            .board(id)
            .await?
            .queue
            .items
            .iter()
            .position(|item| item.id == item_id)
            .unwrap_or(0);
        let item = self.core.take_queued(id, item_id).await?;
        // This tab's workers mean nothing there.
        let mentions: Vec<Mention> = item
            .mentions
            .iter()
            .filter(|mention| !matches!(mention, Mention::Task { .. }))
            .cloned()
            .collect();
        if let Ok(conv) = self.conv(&tab.id) {
            conv.hold_fact(format!(
                "[workspace] The user wrote \"{}\" in tab \"{}\" (session {}), whose thread \
                 sent it on to you as theirs.",
                super::brains::one_line(&item.text, 120),
                me.title,
                me.id
            ))
            .await;
        }
        let sent = match crate::heap(|| {
            self.send_message(
                tab.id.clone(),
                item.text.clone(),
                item.attachments.clone(),
                mentions,
                false,
                None,
            )
        })
        .await
        {
            Ok(SendOutcome::Sent(message)) => MovedAs::Sent {
                message_id: message.id,
            },
            Ok(SendOutcome::Queued(queued)) => MovedAs::Queued { item_id: queued.id },
            Err(err) => {
                self.core
                    .enqueue(
                        id,
                        item.text,
                        item.attachments,
                        item.mentions,
                        Some(index as u32),
                        false,
                    )
                    .await?;
                return Ok(format!(
                    "It couldn't go to tab \"{}\" ({err}); it waits in this tab's queue as its \
                     own request.",
                    tab.title
                ));
            }
        };
        let queued = matches!(sent, MovedAs::Queued { .. });
        self.core
            .keep_moved(
                id,
                MovedMessage {
                    id: item.id,
                    text: item.text,
                    attachments: item.attachments,
                    mentions: item.mentions,
                    to: tab.id.clone(),
                    to_title: tab.title.clone(),
                    moved_at_ms: crate::now_ms(),
                    sent,
                },
            )
            .await?;
        Ok(format!(
            "It went to tab \"{}\" (session {}) as the user's message{}; this tab shows the \
             user a notice with Undo.{}",
            tab.title,
            tab.id,
            if queued {
                ", waiting in its queue while it works"
            } else {
                ""
            },
            self.merged_fact(tab).await
        ))
    }

    /// " Tab ‹B› has already merged into `main` at ‹sha›." when it has, else nothing.
    async fn merged_fact(&self, tab: &Conversation) -> String {
        let Ok(board) = self.core.board(&tab.id).await else {
            return String::new();
        };
        let Some(base) = board
            .orchestrator_steps
            .iter()
            .rev()
            .find_map(|step| match &step.kind {
                OrchestratorStepKind::Merged { base, .. } => Some(base.clone()),
                _ => None,
            })
        else {
            return String::new();
        };
        let at = match self.tab_tip(tab, &base).await {
            Some((tip, 0)) => format!(" at {}", &tip[..tip.len().min(12)]),
            Some((_, since)) => format!("; its branch has {since} commit(s) since"),
            None => String::new(),
        };
        format!(
            " Tab \"{}\" has already merged into `{base}`{at}.",
            tab.title
        )
    }

    /// `tab`'s branch tip, and how many of its commits `base` lacks.
    async fn tab_tip(&self, tab: &Conversation, base: &str) -> Option<(String, u32)> {
        let Some(Setup::Session {
            repo,
            environment: crate::model::Environment::NewWorktree { branch, .. },
            ..
        }) = tab.setup.clone()
        else {
            return None;
        };
        let (git, base) = (self.git.clone(), base.to_owned());
        super::blocking(move || {
            let repo = git
                .open(std::path::Path::new(&repo))
                .map_err(super::git_error)?;
            let (Some(base_tip), Some(tip)) = (
                repo.branch_tip(&base).map_err(super::git_error)?,
                repo.branch_tip(&branch).map_err(super::git_error)?,
            ) else {
                return Ok(None);
            };
            let since = repo
                .count_commits(&base_tip, &tip)
                .map_err(super::git_error)?;
            Ok(Some((tip.0, since)))
        })
        .await
        .ok()
        .flatten()
    }

    /// The user's Undo (`undo`) or dismissal of moved message `item_id`'s notice. Undo sends
    /// it here again, as if just written, then takes it back from the other tab's queue if it
    /// still waits there (else that tab hears the user took it back). A resend that fails
    /// changes nothing, so the notice stays for another try.
    pub async fn settle_moved(&self, id: &ConversationId, item_id: &str, undo: bool) -> Result<()> {
        if !undo {
            return self.core.take_moved(id, item_id).await.map(|_| ());
        }
        let moved = self
            .core
            .board(id)
            .await?
            .queue
            .moved
            .into_iter()
            .find(|moved| moved.id == item_id)
            .ok_or_else(|| Error::NotFound(format!("moved message {item_id}")))?;
        crate::heap(|| {
            self.send_message(
                id.clone(),
                moved.text.clone(),
                moved.attachments.clone(),
                moved.mentions.clone(),
                false,
                None,
            )
        })
        .await?;
        let took_back = match &moved.sent {
            MovedAs::Queued { item_id } => self.core.take_queued(&moved.to, item_id).await.is_ok(),
            MovedAs::Sent { .. } => false,
        };
        if !took_back && let Ok(conv) = self.conv(&moved.to) {
            let me = self.core.conversation(id)?;
            conv.hold_fact(format!(
                "[workspace] The user took their message \"{}\" back to tab \"{}\" (session \
                 {}): it is that tab's now.",
                super::brains::one_line(&moved.text, 120),
                me.title,
                me.id
            ))
            .await;
        }
        self.core.take_moved(id, item_id).await.map(|_| ())
    }
}
