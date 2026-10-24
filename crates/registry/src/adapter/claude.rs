//! Claude Code: its own `plugin` and `mcp` commands, each with its own scope (`user`, `local`
//! for one project and this user, `project` for the team). It has no per-project off for a
//! server of the user's own config: that one goes into the Library first, then its plugin is
//! switched off for the project (D9).

use std::path::Path;

use super::{
    AgentAdapter, Capabilities, CliCall, Listed, Place, ProjectScope, Step, Unsupported,
    plain_text, server_json,
};
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

fn plugin_call(args: &[&str], id: &str, place: &Place) -> CliCall {
    let (scope, dir) = scope(place);
    let call = CliCall::new(PROGRAM, args)
        .arg(id)
        .arg("--scope")
        .arg(scope)
        .arg("--json");
    match dir {
        Some(dir) => call.in_dir(dir),
        None => call,
    }
}

impl AgentAdapter for Claude {
    fn agent(&self) -> Agent {
        Agent::Claude
    }

    fn label(&self) -> &'static str {
        "Claude Code"
    }

    fn program(&self) -> &'static str {
        PROGRAM
    }

    fn capabilities(&self) -> Capabilities {
        Capabilities {
            plugins: true,
            project_scope: ProjectScope::Native,
            login_needs_terminal: true,
        }
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

    fn trusts(&self, _: &Catalog, _: &Path) -> bool {
        // Its plugin scopes are its settings files, read in every folder.
        true
    }

    fn add_marketplace(&self, dir: &Path) -> CliCall {
        CliCall::new(PROGRAM, &["plugin", "marketplace", "add"]).arg(dir.display().to_string())
    }

    fn installed(&self, roots: &Roots, id: &str) -> bool {
        crate::plugins::claude_installed(roots, id)
    }

    fn installed_in_projects(&self, roots: &Roots, id: &str) -> Vec<Place> {
        crate::plugins::claude_installed_in_projects(roots, id)
            .into_iter()
            .map(|(path, shared)| Place::Project { path, shared })
            .collect()
    }

    fn switched_on_in(&self, project: &Path, id: &str) -> bool {
        crate::plugins::claude_project_switch(project, id) == Some(true)
    }

    fn refresh_marketplace(&self, name: &str) -> Option<CliCall> {
        Some(CliCall::new(PROGRAM, &["plugin", "marketplace", "update"]).arg(name))
    }

    fn update_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported> {
        let (scope, dir) = scope(place);
        let call = CliCall::new(PROGRAM, &["plugin", "update"])
            .arg(id)
            .arg("--scope")
            .arg(scope);
        Ok(vec![Step::Run(match dir {
            Some(dir) => call.in_dir(dir),
            None => call,
        })])
    }

    fn install_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported> {
        Ok(vec![Step::Run(plugin_call(
            &["plugin", "install"],
            id,
            place,
        ))])
    }

    fn remove_plugin(&self, id: &str, place: &Place) -> Result<Vec<Step>, Unsupported> {
        Ok(vec![Step::Run(plugin_call(
            &["plugin", "uninstall"],
            id,
            place,
        ))])
    }

    fn add_server(
        &self,
        name: &str,
        spec: &ServerSpec,
        place: &Place,
    ) -> Result<Vec<Step>, Unsupported> {
        let (scope, dir) = scope(place);
        let call = CliCall::new(PROGRAM, &["mcp", "add-json"])
            .arg(name)
            .arg(server_json(spec, "headers").to_string())
            .arg("--scope")
            .arg(scope);
        Ok(vec![Step::Run(match dir {
            Some(dir) => call.in_dir(dir),
            None => call,
        })])
    }

    fn remove_server(&self, name: &str, place: &Place) -> Result<Vec<Step>, Unsupported> {
        let (scope, dir) = scope(place);
        let call = CliCall::new(PROGRAM, &["mcp", "remove"])
            .arg(name)
            .arg("--scope")
            .arg(scope);
        Ok(vec![Step::Run(match dir {
            Some(dir) => call.in_dir(dir),
            None => call,
        })])
    }

    fn set_plugin(&self, id: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported> {
        let verb = if on { "enable" } else { "disable" };
        Ok(vec![Step::Run(plugin_call(&["plugin", verb], id, place))])
    }

    fn set_server(&self, _: &str, _: &Place, _: bool) -> Result<Vec<Step>, Unsupported> {
        Err(Unsupported(
            "Claude Code can turn it on or off per project once it's in your Library.".to_owned(),
        ))
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
    /// `! Needs authentication` for one that needs a sign-in (2.1.310).
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
