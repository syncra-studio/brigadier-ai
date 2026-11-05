//! The launch/menu decision in one place. All filesystem access is read-only.
use brigadier_providers::ProviderKind;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use brigadier_providers::{
    McpGateway, McpServer, McpSources, McpTransport, McpValuePart, SwitchedOff,
};
use brigadier_registry::catalog::{Content, Definition, Kind, Row};
use brigadier_registry::{Catalog, Roots, Transport, WrittenPart};

use super::choices::Choices;

#[derive(Debug, Clone, Copy)]
pub enum Role<'a> {
    Thread,
    /// Already validated explicit grants; workers never inherit catalog items or skills.
    Worker {
        grants: &'a [McpServer],
    },
}

/// Gateway authorization is supplied by the daemon. Returning None preserves a CLI-native
/// transport for providers using the explicit CLI OAuth fallback.
pub trait Gateway {
    fn endpoint(&self, row_id: &str, resource: &str) -> Option<McpGateway>;
}

pub struct Context<'a> {
    pub agent: ProviderKind,
    /// Effective account roots, including any divergent CLAUDE_CONFIG_DIR/CODEX_HOME.
    pub account_home: &'a Roots,
    /// Registered identity used for choices, even when cwd belongs to a linked worktree.
    pub project_root: Option<&'a Path>,
    pub cwd: &'a Path,
    pub trusted: bool,
    pub role: Role<'a>,
    pub rows: &'a [Row],
    pub choices: &'a Choices,
    pub gateway: Option<&'a dyn Gateway>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedSkill {
    pub row_id: String,
    pub display: String,
    pub invocation: String,
    /// The SKILL.md accepted by Codex's typed input, never a frontend-provided path.
    pub path: PathBuf,
    pub native: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolveWarning {
    pub server: String,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Resolved {
    pub servers: Vec<McpServer>,
    pub skills: Vec<ResolvedSkill>,
    pub off: SwitchedOff,
    pub warnings: Vec<ResolveWarning>,
}

/// An actual checkout, rather than the registered project's files, supplies native presence.
fn checkout(cwd: &Path) -> &Path {
    cwd.ancestors()
        .find(|path| path.join(".git").exists())
        .unwrap_or(cwd)
}

fn relevant(definition: &Definition, ctx: &Context<'_>) -> bool {
    definition.provenance.agent.is_some()
        && !definition.provenance.provider_supplied()
        && (definition.provenance.project.is_none() || ctx.trusted)
}

fn rank(
    definition: &Definition,
    ctx: &Context<'_>,
) -> (bool, bool, bool, bool, std::cmp::Reverse<PathBuf>) {
    let origin = &definition.provenance;
    (
        origin.project.as_deref().is_some_and(|project| {
            Some(project) == ctx.project_root || project == checkout(ctx.cwd)
        }),
        // The caller's own checkout (its branch) beats the registered root's copy.
        // (Never another registered project's copy that merely lies inside this checkout.)
        origin.project.is_some()
            && origin.project.as_deref() == ctx.project_root
            && origin.file.starts_with(checkout(ctx.cwd)),
        origin.project.is_none(),
        origin.agent == Some(ctx.agent),
        std::cmp::Reverse(origin.file.clone()),
    )
}

fn suppress(off: &mut SwitchedOff, definition: &Definition) {
    let origin = &definition.provenance;
    match &definition.content {
        Content::Server(server) => {
            for alias in &origin.aliases {
                if !off.servers.contains(alias) {
                    off.servers.push(alias.clone());
                }
            }
            if let Some(plugin) = &origin.plugin {
                let identity = (plugin.clone(), server.name.clone());
                if !off.plugin_servers.contains(&identity) {
                    off.plugin_servers.push(identity);
                }
            }
        }
        Content::Skill { .. } => {
            let names = if origin.plugin.is_some() {
                &mut off.plugin_skills
            } else {
                &mut off.skill_names
            };
            if !names.contains(&origin.native) {
                names.push(origin.native.clone());
            }
            if !off.skill_files.contains(&origin.file) {
                off.skill_files.push(origin.file.clone());
            }
        }
    }
}

fn server(server: &brigadier_registry::Server, name: &str) -> McpServer {
    let expose = |values: &[(String, brigadier_registry::Secret)]| {
        values
            .iter()
            .map(|(name, value)| (name.clone(), value.expose().to_owned()))
            .collect()
    };
    let (transport, env) = match &server.transport {
        Transport::Stdio {
            command, args, env, ..
        } => (
            McpTransport::Stdio {
                command: PathBuf::from(command),
                args: args.clone(),
                cwd: server.transport.working_dir().map(Path::to_owned),
            },
            expose(env),
        ),
        Transport::Http { url, headers } => {
            (McpTransport::Http { url: url.clone() }, expose(headers))
        }
    };
    McpServer {
        gateway: None,
        name: name.to_owned(),
        transport,
        env,
        tool_timeout_secs: None,
        trusted: false,
        always_load: false,
        sources: Default::default(),
    }
}

/// Fills in what `agent` reads of `item` for itself. `Err`: why `item` can't reach it: a header
/// whose variable is unset, for any agent (never an empty credential or a literal `${VAR}`), or
/// a headers helper outside Claude. The message names variables and keys, never a value.
fn read_sources(
    item: &brigadier_registry::Server,
    server: &mut McpServer,
    agent: ProviderKind,
    vars: &BTreeMap<String, String>,
) -> Result<(), String> {
    let name = &server.name;
    let http = matches!(item.transport, Transport::Http { .. });
    let mut references = Vec::new();
    'references: for (key, parts) in &item.written.references {
        let mut own = Vec::with_capacity(parts.len());
        for part in parts {
            own.push(match part {
                WrittenPart::Text(text) => McpValuePart::Text(text.expose().to_owned()),
                WrittenPart::Var { name, .. }
                    if vars.get(name).is_some_and(|value| !value.is_empty()) =>
                {
                    McpValuePart::Var(name.clone())
                }
                WrittenPart::Var {
                    default: Some(default),
                    ..
                } => McpValuePart::Text(default.expose().to_owned()),
                WrittenPart::Var { name: var, .. } if http => {
                    return Err(format!(
                        "MCP server {name:?} was left out because the variable {var} it reads for \
                         {key} isn't set. Set it in your shell and restart Brigadier to use it."
                    ));
                }
                // An optional setting of a stdio server: `env` keeps its value as it is.
                WrittenPart::Var { .. } => continue 'references,
            });
        }
        // Only defaults in use: a plain value, which `env` already has.
        if own.iter().any(|part| matches!(part, McpValuePart::Var(_))) {
            references.push((key.clone(), own));
        }
    }
    let sources = McpSources {
        references,
        headers_helper: item.written.headers_helper.clone(),
    };
    if agent != ProviderKind::Claude && sources.headers_helper.is_some() {
        return Err(format!(
            "MCP server {name:?} was left out of this session because its headers come from a \
             headersHelper, which only Claude Code can run."
        ));
    }
    // Any other CLI keeps the launch's values in `env`.
    if matches!(agent, ProviderKind::Claude | ProviderKind::Codex) {
        server.sources = sources;
    }
    Ok(())
}

