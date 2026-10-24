use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::adapter::{Place, adapter, codex};
use crate::{Agent, Catalog, Roots};

/// A fresh folder for one test, its symlinks resolved, removed when the test ends.
struct Fixture(PathBuf);

fn fixture(name: &str) -> Fixture {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "brigadier-apply-{name}-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Fixture(std::fs::canonicalize(dir).unwrap())
}

impl std::ops::Deref for Fixture {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn key(path: &[&str]) -> Vec<String> {
    path.iter().map(|part| (*part).to_owned()).collect()
}

fn set(path: &[&str], on: bool) -> TomlEdit {
    TomlEdit {
        key: key(path),
        value: Some(toml::Value::Boolean(on)),
    }
}

fn unset(path: &[&str]) -> TomlEdit {
    TomlEdit {
        key: key(path),
        value: None,
    }
}

const USER_FILE: &str = r#"# Written by hand.
model = "o3"   # the fast one
approval_policy = "never"

[projects."/Users/me/work"]
trust_level = "trusted"

[mcp_servers.mine]
# keep me
command = "npx"
args = ["-y",   "mine"]  # odd spacing

[plugins."other@lib"]
enabled = false # off for now

[features]
web_search = true
"#;

#[test]
fn an_edit_keeps_the_rest_of_the_file_byte_for_byte() {
    let added = edit_text(USER_FILE, &set(&["plugins", "x@lib", "enabled"], true)).unwrap();
    // Next to its own kind.
    assert_eq!(
        added,
        USER_FILE.replace(
            "for now\n",
            "for now\n\n[plugins.\"x@lib\"]\nenabled = true\n"
        )
    );
    // A new kind at the end.
    let skill = edit_text(USER_FILE, &set(&["skills", "s", "enabled"], false)).unwrap();
    assert_eq!(skill, format!("{USER_FILE}\n[skills.s]\nenabled = false\n"));
    // After the file's closing comment.
    let closing = "[features]\nweb_search = true\n# the end\n";
    assert_eq!(
        edit_text(closing, &set(&["skills", "s", "enabled"], false)).unwrap(),
        format!("{closing}\n[skills.s]\nenabled = false\n")
    );

    let changed = edit_text(USER_FILE, &set(&["plugins", "other@lib", "enabled"], true)).unwrap();
    assert_eq!(
        changed,
        USER_FILE.replace("enabled = false # off", "enabled = true # off")
    );

    let fresh = edit_text("", &set(&["mcp_servers", "my.server", "enabled"], false)).unwrap();
    assert_eq!(fresh, "[mcp_servers.\"my.server\"]\nenabled = false\n");
}

#[test]
fn a_removal_takes_only_the_tables_it_leaves_empty() {
    let text = format!(
        "{USER_FILE}\n[plugins.\"x@lib\"]\nenabled = true\n\n[plugins.\"y@lib\"]\nenabled = true\nnote = \"mine\"\n\n[plugins.\"empty@lib\"]\n"
    );
    let removed = edit_text(&text, &unset(&["plugins", "x@lib", "enabled"])).unwrap();
    assert_eq!(
        removed,
        format!(
            "{USER_FILE}\n[plugins.\"y@lib\"]\nenabled = true\nnote = \"mine\"\n\n[plugins.\"empty@lib\"]\n"
        )
    );
    // A table with the user's own key stays.
    let kept = edit_text(&removed, &unset(&["plugins", "y@lib", "enabled"])).unwrap();
    assert!(
        kept.contains("[plugins.\"y@lib\"]\nnote = \"mine\"\n"),
        "{kept}"
    );
    assert!(kept.contains("[plugins.\"empty@lib\"]\n"), "{kept}");
    // Nothing to remove: nothing changes.
    assert_eq!(
        edit_text(USER_FILE, &unset(&["plugins", "nope@lib", "enabled"])).unwrap(),
        USER_FILE
    );
    // The only entry: the file is left with nothing of Brigadier's.
    let only = edit_text("", &set(&["plugins", "x@lib", "enabled"], true)).unwrap();
    assert_eq!(
        edit_text(&only, &unset(&["plugins", "x@lib", "enabled"])).unwrap(),
        ""
    );
}

#[test]
fn the_user_config_is_addressed_by_quoted_key_paths() {
    assert_eq!(
        key_path(&key(&["plugins", "x@lib", "enabled"])),
        "plugins.\"x@lib\".enabled"
    );
    assert_eq!(
        key_path(&key(&["mcp_servers", "my-server_2", "enabled"])),
        "mcp_servers.my-server_2.enabled"
    );
    let text =
        "[plugins.\"x@lib\"]\nenabled = false\n\n[plugins.\"y@lib\"]\nenabled = true\nnote = 1\n";
    // The table holds nothing else: it goes; `plugins` keeps `y@lib`.
    assert_eq!(
        removal_key(Some(text), &key(&["plugins", "x@lib", "enabled"])),
        key(&["plugins", "x@lib"])
    );
    assert_eq!(
        removal_key(Some(text), &key(&["plugins", "y@lib", "enabled"])),
        key(&["plugins", "y@lib", "enabled"])
    );
    assert_eq!(
        removal_key(
            Some("[mcp_servers.s]\nenabled = false\n"),
            &key(&["mcp_servers", "s", "enabled"])
        ),
        key(&["mcp_servers"])
    );
    assert_eq!(
        removal_key(None, &key(&["plugins", "x@lib", "enabled"])),
        key(&["plugins", "x@lib", "enabled"])
    );
}

fn roots(home: &Path) -> Roots {
    Roots::from_vars([
        ("HOME".to_owned(), home.display().to_string()),
        (
            "CODEX_HOME".to_owned(),
            home.join("codex-home").display().to_string(),
        ),
    ])
    .unwrap()
}

fn trusting(projects: &[&Path]) -> Catalog {
    Catalog {
        codex_trusted: projects.iter().map(|path| path.to_path_buf()).collect(),
        ..Catalog::default()
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .args([
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "init.defaultBranch=main",
        ])
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

fn repo(dir: &Path) -> PathBuf {
    let repo = dir.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q"]);
    git(&repo, &["commit", "-q", "--allow-empty", "-m", "start"]);
    repo
}

/// Turns `x@lib` on in `project` with Codex.
fn plugin_on(project: &Path, shared: bool) -> Result<Applied, ApplyError> {
    let codex = adapter(Agent::Codex);
    let place = Place::Project {
        path: project.to_owned(),
        shared,
    };
    let steps = codex.set_plugin("x@lib", &place, true).unwrap();
    apply(
        codex,
        &roots(project),
        &trusting(&[project]),
        &steps,
        &[],
        None,
    )
}

fn exclude_of(repo: &Path) -> String {
    std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap_or_default()
}

fn ignored_by(dir: &Path) -> String {
    git(dir, &["check-ignore", "-v", ".codex/config.toml"])
}

#[test]
fn just_me_is_kept_out_of_git_once() {
    let dir = fixture("just-me");
    let repo = repo(&dir);
    plugin_on(&repo, false).unwrap();
    plugin_on(&repo, false).unwrap();
    assert_eq!(
        std::fs::read_to_string(codex::project_file(&repo)).unwrap(),
        "[plugins.\"x@lib\"]\nenabled = true\n"
    );
    assert_eq!(
        exclude_of(&repo)
            .lines()
            .filter(|line| *line == "/.codex/config.toml")
            .count(),
        1
    );
    assert!(
        ignored_by(&repo).starts_with(".git/info/exclude:"),
        "{}",
        ignored_by(&repo)
    );
    assert_eq!(git(&repo, &["status", "--porcelain"]), "");

    // A project in a folder of the checkout: anchored to its path there.
    let sub = repo.join("apps/web");
    std::fs::create_dir_all(&sub).unwrap();
    plugin_on(&sub, false).unwrap();
    assert!(exclude_of(&repo).contains("\n/apps/web/.codex/config.toml\n"));
    assert!(
        ignored_by(&sub).contains("info/exclude:"),
        "{}",
        ignored_by(&sub)
    );
}

#[test]
fn just_me_in_a_linked_worktree_uses_the_shared_exclude() {
    let dir = fixture("worktree");
    let repo = repo(&dir);
    let tree = dir.join("tree");
    git(&repo, &["worktree", "add", "-q", tree.to_str().unwrap()]);
    plugin_on(&tree, false).unwrap();
    plugin_on(&tree, false).unwrap();
    assert_eq!(
        exclude_of(&repo)
            .lines()
            .filter(|line| *line == "/.codex/config.toml")
            .count(),
        1
    );
    let named = ignored_by(&tree);
    let exclude = repo.join(".git/info/exclude");
    assert!(
        named.starts_with(&format!("{}:", exclude.display())),
        "{named}"
    );
    assert_eq!(git(&tree, &["status", "--porcelain"]), "");
}

#[test]
fn a_committed_file_is_refused_for_just_me() {
    let dir = fixture("tracked");
    let repo = repo(&dir);
    let file = codex::project_file(&repo);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "# the team's\n").unwrap();
    git(&repo, &["add", ".codex/config.toml"]);
    git(&repo, &["commit", "-q", "-m", "team"]);

    let err = plugin_on(&repo, false).unwrap_err();
    assert_eq!(
        err,
        ApplyError::TrackedFile(".codex/config.toml".to_owned())
    );
    assert_eq!(
        err.to_string(),
        "This project's .codex/config.toml is committed, so a change here would reach your team. Use Share with the team instead."
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "# the team's\n");
    assert_eq!(
        exclude_of(&repo)
            .lines()
            .filter(|l| l.contains(".codex"))
            .count(),
        0
    );

    // Shared writes it.
    plugin_on(&repo, true).unwrap();
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# the team's\n\n[plugins.\"x@lib\"]\nenabled = true\n"
    );
}

#[test]
fn shared_takes_the_just_me_exclusion_away() {
    let dir = fixture("shared");
    let repo = repo(&dir);
    std::fs::write(repo.join(".git/info/exclude"), "# mine\n*.log\n").unwrap();
    plugin_on(&repo, false).unwrap();
    assert_eq!(exclude_of(&repo), "# mine\n*.log\n/.codex/config.toml\n");
    plugin_on(&repo, true).unwrap();
    assert_eq!(exclude_of(&repo), "# mine\n*.log\n");
    assert_eq!(
        git(&repo, &["status", "--porcelain", "--untracked-files=all"]),
        "?? .codex/config.toml\n"
    );
}

#[test]
fn an_untrusted_project_is_refused_and_a_plain_folder_needs_no_exclude() {
    let dir = fixture("untrusted");
    let codex = adapter(Agent::Codex);
    let place = Place::Project {
        path: dir.to_path_buf(),
        shared: false,
    };
    let steps = codex.set_plugin("x@lib", &place, true).unwrap();
    let err = apply(codex, &roots(&dir), &trusting(&[]), &steps, &[], None).unwrap_err();
    assert_eq!(err, ApplyError::Untrusted("Codex"));
    assert_eq!(
        err.to_string(),
        "Codex only reads this project's settings once you trust the project."
    );
    assert!(!dir.join(".codex").exists());

    // Not a git checkout: written, nothing else.
    plugin_on(&dir, false).unwrap();
    assert!(codex::project_file(&dir).exists());
}

#[cfg(unix)]
const STUB: &str = "brigadier-apply-stub";

/// A folder with a stub CLI in it: it prints its arguments, folder and Codex home, says why on
/// stderr and fails for `fail`, and leaves a mark in its folder for `mark`.
#[cfg(unix)]
fn stub(dir: &Path) -> Vec<(String, String)> {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let script = bin.join(STUB);
    std::fs::write(
        &script,
        "#!/bin/sh\n\
         if [ \"$1\" = fail ]; then echo 'warming up' >&2; echo 'error: it broke' >&2; echo 'Usage: stub' >&2; exit 3; fi\n\
         if [ \"$1\" = mark ]; then touch ran; fi\n\
         if [ \"$1\" = again ]; then echo '{\"outcome\":\"failed\",\"alreadyInGoalState\":true}'; exit 1; fi\n\
         echo \"args=$* cwd=$(pwd) home=$CODEX_HOME\"\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    vec![(
        "PATH".to_owned(),
        format!("{}:/usr/bin:/bin", bin.display()),
    )]
}

