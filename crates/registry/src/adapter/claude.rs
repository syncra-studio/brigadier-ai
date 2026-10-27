//! Claude Code MCP sign-in, listing, and one-time original restoration.
use std::path::Path;

use super::{AgentAdapter, CliCall, Listed, Place, Step, Unsupported, plain_text, server_json};
use crate::item::ServerSpec;
use crate::{Agent, Catalog, Roots, Source};

const PROGRAM: &str = "claude";

pub struct Claude;

/// The `--scope` for `place`, and the folder the command runs in.
fn scope(place: &Place) -> (&'static str, Option<&Path>) {
    match place {
        Place::Everywhere => ("user", None),
        Place::Project { path, shared } => (if *shared { "project" } else { "local" }, Some(path)),
    }
}

impl AgentAdapter for Claude {
    fn trusts(&self, _: &Catalog, _: &Path) -> bool {
        // Its plugin scopes are its settings files, read in every folder.
        true
    }

    fn agent(&self) -> Agent {
        Agent::Claude
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn owns(&self, source: Source) -> bool {
        matches!(
            source,
            Source::ClaudeUser | Source::ClaudeLocal | Source::ClaudeProject | Source::ClaudePlugin
        )
    }

    fn home_env(&self, roots: &Roots) -> Vec<(String, String)> {
        // Set, it moves `.claude.json` into the folder too: only for a home that is moved.
        if roots.claude_json == roots.home.join(".claude.json") {
            Vec::new()
        } else {
            vec![(
                "CLAUDE_CONFIG_DIR".to_owned(),
                roots.claude_dir.display().to_string(),
            )]
        }
    }

    fn add_server(
        &self,
        name: &str,
        spec: &ServerSpec,
        place: &Place,
    ) -> Result<Vec<Step>, Unsupported> {
        let (scope, dir) = scope(place);
        let mut entry = server_json(spec, "headers");
        if let Some(oauth) = &spec.oauth {
            entry["oauth"] = serde_json::json!({ "clientId": oauth.client_id });
        }
        let call = CliCall::new(PROGRAM, &["mcp", "add-json"])
            .arg(name)
            .arg(entry.to_string())
            .arg("--scope")
            .arg(scope);
        Ok(vec![Step::Run(match dir {
            Some(dir) => call.in_dir(dir),
            None => call,
        })])
    }

    fn login(&self, name: &str, project: Option<&Path>) -> CliCall {
        // Without a terminal it stops at once (research P4).
        CliCall {
            needs_terminal: true,
            ..CliCall::new(PROGRAM, &["mcp", "login"])
                .arg(name)
                .arg("--no-browser")
                .in_project(project)
        }
    }

    fn logout(&self, name: &str, project: Option<&Path>) -> CliCall {
        CliCall::new(PROGRAM, &["mcp", "logout"])
            .arg(name)
            .in_project(project)
    }

    fn list_servers(&self, project: Option<&Path>) -> CliCall {
        CliCall::new(PROGRAM, &["mcp", "list"]).in_project(project)
    }

    /// One line a server: `<name>: <command or address> - <status>`, the status
    /// `! Needs authentication` for one that needs a sign-in (2.1.310). A server switched off
    /// in the project (`⊘ Disabled for this project`) doesn't load there, so it isn't listed.
    fn parse_servers(&self, output: &str) -> Vec<Listed> {
        plain_text(output)
            .lines()
            .filter_map(|line| {
                let (name, rest) = line.split_once(": ")?;
                let name = name.trim();
                // The status follows the last ` - `: `✘ Failed to connect — <why>`.
                let status = rest
                    .rsplit_once(" - ")
                    .map_or("", |(_, status)| status.trim());
                if status.contains("Disabled for this project") {
                    return None;
                }
                let failed = status
                    .contains("Failed")
                    .then(|| status.trim_start_matches(['✘', '✗', ' ']).to_owned());
                (!name.is_empty() && !name.contains(char::is_whitespace)).then(|| Listed {
                    name: name.to_owned(),
                    needs_sign_in: rest.contains("Needs authentication"),
                    failed,
                })
            })
            .collect()
    }

    fn is_server(&self, listed: &str, server: &str, plugin: Option<&str>) -> bool {
        match plugin {
            Some(plugin) => listed == format!("plugin:{plugin}:{server}"),
            None => listed == server,
        }
    }
}
