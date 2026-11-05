//! OpenCode's own setup, as its 2.2.5 server reports it: `opencode.json(c)` servers and skill
//! folders, its own and the other CLIs' it also loads. Its MCP sign-in is its own command.
//! Brigadier reads these files and never writes them.
use std::path::{Path, PathBuf};

use super::{AgentAdapter, CliCall, Listed, Place, Step, Unsupported};
use crate::item::ServerSpec;
use crate::{Catalog, ProviderKind, Roots, Source, servers, skills};

const PROGRAM: &str = "opencode";
/// A private server for the call: without it the call goes through the user's background
/// service, or fails when another home's has its port.
const STANDALONE: &str = "--standalone";
/// The files of one config folder, the later winning per server.
const CONFIG_FILES: [&str; 2] = ["opencode.json", "opencode.jsonc"];

pub struct OpenCode;

fn config_files(folder: &Path) -> impl Iterator<Item = PathBuf> + '_ {
    CONFIG_FILES.iter().map(|name| folder.join(name))
}

impl AgentAdapter for OpenCode {
    fn agent(&self) -> ProviderKind {
        ProviderKind::OpenCode
    }

    fn label(&self) -> &'static str {
        "OpenCode"
    }

    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn owns(&self, source: Source) -> bool {
        matches!(source, Source::OpenCodeUser | Source::OpenCodeProject)
    }

    fn home_env(&self, _: &Roots) -> Vec<(String, String)> {
        Vec::new()
    }

    fn home<'a>(&self, roots: &'a Roots) -> &'a Path {
        &roots.opencode_dir
    }

    // An account moves only its database: its config and skills stay in the one folder.
    fn at_home(&self, _: &mut Roots, _: &Path) {}

    fn plugin_servers_by_name(&self) -> bool {
        false
    }

    fn disables_skills_in_config(&self) -> bool {
        false
    }

    fn overlay_namespace(&self) -> Option<&'static str> {
        None
    }

    fn renames_colliding_servers(&self) -> bool {
        false
    }

    fn trusts(&self, _: &Catalog, _: &Path) -> bool {
        // It asks nothing before reading a project's config.
        true
    }

    fn read_user(&self, roots: &Roots, catalog: &mut Catalog) {
        let files: Vec<PathBuf> = config_files(&roots.opencode_dir).collect();
        for server in servers::opencode(
            &files,
            Source::OpenCodeUser,
            None,
            roots,
            &mut catalog.problems,
        ) {
            if server.disabled {
                catalog.off.servers.push(server);
            } else {
                catalog.servers.push(server);
            }
        }
        skills::add_opencode_user(&mut catalog.skills, &roots.opencode_dir);
    }

    fn read_project(&self, roots: &Roots, project: &Path, catalog: &mut Catalog) {
        // The project's folder, then its `.opencode`, which wins.
        let own = project.join(".opencode");
        let files: Vec<PathBuf> = config_files(project).chain(config_files(&own)).collect();
        for server in servers::opencode(
            &files,
            Source::OpenCodeProject,
            Some(project),
            roots,
            &mut catalog.problems,
        ) {
            if server.disabled {
                catalog.off.servers.push(server);
            } else {
                catalog.servers.push(server);
            }
        }
        skills::add_opencode_project(&mut catalog.skills, project);
    }

    fn also_loads_skills_at(&self, roots: &Roots, project: Option<&Path>, folder: &Path) -> bool {
        // Whatever `CLAUDE_CONFIG_DIR` says: it looks in the home's.
        let of = |base: &Path| {
            folder == base.join(".claude/skills") || folder == base.join(".agents/skills")
        };
        of(&roots.home) || project.is_some_and(of)
    }

    fn add_server(&self, _: &str, _: &ServerSpec, _: &Place) -> Result<Vec<Step>, Unsupported> {
        Err(Unsupported(
            "Brigadier doesn't change OpenCode's own config.".to_owned(),
        ))
    }

    fn login(&self, name: &str, project: Option<&Path>) -> CliCall {
        // It asks and prints its link in a terminal.
        CliCall {
            needs_terminal: true,
            ..CliCall::new(PROGRAM, &["mcp", "auth", STANDALONE])
                .arg(name)
                .in_project(project)
        }
    }

    fn logout(&self, name: &str, project: Option<&Path>) -> CliCall {
        CliCall::new(PROGRAM, &["mcp", "logout", STANDALONE])
            .arg(name)
            .in_project(project)
    }

    /// `opencode mcp list` says "No MCP servers configured" with servers configured (2.2.5); its
    /// config report names them all, merged and in one shape.
    fn list_servers(&self, project: Option<&Path>) -> CliCall {
        CliCall::new(PROGRAM, &["debug", "config", STANDALONE]).in_project(project)
    }

    /// The report is a JSON list of sources, each file's `info.mcp.servers`. It says nothing of
    /// a sign-in, so a remote server that may have one counts as needing it.
    fn parse_servers(&self, output: &str) -> Vec<Listed> {
        let Some(start) = output.find('[') else {
            return Vec::new();
        };
        let Ok(serde_json::Value::Array(sources)) = serde_json::from_str(output[start..].trim())
        else {
            return Vec::new();
        };
        let mut listed: std::collections::BTreeMap<String, Listed> = Default::default();
        for servers in sources
            .iter()
            .filter_map(|source| source.pointer("/info/mcp/servers")?.as_object())
        {
            for (name, server) in servers {
                if server.get("disabled").and_then(serde_json::Value::as_bool) == Some(true) {
                    listed.remove(name);
                    continue;
                }
                let authorized = server
                    .get("headers")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|headers| {
                        headers
                            .keys()
                            .any(|header| header.eq_ignore_ascii_case("authorization"))
                    });
                listed.insert(
                    name.clone(),
                    Listed {
                        name: name.clone(),
                        needs_sign_in: server.get("type").and_then(serde_json::Value::as_str)
                            == Some("remote")
                            && server.get("oauth") != Some(&serde_json::Value::Bool(false))
                            && !authorized,
                        // It lists without connecting.
                        failed: None,
                    },
                );
            }
        }
        listed.into_values().collect()
    }

    fn is_server(&self, listed: &str, server: &str, _: Option<&str>) -> bool {
        listed == server
    }
}