#[cfg(unix)]
fn call(args: &[&str], cwd: &Path) -> CliCall {
    CliCall {
        program: STUB,
        args: key(args),
        cwd: Some(cwd.to_owned()),
        needs_terminal: false,
    }
}

#[test]
#[cfg(unix)]
fn a_run_step_runs_the_cli_with_its_folder_and_home() {
    let dir = fixture("run");
    let env = stub(&dir);
    let codex = adapter(Agent::Codex);
    let roots = roots(&dir);
    let catalog = Catalog::default();

    let applied = apply(
        codex,
        &roots,
        &catalog,
        &[Step::Run(call(&["plugin", "add", "x@lib"], &dir))],
        &env,
        None,
    )
    .unwrap();
    assert_eq!(
        applied.output,
        [format!(
            "args=plugin add x@lib cwd={} home={}\n",
            dir.display(),
            dir.join("codex-home").display()
        )]
    );

    let err = apply(
        codex,
        &roots,
        &catalog,
        &[Step::Run(call(&["fail"], &dir))],
        &env,
        None,
    )
    .unwrap_err();
    assert_eq!(err, ApplyError::Cli("error: it broke".to_owned()));

    // Asked for what already holds (a plugin on already): done, not failed.
    apply(
        codex,
        &roots,
        &catalog,
        &[Step::Run(call(&["again"], &dir))],
        &env,
        None,
    )
    .unwrap();

    // A step that needs a terminal stops the plan before anything runs.
    let mut login = call(&["mcp", "login", "s"], &dir);
    login.needs_terminal = true;
    let err = apply(
        codex,
        &roots,
        &catalog,
        &[Step::Run(call(&["mark"], &dir)), Step::Run(login)],
        &env,
        None,
    )
    .unwrap_err();
    assert_eq!(
        err,
        ApplyError::NeedsTerminal(format!("{STUB} mcp login s"))
    );
    assert!(!dir.join("ran").exists());

    let missing = CliCall {
        program: "brigadier-apply-no-such-cli",
        ..call(&[], &dir)
    };
    let err = apply(codex, &roots, &catalog, &[Step::Run(missing)], &env, None).unwrap_err();
    assert!(err.to_string().contains("couldn't find"), "{err}");
}

