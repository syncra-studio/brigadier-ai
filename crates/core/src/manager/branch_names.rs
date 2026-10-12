//! The names of the branches Brigadier makes: the user's prefix (the project's own, else
//! Settings › Git's) before `<session>/session`, `<session>/fork` and
//! `<session>/task-<n>-<slug>`. Only new branches take a changed prefix; existing ones keep
//! their names.

use std::path::Path;

use brigadier_providers::NoticeLevel;

use super::SessionManager;
use crate::model::{ConversationId, DEFAULT_BRANCH_PREFIX, ProjectId};

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
    }
}
