use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use super::*;
use crate::adapter::{AgentAdapter, adapter, adapters};
use crate::item::SkillSource;

/// A Keychain that remembers.
#[derive(Default)]
struct Kept(RefCell<Vec<(String, String, String)>>);

impl SecretSink for Kept {
    fn store(&self, server: &str, name: &str, value: &Secret) -> Result<(), String> {
        self.0.borrow_mut().push((
            server.to_owned(),
            name.to_owned(),
            value.expose().to_owned(),
        ));
        Ok(())
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brigadier-library-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn stdio(name: &str, command: &str, args: &[&str], env: &[(&str, &str)]) -> Item {
    Item::Server(ServerSpec {
        name: name.to_owned(),
        transport: TransportSpec::Stdio {
            command: command.to_owned(),
            args: args.iter().map(|arg| (*arg).to_owned()).collect(),
            env: env
                .iter()
                .map(|(k, v)| ((*k).to_owned(), Secret::new(*v)))
                .collect(),
            cwd: None,
        },
    })
}

#[test]
fn a_server_is_packed_for_each_agent_with_no_value_in_any_file() {
    let dir = scratch("server");
    let library = Library::at(dir.join("lib"), PathBuf::from("/bin/brigadier"));
    let kept = Kept::default();
    let item = stdio(
        "db",
        "${CLAUDE_PLUGIN_ROOT}/run",
        &["--root", "${CLAUDE_PLUGIN_ROOT}"],
        &[("TOKEN", "s3cret")],
    );
    let packed = library.pack("db", &item, None, &[], false, &kept).unwrap();
    assert!(packed.changed);
    assert_eq!(packed.entry.id(), "db@brigadier");
    assert_eq!(
        kept.0.borrow().as_slice(),
        [("db/db".to_owned(), "TOKEN".to_owned(), "s3cret".to_owned())]
    );
    let root = packed.entry.dir.display().to_string();
    let claude = read(&packed.entry.dir.join(".mcp.json"));
    assert_eq!(
        claude["mcpServers"]["db"]["args"],
        json!([
            "mcp",
            "run",
            "db/db",
            "--",
            "${CLAUDE_PLUGIN_ROOT}/run",
            "--root",
            "${CLAUDE_PLUGIN_ROOT}"
        ])
    );
    let codex = read(&packed.entry.dir.join(".codex.mcp.json"));
    assert_eq!(
        codex["mcpServers"]["db"]["command"],
        json!("/bin/brigadier")
    );
    assert_eq!(
        codex["mcpServers"]["db"]["args"],
        json!([
            "mcp",
            "run",
            "db/db",
            "--",
            format!("{root}/run"),
            "--root",
            root
        ])
    );
    // No value anywhere in the Library.
    let all = std::process::Command::new("grep")
        .args(["-r", "s3cret"])
        .arg(library.root())
        .output()
        .unwrap();
    assert!(all.stdout.is_empty());
    let market = read(&library.root().join(".claude-plugin/marketplace.json"));
    assert_eq!(market["name"], json!("brigadier"));
    assert_eq!(market["plugins"][0]["source"], json!("./plugins/db"));
    let codex_manifest = read(&packed.entry.dir.join(".codex-plugin/plugin.json"));
    assert_eq!(codex_manifest["mcpServers"], json!("./.codex.mcp.json"));
    let _ = std::fs::remove_dir_all(dir);
}

/// A remote server's headers reach every agent through the Keychain helper (Codex 0.164's
/// `http_headers_helper`, Claude Code's `headersHelper`), never as a value in a file.
#[test]
fn a_remote_servers_headers_reach_each_agent_through_the_helper() {
    let dir = scratch("headers");
    let library = Library::at(dir.join("lib"), PathBuf::from("/bin/brigadier"));
    let kept = Kept::default();
    let item = Item::Server(ServerSpec {
        name: "api".to_owned(),
        transport: TransportSpec::Http {
            url: "https://mcp.example.com/mcp".to_owned(),
            headers: vec![("Authorization".to_owned(), Secret::new("Bearer s3cret"))],
        },
    });
    let packed = library.pack("api", &item, None, &[], false, &kept).unwrap();
    let helper = json!("/bin/brigadier secret headers api/api");
    let claude = read(&packed.entry.dir.join(".mcp.json"));
    assert_eq!(claude["mcpServers"]["api"]["headersHelper"], helper);
    let codex = read(&packed.entry.dir.join(".codex.mcp.json"));
    assert_eq!(codex["mcpServers"]["api"]["http_headers_helper"], helper);
    assert_eq!(
        codex["mcpServers"]["api"]["url"],
        json!("https://mcp.example.com/mcp")
    );
    let all = std::process::Command::new("grep")
        .args(["-r", "s3cret"])
        .arg(library.root())
        .output()
        .unwrap();
    assert!(all.stdout.is_empty());

    // None without headers.
    let bare = Item::Server(ServerSpec {
        name: "open".to_owned(),
        transport: TransportSpec::Http {
            url: "https://open.example.com/mcp".to_owned(),
            headers: Vec::new(),
        },
    });
    let packed = library
        .pack("open", &bare, None, &[], false, &kept)
        .unwrap();
    let codex = read(&packed.entry.dir.join(".codex.mcp.json"));
    assert!(
        codex["mcpServers"]["open"]
            .get("http_headers_helper")
            .is_none()
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_version_counts_up_only_when_the_files_change() {
    let dir = scratch("version");
    let library = Library::at(dir.join("lib"), PathBuf::from("/bin/brigadier"));
    let kept = Kept::default();
    let item = stdio("echo", "echo-mcp", &[], &[]);
    let first = library
        .pack("echo", &item, None, &[], false, &kept)
        .unwrap();
    assert_eq!(first.entry.version, "1.0.0");
    assert_eq!(
        library.pack("echo", &item, None, &[], false, &kept),
        Err(LibraryError::NameTaken("echo".to_owned()))
    );
    let again = library.pack("echo", &item, None, &[], true, &kept).unwrap();
    assert!(!again.changed);
    assert_eq!(again.entry.version, "1.0.0");
    let other = stdio("echo", "echo-mcp", &["--loud"], &[]);
    let changed = library
        .pack("echo", &other, None, &[], true, &kept)
        .unwrap();
    assert!(changed.changed);
    assert_eq!(changed.entry.version, "1.0.1");
    assert_eq!(library.entries().len(), 1);
    assert_eq!(library.entries()[0].version, "1.0.1");
    library.remove("echo").unwrap();
    assert!(library.entries().is_empty());
    assert!(!dir.join("lib/plugins/echo").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_server_name_another_server_has_is_refused() {
    let dir = scratch("taken");
    let library = Library::at(dir.join("lib"), PathBuf::from("/bin/brigadier"));
    let item = stdio("pencil", "pencil-mcp", &[], &[]);
    assert_eq!(
        library.pack(
            "pencil",
            &item,
            None,
            &["pencil".to_owned()],
            false,
            &Kept::default()
        ),
        Err(LibraryError::ServerNameTaken("pencil".to_owned()))
    );
    assert!(matches!(
        library.pack("bad name", &item, None, &[], false, &Kept::default()),
        Err(LibraryError::BadName(_))
    ));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_skill_is_copied_with_its_files() {
    let dir = scratch("skill");
    let source = dir.join("src/grill");
    std::fs::create_dir_all(source.join("refs")).unwrap();
    std::fs::write(
        source.join("SKILL.md"),
        "---\nname: grill\ndescription: Asks hard questions.\n---\nBe tough.\n",
    )
    .unwrap();
    std::fs::write(source.join("refs/q.md"), "why?").unwrap();
    let library = Library::at(dir.join("lib"), PathBuf::from("/bin/brigadier"));
    let item = Item::Skill(SkillSpec {
        name: None,
        source: SkillSource::Folder(source.clone()),
    });
    let packed = library
        .pack("grill", &item, Some(&source), &[], false, &Kept::default())
        .unwrap();
    assert_eq!(packed.entry.description, "Asks hard questions.");
    let skill = packed.entry.dir.join("skills/grill");
    assert_eq!(
        std::fs::read_to_string(skill.join("refs/q.md")).unwrap(),
        "why?"
    );
    let manifest = read(&packed.entry.dir.join(".claude-plugin/plugin.json"));
    assert_eq!(manifest["skills"], json!("./skills/"));
    assert!(manifest.get("mcpServers").is_none());
    let empty = dir.join("src/empty");
    std::fs::create_dir_all(&empty).unwrap();
    assert!(matches!(
        library.pack("empty", &item, Some(&empty), &[], false, &Kept::default()),
        Err(LibraryError::NotASkill(_))
    ));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_new_item_is_installed_and_a_known_one_updated() {
    let library = Library::at(PathBuf::from("/lib"), PathBuf::from("/bin/brigadier"));
    let entry = Entry {
        name: "x".to_owned(),
        version: "1.0.0".to_owned(),
        description: String::new(),
        dir: PathBuf::from("/lib/plugins/x"),
    };
    for adapter in adapters() {
        let first = library
            .sync(adapter, &entry, &Place::Everywhere, false)
            .unwrap();
        assert!(first.len() >= 2, "{}", adapter.label());
        assert!(matches!(&first[0], Step::Run(call) if call.args.contains(&"/lib".to_owned())));
    }
    let claude = library
        .sync(adapter(Agent::Claude), &entry, &Place::Everywhere, true)
        .unwrap();
    let args: Vec<Vec<String>> = claude
        .iter()
        .map(|step| match step {
            Step::Run(call) => call.args.clone(),
            _ => Vec::new(),
        })
        .collect();
    assert_eq!(args[0], ["plugin", "marketplace", "add", "/lib"]);
    assert_eq!(args[1], ["plugin", "marketplace", "update", "brigadier"]);
    assert_eq!(args[2][..3], ["plugin", "update", "x@brigadier"]);
}

#[test]
fn a_git_skill_is_fetched_by_its_name() {
    let dir = scratch("git");
    let repo = dir.join("repo");
    for (folder, name) in [("skills/one", "one"), ("skills/two", "two")] {
        std::fs::create_dir_all(repo.join(folder)).unwrap();
        std::fs::write(
            repo.join(folder).join("SKILL.md"),
            format!("---\nname: {name}\n---\n"),
        )
        .unwrap();
    }
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-C"])
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    git(&["init", "-q"]);
    git(&["add", "."]);
    git(&["commit", "-qm", "skills"]);
    let url = format!("file://{}", repo.display());
    let whole = SkillSource::Git {
        url: url.clone(),
        rev: None,
        subdir: None,
    };
    let scratch_one = dir.join("s1");
    assert!(
        fetch_skill(&whole, &scratch_one)
            .unwrap_err()
            .contains("several skills")
    );
    let one = SkillSource::Git {
        url,
        rev: None,
        subdir: Some("skills/two".to_owned()),
    };
    let fetched = fetch_skill(&one, &dir.join("s2")).unwrap();
    assert!(fetched.ends_with("skills/two"));
    let _ = std::fs::remove_dir_all(dir);
}

/// A stdio MCP server in Python that has one tool.
const SERVER: &str = r#"import json, sys
for line in sys.stdin:
    req = json.loads(line)
    if 'id' not in req:
        continue
    if req['method'] == 'initialize':
        result = {'protocolVersion': req['params']['protocolVersion'], 'capabilities': {'tools': {}}, 'serverInfo': {'name': 'fixture', 'version': '1'}}
    elif req['method'] == 'tools/list':
        result = {'tools': [{'name': 'probe', 'inputSchema': {'type': 'object'}}]}
    else:
        result = {}
    print(json.dumps({'jsonrpc': '2.0', 'id': req['id'], 'result': result}), flush=True)
"#;

#[test]
fn a_packed_server_is_checked_by_its_tools() {
    let dir = scratch("tools");
    std::fs::write(dir.join("server.py"), SERVER).unwrap();
    let tools = check_tools(
        "/usr/bin/python3",
        &[dir.join("server.py").display().to_string()],
        &BTreeMap::new(),
        None,
        Duration::from_secs(10),
    )
    .unwrap();
    assert_eq!(tools, ["probe"]);
    let missing = check_tools(
        "/no/such/server",
        &[],
        &BTreeMap::new(),
        None,
        Duration::from_secs(1),
    );
    assert!(missing.unwrap_err().starts_with("It won't start"));
    let _ = std::fs::remove_dir_all(dir);
}

/// Runs `steps`, all the CLI's own commands.
fn run_all(
    adapter: &dyn AgentAdapter,
    roots: &crate::Roots,
    steps: &[Step],
    vars: &[(String, String)],
) {
    for step in steps {
        let Step::Run(call) = step else {
            panic!("not a command: {step:?}");
        };
        let out = std::process::Command::new(call.program)
            .args(&call.args)
            .envs(vars.iter().cloned())
            .envs(adapter.home_env(roots))
            .current_dir(call.cwd.clone().unwrap_or_else(std::env::temp_dir))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{} {:?}: {}{}",
            call.program,
            call.args,
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

/// Both real CLIs, in throwaway homes: a skill and an MCP server from the Library are listed by
/// each, and the server answers `tools/list` as each agent's copy starts it.
#[test]
#[ignore = "runs the installed claude and codex"]
fn both_clis_list_a_library_skill_and_server() {
    let dir = scratch("live");
    std::fs::write(dir.join("server.py"), SERVER).unwrap();
    let skill = dir.join("src/marker");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "---\nname: marker\ndescription: Library marker skill.\n---\nSay marker.\n",
    )
    .unwrap();
    let home = dir.join("home");
    let vars = [
        ("HOME".to_owned(), home.display().to_string()),
        (
            "CLAUDE_CONFIG_DIR".to_owned(),
            dir.join("claude").display().to_string(),
        ),
        (
            "CODEX_HOME".to_owned(),
            dir.join("codex").display().to_string(),
        ),
        ("PATH".to_owned(), std::env::var("PATH").unwrap()),
    ];
    std::fs::create_dir_all(dir.join("claude")).unwrap();
    std::fs::create_dir_all(dir.join("codex")).unwrap();
    let roots = crate::Roots::from_vars(vars.clone()).unwrap();
    let library = Library::in_home(&home, PathBuf::from("/bin/false"));
    let kept = Kept::default();
    let server = stdio(
        "libprobe",
        "/usr/bin/python3",
        &[&dir.join("server.py").display().to_string()],
        &[],
    );
    let packed_server = library
        .pack("libprobe", &server, None, &[], false, &kept)
        .unwrap();
    let item = Item::Skill(SkillSpec {
        name: None,
        source: SkillSource::Folder(skill.clone()),
    });
    let packed_skill = library
        .pack("marker", &item, Some(&skill), &[], false, &kept)
        .unwrap();
    for adapter in adapters() {
        for entry in [&packed_server.entry, &packed_skill.entry] {
            let steps = library
                .sync(adapter, entry, &Place::Everywhere, false)
                .unwrap();
            run_all(adapter, &roots, &steps, &vars);
            assert!(
                adapter.installed(&roots, &entry.id()),
                "{}",
                adapter.label()
            );
        }
        let (command, args) = library
            .server_command(&packed_server.entry, adapter.agent())
            .unwrap();
        let tools = check_tools(
            &command,
            &args,
            &BTreeMap::new(),
            None,
            Duration::from_secs(10),
        );
        assert_eq!(tools.unwrap(), ["probe"], "{}", adapter.label());
    }
    let catalog = crate::Catalog::read(&roots, None);
    for agent in [Agent::Claude, Agent::Codex] {
        let plugins: Vec<&str> = catalog
            .plugins
            .iter()
            .filter(|plugin| plugin.agent == agent)
            .map(|plugin| plugin.id.as_str())
            .collect();
        assert!(
            plugins.contains(&"libprobe@brigadier"),
            "{agent:?}: {plugins:?}"
        );
        assert!(
            plugins.contains(&"marker@brigadier"),
            "{agent:?}: {plugins:?}"
        );
    }
    let claude = std::process::Command::new("claude")
        .args(["mcp", "list"])
        .envs(vars.iter().cloned())
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&claude.stdout);
    println!("claude mcp list:\n{listed}");
    assert!(listed.contains("plugin:libprobe:libprobe"), "{listed}");
    let codex = std::process::Command::new("codex")
        .args(["mcp", "list", "--json"])
        .envs(vars.iter().cloned())
        .output()
        .unwrap();
    let listed = String::from_utf8_lossy(&codex.stdout);
    println!("codex mcp list --json:\n{listed}");
    assert!(listed.contains("libprobe"), "{listed}");
    assert!(catalog.skills.iter().any(|skill| skill.name == "marker"));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_item_fetched_from_git_knows_when_its_source_moved_on() {
    let dir = scratch("origin");
    let repo = dir.join("repo");
    let git = |args: &[&str]| {
        let out = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
    };
    std::fs::create_dir_all(repo.join("tidy")).unwrap();
    std::fs::write(repo.join("tidy/SKILL.md"), "---\nname: tidy\n---\nv1\n").unwrap();
    git(&["init", "-q", "-b", "main"]);
    git(&["add", "."]);
    git(&["commit", "-q", "-m", "v1"]);
    let source = SkillSource::Git {
        url: repo.display().to_string(),
        rev: None,
        subdir: None,
    };
    let fetched = dir.join("fetch");
    std::fs::create_dir_all(&fetched).unwrap();
    fetch_skill(&source, &fetched).unwrap();
    let commit = fetched_commit(&fetched).unwrap();
    assert_eq!(latest_commit(&source).unwrap(), commit);

    let library = Library::at(dir.join("library"), PathBuf::from("/bin/brigadier"));
    let origin = Origin {
        source: source.clone(),
        commit: commit.clone(),
    };
    library.set_origin("tidy", Some(&origin)).unwrap();
    assert_eq!(library.origin("tidy"), Some(origin));

    std::fs::write(repo.join("tidy/SKILL.md"), "---\nname: tidy\n---\nv2\n").unwrap();
    git(&["commit", "-q", "-am", "v2"]);
    assert_ne!(latest_commit(&source).unwrap(), commit);
    library.set_origin("tidy", None).unwrap();
    assert_eq!(library.origin("tidy"), None);
    assert!(latest_commit(&SkillSource::Folder(dir.clone())).is_err());
    let _ = std::fs::remove_dir_all(dir);
}
