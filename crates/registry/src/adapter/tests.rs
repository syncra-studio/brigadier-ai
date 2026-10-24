use std::path::{Path, PathBuf};

use super::*;

fn project() -> Place {
    Place::Project {
        path: PathBuf::from("/p"),
        shared: false,
    }
}

fn run(step: &Step) -> (&'static str, Vec<&str>, Option<&Path>) {
    match step {
        Step::Run(call) => (
            call.program,
            call.args.iter().map(String::as_str).collect(),
            call.cwd.as_deref(),
        ),
        other => panic!("not a run: {other:?}"),
    }
}

#[test]
fn every_agent_has_one_adapter_and_owns_its_sources() {
    let agents: Vec<Agent> = adapters().iter().map(|adapter| adapter.agent()).collect();
    assert_eq!(agents, [Agent::Claude, Agent::Codex]);
    for agent in agents {
        assert_eq!(adapter(agent).agent(), agent);
    }
    assert_eq!(owner(Source::ClaudeLocal), Some(Agent::Claude));
    assert_eq!(owner(Source::CodexPlugin), Some(Agent::Codex));
    assert_eq!(owner(Source::Desktop), None);
    assert_eq!(owner(Source::CursorProject), None);
}

#[test]
fn claude_keeps_one_project_with_its_own_scopes() {
    let claude = adapter(Agent::Claude);
    let steps = claude.install_plugin("x@lib", &project()).unwrap();
    assert_eq!(
        run(&steps[0]),
        (
            "claude",
            vec!["plugin", "install", "x@lib", "--scope", "local", "--json"],
            Some(Path::new("/p"))
        )
    );
    let shared = Place::Project {
        path: PathBuf::from("/p"),
        shared: true,
    };
    let steps = claude.set_plugin("x@lib", &shared, false).unwrap();
    assert_eq!(
        run(&steps[0]).1,
        ["plugin", "disable", "x@lib", "--scope", "project", "--json"]
    );
    let steps = claude
        .set_plugin("x@lib", &Place::Everywhere, true)
        .unwrap();
    assert_eq!(
        run(&steps[0]),
        (
            "claude",
            vec!["plugin", "enable", "x@lib", "--scope", "user", "--json"],
            None
        )
    );
    // No per-project off for the user's own servers: the Library first (D9).
    assert!(claude.set_server("pencil", &project(), false).is_err());
    assert!(claude.login("plugin:x:s").needs_terminal);
}

#[test]
fn codex_keeps_one_project_in_the_projects_own_file() {
    let codex = adapter(Agent::Codex);
    assert_eq!(
        codex.capabilities().project_scope,
        ProjectScope::ProjectFile
    );
    let steps = codex.install_plugin("x@lib", &project()).unwrap();
    assert_eq!(
        run(&steps[0]),
        ("codex", vec!["plugin", "add", "x@lib"], None)
    );
    let key = vec![
        "plugins".to_owned(),
        "x@lib".to_owned(),
        "enabled".to_owned(),
    ];
    assert_eq!(
        steps[1],
        Step::UserConfig(TomlEdit {
            key: key.clone(),
            value: Some(toml::Value::Boolean(false)),
        })
    );
    assert_eq!(
        steps[2],
        Step::ProjectFile {
            file: PathBuf::from("/p/.codex/config.toml"),
            edit: TomlEdit {
                key,
                value: Some(toml::Value::Boolean(true)),
            },
            shared: false,
        }
    );
    let steps = codex.set_server("pencil", &project(), false).unwrap();
    assert!(matches!(
        &steps[0],
        Step::ProjectFile { edit, .. } if edit.key == ["mcp_servers", "pencil", "enabled"]
    ));
    assert!(!codex.login("linear").needs_terminal);
}

#[test]
fn a_moved_home_is_pointed_at_and_the_default_one_is_left_alone() {
    let vars = |extra: &[(&str, &str)]| {
        let mut vars = vec![("HOME".to_owned(), "/h".to_owned())];
        vars.extend(
            extra
                .iter()
                .map(|(k, v)| ((*k).to_owned(), (*v).to_owned())),
        );
        Roots::from_vars(vars).unwrap()
    };
    let plain = vars(&[]);
    for adapter in adapters() {
        assert!(adapter.home_env(&plain).is_empty(), "{}", adapter.label());
    }
    let moved = vars(&[("CLAUDE_CONFIG_DIR", "/c"), ("CODEX_HOME", "/x")]);
    assert_eq!(
        adapter(Agent::Claude).home_env(&moved),
        [("CLAUDE_CONFIG_DIR".to_owned(), "/c".to_owned())]
    );
    assert_eq!(
        adapter(Agent::Codex).home_env(&moved),
        [("CODEX_HOME".to_owned(), "/x".to_owned())]
    );
}