pub fn resolve(ctx: Context<'_>) -> Resolved {
    // The agent's own setup, as its adapter reads it; none for a CLI it doesn't read.
    let cli = brigadier_registry::adapter::adapter(ctx.agent);
    let cwd_root = checkout(ctx.cwd);
    let catalog = Catalog::read(ctx.account_home, ctx.trusted.then_some(cwd_root));
    let native_rows = Catalog::rows(std::slice::from_ref(&catalog));
    let native: Vec<(&Row, &Definition)> = native_rows
        .iter()
        .flat_map(|row| {
            row.definitions
                .iter()
                .map(move |definition| (row, definition))
        })
        .filter(|(_, definition)| {
            let origin = &definition.provenance;
            origin.agent == Some(ctx.agent)
                && origin.project.as_deref().is_none_or(|project| {
                    ctx.trusted && (project == cwd_root || Some(project) == ctx.project_root)
                })
        })
        .collect();
    // Installed but disabled plugins are definitions, not native availability. Codex also
    // identifies individual disabled skills by their effective file path.
    let available: Vec<_> = native
        .iter()
        .copied()
        .filter(|(_, definition)| {
            let Content::Skill { path, .. } = &definition.content else {
                return true;
            };
            catalog
                .skills
                .iter()
                .any(|skill| skill.entries.iter().any(|(_, entry)| entry == path))
                && !(cli.is_some_and(|cli| cli.disables_skills_in_config())
                    && catalog.codex_skill_config.iter().any(|entry| {
                        entry.get("enabled").and_then(serde_json::Value::as_bool) == Some(false)
                            && entry
                                .get("path")
                                .and_then(serde_json::Value::as_str)
                                .is_some_and(|disabled| {
                                    Path::new(disabled) == path
                                        || Path::new(disabled) == definition.provenance.file
                                })
                    }))
        })
        .collect();
    let mut resolved = Resolved::default();
    if let Role::Worker { grants } = ctx.role {
        resolved.servers = grants.to_vec();
        for (_, definition) in &native {
            let granted = match &definition.content {
                // Plugin natives have a separate namespace. Even when a grant has the same
                // plain name, suppress the plugin and inject only the granted definition.
                Content::Server(server) => {
                    server.plugin.is_none()
                        && grants
                            .iter()
                            .any(|grant| definition.provenance.aliases.contains(&grant.name))
                }
                Content::Skill { .. } => false,
            };
            if !granted {
                suppress(&mut resolved.off, definition);
            }
        }
        resolved.off.codex_skill_config = catalog.codex_skill_config;
        return resolved;
    }
    // Per-name precedence is applied after choices/scope. The active project beats user
    // definitions; a globalized foreign-project definition cannot displace either.
    let mut selected: BTreeMap<(Kind, &str), (&Row, &Definition)> = BTreeMap::new();
    for row in ctx.rows {
        let choice = ctx.choices.choice(row);
        if !choice.on || !choice.scope.contains(ctx.project_root) {
            continue;
        }
        // L12 applies to the effective account's own version too, even if that account
        // diverges from the catalog account. It can never fall back to an injected version.
        let candidates: Vec<_> = if row.different_per_agent {
            available
                .iter()
                .filter(|(own, _)| own.kind == Kind::Skill && own.name == row.name)
                .map(|(_, definition)| *definition)
                .collect()
        } else {
            row.definitions
                .iter()
                .filter(|definition| relevant(definition, &ctx))
                .collect()
        };
        for definition in candidates {
            let key = (row.kind, row.name.as_str());
            if selected
                .get(&key)
                .is_none_or(|(_, known)| rank(definition, &ctx) > rank(known, &ctx))
            {
                selected.insert(key, (row, definition));
            }
        }
    }
    let mut left_out = Vec::new();
    for ((kind, name), (row, definition)) in &selected {
        match &definition.content {
            Content::Server(item) => {
                let mut server = server(item, name);
                if let Err(message) =
                    read_sources(item, &mut server, ctx.agent, &ctx.account_home.vars)
                {
                    resolved.warnings.push(ResolveWarning {
                        server: name.to_string(),
                        message,
                    });
                    left_out.push((*kind, *name));
                    continue;
                }
                if item.signs_in
                    && let Transport::Http { url, .. } = &item.transport
                    && let Some(gateway) = ctx
                        .gateway
                        .and_then(|gateway| gateway.endpoint(&row.id, url))
                {
                    server.transport = McpTransport::Http {
                        url: gateway.url.clone(),
                    };
                    server.env = vec![gateway.secret_header.clone()];
                    server.sources = McpSources::default();
                    server.gateway = Some(gateway);
                }
                resolved.servers.push(server);
            }
            Content::Skill { .. } => {
                let own = available
                    .iter()
                    .filter(|(own, candidate)| {
                        own.kind == *kind
                            && own.name == *name
                            && candidate.provenance.fingerprint == definition.provenance.fingerprint
                    })
                    .max_by_key(|(_, candidate)| rank(candidate, &ctx));
                let (invocation, path, native) = if let Some((_, own)) = own {
                    (
                        own.provenance.native.clone(),
                        own.provenance.file.clone(),
                        true,
                    )
                } else {
                    let collision = native
                        .iter()
                        .any(|(own, _)| own.kind == Kind::Skill && own.name == *name);
                    let invocation = match cli.and_then(|cli| cli.overlay_namespace()) {
                        Some(namespace) if collision => format!("{namespace}:{name}"),
                        _ => name.to_string(),
                    };
                    (invocation, definition.provenance.file.clone(), false)
                };
                resolved.skills.push(ResolvedSkill {
                    row_id: row.id.clone(),
                    display: name.to_string(),
                    invocation,
                    path,
                    native,
                });
            }
        }
    }
    // Off in the CLI's own config too, where it would still load (Codex's).
    for key in left_out {
        selected.remove(&key);
    }
    for (row, definition) in native {
        let keep = selected
            .get(&(row.kind, row.name.as_str()))
            .is_some_and(|(_, chosen)| {
                chosen.provenance.fingerprint == definition.provenance.fingerprint
            });
        if !keep {
            suppress(&mut resolved.off, definition);
        }
    }
    if cli.is_some_and(|cli| cli.renames_colliding_servers()) {
        let mut names: std::collections::BTreeSet<_> =
            resolved.servers.iter().map(|s| s.name.clone()).collect();
        for selected in &mut resolved.servers {
            let collision = native_rows
                .iter()
                .flat_map(|row| &row.definitions)
                .any(|definition| {
                    matches!(&definition.content, Content::Server(own)
                    if definition.provenance.agent == Some(ctx.agent)
                        && own.plugin.is_none() && own.name == selected.name
                        && matches!((&own.transport, &selected.transport),
                            (Transport::Http { .. }, McpTransport::Stdio { .. }) |
                            (Transport::Stdio { .. }, McpTransport::Http { .. })))
                });
            if collision {
                let original = selected.name.clone();
                let mut alias = format!("{original}-brigadier");
                let mut suffix = 2;
                while names.contains(&alias) || native_rows.iter().any(|row| row.name == alias) {
                    alias = format!("{original}-brigadier-{suffix}");
                    suffix += 1;
                }
                names.insert(alias.clone());
                selected.name = alias;
                if !resolved.off.servers.contains(&original) {
                    resolved.off.servers.push(original);
                }
            }
        }
    }
    resolved
        .off
        .servers
        .retain(|alias| !resolved.servers.iter().any(|server| server.name == *alias));
    resolved.off.codex_skill_config = catalog.codex_skill_config;
    resolved
}