#[test]
fn an_agent_without_a_config_writer_refuses_a_user_config_step() {
    let dir = fixture("no-writer");
    let claude = adapter(Agent::Claude);
    let err = apply(
        claude,
        &roots(&dir),
        &Catalog::default(),
        &[Step::UserConfig(set(
            &["plugins", "x@lib", "enabled"],
            true,
        ))],
        &[],
        None,
    )
    .unwrap_err();
    assert!(matches!(err, ApplyError::Failed(_)), "{err:?}");
}

/// The real `codex` on PATH, in a throwaway Codex home.
#[test]
#[ignore = "runs the real codex"]
fn codex_writes_the_user_config_through_its_own_writer() {
    let dir = fixture("live-codex");
    let roots = roots(&dir);
    std::fs::create_dir_all(&roots.codex_home).unwrap();
    let config = roots.codex_home.join("config.toml");
    std::fs::write(&config, "# mine\nmodel = \"o3\"\n").unwrap();
    let codex = adapter(Agent::Codex);
    let catalog = Catalog::default();

    let on = [Step::UserConfig(set(&["plugins", "x@y", "enabled"], false))];
    apply(codex, &roots, &catalog, &on, &[], None).unwrap();
    let written = std::fs::read_to_string(&config).unwrap();
    println!("after the write:\n{written}");
    assert!(written.starts_with("# mine\nmodel = \"o3\"\n"), "{written}");
    assert!(
        written.contains("[plugins.\"x@y\"]\nenabled = false\n"),
        "{written}"
    );

    let off = [Step::UserConfig(unset(&["plugins", "x@y", "enabled"]))];
    apply(codex, &roots, &catalog, &off, &[], None).unwrap();
    let removed = std::fs::read_to_string(&config).unwrap();
    println!("after the removal:\n{removed}");
    assert!(!removed.contains("x@y"), "{removed}");
    assert!(removed.starts_with("# mine\nmodel = \"o3\"\n"), "{removed}");
}

