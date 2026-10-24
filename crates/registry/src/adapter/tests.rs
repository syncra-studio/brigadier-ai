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

#[test]
fn each_cli_signs_in_on_its_own_list_and_prints_the_page() {
    let claude = adapter(Agent::Claude);
    let codex = adapter(Agent::Codex);
    assert_eq!(
        claude.login("plugin:x:s").args,
        ["mcp", "login", "plugin:x:s", "--no-browser"]
    );
    assert_eq!(
        codex.login("linear").args,
        ["mcp", "login", "linear", "--no-browser"]
    );
    assert_eq!(claude.logout("linear").args, ["mcp", "logout", "linear"]);
    assert_eq!(codex.logout("linear").args, ["mcp", "logout", "linear"]);

    // Claude Code 2.1.310's own words, colours and all.
    let listed = claude.parse_servers(
        "Checking MCP server health…\r\n\r\n\
         plugin:oauth:oauth-probe: http://127.0.0.1:53127/mcp (HTTP) - \u{1b}[33m! Needs authentication\u{1b}[39m\n\
         user-one: /usr/bin/python3 /tmp/server.py - \u{1b}[32m✔ Connected\u{1b}[39m\n\
         broken: /usr/bin/false  - \u{1b}[31m✘ Failed to connect — CONNECTION_CLOSED: Connection closed\u{1b}[39m\n",
    );
    assert_eq!(
        listed,
        [
            Listed {
                name: "plugin:oauth:oauth-probe".into(),
                needs_sign_in: true,
                failed: None,
            },
            Listed {
                name: "user-one".into(),
                needs_sign_in: false,
                failed: None,
            },
            Listed {
                name: "broken".into(),
                needs_sign_in: false,
                failed: Some("Failed to connect — CONNECTION_CLOSED: Connection closed".into()),
            },
        ]
    );
    assert!(
        claude
            .parse_servers("No MCP servers configured. Use `claude mcp add` to add a server.\n")
            .is_empty()
    );
    assert!(claude.is_server("plugin:oauth:oauth-probe", "oauth-probe", Some("oauth")));
    assert!(!claude.is_server("plugin:oauth:oauth-probe", "oauth-probe", None));
    assert!(claude.is_server("linear", "linear", None));

    let listed = codex.parse_servers(
        r#"[{"name":"oauth-probe","enabled":true,"auth_status":"not_logged_in"},
            {"name":"docs","enabled":true,"auth_status":"o_auth"},
            {"name":"local","auth_status":"unsupported"}]"#,
    );
    assert_eq!(
        listed
            .iter()
            .map(|s| (s.name.as_str(), s.needs_sign_in))
            .collect::<Vec<_>>(),
        [("oauth-probe", true), ("docs", false), ("local", false)]
    );
    assert!(codex.parse_servers("not json").is_empty());
    assert!(codex.is_server("oauth-probe", "oauth-probe", Some("oauth")));
}

#[test]
fn terminal_escapes_and_links_are_left_out() {
    assert_eq!(
        plain_text(
            "  \u{1b}]8;;http://a/b?c=1\u{7}http://a/b?c=1\u{1b}]8;;\u{7}\r\nnext \u{1b}[1mbold\u{1b}[22m"
        ),
        "  http://a/b?c=1\nnext bold"
    );
}

#[test]
fn a_server_goes_back_through_each_agents_own_setup() {
    use crate::Secret;
    use crate::item::{ServerSpec, TransportSpec};
    let spec = ServerSpec {
        name: "pg".into(),
        transport: TransportSpec::Stdio {
            command: "pg-mcp".into(),
            args: vec!["--ro".into()],
            env: vec![("PGPASS".into(), Secret::new("s"))],
            cwd: None,
        },
    };
    let steps = adapter(Agent::Claude)
        .add_server("pg", &spec, &project())
        .unwrap();
    let (program, args, dir) = run(&steps[0]);
    assert_eq!((program, dir), ("claude", Some(Path::new("/p"))));
    assert_eq!(&args[..3], ["mcp", "add-json", "pg"]);
    let json: serde_json::Value = serde_json::from_str(args[3]).unwrap();
    assert_eq!(json["command"], "pg-mcp");
    assert_eq!(json["env"]["PGPASS"], "s");
    assert_eq!(&args[4..], ["--scope", "local"]);
    assert_eq!(
        run(&adapter(Agent::Claude)
            .remove_server("pg", &Place::Everywhere)
            .unwrap()[0])
        .1,
        ["mcp", "remove", "pg", "--scope", "user"]
    );

    let http = ServerSpec {
        name: "linear".into(),
        transport: TransportSpec::Http {
            url: "https://mcp.linear.app/mcp".into(),
            headers: vec![("X-Team".into(), Secret::new("t"))],
        },
    };
    let steps = adapter(Agent::Codex)
        .add_server("linear", &http, &Place::Everywhere)
        .unwrap();
    let Step::UserConfig(edit) = &steps[0] else {
        panic!("{steps:?}")
    };
    assert_eq!(edit.key, ["mcp_servers", "linear"]);
    let table = edit.value.as_ref().unwrap();
    assert_eq!(table["url"].as_str(), Some("https://mcp.linear.app/mcp"));
    assert_eq!(table["http_headers"]["X-Team"].as_str(), Some("t"));
    assert!(table.get("type").is_none());
    let steps = adapter(Agent::Codex)
        .remove_server("linear", &project())
        .unwrap();
    assert!(matches!(
        &steps[0],
        Step::ProjectFile { edit, .. } if edit.key == ["mcp_servers", "linear"] && edit.value.is_none()
    ));
    assert_eq!(
        run(&adapter(Agent::Codex)
            .remove_server("linear", &Place::Everywhere)
            .unwrap()[0])
        .1,
        ["mcp", "remove", "linear"]
    );
}
