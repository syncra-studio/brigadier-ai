//! Claude Code: its own `plugin` and `mcp` commands, each with its own scope (`user`, `local`
//! for one project and this user, `project` for the team). It has no per-project off for a
//! server of the user's own config: that one goes into the Library first, then its plugin is
//! switched off for the project (D9).

use std::path::Path;

use super::{AgentAdapter, Capabilities, CliCall, Place, ProjectScope, Step, Unsupported};
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

    fn set_plugin(&self, id: &str, place: &Place, on: bool) -> Result<Vec<Step>, Unsupported> {
        let verb = if on { "enable" } else { "disable" };
        Ok(vec![Step::Run(plugin_call(&["plugin", verb], id, place))])
    }

    fn set_server(&self, _: &str, _: &Place, _: bool) -> Result<Vec<Step>, Unsupported> {
        Err(Unsupported(
            "Claude Code can turn it on or off per project once it's in your Library.".to_owned(),
        ))
    }

    fn login(&self, name: &str) -> CliCall {
        CliCall {
            needs_terminal: true,
            ..CliCall::new(PROGRAM, &["mcp", "login"]).arg(name)
        }
    }

    fn list_servers(&self, project: Option<&Path>) -> CliCall {
        let call = CliCall::new(PROGRAM, &["mcp", "list"]);
        match project {
            Some(dir) => call.in_dir(dir),
            None => call,
        }
    }
}