#[test]
#[cfg(unix)]
fn a_project_file_it_cannot_edit_stops_the_plan_first() {
    let dir = fixture("broken-file");
    let env = stub(&dir);
    let file = codex::project_file(&dir);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, "broken = [\n").unwrap();
    let steps = [
        Step::Run(call(&["mark"], &dir)),
        Step::ProjectFile {
            file: file.clone(),
            edit: set(&["plugins", "x@lib", "enabled"], true),
            shared: false,
        },
    ];
    let codex = adapter(Agent::Codex);
    let err = apply(codex, &roots(&dir), &trusting(&[&dir]), &steps, &env, None).unwrap_err();
    assert!(err.to_string().contains("isn't valid TOML"), "{err}");
    assert!(!dir.join("ran").exists());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "broken = [\n");
}

/// The agent's CLI by the path sessions use (chosen in Settings, or where its installer put
/// it), not its bare name on PATH: for its commands and for its config writer.
#[test]
#[cfg(unix)]
fn the_agents_own_cli_runs_from_its_resolved_path() {
    let dir = fixture("resolved");
    let chosen = dir.join("elsewhere").join("my-codex");
    std::fs::create_dir_all(chosen.parent().unwrap()).unwrap();
    std::fs::write(
        &chosen,
        "#!/bin/sh\necho \"$*\" >> \"$(dirname \"$0\")/ran\"\necho chosen\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&chosen, std::fs::Permissions::from_mode(0o755)).unwrap();
    // No `codex` on this PATH.
    let env = vec![("PATH".to_owned(), "/usr/bin:/bin".to_owned())];
    let codex = adapter(Agent::Codex);
    let roots = roots(&dir);

    let applied = apply(
        codex,
        &roots,
        &Catalog::default(),
        &[Step::Run(codex.add_marketplace(&dir))],
        &env,
        Some(&chosen),
    )
    .unwrap();
    assert_eq!(applied.output, ["chosen\n"]);

    // Its config writer too (the stand-in answers nothing, so the write itself fails).
    let _ = apply(
        codex,
        &roots,
        &Catalog::default(),
        &[Step::UserConfig(set(
            &["plugins", "x@lib", "enabled"],
            true,
        ))],
        &env,
        Some(&chosen),
    );
    let ran = std::fs::read_to_string(dir.join("elsewhere").join("ran")).unwrap();
    assert_eq!(
        ran.lines().collect::<Vec<_>>(),
        [
            format!("plugin marketplace add {}", dir.display()),
            "app-server".to_owned()
        ]
    );
}
