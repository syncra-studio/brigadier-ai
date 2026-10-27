use super::super::cleanup_context::{Ctx, Secrets};
use super::*;
use brigadier_registry::Roots;
use brigadier_registry::adapter::{AgentAdapter, Step};
use brigadier_registry::library::Library;
use std::path::{Path, PathBuf};
use std::process::Command;
#[derive(Default)]
struct Kept;
impl Secrets for Kept {
    fn load(&self, _: &str) -> Result<BTreeMap<String, String>, String> {
        Ok(BTreeMap::new())
    }
}
fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("brigadier-ops-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::canonicalize(dir).unwrap()
}

fn roots(home: &Path) -> Roots {
    Roots::from_vars([("HOME".to_owned(), home.display().to_string())]).unwrap()
}

struct Fixture {
    home: PathBuf,
    project: PathBuf,
    roots: Roots,
    library: Library,
    secrets: Kept,
}
impl Fixture {
    fn new(name: &str, _: Option<bool>, _: Option<bool>) -> Self {
        let home = home(name);
        let project = home.join("proj");
        std::fs::create_dir_all(&project).unwrap();
        Self {
            roots: roots(&home),
            library: Library::at(home.join("data/library"), PathBuf::from("/no-cli")),
            secrets: Kept,
            project,
            home,
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.home);
    }
}
fn places_ctx<'a>(fx: &'a Fixture, projects: &'a [PathBuf]) -> Ctx<'a> {
    Ctx {
        roots: &fx.roots,
        env: &[],
        library: &fx.library,
        secrets: &fx.secrets,
        projects,
        claude_configs: &[],
        binary: &|_| None,
    }
}
fn add(
    ctx: &Ctx<'_>,
    text: &str,
    name: Option<&str>,
    scope: Option<&[PathBuf]>,
    shared: bool,
    options: &AddOptions,
    _: &impl Fn(&dyn AgentAdapter, &[Step]) -> Result<(), String>,
) -> Result<(Vec<String>, Option<String>), String> {
    if shared {
        return Err("--shared is no longer supported".into());
    }
    let mut items = add_parse::parse(text, name, &ctx.roots.home).map_err(|e| e.to_string())?;
    give_values(&mut items, &options.values)?;
    add_plain(ctx.library.root(), &items, scope).map(|keys| (keys, None))
}
#[test]
fn a_paste_is_read_before_anything_is_added() {
    let home = home("check");
    let check = check_add(
        &home,
        "claude mcp add --transport http sentry https://mcp.sentry.dev/mcp",
        None,
    );
    assert_eq!(check.error, None);
    assert_eq!(check.items.len(), 1);
    let sentry = &check.items[0];
    assert_eq!((sentry.name.as_str(), sentry.remote), ("sentry", true));
    assert!(
        sentry.summary.contains("mcp.sentry.dev"),
        "{}",
        sentry.summary
    );
    let check = check_add(&home, "{\"command\": \"x\"}", None);
    assert!(check.needs_name, "{check:?}");
    assert!(check.error.is_some());
    let check = check_add(
        &home,
        "{\"command\": \"x\", \"env\": {\"KEY\": \"v\"}}",
        Some("x"),
    );
    assert_eq!(check.items[0].secrets, ["KEY"]);
    let _ = std::fs::remove_dir_all(home);
}

fn bytes_under(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut found = BTreeMap::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(bytes_under(&path));
        } else {
            found.insert(path, std::fs::read(entry.path()).unwrap());
        }
    }
    found
}