#[cfg(test)]
mod tests {
    use super::super::choices::{Choice, Global, Scope, import_sharing, import_sharing_file};
    use super::*;

    struct Fixture {
        dir: PathBuf,
        roots: Roots,
        project: PathBuf,
        worktree: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let dir =
                std::env::temp_dir().join(format!("plugins-resolve-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir_all(&dir).unwrap();
            let dir = std::fs::canonicalize(dir).unwrap();
            let home = dir.join("home");
            let roots = Roots::from_vars([
                ("HOME".to_owned(), home.to_string_lossy().into_owned()),
                (
                    "USERPROFILE".to_owned(),
                    home.to_string_lossy().into_owned(),
                ),
            ])
            .unwrap();
            let project = dir.join("project");
            let worktree = dir.join("worktree");
            write(&project.join(".git/HEAD"), "ref: refs/heads/main");
            write(&worktree.join(".git"), "gitdir: fixture");
            Self {
                dir,
                roots,
                project,
                worktree,
            }
        }
        fn rows(&self) -> Vec<Row> {
            Catalog::rows(&[Catalog::read(&self.roots, Some(&self.project))])
        }
        fn resolve(&self, agent: ProviderKind, rows: &[Row], choices: &Choices) -> Resolved {
            resolve(Context {
                agent,
                account_home: &self.roots,
                project_root: Some(&self.project),
                cwd: &self.worktree,
                trusted: true,
                role: Role::Thread,
                rows,
                choices,
                gateway: None,
            })
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).unwrap();
        }
    }
    fn write(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, contents).unwrap();
    }
    fn skill(path: &Path, body: &str) {
        write(
            &path.join("SKILL.md"),
            &format!("---\nname: example\n---\n{body}"),
        );
    }

    #[test]
    fn worktree_matches_registered_scope_but_native_presence_uses_checkout() {
        let fixture = Fixture::new();
        skill(&fixture.project.join(".claude/skills/example"), "project");
        let rows = fixture.rows();
        let resolved = fixture.resolve(ProviderKind::Claude, &rows, &Choices::default());
        assert_eq!(resolved.skills.len(), 1);
        assert!(!resolved.skills[0].native);
        assert_eq!(
            resolved.skills[0].path,
            fixture.project.join(".claude/skills/example/SKILL.md")
        );
        skill(&fixture.worktree.join(".claude/skills/example"), "project");
        let resolved = fixture.resolve(ProviderKind::Claude, &rows, &Choices::default());
        assert!(resolved.skills[0].native);
        assert_eq!(
            resolved.skills[0].path,
            fixture.worktree.join(".claude/skills/example/SKILL.md")
        );
        let choices = Choices(BTreeMap::from([(
            rows[0].id.clone(),
            Choice {
                removed: None,
                on: true,
                scope: Scope::Projects(vec![fixture.worktree.clone()]),
            },
        )]));
        assert!(
            fixture
                .resolve(ProviderKind::Claude, &rows, &choices)
                .skills
                .is_empty()
        );
    }

    #[test]
    fn project_beats_user_for_same_server_name() {
        let fixture = Fixture::new();
        write(
            &fixture.roots.claude_json,
            r#"{"mcpServers":{"db":{"url":"https://user.test/mcp"}}}"#,
        );
        write(
            &fixture.project.join(".mcp.json"),
            r#"{"mcpServers":{"db":{"url":"https://project.test/mcp"}}}"#,
        );
        let rows = fixture.rows();
        assert_eq!(rows.len(), 2);
        let resolved = fixture.resolve(ProviderKind::Codex, &rows, &Choices::default());
        assert_eq!(resolved.servers.len(), 1);
        assert_eq!(
            resolved.servers[0].transport,
            McpTransport::Http {
                url: "https://project.test/mcp".into()
            }
        );
        let resolved = fixture.resolve(ProviderKind::Claude, &rows, &Choices::default());
        assert!(!resolved.off.servers.contains(&"db".to_owned()));
    }

    #[test]
    fn off_plugin_keeps_qualified_suppression_and_tool_aliases() {
        let fixture = Fixture::new();
        let plugin = fixture.dir.join("plugin");
        write(
            &plugin.join(".claude-plugin/plugin.json"),
            r#"{"name":"tools"}"#,
        );
        write(
            &plugin.join(".mcp.json"),
            r#"{"mcpServers":{"db":{"url":"https://plugin.test/mcp"}}}"#,
        );
        skill(&plugin.join("skills/example"), "plugin");
        write(
            &fixture
                .roots
                .claude_dir
                .join("plugins/installed_plugins.json"),
            &serde_json::json!({"plugins":{"tools@test":[{"scope":"user", "installPath":plugin}]}})
                .to_string(),
        );
        let rows = fixture.rows();
        let choices = Choices(
            rows.iter()
                .map(|row| {
                    (
                        row.id.clone(),
                        Choice {
                            removed: None,
                            on: false,
                            scope: Scope::Global(Global::Global),
                        },
                    )
                })
                .collect(),
        );
        let resolved = fixture.resolve(ProviderKind::Claude, &rows, &choices);
        assert!(resolved.off.servers.contains(&"plugin:tools:db".to_owned()));
        assert!(resolved.off.servers.contains(&"plugin_tools_db".to_owned()));
        assert!(
            resolved
                .off
                .plugin_skills
                .contains(&"tools:example".to_owned())
        );
    }

    #[test]
    fn different_per_agent_skills_are_never_injected() {
        let fixture = Fixture::new();
        skill(&fixture.roots.claude_dir.join("skills/example"), "claude");
        skill(&fixture.roots.codex_home.join("skills/example"), "codex");
        let rows = fixture.rows();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].different_per_agent);
        for agent in [ProviderKind::Claude, ProviderKind::Codex] {
            let result = fixture.resolve(agent, &rows, &Choices::default());
            assert_eq!(result.skills.len(), 1);
            assert!(result.skills[0].native);
        }
        let other = Fixture::new();
        assert!(
            other
                .resolve(ProviderKind::Codex, &rows, &Choices::default())
                .skills
                .is_empty()
        );
        skill(
            &other.roots.codex_home.join("skills/example"),
            "alternate account version",
        );
        let resolved = other.resolve(ProviderKind::Codex, &rows, &Choices::default());
        assert_eq!(resolved.skills.len(), 1);
        assert!(resolved.skills[0].native);
        assert_eq!(
            resolved.skills[0].path,
            other.roots.codex_home.join("skills/example/SKILL.md")
        );
    }

    #[test]
    fn worker_gets_only_explicit_grants_and_no_skills() {
        let fixture = Fixture::new();
        skill(&fixture.roots.claude_dir.join("skills/example"), "user");
        write(
            &fixture.roots.claude_json,
            r#"{"mcpServers":{"db":{"url":"https://user.test/mcp"}}}"#,
        );
        let rows = fixture.rows();
        let grants = vec![server(
            match &rows
                .iter()
                .find(|row| row.kind == Kind::Server)
                .unwrap()
                .definitions[0]
                .content
            {
                Content::Server(server) => server,
                _ => unreachable!(),
            },
            "explicit",
        )];
        let resolved = resolve(Context {
            agent: ProviderKind::Claude,
            account_home: &fixture.roots,
            project_root: Some(&fixture.project),
            cwd: &fixture.worktree,
            trusted: true,
            role: Role::Worker { grants: &grants },
            rows: &rows,
            choices: &Choices::default(),
            gateway: None,
        });
        assert_eq!(resolved.servers, grants);
        assert!(resolved.skills.is_empty());
        assert!(resolved.off.servers.contains(&"db".into()));
        assert!(resolved.off.skill_names.contains(&"example".into()));
    }

    #[test]
    fn divergent_account_home_and_collision_use_effective_files_and_invocation() {
        let fixture = Fixture::new();
        skill(&fixture.project.join(".agents/skills/example"), "selected");
        skill(
            &fixture.roots.claude_dir.join("skills/example"),
            "collision",
        );
        let mut rows = fixture.rows();
        // Same-agent collision case is independent of L12's cross-agent rule.
        rows.retain(|row| {
            row.definitions
                .iter()
                .any(|d| d.provenance.project.is_some())
        });
        rows[0].different_per_agent = false;
        let resolved = fixture.resolve(ProviderKind::Claude, &rows, &Choices::default());
        assert_eq!(resolved.skills[0].invocation, "overlay:example");
        assert!(!resolved.skills[0].native);
        let alternate = Fixture::new();
        let resolved = resolve(Context {
            agent: ProviderKind::Claude,
            account_home: &alternate.roots,
            project_root: Some(&fixture.project),
            cwd: &fixture.worktree,
            trusted: true,
            role: Role::Thread,
            rows: &rows,
            choices: &Choices::default(),
            gateway: None,
        });
        assert_eq!(resolved.skills[0].invocation, "example");
        assert!(!resolved.skills[0].native);
    }

    #[test]
    fn defaults_and_sharing_import_preserve_off_scope_and_declined() {
        let fixture = Fixture::new();
        skill(&fixture.project.join(".claude/skills/example"), "project");
        write(
            &fixture.roots.claude_json,
            r#"{"mcpServers":{"db":{"url":"https://user.test/mcp"},"removed":{"url":"https://removed.test/mcp"}}}"#,
        );
        let rows = fixture.rows();
        let skill = rows.iter().find(|row| row.kind == Kind::Skill).unwrap();
        assert_eq!(
            Choice::default_for(skill),
            Choice {
                removed: None,
                on: true,
                scope: Scope::Projects(vec![fixture.project.clone()])
            }
        );
        let db = rows.iter().find(|row| row.name == "db").unwrap();
        assert_eq!(Choice::default_for(db).scope, Scope::Global(Global::Global));
        let legacy = serde_json::json!({"chosen":{"skill:example":{"on":true,"scope":[fixture.project]}}, "offScopes":{"server:db":[fixture.project]}, "declined":[{"kind":"server","name":"removed"}]});
        let choices = import_sharing(&legacy, &rows).unwrap();
        assert!(!choices.choice(db).on);
        assert_eq!(
            choices.choice(db).scope,
            Scope::Projects(vec![fixture.project.clone()])
        );
        assert!(choices.choice(skill).on);
        assert!(
            !choices
                .choice(rows.iter().find(|row| row.name == "removed").unwrap())
                .on
        );
        let data = fixture.dir.join("data");
        let old = fixture.dir.join(".sharing.json");
        write(&old, &legacy.to_string());
        assert_eq!(import_sharing_file(&data, &old, &rows).unwrap(), choices);
        assert_eq!(Choices::load(&data).unwrap(), choices);
        let mut changed = choices;
        changed.0.get_mut(&db.id).unwrap().on = true;
        changed.save(&data).unwrap();
        assert_eq!(import_sharing_file(&data, &old, &rows).unwrap(), changed);
        write(&data.join("plugins.json"), "broken");
        assert!(import_sharing_file(&data, &old, &rows).is_err());
        assert_eq!(std::fs::read_to_string(&old).unwrap(), legacy.to_string());
    }
    #[test]
    fn oauth_servers_use_gateway_resource_and_local_secret_header() {
        struct Local;
        impl Gateway for Local {
            fn endpoint(&self, row_id: &str, resource: &str) -> Option<McpGateway> {
                assert!(row_id.starts_with("server:db:"));
                assert_eq!(resource, "https://user.test/mcp");
                Some(McpGateway {
                    resource: resource.into(),
                    row_id: row_id.into(),
                    url: "http://127.0.0.1:1234/mcp/db".into(),
                    secret_header: ("X-Brigadier-Session".into(), "local-secret".into()),
                })
            }
        }
        let fixture = Fixture::new();
        write(
            &fixture.roots.claude_json,
            r#"{"mcpServers":{"db":{"url":"https://user.test/mcp"}}}"#,
        );
        let rows = fixture.rows();
        let resolved = resolve(Context {
            agent: ProviderKind::Claude,
            account_home: &fixture.roots,
            project_root: Some(&fixture.project),
            cwd: &fixture.worktree,
            trusted: true,
            role: Role::Thread,
            rows: &rows,
            choices: &Choices::default(),
            gateway: Some(&Local),
        });
        assert_eq!(
            resolved.servers[0].transport,
            McpTransport::Http {
                url: "http://127.0.0.1:1234/mcp/db".into()
            }
        );
        assert_eq!(
            resolved.servers[0].env,
            [("X-Brigadier-Session".into(), "local-secret".into())]
        );
        assert!(!format!("{:?}", resolved.servers[0].gateway).contains("local-secret"));
    }

    /// #809: each CLI reads a server's header source itself, or goes without it and says why.
    #[test]
    fn header_sources_reach_each_cli_and_a_source_it_cant_read_is_left_out() {
        let mut fixture = Fixture::new();
        fixture
            .roots
            .vars
            .insert("FIXTURE_TOKEN".into(), "frozen-value".into());
        write(
            &fixture.roots.claude_json,
            r#"{"mcpServers":{
                "helper":{"url":"https://helper.test/mcp","headersHelper":"print-headers"},
                "bearer":{"url":"https://bearer.test/mcp","headers":{"Authorization":"Bearer ${FIXTURE_TOKEN}"}},
                "mixed":{"url":"https://mixed.test/mcp","headers":{"X-Key":"a-${FIXTURE_TOKEN}"}},
                "fallback":{"url":"https://fallback.test/mcp","headers":{"Authorization":"Bearer x","X-Key":"${FIXTURE_MISSING:-used}"}},
                "unset":{"url":"https://unset.test/mcp","headers":{"Authorization":"Bearer ${FIXTURE_MISSING}"}},
                "optional":{"command":"optional-server","env":{"OPTIONAL":"${FIXTURE_MISSING}"}},
                "renamed":{"command":"renamed-server","env":{"API_KEY":"${FIXTURE_TOKEN}"}}
            }}"#,
        );
        write(
            &fixture.roots.codex_home.join("config.toml"),
            "[mcp_servers.own]\nurl='https://own.test/mcp'\nbearer_token_env_var='FIXTURE_MISSING'\n",
        );
        let rows = fixture.rows();
        let helper = rows.iter().find(|row| row.name == "helper").unwrap();
        let Content::Server(helper) = &helper.definitions[0].content else {
            unreachable!()
        };
        assert!(!helper.signs_in, "a headers helper is not an OAuth sign-in");
        let names = |resolved: &Resolved| -> Vec<String> {
            resolved.servers.iter().map(|s| s.name.clone()).collect()
        };
        let left_out = |resolved: &Resolved| -> Vec<String> {
            resolved.warnings.iter().map(|w| w.server.clone()).collect()
        };
        let by_name = |resolved: &'_ Resolved, name: &str| {
            resolved
                .servers
                .iter()
                .find(|server| server.name == name)
                .unwrap()
                .clone()
        };

        let claude = fixture.resolve(ProviderKind::Claude, &rows, &Choices::default());
        assert_eq!(
            names(&claude),
            [
                "bearer", "fallback", "helper", "mixed", "optional", "renamed"
            ]
        );
        assert_eq!(left_out(&claude), ["own", "unset"]);
        assert_eq!(
            by_name(&claude, "helper").sources.headers_helper.as_deref(),
            Some("print-headers")
        );
        assert_eq!(
            by_name(&claude, "bearer").sources.references,
            [(
                "Authorization".to_owned(),
                vec![
                    McpValuePart::Text("Bearer ".into()),
                    McpValuePart::Var("FIXTURE_TOKEN".into())
                ]
            )]
        );
        // The default is what's used, so it is a plain value, not a reference.
        assert!(by_name(&claude, "fallback").sources.references.is_empty());
        // An unset variable in a stdio server's env keeps today's value: only a header skips.
        let optional = by_name(&claude, "optional");
        assert_eq!(optional.sources, Default::default());
        assert_eq!(optional.env, [("OPTIONAL".to_owned(), String::new())]);

        let codex = fixture.resolve(ProviderKind::Codex, &rows, &Choices::default());
        // A set variable Codex can't name keeps its value (`env`); the provider keeps it off
        // the command line.
        assert_eq!(
            names(&codex),
            ["bearer", "fallback", "mixed", "optional", "renamed"]
        );
        assert_eq!(left_out(&codex), ["helper", "own", "unset"]);
        assert_eq!(
            by_name(&codex, "renamed").env,
            [("API_KEY".to_owned(), "frozen-value".to_owned())]
        );
        // Its own server goes too, not only Brigadier's copy of it.
        assert!(codex.off.servers.contains(&"own".to_owned()));

        // OpenCode keeps the launch's value (only a set variable), and no reference.
        let opencode = fixture.resolve(ProviderKind::OpenCode, &rows, &Choices::default());
        assert_eq!(
            names(&opencode),
            ["bearer", "fallback", "mixed", "optional", "renamed"]
        );
        let bearer = by_name(&opencode, "bearer");
        assert_eq!(bearer.sources, Default::default());
        assert_eq!(
            bearer.env,
            [("Authorization".to_owned(), "Bearer frozen-value".to_owned())]
        );

        for warning in claude.warnings.iter().chain(&codex.warnings) {
            assert!(!warning.message.contains("frozen-value"), "{warning:?}");
        }
        let unset = claude
            .warnings
            .iter()
            .find(|w| w.server == "unset")
            .unwrap();
        assert!(unset.message.contains("FIXTURE_MISSING"), "{unset:?}");
    }

    #[test]
    fn untrusted_context_excludes_project_definitions_even_when_globalized() {
        let fixture = Fixture::new();
        skill(&fixture.project.join(".claude/skills/example"), "project");
        let rows = fixture.rows();
        let choices = Choices(
            rows.iter()
                .map(|row| {
                    (
                        row.id.clone(),
                        Choice {
                            removed: None,
                            on: true,
                            scope: Scope::Global(Global::Global),
                        },
                    )
                })
                .collect(),
        );
        let resolved = resolve(Context {
            agent: ProviderKind::Claude,
            account_home: &fixture.roots,
            project_root: Some(&fixture.project),
            cwd: &fixture.worktree,
            trusted: false,
            role: Role::Thread,
            rows: &rows,
            choices: &choices,
            gateway: None,
        });
        assert!(resolved.skills.is_empty());
    }

    #[test]
    fn import_covers_global_off_removed_and_renamed_endpoint_records() {
        let fixture = Fixture::new();
        write(
            &fixture.roots.claude_json,
            r#"{"mcpServers":{"renamed":{"url":"https://old.test/mcp"},"removed":{"url":"https://removed.test/mcp"},"off":{"url":"https://off.test/mcp"}}}"#,
        );
        let rows = fixture.rows();
        let renamed = rows.iter().find(|row| row.name == "renamed").unwrap();
        let Content::Server(server) = &renamed.definitions[0].content else {
            unreachable!()
        };
        let old_endpoint = super::super::model::endpoint(server);
        let choices = import_sharing(&serde_json::json!({"offScopes":{"server:off":null}, "removed":["server:removed"], "declined":[{"kind":"server","name":"old-name","endpoint":old_endpoint}]}), &rows).unwrap();
        for row in &rows {
            assert!(!choices.choice(row).on);
            assert_eq!(choices.choice(row).scope, Scope::Global(Global::Global));
        }
        assert!(import_sharing(&serde_json::json!({"chosen":"invalid"}), &rows).is_err());
    }
    #[test]
    fn codex_transport_collisions_use_an_alias_and_disable_the_native_entry() {
        for native_http in [false, true] {
            let fixture = Fixture::new();
            write(
                &fixture.roots.codex_home.join("config.toml"),
                if native_http {
                    "[mcp_servers.db]\nurl='https://native.invalid/mcp'\n"
                } else {
                    "[mcp_servers.db]\ncommand='native-server'\n"
                },
            );
            write(
                &fixture.roots.claude_json,
                if native_http {
                    r#"{"mcpServers":{"db":{"command":"selected-server"}}}"#
                } else {
                    r#"{"mcpServers":{"db":{"url":"https://selected.invalid/mcp"}}}"#
                },
            );
            let rows = fixture.rows();
            let mut choices = Choices::default();
            for row in &rows {
                if row
                    .definitions
                    .iter()
                    .any(|d| d.provenance.agent == Some(ProviderKind::Codex))
                {
                    let mut choice = choices.choice(row);
                    choice.on = false;
                    choices.0.insert(row.id.clone(), choice);
                }
            }
            let resolved = fixture.resolve(ProviderKind::Codex, &rows, &choices);
            assert_eq!(resolved.servers.len(), 1);
            assert_eq!(resolved.servers[0].name, "db-brigadier");
            assert!(resolved.off.servers.iter().any(|name| name == "db"));
            assert_eq!(
                matches!(resolved.servers[0].transport, McpTransport::Stdio { .. }),
                native_http
            );
        }
    }
}
