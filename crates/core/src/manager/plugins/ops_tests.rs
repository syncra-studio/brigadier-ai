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
    assert_eq!(saved["mcpServers"]["docs"]["env"]["TOKEN"], "typed");
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
    let choice = Choice { on: false, scope };
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