#[test]
fn add_remove_only_write_the_data_dir_library_and_catalog_resolves_both_agents() {
    let home = home("plain-library");
    let roots = roots(&home);
    write(&home.join(".claude/settings.json"), "{\"user\":true}\n");
    write(
        &home.join(".claude/skills/native/SKILL.md"),
        "native user skill\n",
    );
    write(&home.join(".codex/config.toml"), "# user config\n");
    std::fs::write(home.join(".codex/binary"), [0, 255, 1, 13, 10]).unwrap();
    write(
        &home.join(".agents/skills/own/SKILL.md"),
        "own shared skill",
    );
    let before_claude = bytes_under(&home.join(".claude"));
    let before_codex = bytes_under(&home.join(".codex"));
    let before_agents = bytes_under(&home.join(".agents"));
    let data_dir = home.join("data");
    let root = data_dir.join("library");
    let library = Library::at(root.clone(), PathBuf::from("/not-a-cli"));
    let secrets = Kept;
    let ctx = Ctx {
        roots: &roots,
        env: &[],
        library: &library,
        secrets: &secrets,
        projects: &[],
        claude_configs: &[],
        binary: &|_| panic!("no CLI executable"),
    };
    let no_cli =
        |_: &dyn AgentAdapter, _: &[Step]| -> Result<(), String> { panic!("no adapter steps") };
    let paste = r#"{"mcpServers":{"docs":{"command":"/not-started","env":{"TOKEN":"pasted"}}}}"#;
    let options = AddOptions {
        values: BTreeMap::from([("TOKEN".into(), "typed".into())]),
    };
    let (keys, warning) = add(&ctx, paste, None, None, false, &options, &no_cli).unwrap();
    assert_eq!(keys, ["lib:docs"]);
    assert!(warning.is_none());
    let file = root.join("servers/docs.json");
    let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(saved["mcpServers"]["docs"]["env"]["TOKEN"], "");
    let id = saved["mcpServers"]["docs"]["brigadierSecrets"]["id"]
        .as_str()
        .unwrap();
    let (namespace, server) = id.split_once('/').unwrap();
    assert_eq!(server, "docs");
    assert!(uuid::Uuid::parse_str(namespace.strip_prefix("docs-").unwrap()).is_ok());
    assert!(!std::fs::read_to_string(&file).unwrap().contains("typed"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(file).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let source = home.join("src/tidy");
    write(
        &source.join("SKILL.md"),
        "---\nname: tidy\n---\nDo tidy work.\n",
    );
    write(&source.join("reference.bin"), "supporting content");
    let duplicate = Item::Skill(SkillSpec {
        name: Some("docs".into()),
        source: SkillSource::Folder(source.clone()),
    });
    assert!(
        add_plain(&root, &[duplicate], None)
            .unwrap_err()
            .contains("already has")
    );

    add(
        &ctx,
        &source.to_string_lossy(),
        None,
        None,
        false,
        &AddOptions::default(),
        &no_cli,
    )
    .unwrap();
    assert!(root.join("skills/tidy/SKILL.md").is_file());
    assert!(root.join("skills/tidy/reference.bin").is_file());
    assert!(!root.join(".claude-plugin").exists());
    assert!(!root.join("plugins").exists());
    let rows = Catalog::read_rows_with_library(&roots, &[], &data_dir);
    for agent in [Agent::Claude, Agent::Codex] {
        let resolved = super::super::resolve::resolve(super::super::resolve::Context {
            agent,
            account_home: &roots,
            project_root: None,
            cwd: &home,
            trusted: true,
            role: super::super::resolve::Role::Thread,
            rows: &rows,
            choices: &super::super::choices::Choices::default(),
            gateway: None,
        });
        assert!(resolved.servers.iter().any(|server| server.name == "docs"));
        assert!(
            resolved
                .skills
                .iter()
                .any(|skill| skill.display == "tidy" && !skill.native)
        );
    }
    remove_plain(ctx.library.root(), "docs").unwrap();
    remove_plain(ctx.library.root(), "tidy").unwrap();
    assert!(!root.join("servers/docs.json").exists());
    assert!(!root.join("skills/tidy").exists());
    assert!(
        remove_plain(ctx.library.root(), "native")
            .unwrap_err()
            .contains("only be turned off")
    );
    assert_eq!(bytes_under(&home.join(".claude")), before_claude);
    assert_eq!(bytes_under(&home.join(".codex")), before_codex);
    assert_eq!(bytes_under(&home.join(".agents")), before_agents);
    assert!(!home.join(".brigadier").exists());
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn choices_for_a_plain_library_row_survive_a_fresh_read() {
    use super::super::choices::{Choice, Choices, Scope};
    let home = home("plain-choices");
    let roots = roots(&home);
    let data = home.join("data");
    write(&data.join("library/skills/tidy/SKILL.md"), "tidy");
    let rows = Catalog::read_rows_with_library(&roots, &[], &data);
    let row = rows.iter().find(|row| row.name == "tidy").unwrap();
    let scope = Scope::Projects(vec![home.join("project-a"), home.join("project-b")]);
    let choice = Choice {
        removed: None,
        on: false,
        scope,
    };
    Choices(BTreeMap::from([(row.id.clone(), choice.clone())]))
        .save(&data)
        .unwrap();
    let rows = Catalog::read_rows_with_library(&roots, &[], &data);
    let row = rows.iter().find(|row| row.name == "tidy").unwrap();
    assert_eq!(Choices::load(&data).unwrap().choice(row), choice);
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn add_shared_refuses_without_changing_project_or_cli_files() {
    let fx = Fixture::new("add-shared-refused", None, None);
    let before = bytes_under(&fx.home);
    let projects = [fx.project.clone()];
    let ctx = places_ctx(&fx, &projects);
    assert!(
        add(
            &ctx,
            "https://example.test/mcp",
            Some("example"),
            Some(&projects),
            true,
            &AddOptions::default(),
            &|_, _| panic!("no CLI")
        )
        .unwrap_err()
        .contains("no longer supported")
    );
    assert_eq!(bytes_under(&fx.home), before);
}

#[test]
fn git_add_copies_plain_skill_without_cli_configuration_changes() {
    let home = home("plain-git");
    write(&home.join(".claude/settings.json"), "{\"untouched\":true}");
    write(&home.join(".codex/config.toml"), "# untouched\n");
    let before_claude = bytes_under(&home.join(".claude"));
    let before_codex = bytes_under(&home.join(".codex"));
    let repo = home.join("remote");
    write(
        &repo.join("SKILL.md"),
        "---\nname: git-skill\n---\nGit instructions\n",
    );
    for args in [
        vec!["init", "--quiet"],
        vec!["add", "."],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "-c",
            "core.hooksPath=/dev/null",
            "commit",
            "--quiet",
            "-m",
            "fixture",
        ],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let root = home.join("data/library");
    let item = Item::Skill(SkillSpec {
        name: None,
        source: SkillSource::Git {
            url: repo.to_string_lossy().into_owned(),
            rev: None,
            subdir: None,
        },
    });
    assert_eq!(add_plain(&root, &[item], None).unwrap(), ["lib:git-skill"]);
    assert_eq!(
        std::fs::read(root.join("skills/git-skill/SKILL.md")).unwrap(),
        std::fs::read(repo.join("SKILL.md")).unwrap()
    );
    assert!(root.join("origins/git-skill.json").is_file());
    assert!(!root.join("skills/git-skill/.git").exists());
    remove_plain(&root, "git-skill").unwrap();
    assert!(repo.join("SKILL.md").exists());
    assert_eq!(bytes_under(&home.join(".claude")), before_claude);
    assert_eq!(bytes_under(&home.join(".codex")), before_codex);
    let _ = std::fs::remove_dir_all(home);
}

#[test]
fn migrated_library_packages_are_catalog_sources_and_only_their_library_copy_is_removed() {
    let home = home("plain-migrated");
    let data = home.join("data");
    let root = data.join("library");
    let source = home.join("original");
    write(
        &source.join("SKILL.md"),
        "---\nname: migrated\n---\nOriginal instructions\n",
    );
    let source_bytes = bytes_under(&source);
    write(
        &root.join(".claude-plugin/marketplace.json"),
        r#"{"plugins":[{"name":"migrated"}]}"#,
    );
    write(
        &root.join("plugins/migrated/skills/migrated/SKILL.md"),
        &std::fs::read_to_string(source.join("SKILL.md")).unwrap(),
    );
    let rows = Catalog::read_rows_with_library(&roots(&home), &[], &data);
    let row = rows.iter().find(|row| row.name == "migrated").unwrap();
    assert!(
        row.definitions
            .iter()
            .any(|definition| definition.provenance.file.starts_with(&root))
    );
    remove_plain(&root, "migrated").unwrap();
    assert!(
        !Catalog::read_rows_with_library(&roots(&home), &[], &data)
            .iter()
            .any(|row| row.name == "migrated")
    );
    assert_eq!(bytes_under(&source), source_bytes);
    let _ = std::fs::remove_dir_all(home);
}

#[derive(Default)]
struct MemoryStore {
    values: std::sync::Mutex<BTreeMap<String, brigadier_cli::secrets::Values>>,
    refuse_forget: std::sync::atomic::AtomicBool,
}
impl brigadier_cli::secrets::Store for MemoryStore {
    fn get(&self, id: &str) -> Result<brigadier_cli::secrets::Values, String> {
        Ok(self
            .values
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_default())
    }
    fn put(&self, id: &str, values: &brigadier_cli::secrets::Values) -> Result<(), String> {
        if values.is_empty() && self.refuse_forget.load(std::sync::atomic::Ordering::SeqCst) {
            return Err("store unavailable".into());
        }
        let mut kept = self.values.lock().unwrap();
        if values.is_empty() {
            kept.remove(id);
        } else {
            kept.insert(id.into(), values.clone());
        }
        Ok(())
    }
}

#[test]
fn stored_env_and_headers_never_enter_json_and_launch_recovers_them_for_both_agents() {
    use brigadier_cli::secrets::Store;
    let home = home("secret-round-trip");
    let roots = roots(&home);
    let data = home.join("data");
    let root = data.join("library");
    let items=add_parse::parse(r#"{"mcpServers":{"runner":{"command":"/not-started","args":["--token","${TOKEN}"],"env":{"TOKEN":"env-only-in-memory-8391","EMPTY":""}},"web":{"type":"http","url":"https://fixture.invalid/mcp","headers":{"Authorization":"Bearer header-only-in-memory-9173"}}}}"#,None,&home).unwrap();
    let store = MemoryStore::default();
    add_plain(&root, &items, None).unwrap();
    let mut transaction = StoredValues::new(&store);
    transaction.save_items(&items, &root).unwrap();
    transaction.commit();
    for bytes in bytes_under(&root).values() {
        let text = String::from_utf8_lossy(bytes);
        assert!(!text.contains("env-only-in-memory-8391"));
        assert!(!text.contains("header-only-in-memory-9173"));
    }
    let saved: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("servers/web.json")).unwrap()).unwrap();
    let web_id = saved["mcpServers"]["web"]["brigadierSecrets"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let runner: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("servers/runner.json")).unwrap()).unwrap();
    let runner_id = runner["mcpServers"]["runner"]["brigadierSecrets"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        saved["mcpServers"]["web"]["headersHelper"],
        format!("brigadier secret headers {web_id}")
    );
    assert_eq!(saved["mcpServers"]["web"]["headers"]["Authorization"], "");
    let mut rows = Catalog::read_rows_with_library(&roots, &[], &data);
    let warnings = super::super::launch::fill_library_values(
        &mut rows,
        &data,
        &super::super::choices::Choices::default(),
        None,
        &store,
    );
    assert!(warnings.is_empty());
    for agent in [Agent::Claude, Agent::Codex] {
        let resolved = super::super::resolve::resolve(super::super::resolve::Context {
            agent,
            account_home: &roots,
            project_root: None,
            cwd: &home,
            trusted: false,
            role: super::super::resolve::Role::Thread,
            rows: &rows,
            choices: &super::super::choices::Choices::default(),
            gateway: None,
        });
        assert_eq!(resolved.servers.len(), 2);
        let env: BTreeMap<_, _> = resolved
            .servers
            .iter()
            .find(|server| server.name == "runner")
            .unwrap()
            .env
            .iter()
            .cloned()
            .collect();
        assert_eq!(
            env,
            BTreeMap::from([
                ("TOKEN".into(), "env-only-in-memory-8391".into()),
                ("EMPTY".into(), String::new())
            ])
        );
        let brigadier_providers::McpTransport::Stdio { args, .. } = &resolved
            .servers
            .iter()
            .find(|server| server.name == "runner")
            .unwrap()
            .transport
        else {
            panic!("stdio server");
        };
        assert_eq!(
            args.iter().map(String::as_str).collect::<Vec<_>>(),
            ["--token", "env-only-in-memory-8391"]
        );
        assert_eq!(
            resolved
                .servers
                .iter()
                .find(|server| server.name == "web")
                .unwrap()
                .env,
            [(
                "Authorization".into(),
                "Bearer header-only-in-memory-9173".into()
            )]
        );
    }
    let other_root = home.join("other-data/library");
    let other_items=add_parse::parse(r#"{"mcpServers":{"web":{"url":"https://other.invalid/mcp","headers":{"Authorization":"Bearer other-instance-value"}}}}"#,None,&home).unwrap();
    add_plain(&other_root, &other_items, None).unwrap();
    let mut other_transaction = StoredValues::new(&store);
    other_transaction
        .save_items(&other_items, &other_root)
        .unwrap();
    other_transaction.commit();
    let other_json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(other_root.join("servers/web.json")).unwrap())
            .unwrap();
    let other_id = other_json["mcpServers"]["web"]["brigadierSecrets"]["id"]
        .as_str()
        .unwrap();
    assert_ne!(web_id, other_id);
    assert_eq!(
        store.get(&web_id).unwrap()["Authorization"],
        "Bearer header-only-in-memory-9173"
    );
    assert_eq!(
        store.get(other_id).unwrap()["Authorization"],
        "Bearer other-instance-value"
    );
    store
        .refuse_forget
        .store(true, std::sync::atomic::Ordering::SeqCst);
    assert!(remove_with_secrets(&root, "web", &store).is_err());
    assert!(root.join("servers/web.json").exists());
    assert!(!store.get(&web_id).unwrap().is_empty());
    store
        .refuse_forget
        .store(false, std::sync::atomic::Ordering::SeqCst);
    remove_with_secrets(&root, "web", &store).unwrap();
    remove_with_secrets(&root, "runner", &store).unwrap();
    assert!(store.get(&web_id).unwrap().is_empty());
    assert!(store.get(&runner_id).unwrap().is_empty());
    assert_eq!(
        store.get(other_id).unwrap()["Authorization"],
        "Bearer other-instance-value"
    );
    remove_with_secrets(&other_root, "web", &store).unwrap();
    assert!(!root.join("servers/web.json").exists());
    assert!(!root.join("servers/runner.json").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[test]
fn failed_secret_publication_restores_the_previous_store_and_remove_rejects_foreign_references() {
    use brigadier_cli::secrets::Store;
    let home = home("secret-rollback");
    let root = home.join("data/library");
    let store = MemoryStore::default();
    let items = add_parse::parse(
        r#"{"runner":{"command":"x","env":{"TOKEN":"replacement-value"}}}"#,
        None,
        &home,
    )
    .unwrap();
    add_plain(&root, &items, None).unwrap();
    let raw: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("servers/runner.json")).unwrap()).unwrap();
    let id = raw["mcpServers"]["runner"]["brigadierSecrets"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let before = BTreeMap::from([("TOKEN".into(), "existing-token-value".into())]);
    store.put(&id, &before).unwrap();
    {
        let mut transaction = StoredValues::new(&store);
        transaction.save_items(&items, &root).unwrap();
    }
    assert_eq!(store.get(&id).unwrap(), before);
    store.put("runner/runner", &before).unwrap();
    write(
        &root.join("servers/other.json"),
        r#"{"mcpServers":{"other":{"command":"x","brigadierSecrets":{"id":"runner/runner","names":["TOKEN"]}}}}"#,
    );
    assert!(
        remove_with_secrets(&root, "other", &store)
            .unwrap_err()
            .contains("another library item")
    );
    assert_eq!(store.get("runner/runner").unwrap(), before);
    assert!(root.join("servers/other.json").exists());
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn merged_pencil_launches_each_cli_definition_and_one_toggle_controls_both() {
    use crate::manager::flow::{Flow, Options, Reply, Script};
    use brigadier_providers::{McpTransport, ProviderKind};
    use std::sync::Arc;

    let script: Script = Arc::new(|_| Box::pin(async { Reply::text("Ready.") }));
    let flow = Flow::start(
        "merged-pencil-launch-and-toggle",
        Options::default(),
        script,
    )
    .await;
    let roots = flow.manager.registry().roots().clone();
    write(
        &roots.claude_json,
        r#"{"mcpServers":{"pencil":{"command":"/app/mcp-server","args":["--app","desktop","--agent","claudeCodeCLI"]}}}"#,
    );
    write(
        &roots.codex_home.join("config.toml"),
        "[mcp_servers.pencil]\ncommand='/app/mcp-server'\nargs=['--app','desktop','--agent','codexCLI']\nstartup_timeout_sec=20\n",
    );
    let rows = flow.manager.plugin_catalog_rows();
    let pencil: Vec<_> = rows.iter().filter(|row| row.name == "pencil").collect();
    assert_eq!(pencil.len(), 1);
    assert_eq!(pencil[0].definitions.len(), 2);
    let row_id = &pencil[0].id;
    assert!(!flow.manager.data_dir.join("plugins.json").exists());

    for on in [true, false, true] {
        // The first pass launches from the default; each subsequent pass flips this one row.
        if !flow.specs().is_empty() {
            flow.manager.set_plugin_choice(row_id, on).unwrap();
            let saved = super::super::choices::Choices::load(&flow.manager.data_dir).unwrap();
            assert_eq!(saved.0.len(), 1);
            assert_eq!(saved.0[row_id].on, on);
        }
        for (provider, client) in [
            (ProviderKind::Claude, "claudeCodeCLI"),
            (ProviderKind::Codex, "codexCLI"),
        ] {
            let before = flow.specs().len();
            let session = flow
                .add_session("Pencil verification", None, provider)
                .await;
            flow.say_to(&session, "Check this launch.").await;
            flow.idle_in(&session).await;
            let specs = flow.specs();
            let (_, spec) = specs[before..]
                .iter()
                .find(|(kind, _)| *kind == provider)
                .expect("the CLI launched");
            let pencil = spec
                .mcp_servers
                .iter()
                .find(|server| server.name == "pencil");
            if on {
                let McpTransport::Stdio { command, args, .. } = &pencil.unwrap().transport else {
                    panic!("pencil should be stdio")
                };
                assert_eq!(command, &PathBuf::from("/app/mcp-server"));
                assert_eq!(args, &["--app", "desktop", "--agent", client]);
                println!("{provider:?} on: launch args {args:?}");
            } else {
                assert!(pencil.is_none());
                assert!(
                    spec.switched_off
                        .servers
                        .iter()
                        .any(|name| name == "pencil")
                );
                println!("{provider:?} off: pencil absent and native definition suppressed");
            }
        }
    }
    flow.stop().await;
}

#[tokio::test]
async fn first_toggle_persists_the_project_default_scope_through_the_manager() {
    use crate::manager::flow::{Flow, Options, Reply, Script};
    use std::sync::Arc;

    let script: Script = Arc::new(|_| Box::pin(async { Reply::text("Ready.") }));
    let flow = Flow::start(
        "first-toggle-project-scope",
        Options {
            files: vec![(
                ".mcp.json",
                r#"{"mcpServers":{"proj-everything":{"url":"https://project.test/mcp"}}}"#,
            )],
            ..Options::default()
        },
        script,
    )
    .await;
    // Reproduce the live fixture: a global old sharing cache mirrors the project server.
    let roots = flow.manager.registry().roots().clone();
    write(
        &roots.codex_home.join("config.toml"),
        "[plugins.\"proj-everything@brigadier\"]\nenabled=true\n",
    );
    let plugin = roots
        .codex_home
        .join("plugins/cache/brigadier/proj-everything/1.0.0");
    write(
        &plugin.join(".codex-plugin/plugin.json"),
        r#"{"name":"proj-everything","mcpServers":"./.codex.mcp.json"}"#,
    );
    write(
        &plugin.join(".codex.mcp.json"),
        r#"{"mcpServers":{"proj-everything":{"url":"https://project.test/mcp"}}}"#,
    );
    let rows = flow.manager.plugin_catalog_rows();
    let row = rows
        .iter()
        .find(|row| row.name == "proj-everything")
        .unwrap();
    assert_eq!(row.definitions.len(), 2);
    let file = flow.manager.data_dir.join("plugins.json");
    for on in [false, true] {
        assert!(!file.exists(), "each toggle starts without a saved choice");
        flow.manager.set_plugin_choice(&row.id, on).unwrap();
        let saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(saved[&row.id]["on"], on);
        assert_eq!(saved[&row.id]["scope"], serde_json::json!([flow.repo]));
        println!("No saved choice -> on={on}: {}", saved[&row.id]);
        std::fs::remove_file(&file).unwrap();
    }
    flow.stop().await;
}

#[tokio::test]
async fn hide_persists_without_changing_files_and_suppresses_both_agents() {
    use super::super::choices::Choices;
    use crate::manager::flow::{Flow, Options, Reply, Script};
    use brigadier_providers::ProviderKind;
    use std::sync::Arc;

    let script: Script = Arc::new(|_| Box::pin(async { Reply::text("Ready.") }));
    let flow = Flow::start("hide-native-plugins", Options::default(), script).await;
    let roots = flow.manager.registry().roots().clone();
    write(
        &roots.claude_json,
        r#"{"mcpServers":{"hidden-server":{"command":"/app/mcp-server"},"survivor":{"command":"/app/keep","args":["--keep"]}}}"#,
    );
    write(
        &roots.codex_home.join("config.toml"),
        "[mcp_servers.hidden-server]\ncommand='/app/mcp-server'\n",
    );
    for root in [&roots.claude_dir, &roots.codex_home] {
        write(
            &root.join("skills/hidden-skill/SKILL.md"),
            "---\nname: hidden-skill\n---\nUser skill\n",
        );
    }
    let rows = flow.manager.plugin_catalog_rows();
    let server = rows.iter().find(|row| row.name == "hidden-server").unwrap();
    let skill = rows.iter().find(|row| row.name == "hidden-skill").unwrap();
    assert_eq!(server.definitions.len(), 2);
    assert_eq!(skill.definitions.len(), 2);
    flow.manager.set_plugin_choice(&skill.id, true).unwrap();
    let original_version = flow.manager.plugin_catalog().unwrap().version;
    let before_claude = bytes_under(&roots.claude_dir);
    let before_codex = bytes_under(&roots.codex_home);
    let before_json = std::fs::read(&roots.claude_json).unwrap();

    for row in [server, skill] {
        // Repeated Remove must retain the original choice, including an unsaved default.
        flow.manager
            .remove_library_item(row.id.clone())
            .await
            .unwrap();
        flow.manager
            .remove_library_item(row.id.clone())
            .await
            .unwrap();
    }
    let view = flow.manager.plugin_catalog().unwrap();
    assert_ne!(view.version, original_version);
    assert!(
        view.rows
            .iter()
            .all(|row| row.name != "hidden-server" && row.name != "hidden-skill")
    );
    let survivor = view.rows.iter().find(|row| row.name == "survivor").unwrap();
    assert_eq!(
        survivor.definitions[0].endpoint.as_deref(),
        Some("/app/keep --keep")
    );
    let saved = Choices::load(&flow.manager.data_dir).unwrap();
    assert!(
        saved.0[&server.id].on,
        "hiding must not overwrite the default on choice"
    );
    assert!(saved.0[&skill.id].on);
    assert!(
        super::super::CatalogView::new(&rows, &saved)
            .rows
            .iter()
            .all(|row| row.name != "hidden-server" && row.name != "hidden-skill")
    );
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let before = flow.specs().len();
        let session = flow.add_session("Hidden items", None, provider).await;
        flow.say_to(&session, "Check launch.").await;
        flow.idle_in(&session).await;
        let specs = flow.specs();
        let (_, spec) = specs[before..]
            .iter()
            .find(|(kind, _)| *kind == provider)
            .unwrap();
        assert!(
            spec.mcp_servers
                .iter()
                .all(|server| server.name != "hidden-server")
        );
        assert!(
            spec.switched_off
                .servers
                .iter()
                .any(|name| name == "hidden-server")
        );
        assert!(
            spec.switched_off
                .skill_names
                .iter()
                .any(|name| name == "hidden-skill")
        );
        let own = if provider == ProviderKind::Claude {
            &roots.claude_dir
        } else {
            &roots.codex_home
        };
        assert!(
            spec.switched_off
                .skill_files
                .contains(&own.join("skills/hidden-skill/SKILL.md"))
        );
        for overlay in spec.skill_plugins.iter().chain(&spec.add_dirs) {
            assert!(!overlay.join("skills/hidden-skill/SKILL.md").exists());
            assert!(
                !overlay
                    .join(".claude/skills/hidden-skill/SKILL.md")
                    .exists()
            );
        }
    }
    assert_eq!(bytes_under(&roots.claude_dir), before_claude);
    assert_eq!(bytes_under(&roots.codex_home), before_codex);
    assert_eq!(std::fs::read(&roots.claude_json).unwrap(), before_json);
    flow.stop().await;
}

#[tokio::test]
async fn library_remove_still_deletes_the_library_item() {
    use crate::manager::flow::{Flow, Options, Reply, Script};
    use std::sync::Arc;
    let script: Script = Arc::new(|_| Box::pin(async { Reply::text("Ready.") }));
    let flow = Flow::start("delete-library-row", Options::default(), script).await;
    let file = flow
        .manager
        .data_dir
        .join("library/skills/delete-me/SKILL.md");
    write(&file, "---\nname: delete-me\n---\nLibrary skill");
    let row = flow
        .manager
        .plugin_catalog_rows()
        .into_iter()
        .find(|row| row.name == "delete-me")
        .unwrap();
    flow.manager.remove_library_item(row.id).await.unwrap();
    assert!(!file.exists());
    let view = flow.manager.plugin_catalog().unwrap();
    assert!(view.rows.iter().all(|row| row.name != "delete-me"));
    flow.stop().await;
}
