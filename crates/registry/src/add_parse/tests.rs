use std::path::{Path, PathBuf};

use super::*;

const HOME: &str = "/home/me";

fn items(text: &str) -> Vec<Item> {
    items_named(text, None)
}

fn items_named(text: &str, name: Option<&str>) -> Vec<Item> {
    parse(text, name, Path::new(HOME)).unwrap_or_else(|error| panic!("{text:?}: {error}"))
}

fn error(text: &str) -> AddError {
    match parse(text, None, Path::new(HOME)) {
        Ok(items) => panic!("{text:?} read as {items:?}"),
        Err(error) => error,
    }
}

fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, Secret)> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), Secret::new(*value)))
        .collect()
}

fn stdio(name: &str, command: &str, args: &[&str], env: &[(&str, &str)]) -> Item {
    Item::Server(ServerSpec {
        name: name.into(),
        transport: TransportSpec::Stdio {
            command: command.into(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: pairs(env),
            cwd: None,
        },
    })
}

fn http(name: &str, url: &str, headers: &[(&str, &str)]) -> Item {
    Item::Server(ServerSpec {
        name: name.into(),
        transport: TransportSpec::Http {
            url: url.into(),
            headers: pairs(headers),
        },
    })
}

fn git(url: &str, rev: Option<&str>, subdir: Option<&str>) -> Item {
    Item::Skill(SkillSpec {
        name: None,
        source: SkillSource::Git {
            url: url.into(),
            rev: rev.map(str::to_owned),
            subdir: subdir.map(str::to_owned),
        },
    })
}

fn skills_sh(id: &str) -> Item {
    Item::Skill(SkillSpec {
        name: None,
        source: SkillSource::SkillsSh(id.into()),
    })
}

/// The secret values, which `Item`'s `Debug` hides, checked on their own.
fn secrets(item: &Item) -> Vec<(String, String)> {
    let Item::Server(server) = item else {
        return Vec::new();
    };
    let pairs = match &server.transport {
        TransportSpec::Stdio { env, .. } => env,
        TransportSpec::Http { headers, .. } => headers,
    };
    pairs
        .iter()
        .map(|(key, value)| (key.clone(), value.expose().to_owned()))
        .collect()
}

#[test]
fn claude_mcp_add_lines() {
    let table: Vec<(&str, Item)> = vec![
        (
            "claude mcp add my-server -e API_KEY=xxx -- npx my-mcp-server",
            stdio(
                "my-server",
                "npx",
                &["my-mcp-server"],
                &[("API_KEY", "xxx")],
            ),
        ),
        (
            "claude mcp add -e A=1 B=2 --env C=3 -s user fs -- npx -y @mcp/fs /tmp",
            stdio(
                "fs",
                "npx",
                &["-y", "@mcp/fs", "/tmp"],
                &[("A", "1"), ("B", "2"), ("C", "3")],
            ),
        ),
        (
            "claude mcp add --transport stdio --scope=project tool node server.js --port 3",
            stdio("tool", "node", &["server.js", "--port", "3"], &[]),
        ),
        (
            "claude mcp add --transport http sentry https://mcp.sentry.dev/mcp",
            http("sentry", "https://mcp.sentry.dev/mcp", &[]),
        ),
        (
            "claude mcp add -t sse linear https://mcp.linear.app/sse",
            http("linear", "https://mcp.linear.app/sse", &[]),
        ),
        (
            r#"claude mcp add --transport http corridor https://app.corridor.dev/api/mcp --header "Authorization: Bearer abc" -H 'X-Team: t1'"#,
            http(
                "corridor",
                "https://app.corridor.dev/api/mcp",
                &[("Authorization", "Bearer abc"), ("X-Team", "t1")],
            ),
        ),
        (
            // A lone URL with no transport is a remote server, not a command.
            "claude mcp add notion https://mcp.notion.com/mcp",
            http("notion", "https://mcp.notion.com/mcp", &[]),
        ),
        (
            // `${VAR}` stays as written.
            "claude mcp add gh -e 'TOKEN=${GITHUB_TOKEN}' -- docker run -i ghcr.io/github/server",
            stdio(
                "gh",
                "docker",
                &["run", "-i", "ghcr.io/github/server"],
                &[("TOKEN", "${GITHUB_TOKEN}")],
            ),
        ),
        (
            r#"claude mcp add-json weather '{"type":"stdio","command":"/bin/weather","args":["--api-key","abc"],"env":{"CACHE_DIR":"/tmp"}}'"#,
            stdio(
                "weather",
                "/bin/weather",
                &["--api-key", "abc"],
                &[("CACHE_DIR", "/tmp")],
            ),
        ),
        (
            r#"claude mcp add-json -s user api "{\"type\":\"http\",\"url\":\"https://api.x.dev/mcp\",\"headers\":{\"X-Key\":\"k\"}}""#,
            http("api", "https://api.x.dev/mcp", &[("X-Key", "k")]),
        ),
    ];
    for (text, expected) in table {
        let got = items(text);
        assert_eq!(got, std::slice::from_ref(&expected), "{text}");
        assert_eq!(secrets(&got[0]), secrets(&expected), "{text}");
    }
}

#[test]
fn codex_mcp_add_lines() {
    let table: Vec<(&str, Item)> = vec![
        (
            "codex mcp add docs --env API_KEY=k --env=MODE=fast -- npx -y docs-mcp",
            stdio(
                "docs",
                "npx",
                &["-y", "docs-mcp"],
                &[("API_KEY", "k"), ("MODE", "fast")],
            ),
        ),
        (
            "codex mcp add figma --url https://mcp.figma.com/mcp",
            http("figma", "https://mcp.figma.com/mcp", &[]),
        ),
        (
            "codex mcp add gh --url=https://api.githubcopilot.com/mcp/ --bearer-token-env-var GITHUB_PAT",
            http(
                "gh",
                "https://api.githubcopilot.com/mcp/",
                &[("Authorization", "Bearer ${GITHUB_PAT}")],
            ),
        ),
        (
            // Settings of the Codex run itself are not the server's.
            "codex mcp add -c model=o3 --enable x tool -- ./run.sh",
            stdio("tool", "./run.sh", &[], &[]),
        ),
    ];
    for (text, expected) in table {
        let got = items(text);
        assert_eq!(got, std::slice::from_ref(&expected), "{text}");
        assert_eq!(secrets(&got[0]), secrets(&expected), "{text}");
    }
}

#[test]
fn quoting_and_line_breaks_read_as_a_shell_does() {
    let table: Vec<(&str, Item)> = vec![
        (
            r#"claude mcp add q -e "MSG=say \"hi\" \$HOME" -e 'RAW=a\b' -- cmd "two words" it\'s"#,
            stdio(
                "q",
                "cmd",
                &["two words", "it's"],
                &[("MSG", r#"say "hi" $HOME"#), ("RAW", r"a\b")],
            ),
        ),
        (
            "claude mcp add long \\\n  -e K=v \\\n  -- npx \\\n  server",
            stdio("long", "npx", &["server"], &[("K", "v")]),
        ),
        (
            "$ claude mcp add p -- 'C:/Program Files/x.exe' # a comment",
            stdio("p", "C:/Program Files/x.exe", &[], &[]),
        ),
        (
            r#"claude mcp add j -- node -e 'console.log("a b")' """#,
            stdio("j", "node", &["-e", r#"console.log("a b")"#, ""], &[]),
        ),
    ];
    for (text, expected) in table {
        let got = items(text);
        assert_eq!(got, std::slice::from_ref(&expected), "{text}");
        assert_eq!(secrets(&got[0]), secrets(&expected), "{text}");
    }

    // Several commands in one paste, by line, `;` or `&&`.
    assert_eq!(
        items(
            "# both\nclaude mcp add a -- x\ncodex mcp add b --url https://b.dev/mcp && claude mcp add c -- y; claude mcp add d -- z"
        ),
        [
            stdio("a", "x", &[], &[]),
            http("b", "https://b.dev/mcp", &[]),
            stdio("c", "y", &[], &[]),
            stdio("d", "z", &[], &[]),
        ]
    );
}

#[test]
fn server_json_in_every_shape() {
    let desktop = r#"{
        "mcpServers": {
            "alpha": {"command": "npx", "args": ["-y", "a", 3], "env": {"KEY": "${A_KEY}", "N": 1}},
            "beta": {"url": "https://beta.dev/mcp", "headers": {"Authorization": "Bearer t"}},
            "gamma": {"type": "sse", "url": "https://gamma.dev/sse"},
            "delta": {"type": "stdio", "command": "/bin/d", "cwd": "/work"}
        },
        "other": 1
    }"#;
    let got = items(desktop);
    let mut names: Vec<&str> = got
        .iter()
        .map(|item| match item {
            Item::Server(server) => server.name.as_str(),
            Item::Skill(_) => "",
        })
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["alpha", "beta", "delta", "gamma"]);
    let find = |name: &str| {
        got.iter()
            .find(|item| matches!(item, Item::Server(server) if server.name == name))
            .unwrap()
            .clone()
    };
    let alpha = stdio(
        "alpha",
        "npx",
        &["-y", "a", "3"],
        &[("KEY", "${A_KEY}"), ("N", "1")],
    );
    assert_eq!(find("alpha"), alpha);
    assert_eq!(secrets(&find("alpha")), secrets(&alpha));
    let beta = http(
        "beta",
        "https://beta.dev/mcp",
        &[("Authorization", "Bearer t")],
    );
    assert_eq!(find("beta"), beta);
    assert_eq!(secrets(&find("beta")), secrets(&beta));
    assert_eq!(find("gamma"), http("gamma", "https://gamma.dev/sse", &[]));
    assert_eq!(
        find("delta"),
        Item::Server(ServerSpec {
            name: "delta".into(),
            transport: TransportSpec::Stdio {
                command: "/bin/d".into(),
                args: Vec::new(),
                env: Vec::new(),
                cwd: Some(PathBuf::from("/work")),
            },
        })
    );

    // A bare map, and one entry copied out of a file.
    assert_eq!(
        items(r#"{"one": {"command": "a"}, "two": {"serverUrl": "https://two.dev"}}"#),
        [
            stdio("one", "a", &[], &[]),
            http("two", "https://two.dev", &[])
        ]
    );
    assert_eq!(
        items(r#""pencil": {"command": "pencil", "args": ["mcp"]},"#),
        [stdio("pencil", "pencil", &["mcp"], &[])]
    );
    // One server object, named beside it.
    assert_eq!(
        items_named(r#"{"type": "http", "url": "https://x.dev/mcp"}"#, Some("x")),
        [http("x", "https://x.dev/mcp", &[])]
    );
    // A server that happens to be called `servers`.
    assert_eq!(
        items(r#"{"servers": {"command": "s"}}"#),
        [stdio("servers", "s", &[], &[])]
    );
}

#[test]
fn urls() {
    let table: Vec<(&str, Item)> = vec![
        (
            "https://mcp.linear.app/mcp",
            http("linear", "https://mcp.linear.app/mcp", &[]),
        ),
        (
            "https://api.githubcopilot.com/mcp/",
            http("githubcopilot", "https://api.githubcopilot.com/mcp/", &[]),
        ),
        (
            "http://localhost:3000/mcp",
            http("localhost", "http://localhost:3000/mcp", &[]),
        ),
        (
            "https://mcp.example.co.uk/sse?x=1",
            http("example", "https://mcp.example.co.uk/sse?x=1", &[]),
        ),
        (
            "https://github.com/anthropics/skills",
            git("https://github.com/anthropics/skills", None, None),
        ),
        (
            "https://github.com/anthropics/skills.git",
            git("https://github.com/anthropics/skills", None, None),
        ),
        (
            "https://github.com/anthropics/skills/tree/main/document-skills/pdf",
            git(
                "https://github.com/anthropics/skills",
                Some("main"),
                Some("document-skills/pdf"),
            ),
        ),
        (
            "https://github.com/o/r/tree/v1.2",
            git("https://github.com/o/r", Some("v1.2"), None),
        ),
        (
            "https://github.com/o/r/blob/main/skills/pdf/SKILL.md",
            git("https://github.com/o/r", Some("main"), Some("skills/pdf")),
        ),
        ("github.com/o/r", git("https://github.com/o/r", None, None)),
        (
            "git@github.com:o/r.git",
            git("git@github.com:o/r.git", None, None),
        ),
        (
            "https://gitlab.com/o/r.git",
            git("https://gitlab.com/o/r.git", None, None),
        ),
    ];
    for (text, expected) in table {
        assert_eq!(items(text), [expected], "{text}");
    }
    // A name given beside a URL wins over the host's.
    assert_eq!(
        items_named("https://mcp.linear.app/mcp", Some("work-linear")),
        [http("work-linear", "https://mcp.linear.app/mcp", &[])]
    );
}

#[test]
fn skills_sh_ids_links_and_lines() {
    let table: Vec<(&str, Vec<Item>)> = vec![
        (
            "vercel-labs/agent-skills/frontend-design",
            vec![skills_sh("vercel-labs/agent-skills/frontend-design")],
        ),
        (
            "vercel-labs/agent-skills",
            vec![skills_sh("vercel-labs/agent-skills")],
        ),
        (
            "https://skills.sh/vercel-labs/agent-skills/web-design",
            vec![skills_sh("vercel-labs/agent-skills/web-design")],
        ),
        ("skills.sh/o/r", vec![skills_sh("o/r")]),
        ("npx skills add o/r", vec![skills_sh("o/r")]),
        (
            "npx -y skills@latest add o/r --skill a -s b -g -a claude-code",
            vec![skills_sh("o/r/a"), skills_sh("o/r/b")],
        ),
        ("bunx skills add o/r --skill=a", vec![skills_sh("o/r/a")]),
        (
            "npx skills add https://github.com/o/r --skill pdf",
            vec![Item::Skill(SkillSpec {
                name: Some("pdf".into()),
                source: SkillSource::Git {
                    url: "https://github.com/o/r".into(),
                    rev: None,
                    subdir: None,
                },
            })],
        ),
    ];
    for (text, expected) in table {
        assert_eq!(items(text), expected, "{text}");
    }
}

#[test]
fn folders() {
    let root = std::env::temp_dir().join(format!(
        "20261024-1114-plugins-redesign-w05-w05a-{}",
        std::process::id()
    ));
    let one = root.join("one");
    let many = root.join("many");
    let empty = root.join("empty");
    std::fs::create_dir_all(&one).unwrap();
    std::fs::write(one.join("SKILL.md"), "---\nname: one\n---\n").unwrap();
    std::fs::create_dir_all(many.join("a")).unwrap();
    std::fs::write(many.join("a/SKILL.md"), "").unwrap();
    std::fs::create_dir_all(&empty).unwrap();

    let folder_item = |path: &Path| {
        vec![Item::Skill(SkillSpec {
            name: None,
            source: SkillSource::Folder(path.to_owned()),
        })]
    };
    let home = root.to_str().unwrap();
    let parse_at = |text: &str| parse(text, None, &root);
    assert_eq!(parse_at(one.to_str().unwrap()), Ok(folder_item(&one)));
    assert_eq!(parse_at("~/many"), Ok(folder_item(&many.join("a"))));
    assert_eq!(
        parse_at(&format!("{home}/one/SKILL.md")),
        Ok(folder_item(&one))
    );
    assert_eq!(
        parse_at(&format!("'{home}/one'")),
        Ok(folder_item(&one)),
        "a quoted path"
    );
    assert_eq!(folder(&many, None), Ok(folder_item(&many.join("a"))));
    assert_eq!(parse_at("~/empty"), Err(AddError::NoSkill(empty.clone())));
    assert_eq!(
        parse_at("~/missing"),
        Err(AddError::NoFolder(root.join("missing")))
    );

    std::fs::remove_dir_all(&root).unwrap();
}

/// A folder of skills gives each skill inside it, each a folder with its own `SKILL.md`, so
/// what Continue accepts can be added.
#[test]
fn folder_of_skills_gives_each_skill() {
    let root = std::env::temp_dir().join(format!(
        "20261024-1114-plugins-redesign-w10b-many-{}",
        std::process::id()
    ));
    for skill in ["b", "a"] {
        std::fs::create_dir_all(root.join(skill)).unwrap();
        std::fs::write(root.join(skill).join("SKILL.md"), "").unwrap();
    }
    std::fs::create_dir_all(root.join("notes")).unwrap();

    let items = folder(&root, Some("ignored")).unwrap();
    let dirs: Vec<PathBuf> = items
        .iter()
        .map(|item| match item {
            Item::Skill(SkillSpec {
                name: None,
                source: SkillSource::Folder(dir),
            }) => dir.clone(),
            other => panic!("not an unnamed folder skill: {other:?}"),
        })
        .collect();
    assert_eq!(dirs, vec![root.join("a"), root.join("b")]);
    assert!(dirs.iter().all(|dir| dir.join("SKILL.md").is_file()));

    std::fs::remove_dir_all(&root).unwrap();
}

#[test]
fn errors_in_plain_words() {
    let table: Vec<(&str, AddError)> = vec![
        ("", AddError::Unrecognized),
        ("hello there", AddError::Unrecognized),
        ("linear", AddError::Unrecognized),
        ("claude mcp add x -- 'open", AddError::UnclosedQuote),
        ("claude mcp add \"x -- y", AddError::UnclosedQuote),
        (
            "claude mcp add 'my server' -- x",
            AddError::BadName("my server".into()),
        ),
        ("claude mcp add", AddError::NoName),
        ("{\"command\": \"x\"}", AddError::NoName),
        ("{\"mcpServers\": {}}", AddError::NoServers),
        ("{\"a\": 1}", AddError::NoServers),
        (
            "{\"a.b\": {\"command\": \"x\"}}",
            AddError::BadName("a.b".into()),
        ),
        ("https://10.0.0.1/mcp", AddError::NoName),
    ];
    for (text, expected) in table {
        assert_eq!(error(text), expected, "{text}");
    }

    let message = |text: &str| error(text).to_string();
    assert_eq!(
        message("nope"),
        "That doesn't look like an MCP server or a skill."
    );
    assert!(message("{\"a\": ").starts_with("That JSON doesn't read: "));
    assert!(message("claude mcp add x --client-id abc https://x.dev").contains("OAuth"));
    assert!(message("codex mcp add x --url https://x.dev --oauth-client-id c").contains("OAuth"));
    assert!(
        message(r#"{"x": {"url": "https://x.dev", "oauth": {"clientId": "c"}}}"#).contains("OAuth")
    );
    assert_eq!(
        message("claude mcp add-json w '{\"type\":\"ws\",\"url\":\"wss://w.dev\"}'"),
        "Brigadier can't add a “ws” server: only stdio, http and sse ones."
    );
    assert_eq!(
        message("claude mcp add x -e NOPE -- y"),
        "“NOPE” isn't an environment variable: write it as KEY=value."
    );
    assert_eq!(
        message("claude mcp add --frobnicate x -- y"),
        "Brigadier doesn't know the option --frobnicate in that line."
    );
    assert_eq!(
        message("claude mcp add -t http x ftp://x.dev"),
        "The address of “x” must start with https:// or http://."
    );
    assert_eq!(
        message("claude mcp add -H 'A: b' x -- y"),
        "Headers only go with a server reached by URL."
    );
    assert_eq!(
        message("claude mcp add x"),
        "“x” has no command to start it."
    );
    assert_eq!(
        message("codex mcp add x --url https://x.dev -- y"),
        "“x” has both a URL and a command: give one."
    );
    assert_eq!(
        message("codex mcp add x --bearer-token-env-var T -- y"),
        "A bearer token only goes with a server reached by URL."
    );
    assert_eq!(
        message("codex mcp add x --env K=v --url https://x.dev"),
        "Environment variables only go with a server the agent starts, not one reached by URL."
    );
    assert_eq!(
        message("https://github.com/o"),
        "That GitHub link doesn't point at a repository."
    );
    assert_eq!(
        message("{\"mcpServers\": {}}"),
        "That JSON has no MCP servers in it."
    );
    assert_eq!(
        message("{\"x\": {\"env\": {}}}"),
        "“x” has neither a command nor a URL."
    );
}
