//! Shared manager boundary for launches, draft menus, and typed skill references.
use std::path::{Path, PathBuf};

use brigadier_registry::Catalog;

use super::choices::Choices;
use super::resolve::{self, Context, Resolved, Role};
use crate::accounts::AccountRef;
use crate::manager::{SessionManager, blocking};
use crate::{Error, Result};

/// Match a checkout (including a linked worktree) to the registered scope identity.
fn registered_root(registered: &[PathBuf], project: Option<&Path>, cwd: &Path) -> Option<PathBuf> {
    let at = project.unwrap_or(cwd);
    let key = brigadier_registry::claude_config::project_key(at);
    registered
        .iter()
        .find(|root| at.starts_with(root))
        .or_else(|| {
            registered
                .iter()
                .find(|root| brigadier_registry::claude_config::project_key(root) == key)
        })
        .cloned()
}

impl SessionManager {
    /// The same resolved set for a launch or composer request. `project` may be a registered
    /// root or checkout; scope always uses its registered root, native presence uses `cwd`.
    /// Workers supply already validated grants through `Role::Worker`.
    pub async fn resolve_plugins(
        &self,
        account: &AccountRef,
        project: Option<&Path>,
        cwd: &Path,
        trusted: bool,
        role: Role<'_>,
    ) -> Result<Resolved> {
        let roots = self.registry().roots().clone();
        let mut effective = roots.clone();
        if let Some(id) = &account.account {
            let home = self.runtime.account_home(id);
            effective.vars.insert(
                brigadier_providers::accounts::home_var(account.provider).to_owned(),
                home.to_string_lossy().into_owned(),
            );
            match account.provider {
                brigadier_providers::ProviderKind::Claude => {
                    effective.claude_json =
                        brigadier_registry::claude_config::config_file(&roots.home, Some(&home));
                    effective.claude_dir = home;
                }
                brigadier_providers::ProviderKind::Codex => effective.codex_home = home,
            }
        }
        let registered: Vec<_> = self
            .core
            .catalog()
            .projects
            .into_iter()
            .flat_map(|project| {
                project
                    .repos
                    .into_iter()
                    .map(|repo| PathBuf::from(repo.path))
            })
            .collect();
        let project = registered_root(&registered, project, cwd);
        let (cwd, data_dir) = (cwd.to_owned(), self.data_dir.clone());
        let agent = super::super::integrations::agent(account.provider);
        let grants = match role {
            Role::Thread => None,
            Role::Worker { grants } => Some(grants.to_vec()),
        };
        blocking(move || {
            let rows = Catalog::read_rows(&roots, &registered);
            let choices = Choices::load(&data_dir).map_err(Error::Invalid)?;
            Ok(resolve::resolve(Context {
                agent,
                account_home: &effective,
                project_root: project.as_deref(),
                cwd: &cwd,
                trusted,
                role: grants
                    .as_deref()
                    .map_or(Role::Thread, |grants| Role::Worker { grants }),
                rows: &rows,
                choices: &choices,
                gateway: None,
            }))
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linked_worktree_uses_registered_root_without_reading_its_project_files() {
        let dir = std::env::temp_dir().join(format!("plugins-launch-{}", uuid::Uuid::new_v4()));
        let main = dir.join("main");
        let work = dir.join("work");
        std::fs::create_dir_all(main.join(".git/worktrees/w")).unwrap();
        std::fs::create_dir_all(work.join("subdir")).unwrap();
        std::fs::write(main.join(".git/worktrees/w/commondir"), "../..").unwrap();
        std::fs::write(
            work.join(".git"),
            format!("gitdir: {}", main.join(".git/worktrees/w").display()),
        )
        .unwrap();
        let main = std::fs::canonicalize(main).unwrap();
        assert_eq!(
            registered_root(std::slice::from_ref(&main), None, &work.join("subdir")),
            Some(main.clone())
        );
        assert_eq!(
            registered_root(std::slice::from_ref(&main), Some(&main), &work),
            Some(main)
        );
        assert!(registered_root(&[], None, &work).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
