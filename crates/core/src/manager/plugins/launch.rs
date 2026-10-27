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
        .find(|root| root.as_path() == at)
        .or_else(|| {
            registered
                .iter()
                .filter(|root| at.starts_with(root))
                .max_by_key(|root| root.components().count())
        })
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
        let secrets = CliSecretReader {
            cli: self.brigadier_cli(),
            env: roots.vars.clone(),
        };
        let gateway = self.mcp_gateway().clone();
        let agent = super::super::integrations::agent(account.provider);
        let grants = match role {
            Role::Thread => None,
            Role::Worker { grants } => Some(grants.to_vec()),
        };
        blocking(move || {
            let mut rows = Catalog::read_rows_with_library(&roots, &registered, &data_dir);
            let choices = Choices::load(&data_dir).map_err(Error::Invalid)?;
            let warnings = if grants.is_none() {
                fill_library_headers(&mut rows, &data_dir, &choices, project.as_deref(), &secrets)
            } else {
                Vec::new()
            };
            let mut resolved = resolve::resolve(Context {
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
                gateway: Some(gateway.as_ref()),
            });
            // Plugin clients have a separate native namespace from the canonical injected
            // server name. Suppress those clients even when their row is selected.
            let checkout = cwd
                .ancestors()
                .find(|path| path.join(".git").exists())
                .unwrap_or(&cwd);
            let native = Catalog::read(&effective, trusted.then_some(checkout));
            for server in native.servers.iter().filter(|server| {
                server
                    .plugin
                    .as_ref()
                    .is_some_and(|plugin| plugin.agent == agent)
            }) {
                let plugin = server.plugin.as_ref().expect("filtered plugin");
                match agent {
                    brigadier_registry::Agent::Claude => {
                        for alias in server.provenance().aliases {
                            if !resolved.off.servers.contains(&alias) {
                                resolved.off.servers.push(alias);
                            }
                        }
                    }
                    brigadier_registry::Agent::Codex => {
                        let entry = (plugin.id.clone(), server.name.clone());
                        if !resolved.off.plugin_servers.contains(&entry) {
                            resolved.off.plugin_servers.push(entry);
                        }
                    }
                }
            }
            resolved.warnings = warnings;
            Ok(resolved)
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
            Some(main.clone())
        );
        assert!(registered_root(&[], None, &work).is_none());
        let child = main.join("nested");
        let roots = vec![main.clone(), child.clone()];
        assert_eq!(
            registered_root(&roots, Some(&child), &work),
            Some(child.clone())
        );
        assert_eq!(
            registered_root(&roots, None, &child.join("src")),
            Some(child)
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// Materialize only the skills a CLI lacks, in Brigadier's session directory. Both adapters
/// use this same overlay: Claude's add-dir root and Codex's extra-root plugin layout.
pub fn apply_resolved(
    spec: &mut brigadier_providers::SessionSpec,
    resolved: &Resolved,
    session_dir: &Path,
    provider: brigadier_providers::ProviderKind,
) -> Result<()> {
    spec.mcp_servers.extend(
        resolved
            .servers
            .iter()
            .filter(|server| !spec.mcp_servers.iter().any(|own| own.name == server.name))
            .cloned()
            .collect::<Vec<_>>(),
    );
    spec.switched_off = resolved.off.clone();
    // Native configs used helpers before injection. Protect newly materialized values in
    // adapter stdio recordings and events too, including inline Codex thread configuration.
    let values: Vec<_> = spec
        .mcp_servers
        .iter()
        .flat_map(|server| server.env.iter().map(|(_, value)| value.clone()))
        .collect();
    spec.redactor = Some(std::sync::Arc::new(match &spec.redactor {
        Some(redactor) => redactor.with(values),
        None => brigadier_providers::Redactor::new(values),
    }));
    let overlay = session_dir.join("overlay");
    if overlay.exists() {
        std::fs::remove_dir_all(&overlay).map_err(|error| Error::Invalid(error.to_string()))?;
    }
    let skills = overlay.join(".claude/skills");
    let injected: Vec<_> = resolved
        .skills
        .iter()
        .filter(|skill| !skill.native)
        .collect();
    if injected.is_empty() && spec.installed_skills.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(&skills).map_err(|error| Error::Invalid(error.to_string()))?;
    for plugin in std::mem::take(&mut spec.installed_skills) {
        if let Ok(entries) = std::fs::read_dir(plugin.join("skills")) {
            for entry in entries.flatten() {
                if entry.path().join("SKILL.md").is_file() {
                    brigadier_providers::accounts::link_to(
                        &entry.path(),
                        &skills.join(entry.file_name()),
                        true,
                    )
                    .map_err(|error| Error::Invalid(error.to_string()))?;
                }
            }
        }
    }
    for skill in injected {
        if Path::new(&skill.display).components().count() != 1
            || !matches!(
                Path::new(&skill.display).components().next(),
                Some(std::path::Component::Normal(_))
            )
        {
            return Err(Error::Invalid(format!(
                "invalid skill name {:?}",
                skill.display
            )));
        }
        let source = skill
            .path
            .parent()
            .ok_or_else(|| Error::Invalid("skill has no folder".into()))?;
        brigadier_providers::accounts::link_to(source, &skills.join(&skill.display), true)
            .map_err(|error| {
                Error::Invalid(format!("couldn't stage skill {}: {error}", skill.display))
            })?;
    }
    match provider {
        brigadier_providers::ProviderKind::Claude => spec.add_dirs.push(overlay),
        brigadier_providers::ProviderKind::Codex => {
            brigadier_providers::accounts::link_to(&skills, &overlay.join("skills"), true)
                .map_err(|error| Error::Invalid(error.to_string()))?;
            spec.skill_plugins.push(overlay);
        }
    }
    Ok(())
}

#[cfg(test)]
mod overlay_tests {
    use super::*;
    use brigadier_providers::{Access, Origin, ProviderKind, SessionSpec, ToolSet};
    use brigadier_registry::{Agent, Roots};

    fn test_spec(cwd: &Path) -> SessionSpec {
        SessionSpec {
            cwd: cwd.to_owned(),
            model: None,
            effort: None,
            fast: false,
            origin: Origin::New,
            ephemeral: false,
            access: Access::Full,
            append_system_prompt: None,
            mcp_servers: Vec::new(),
            tools: ToolSet::Thread,
            add_dirs: Vec::new(),
            project: None,
            skill_plugins: Vec::new(),
            installed_skills: Vec::new(),
            switched_off: Default::default(),
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            sweep_dir: None,
            keeps_detached: false,
            auto_compact: true,
            allowed_models: None,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            naked_worker: false,
            native_compaction: None,
            temp_tag: None,
        }
    }
    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    #[test]
    fn excluded_project_items_are_staged_for_both_agents_and_divergent_account_homes() {
        let dir = std::env::temp_dir().join(format!("launch-overlay-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        let home = dir.join("home");
        let roots = Roots::from_vars([
            ("HOME".into(), home.display().to_string()),
            ("USERPROFILE".into(), home.display().to_string()),
        ])
        .unwrap();
        let project = dir.join("repo");
        let work = dir.join("worktree");
        write(&work.join(".git"), "gitdir: fixture");
        write(
            &project.join(".mcp.json"),
            r#"{"mcpServers":{"db":{"url":"https://project.invalid/mcp"}}}"#,
        );
        write(
            &roots.claude_dir.join("skills/collision/SKILL.md"),
            "---\nname: collision\n---\nNATIVE",
        );
        write(
            &project.join(".claude/skills/collision/SKILL.md"),
            "---\nname: collision\n---\nPROJECT",
        );
        let rows = Catalog::read_rows(&roots, std::slice::from_ref(&project));
        for agent in [Agent::Claude, Agent::Codex] {
            let provider = match agent {
                Agent::Claude => ProviderKind::Claude,
                Agent::Codex => ProviderKind::Codex,
            };
            for alternate in [false, true] {
                let mut effective = roots.clone();
                if alternate {
                    effective.claude_dir = dir.join("alternate-claude");
                    effective.codex_home = dir.join("alternate-codex");
                }
                let resolved = resolve::resolve(Context {
                    agent,
                    account_home: &effective,
                    project_root: Some(&project),
                    cwd: &work,
                    trusted: true,
                    role: Role::Thread,
                    rows: &rows,
                    choices: &Choices::default(),
                    gateway: None,
                });
                assert_eq!(
                    resolved
                        .servers
                        .iter()
                        .map(|server| server.name.as_str())
                        .collect::<Vec<_>>(),
                    ["db"]
                );
                assert_eq!(resolved.skills.len(), 1);
                let invocation = if agent == Agent::Claude && !alternate {
                    "overlay:collision"
                } else {
                    "collision"
                };
                assert_eq!(resolved.skills[0].invocation, invocation);
                let session = dir.join(format!("session-{provider:?}-{alternate}"));
                let mut spec = test_spec(&work);
                apply_resolved(&mut spec, &resolved, &session, provider).unwrap();
                assert_eq!(spec.mcp_servers, resolved.servers);
                assert_eq!(
                    std::fs::read_to_string(
                        session.join("overlay/.claude/skills/collision/SKILL.md")
                    )
                    .unwrap(),
                    "---\nname: collision\n---\nPROJECT"
                );
                assert!(!work.join(".mcp.json").exists());
                if agent == Agent::Claude {
                    assert_eq!(spec.add_dirs, [session.join("overlay")]);
                } else {
                    assert!(session.join("overlay/skills/collision/SKILL.md").is_file());
                }
                let worker = resolve::resolve(Context {
                    agent,
                    account_home: &effective,
                    project_root: Some(&project),
                    cwd: &work,
                    trusted: true,
                    role: Role::Worker { grants: &[] },
                    rows: &rows,
                    choices: &Choices::default(),
                    gateway: None,
                });
                let mut naked = test_spec(&work);
                naked.naked_worker = true;
                apply_resolved(
                    &mut naked,
                    &worker,
                    &dir.join(format!("worker-{provider:?}-{alternate}")),
                    provider,
                )
                .unwrap();
                assert!(naked.mcp_servers.is_empty());
                assert!(naked.skill_plugins.is_empty());
                assert!(naked.add_dirs.is_empty());
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// Read through the credential-owning executable, never the daemon's own Keychain identity.
struct CliSecretReader {
    cli: PathBuf,
    env: std::collections::BTreeMap<String, String>,
}
impl brigadier_cli::secrets::Store for CliSecretReader {
    fn get(&self, server: &str) -> std::result::Result<brigadier_cli::secrets::Values, String> {
        let mut command = std::process::Command::new(&self.cli);
        command
            .args(["secret", "headers", server])
            .envs(&self.env)
            .stdin(std::process::Stdio::null());
        let output = brigadier_registry::adapter::output_within(
            &mut command,
            std::time::Duration::from_secs(30),
        )
        .map_err(|_| "couldn't read the server's saved credentials".to_owned())?;
        if !output.status.success() {
            return Err("couldn't read the server's saved credentials".into());
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|_| "the server's saved credentials weren't readable".into())
    }
    fn put(&self, _: &str, _: &brigadier_cli::secrets::Values) -> std::result::Result<(), String> {
        Err("launch secret access is read-only".into())
    }
}

fn fill_library_headers(
    rows: &mut [brigadier_registry::catalog::Row],
    data_dir: &Path,
    choices: &Choices,
    project: Option<&Path>,
    store: &dyn brigadier_cli::secrets::Store,
) -> Vec<super::resolve::ResolveWarning> {
    use brigadier_registry::catalog::Content;
    use brigadier_registry::{Secret, Transport};
    let mut warnings = Vec::new();
    for row in rows {
        let choice = choices.choice(row);
        if !choice.on || !choice.scope.contains(project) {
            continue;
        }
        row.definitions.retain_mut(|definition| {
            let owned = definition.provenance.file.starts_with(data_dir.join("library"))
                || definition.provenance.plugin.as_ref().is_some_and(|id| id.ends_with("@brigadier"));
            if !owned {return true;}
            let Content::Server(server)=&mut definition.content else {return true;};
            let Transport::Http{headers,..}=&mut server.transport else{return true;};
            let raw:serde_json::Value=std::fs::read(&definition.provenance.file).ok().and_then(|bytes|serde_json::from_slice(&bytes).ok()).unwrap_or_default();
            let entry=raw.get("mcpServers").and_then(|all|all.get(&server.name)).or_else(||raw.get(&server.name)).unwrap_or(&raw);
            let helper=entry.get("headersHelper").or_else(||entry.get("http_headers_helper")).and_then(serde_json::Value::as_str);
            let Some(id)=helper.and_then(|helper| {
                let words:Vec<_>=helper.split_whitespace().collect();
                let tail=words.len().checked_sub(3).map(|at|&words[at..])?;
                (tail[0]=="secret" && tail[1]=="headers").then_some(tail[2].to_owned())
            }) else{return true;};
            let mut expected:Vec<String>=definition.provenance.file.parent().and_then(|path|std::fs::read(path.join(".brigadier.json")).ok())
                .and_then(|bytes|serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .and_then(|meta|meta.get("values").and_then(serde_json::Value::as_array).cloned())
                .unwrap_or_default().iter().filter_map(|value|value.as_str().map(str::to_owned)).collect();
            // Flat entries may declare empty placeholders; legacy packages keep their
            // expected names in .brigadier.json. Neither holds the credential values.
            expected.extend(headers.iter().filter(|(_, value)| value.expose().is_empty()).map(|(name, _)| name.clone()));
            let loaded=store.get(&id);
            if let Ok(values)=loaded && !values.is_empty() && expected.iter().all(|name|values.get(name).is_some_and(|value|!value.is_empty())) && values.values().all(|value|!value.is_empty()) {
                for (name,value) in values {
                    if let Some((_,old))=headers.iter_mut().find(|(key,_)|key==&name) {*old=Secret::new(value);} else {headers.push((name,Secret::new(value)));}
                }
                server.signs_in=false;
                true
            } else {
                warnings.push(super::resolve::ResolveWarning {server:server.name.clone(),message:format!("MCP server {:?} was left out because its saved credentials are missing or unavailable. Add the missing values in Plugins before starting it again.",server.name)});
                false
            }
        });
    }
    warnings
}

#[cfg(test)]
mod secret_tests {
    use super::*;
    use brigadier_cli::secrets::{Store, Values};
    use brigadier_registry::{Agent, Roots};
    use std::cell::RefCell;

    struct Memory {
        values: RefCell<Values>,
        reads: RefCell<Vec<String>>,
    }
    impl Store for Memory {
        fn get(&self, id: &str) -> std::result::Result<Values, String> {
            self.reads.borrow_mut().push(id.to_owned());
            Ok(self.values.borrow().clone())
        }
        fn put(&self, _: &str, _: &Values) -> std::result::Result<(), String> {
            panic!("launch must never write credentials")
        }
    }
    fn write(path: &Path, value: serde_json::Value) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value.to_string()).unwrap();
    }
    #[test]
    fn library_http_values_stay_in_memory_and_missing_values_omit_the_server_with_a_warning() {
        let dir = std::env::temp_dir().join(format!("launch-secrets-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dir = std::fs::canonicalize(dir).unwrap();
        let roots = Roots::from_vars([
            ("HOME".into(), dir.join("home").display().to_string()),
            ("USERPROFILE".into(), dir.join("home").display().to_string()),
        ])
        .unwrap();
        let plugin = dir.join("library/plugins/fixture");
        write(
            &dir.join("library/.claude-plugin/marketplace.json"),
            serde_json::json!({"plugins":[{"name":"fixture","version":"1"}]}),
        );
        write(
            &plugin.join(".claude-plugin/plugin.json"),
            serde_json::json!({"name":"fixture"}),
        );
        write(
            &plugin.join(".brigadier.json"),
            serde_json::json!({"values":["Authorization","X-Workspace"]}),
        );
        let file = plugin.join(".mcp.json");
        write(
            &file,
            serde_json::json!({"mcpServers":{"api":{"type":"http","url":"https://fixture.invalid/mcp","headersHelper":"brigadier secret headers fixture/api"}}}),
        );
        let original = std::fs::read(&file).unwrap();
        let values = Values::from_iter([
            ("Authorization".into(), "Bearer in-memory-only-token".into()),
            ("X-Workspace".into(), "workspace-id".into()),
        ]);
        let store = Memory {
            values: RefCell::new(values.clone()),
            reads: RefCell::new(Vec::new()),
        };
        let mut rows = Catalog::read_rows_with_library(&roots, &[], &dir);
        assert_eq!(rows.len(), 1);
        assert!(
            fill_library_headers(&mut rows, &dir, &Choices::default(), None, &store).is_empty()
        );
        for agent in [Agent::Claude, Agent::Codex] {
            let resolved = resolve::resolve(Context {
                agent,
                account_home: &roots,
                project_root: None,
                cwd: &dir,
                trusted: false,
                role: Role::Thread,
                rows: &rows,
                choices: &Choices::default(),
                gateway: None,
            });
            assert_eq!(resolved.servers.len(), 1);
            let found: Values = resolved.servers[0].env.iter().cloned().collect();
            assert_eq!(found, values);
        }
        assert_eq!(*store.reads.borrow(), ["fixture/api"]);
        store.values.borrow_mut().remove("X-Workspace");
        let mut rows = Catalog::read_rows_with_library(&roots, &[], &dir);
        let warnings = fill_library_headers(&mut rows, &dir, &Choices::default(), None, &store);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].server, "api");
        assert!(!warnings[0].message.contains("in-memory-only-token"));
        let resolved = resolve::resolve(Context {
            agent: Agent::Claude,
            account_home: &roots,
            project_root: None,
            cwd: &dir,
            trusted: false,
            role: Role::Thread,
            rows: &rows,
            choices: &Choices::default(),
            gateway: None,
        });
        assert!(resolved.servers.is_empty());
        assert_eq!(std::fs::read(&file).unwrap(), original);
        assert!(
            !String::from_utf8(original)
                .unwrap()
                .contains("in-memory-only-token")
        );
        write(&plugin.join(".brigadier.json"), serde_json::json!({}));
        write(
            &file,
            serde_json::json!({"mcpServers":{"api":{"type":"http","url":"https://fixture.invalid/mcp","headers":{"Authorization":"","X-Workspace":""},"headersHelper":"brigadier secret headers fixture/api"}}}),
        );
        let mut rows = Catalog::read_rows_with_library(&roots, &[], &dir);
        assert_eq!(
            fill_library_headers(&mut rows, &dir, &Choices::default(), None, &store).len(),
            1
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
