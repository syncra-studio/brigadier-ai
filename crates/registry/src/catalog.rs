//! Provenance-preserving rows shared by the page and launch resolver. Reading never writes.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::{Agent, Catalog, Server, SkillRoot, Source, Transport};

/// Client labels used by Pencil's CLI integrations do not change the server process.
/// Keep every other argument, including unrecognized --agent values, in its identity.
fn server_args(args: &[String]) -> Vec<&str> {
    let client = |value: &str| matches!(value, "claudeCodeCLI" | "codexCLI");
    let mut normalized = Vec::new();
    let mut args = args.iter().peekable();
    while let Some(arg) = args.next() {
        if arg == "--agent" && args.peek().is_some_and(|value| client(value)) {
            args.next();
        } else if !arg.strip_prefix("--agent=").is_some_and(client) {
            normalized.push(arg.as_str());
        }
    }
    normalized
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Server,
    Skill,
}

/// A native identity is kept separately from its plain display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// None for import offers from Desktop and Cursor.
    pub agent: Option<Agent>,
    pub file: PathBuf,
    pub plugin: Option<String>,
    pub project: Option<PathBuf>,
    pub native: String,
    /// Includes tool aliases as well as the CLI's suppression identity.
    pub aliases: Vec<String>,
    pub fingerprint: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Server(Box<Server>),
    Skill { path: PathBuf, description: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Definition {
    pub provenance: Provenance,
    pub content: Content,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub kind: Kind,
    pub name: String,
    /// A remote resource URL, or a fingerprint of a process and its configuration.
    pub endpoint: Option<String>,
    pub definitions: Vec<Definition>,
    /// L12: keep each agent's native version; never inject this name across agents.
    pub different_per_agent: bool,
}

pub fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Hash file names and bytes, including supporting files and hidden files. Unreadable or
/// cyclic trees get no content identity, so they cannot accidentally merge with another tree.
pub fn skill_fingerprint(path: &Path) -> Option<String> {
    fn visit(path: &Path, hasher: &mut Sha256, ancestors: &mut Vec<PathBuf>) -> Option<()> {
        let canonical = std::fs::canonicalize(path).ok()?;
        if ancestors.contains(&canonical) {
            return None;
        }
        if path.is_dir() {
            ancestors.push(canonical);
            let mut entries = std::fs::read_dir(path)
                .ok()?
                .collect::<Result<Vec<_>, _>>()
                .ok()?;
            entries.sort_by_key(|entry| entry.file_name());
            hasher.update((entries.len() as u64).to_le_bytes());
            for entry in entries {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                hasher.update((name.len() as u64).to_le_bytes());
                hasher.update(name.as_bytes());
                hasher.update([u8::from(entry.path().is_dir())]);
                visit(&entry.path(), hasher, ancestors)?;
            }
            ancestors.pop();
        } else {
            let bytes = std::fs::read(path).ok()?;
            hasher.update((bytes.len() as u64).to_le_bytes());
            hasher.update(bytes);
        }
        Some(())
    }
    let mut hasher = Sha256::new();
    visit(path, &mut hasher, &mut Vec::new())?;
    Some(
        hasher
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect(),
    )
}

impl Server {
    pub fn provenance(&self) -> Provenance {
        let agent = match self.source {
            Source::ClaudeUser
            | Source::ClaudeLocal
            | Source::ClaudeProject
            | Source::ClaudePlugin => Some(Agent::Claude),
            Source::CodexUser | Source::CodexProject | Source::CodexPlugin => Some(Agent::Codex),
            _ => None,
        };
        let native = match &self.plugin {
            Some(plugin) if plugin.agent == Agent::Claude => {
                format!("plugin:{}:{}", plugin.name, self.name)
            }
            _ => self.name.clone(),
        };
        let mut aliases = vec![native.clone(), self.native_name()];
        aliases.sort();
        aliases.dedup();
        // Canonical maps make content identity independent of header/env ordering. Secret
        // values enter only this hash and never the provenance or serialized page model.
        let transport = match &self.transport {
            Transport::Stdio {
                command, args, env, ..
            } => {
                let env: BTreeMap<_, _> = env
                    .iter()
                    .map(|(key, value)| (key, value.expose()))
                    .collect();
                // Old Brigadier sharing packages inherited the cache directory as a
                // base, even though their original had no cwd. That packaging detail
                // must not make the mirror a different server from its original.
                let mirror = self
                    .plugin
                    .as_ref()
                    .is_some_and(|plugin| plugin.id.ends_with("@brigadier"));
                let cwd = if mirror && self.written.cwd.is_none() {
                    None
                } else {
                    self.transport.working_dir()
                };
                serde_json::json!({"kind":"stdio", "command":command, "args":server_args(args), "cwd":cwd, "env":env})
            }
            Transport::Http { url, headers } => {
                let headers: BTreeMap<_, _> = headers
                    .iter()
                    .map(|(key, value)| (key, value.expose()))
                    .collect();
                serde_json::json!({"kind":"http", "url":url, "headers":headers})
            }
        };
        let identity = serde_json::json!({"transport":transport,
            "clientId":self.oauth.as_ref().and_then(|oauth| oauth.client_id.as_ref()),
            "clientSecret":self.oauth.as_ref().and_then(|oauth| oauth.client_secret.as_ref()).map(|secret| secret.expose()),
        });
        Provenance {
            agent,
            file: self.file.clone(),
            plugin: self.plugin.as_ref().map(|plugin| plugin.id.clone()),
            project: self.project.clone().or_else(|| {
                self.plugin
                    .as_ref()
                    .and_then(|plugin| plugin.project.clone())
            }),
            native,
            aliases,
            fingerprint: fingerprint(identity.to_string().as_bytes()),
        }
    }
}

impl Catalog {
    /// Complete page catalog, including disabled project-only plugin definitions. Unlike
    /// `Catalog::read`, presence here does not imply native availability in a session.
    pub fn read_rows(roots: &crate::Roots, projects: &[PathBuf]) -> Vec<Row> {
        let data_dir = roots.vars.get("BRIGADIER_DATA_DIR").map(PathBuf::from);
        Self::read_row_sources(roots, projects, data_dir.as_deref())
    }

    pub fn read_rows_with_library(
        roots: &crate::Roots,
        projects: &[PathBuf],
        data_dir: &Path,
    ) -> Vec<Row> {
        Self::read_row_sources(roots, projects, Some(data_dir))
    }

    fn read_row_sources(
        roots: &crate::Roots,
        projects: &[PathBuf],
        data_dir: Option<&Path>,
    ) -> Vec<Row> {
        let mut catalogs = Vec::new();
        for project in std::iter::once(None).chain(projects.iter().map(|path| Some(path.as_path())))
        {
            let mut catalog = Self::read(roots, project);
            let mut plugins = crate::plugins::claude_definitions(roots, project);
            if let Some(project) = project {
                let layer = crate::servers::read_toml(
                    &project.join(".codex/config.toml"),
                    Source::CodexProject,
                    Some(project),
                    &mut Vec::new(),
                );
                plugins.extend(
                    crate::plugins::codex_switched(roots, layer.as_ref(), Some(project))
                        .into_iter()
                        .map(|(plugin, _)| plugin),
                );
            }
            for plugin in plugins {
                if catalog.plugins.contains(&plugin) || catalog.off.plugins.contains(&plugin) {
                    continue;
                }
                crate::skills::add_plugin(&mut catalog.off.skills, &plugin);
                catalog.off.servers.extend(crate::servers::plugin(
                    roots,
                    &plugin,
                    None,
                    &mut catalog.problems,
                ));
                catalog.off.plugins.push(plugin);
            }
            catalogs.push(catalog);
        }
        if let Some(data_dir) = data_dir {
            catalogs.push(library(data_dir));
        }
        Self::rows(&catalogs)
    }

    /// Combine user and registered-project catalogs without losing definitions or aliases.
    /// Desktop/Cursor rows remain import offers (`agent == None`).
    pub fn rows(catalogs: &[Self]) -> Vec<Row> {
        let mut grouped: BTreeMap<(Kind, String, String), Row> = BTreeMap::new();
        let mut add = |kind: Kind, name: &str, endpoint: Option<String>, definition: Definition| {
            let key = (
                kind,
                name.to_owned(),
                definition.provenance.fingerprint.clone(),
            );
            let row = grouped.entry(key.clone()).or_insert_with(|| Row {
                id: format!(
                    "{}:{}:{}",
                    if kind == Kind::Server {
                        "server"
                    } else {
                        "skill"
                    },
                    name,
                    fingerprint(key.2.as_bytes())
                ),
                kind,
                name: name.to_owned(),
                endpoint,
                definitions: Vec::new(),
                different_per_agent: false,
            });
            if !row.definitions.contains(&definition) {
                row.definitions.push(definition);
            }
        };
        for catalog in catalogs {
            for server in catalog.servers.iter().chain(&catalog.off.servers) {
                let provenance = server.provenance();
                let endpoint = Some(match &server.transport {
                    Transport::Http { url, .. } => url.clone(),
                    _ => provenance.fingerprint.clone(),
                });
                add(
                    Kind::Server,
                    &server.name,
                    endpoint,
                    Definition {
                        provenance,
                        content: Content::Server(Box::new(server.clone())),
                    },
                );
            }
            for skill in catalog.skills.iter().chain(&catalog.off.skills) {
                for (root, path) in &skill.entries {
                    let (agent, project, plugin, native) = match root {
                        SkillRoot::ClaudeUser => (Agent::Claude, None, None, skill.name.clone()),
                        SkillRoot::ClaudeSynced(_) => (
                            Agent::Claude,
                            None,
                            None,
                            format!("synced-skill:{}", skill.name),
                        ),
                        SkillRoot::AgentsUser | SkillRoot::CodexUser => {
                            (Agent::Codex, None, None, skill.name.clone())
                        }
                        SkillRoot::ClaudeProject(project) => (
                            Agent::Claude,
                            Some(project.clone()),
                            None,
                            skill.name.clone(),
                        ),
                        SkillRoot::AgentsProject(project) => (
                            Agent::Codex,
                            Some(project.clone()),
                            None,
                            skill.name.clone(),
                        ),
                        SkillRoot::Plugin(plugin) => (
                            plugin.agent,
                            plugin.project.clone(),
                            Some(plugin.id.clone()),
                            plugin.skill_name(&skill.name),
                        ),
                    };
                    let file = path.join("SKILL.md");
                    let fingerprint = skill_fingerprint(path)
                        .unwrap_or_else(|| fingerprint(file.to_string_lossy().as_bytes()));
                    let provenance = Provenance {
                        agent: Some(agent),
                        file,
                        plugin,
                        project,
                        aliases: vec![native.clone()],
                        native,
                        fingerprint,
                    };
                    add(
                        Kind::Skill,
                        &skill.name,
                        None,
                        Definition {
                            provenance,
                            content: Content::Skill {
                                path: path.clone(),
                                description: skill.description.clone(),
                            },
                        },
                    );
                }
            }
        }
        let mut rows: Vec<Row> = grouped.into_values().collect();
        let conflicts: Vec<String> = rows
            .iter()
            .filter(|row| row.kind == Kind::Skill)
            .filter(|row| {
                rows.iter().any(|other| {
                    other.kind == Kind::Skill
                        && other.name == row.name
                        && other.id != row.id
                        && row.definitions.iter().any(|a| {
                            other
                                .definitions
                                .iter()
                                .any(|b| a.provenance.agent != b.provenance.agent)
                        })
                })
            })
            .map(|row| row.name.clone())
            .collect();
        for name in conflicts {
            let matches: Vec<_> = rows
                .iter()
                .filter(|row| row.kind == Kind::Skill && row.name == name)
                .cloned()
                .collect();
            if matches.len() < 2 {
                continue;
            }
            rows.retain(|row| row.kind != Kind::Skill || row.name != name);
            rows.push(Row {
                id: format!("skill:{name}:different-per-agent"),
                kind: Kind::Skill,
                name,
                endpoint: None,
                definitions: matches
                    .into_iter()
                    .flat_map(|row| row.definitions)
                    .collect(),
                different_per_agent: true,
            });
        }
        rows.sort_by(|a, b| a.id.cmp(&b.id));
        rows
    }
}

/// Brigadier's own plain folders and server entries. These are launch sources, never
/// native CLI plugins. A synthetic Claude origin makes them resolver candidates for either
/// agent, while native availability is still read exclusively from each CLI's actual roots.
pub fn library(data_dir: &Path) -> Catalog {
    let root = data_dir.join("library");
    let mut catalog = Catalog::default();
    if let Ok(entries) = std::fs::read_dir(root.join("skills")) {
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name.starts_with('.') || !path.join("SKILL.md").is_file() {
                continue;
            }
            let name = name.to_owned();
            catalog.skills.push(crate::Skill {
                id: format!("library:skill:{name}"),
                name,
                description: String::new(),
                dir: path.clone(),
                roots: vec![SkillRoot::ClaudeUser],
                entries: vec![(SkillRoot::ClaudeUser, path)],
            });
        }
    }
    if let Ok(entries) = std::fs::read_dir(root.join("servers")) {
        for entry in entries.flatten() {
            let file = entry.path();
            if file.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&file) else {
                continue;
            };
            let Ok(items) = crate::add_parse::parse(&text, None, data_dir) else {
                continue;
            };
            for item in items {
                let crate::item::Item::Server(spec) = item else {
                    continue;
                };
                let (transport, written, signs_in) = match spec.transport {
                    crate::item::TransportSpec::Stdio {
                        command,
                        args,
                        env,
                        cwd,
                    } => (
                        Transport::Stdio {
                            command: command.clone(),
                            args: args.clone(),
                            env,
                            cwd: cwd.clone(),
                            base: None,
                        },
                        crate::AsWritten {
                            command: Some(command),
                            args,
                            cwd: cwd.map(|path| path.to_string_lossy().into_owned()),
                            ..Default::default()
                        },
                        false,
                    ),
                    crate::item::TransportSpec::Http { url, headers } => {
                        let signs_in = headers.is_empty();
                        (
                            Transport::Http {
                                url: url.clone(),
                                headers,
                            },
                            crate::AsWritten {
                                url: Some(url),
                                ..Default::default()
                            },
                            signs_in,
                        )
                    }
                };
                catalog.servers.push(Server {
                    id: format!("library:server:{}", spec.name),
                    name: spec.name,
                    source: Source::ClaudeUser,
                    project: None,
                    transport,
                    signs_in,
                    plugin: None,
                    file: file.clone(),
                    oauth: spec.oauth.map(|oauth| crate::OAuthClient {
                        client_id: Some(oauth.client_id),
                        client_secret: None,
                    }),
                    disabled: false,
                    written,
                    templated: false,
                    overrides_user: false,
                });
            }
        }
    }
    // Cleanup preserves old library-only packages before removing CLI installs. Read
    // their content as library sources too; never register or install their marketplace.
    let saved = crate::library::Library::at(root.clone(), PathBuf::from("brigadier"));
    let roots =
        crate::Roots::from_vars([("HOME".to_owned(), data_dir.to_string_lossy().into_owned())]);
    if let Some(roots) = roots {
        for entry in saved.entries() {
            if !entry.dir.is_dir() {
                continue;
            }
            let plugin = crate::Plugin {
                agent: Agent::Claude,
                id: entry.id(),
                name: entry.name,
                root: entry.dir,
                project: None,
            };
            let mut skills = Vec::new();
            crate::skills::add_plugin(&mut skills, &plugin);
            for mut skill in skills {
                skill.roots = vec![SkillRoot::ClaudeUser];
                skill.entries = skill
                    .entries
                    .into_iter()
                    .map(|(_, path)| (SkillRoot::ClaudeUser, path))
                    .collect();
                catalog.skills.push(skill);
            }
            for mut server in crate::servers::plugin(&roots, &plugin, None, &mut catalog.problems) {
                server.source = Source::ClaudeUser;
                server.plugin = None;
                catalog.servers.push(server);
            }
        }
    }
    catalog
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    #[test]
    fn client_label_is_cosmetic_but_other_arguments_are_not() {
        let args = |values: &[&str]| {
            values
                .iter()
                .map(|value| (*value).to_owned())
                .collect::<Vec<_>>()
        };
        let claude = args(&["--app", "desktop", "--agent", "claudeCodeCLI"]);
        let codex = args(&["--app", "desktop", "--agent", "codexCLI"]);
        assert_eq!(server_args(&claude), server_args(&codex));
        assert_eq!(server_args(&claude), ["--app", "desktop"]);
        assert_eq!(
            server_args(&args(&["--agent=codexCLI"])),
            Vec::<&str>::new()
        );
        assert_eq!(
            server_args(&args(&["--agent", "custom"])),
            ["--agent", "custom"]
        );
    }

    #[test]
    fn project_server_provenance_and_equivalent_cli_processes() {
        let home = std::env::temp_dir().join(format!(
            "catalog-regression-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(home.join(".codex")).unwrap();
        let home = std::fs::canonicalize(home).unwrap();
        let project = home.join("project");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::write(
            project.join(".mcp.json"),
            r#"{"mcpServers":{"proj-everything":{"command":"npx","args":["-y","everything"]}}}"#,
        )
        .unwrap();
        std::fs::write(home.join(".claude.json"), r#"{"mcpServers":{"pencil":{"command":"/app/mcp-server","args":["--app","desktop","--agent","claudeCodeCLI"],"env":{"Z":"z","A":"a"}}}}"#).unwrap();
        let config = "[mcp_servers.pencil]\ncommand='/app/mcp-server'\nargs=['--app','desktop','--agent','codexCLI']\nenv={A='a',Z='z'}\nstartup_timeout_sec=20\n";
        std::fs::write(home.join(".codex/config.toml"), config).unwrap();
        let roots = crate::Roots::from_vars([("HOME".into(), home.display().to_string())]).unwrap();
        let rows = Catalog::read_rows(&roots, std::slice::from_ref(&project));
        let local = rows
            .iter()
            .find(|row| row.name == "proj-everything")
            .unwrap();
        assert!(
            local
                .definitions
                .iter()
                .all(|definition| definition.provenance.project.as_ref() == Some(&project))
        );
        let pencil: Vec<_> = rows.iter().filter(|row| row.name == "pencil").collect();
        assert_eq!(pencil.len(), 1);
        assert_eq!(pencil[0].definitions.len(), 2);
        let Content::Server(original) = &pencil[0].definitions[0].content else {
            panic!("server")
        };
        let mut mirror = original.as_ref().clone();
        mirror.plugin = Some(crate::Plugin {
            agent: Agent::Codex,
            id: "pencil@brigadier".into(),
            name: "pencil".into(),
            root: home.join("cache"),
            project: None,
        });
        if let Transport::Stdio { base, .. } = &mut mirror.transport {
            *base = Some(home.join("cache"));
        }
        assert_eq!(
            mirror.provenance().fingerprint,
            original.provenance().fingerprint
        );
        // An explicit working directory is meaningful, including on a legacy package.
        mirror.written.cwd = Some("/explicit".into());
        if let Transport::Stdio { cwd, .. } = &mut mirror.transport {
            *cwd = Some(PathBuf::from("/explicit"));
        }
        assert_ne!(
            mirror.provenance().fingerprint,
            original.provenance().fingerprint
        );
        mirror.plugin.as_mut().unwrap().project = Some(project.clone());
        assert_eq!(mirror.provenance().project, Some(project.clone()));
        // A different command or meaningful flag still gets a separate row.
        for changed in [
            config.replace("/app/mcp-server", "/other/server"),
            config.replace("'desktop'", "'web'"),
        ] {
            std::fs::write(home.join(".codex/config.toml"), changed).unwrap();
            let rows = Catalog::read_rows(&roots, &[]);
            assert_eq!(rows.iter().filter(|row| row.name == "pencil").count(), 2);
            assert!(rows.iter().all(|row| !row.name.contains('/')));
        }
        std::fs::remove_dir_all(home).unwrap();
    }
}
