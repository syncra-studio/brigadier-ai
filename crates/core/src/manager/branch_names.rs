//! The names of the branches Brigadier makes: the user's prefix (the project's own, else
//! Settings › Git's) before `<session>/session`, `<session>/fork` and
//! `<session>/task-<n>-<slug>`. Only new branches take a changed prefix; existing ones keep
//! their names.

use std::path::Path;

use brigadier_providers::NoticeLevel;

use super::SessionManager;
use crate::model::{ConversationId, DEFAULT_BRANCH_PREFIX, ProjectId};

/// The cleanup-ledger owner of the branch namespaces a session and its workers made branches
/// in ([`brigadier_providers::Artifact::GitBranches`]): only Delete disposes of it.
pub(crate) fn branches_owner(id: &ConversationId) -> String {
    format!("session-branches:{id}")
}

/// The file in a session's `record/` naming what it keeps outside its folder (D7).
pub(crate) const OUTSIDE_FILE: &str = "outside.json";

/// `branch`'s namespace if it is one of `conversation`'s: the name up to and including
/// `<short id>/` (`brigadier/70d02049/`). Prefixes change, so the name, not the current
/// prefix, says which.
pub(crate) fn branch_namespace(conversation: &ConversationId, branch: &str) -> Option<String> {
    let short = conversation.short();
    let at = branch.find(&format!("{short}/"))?;
    let before = &branch[..at];
    // The short id starts a name part, after the prefix: not the tail of a longer one.
    if before
        .chars()
        .next_back()
        .is_some_and(|c| c.is_ascii_hexdigit())
    {
        return None;
    }
    Some(branch[..at + short.len() + 1].to_owned())
}

impl SessionManager {
    /// The prefix for `project`'s new branches: its own, else the user's. A project prefix git
    /// would refuse (one saved before prefixes were checked) gives way to the user's; with it,
    /// why.
    pub(crate) fn branch_prefix_of(&self, project: Option<&ProjectId>) -> (String, Option<String>) {
        let global = self.core.settings().branch_prefix;
        let global = if brigadier_git::branch_prefix_problem(&global).is_none() {
            global
        } else {
            DEFAULT_BRANCH_PREFIX.to_owned()
        };
        let own = project
            .and_then(|id| self.core.project(id).ok())
            .and_then(|project| project.prefs.branch_prefix);
        match own {
            Some(own) => match brigadier_git::branch_prefix_problem(&own) {
                None => (own, None),
                Some(problem) => {
                    let why = format!(
                        "This project's branch prefix “{own}” can't be used ({}), so new branches start with “{global}”.",
                        problem.trim_end_matches('.')
                    );
                    (global, Some(why))
                }
            },
            None => (global, None),
        }
    }

    /// Records that Brigadier made `branch` in `repo`: it may delete it later, whatever its
    /// name.
    pub(crate) async fn note_branch_made(&self, repo: &Path, branch: &str) {
        if let Err(err) = self.runtime.ledger().record_branch_made(repo, branch).await {
            tracing::warn!(branch, error = %err, "could not record a branch Brigadier made");
        }
    }

    /// Records, before it is made, that `conversation` (or one of its workers) makes `branch` in
    /// `repo`: its namespace goes when the session is deleted, with the branches its CLIs make
    /// there too (D7). A branch outside its namespace (one the user named) isn't recorded.
    pub(crate) async fn note_branch_namespace(
        &self,
        conversation: &ConversationId,
        repo: &Path,
        branch: &str,
    ) -> crate::Result<()> {
        let Some(namespace) = branch_namespace(conversation, branch) else {
            return Ok(());
        };
        let owner = branches_owner(conversation);
        let ledger = self.runtime.ledger();
        ledger
            .record(
                &owner,
                brigadier_providers::Artifact::GitBranches {
                    repo: repo.to_string_lossy().into_owned(),
                    namespace,
                },
            )
            .await?;
        // And in its own record, which a lost database leaves behind.
        let record = self.session_dir(conversation).record();
        if record.is_dir() {
            let listed = serde_json::to_vec_pretty(&ledger.artifacts(&owner))
                .map_err(|err| crate::Error::Invalid(err.to_string()))?;
            let file = record.join(OUTSIDE_FILE);
            if let Err(err) = super::blocking(move || {
                std::fs::write(&file, listed).map_err(|err| crate::Error::Invalid(err.to_string()))
            })
            .await
            {
                tracing::warn!(error = %err, "could not list a session's branch namespace in its record");
            }
        }
        Ok(())
    }

    /// The prefix for `conversation`'s new branches, telling it when its project's prefix
    /// gave way.
    pub(crate) async fn branch_prefix(&self, conversation: &ConversationId) -> String {
        let project = self
            .core
            .conversation(conversation)
            .ok()
            .and_then(|conversation| conversation.project_id);
        let (prefix, why) = self.branch_prefix_of(project.as_ref());
        if let Some(why) = why {
            self.notice(conversation, NoticeLevel::Warning, &why).await;
        }
        prefix
    }
}

#[cfg(test)]
mod tests {
    use super::super::workers::task_branch;
    use crate::model::ConversationId;

    #[test]
    fn a_task_branch_takes_the_prefix_and_keeps_the_rest_of_its_name() {
        let id = ConversationId("01a10853-80c6-75fc-9788-21c770d02049".into());
        assert_eq!(
            task_branch("brigadier/", &id, 3, "Fix the login bug!"),
            "brigadier/70d02049/task-3-fix-the-login-bug"
        );
        assert_eq!(
            task_branch("sg-", &id, 3, "Fix the login bug!"),
            "sg-70d02049/task-3-fix-the-login-bug"
        );
        assert_eq!(task_branch("", &id, 4, "!!"), "70d02049/task-4");
        let space = |branch: &str| super::branch_namespace(&id, branch);
        assert_eq!(
            space("brigadier/70d02049/task-3-x").as_deref(),
            Some("brigadier/70d02049/")
        );
        assert_eq!(
            space("sg-70d02049/session").as_deref(),
            Some("sg-70d02049/")
        );
        assert_eq!(space("70d02049/fork").as_deref(), Some("70d02049/"));
        assert_eq!(space("brigadier/a70d02049/session"), None);
        assert_eq!(space("feature/login"), None);
    }
}
