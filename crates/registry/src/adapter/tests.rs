use super::*;
use crate::{Agent, Roots, Source};
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
        claude.login("plugin:x:s", None).args,
        ["mcp", "login", "plugin:x:s", "--no-browser"]
    );
    assert_eq!(
        codex.login("linear", None).args,
        ["mcp", "login", "linear", "--no-browser"]
    );
    assert_eq!(
        claude.logout("linear", None).args,
        ["mcp", "logout", "linear"]
    );
    assert_eq!(
        codex.logout("linear", None).args,
        ["mcp", "logout", "linear"]
    );

    // Claude Code 2.1.310's own words, colours and all.
    let listed = claude.parse_servers(
        "Checking MCP server health…\r\n\r\n\
         plugin:oauth:oauth-probe: http://127.0.0.1:53127/mcp (HTTP) - \u{1b}[33m! Needs authentication\u{1b}[39m\n\
         user-one: /usr/bin/python3 /tmp/server.py - \u{1b}[32m✔ Connected\u{1b}[39m\n\
         broken: /usr/bin/false  - \u{1b}[31m✘ Failed to connect — CONNECTION_CLOSED: Connection closed\u{1b}[39m\n\
         railway: /bin/cat railway - ⊘ Disabled for this project (re-enable via /mcp)\n",
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
