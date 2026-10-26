use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use serde_json::{Value, json};

use super::*;

/// A fresh folder for one test, its symlinks resolved, removed when the test ends.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "brigadier-claude-config-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(std::fs::canonicalize(dir).unwrap())
    }
}

impl std::ops::Deref for Scratch {
    type Target = PathBuf;
    fn deref(&self) -> &PathBuf {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn read(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// A repository at `main` with a linked worktree at `worktree`, laid out as git lays them out.
fn repo_with_worktree(main: &Path, worktree: &Path) {
    std::fs::create_dir_all(main.join(".git/worktrees/wt")).unwrap();
    write(&main.join(".git/worktrees/wt/commondir"), "../..\n");
    write(
        &worktree.join(".git"),
        &format!("gitdir: {}\n", main.join(".git/worktrees/wt").display()),
    );
}

fn flag(folder: &Path, name: &str, off: bool) -> OffFlag {
    OffFlag {
        folder: folder.to_owned(),
        name: name.to_owned(),
        off,
    }
}

const NOW: Duration = Duration::ZERO;

#[test]
fn an_old_installs_config_file_wins_as_claude_code_picks_it() {
    let home = Scratch::new("file");
    assert_eq!(config_file(&home, None), home.join(".claude.json"));
    let dir = home.join("cfg");
    assert_eq!(config_file(&home, Some(&dir)), dir.join(".claude.json"));
    write(&home.join(".claude/.config.json"), "{}");
    assert_eq!(config_file(&home, None), home.join(".claude/.config.json"));
    // Moved, only the moved folder's counts.
    assert_eq!(config_file(&home, Some(&dir)), dir.join(".claude.json"));
    write(&dir.join(".config.json"), "{}");
    assert_eq!(config_file(&home, Some(&dir)), dir.join(".config.json"));
}

#[test]
fn a_subfolder_and_a_linked_worktree_share_the_repositorys_key() {
    let root = Scratch::new("key");
    let main = root.join("repo");
    let worktree = root.join("worktrees/task");
    repo_with_worktree(&main, &worktree);
    std::fs::create_dir_all(main.join("sub/deeper")).unwrap();
    std::fs::create_dir_all(worktree.join("sub")).unwrap();
    assert_eq!(project_key(&main), main);
    assert_eq!(project_key(&main.join("sub/deeper")), main);
    assert_eq!(project_key(&worktree), main);
    assert_eq!(project_key(&worktree.join("sub")), main);
    // A folder outside git is its own; a submodule (no commondir) is its own repository.
    let plain = root.join("plain");
    std::fs::create_dir_all(&plain).unwrap();
    assert_eq!(project_key(&plain), plain);
    let module = main.join("vendor/lib");
    std::fs::create_dir_all(main.join(".git/modules/lib")).unwrap();
    write(&module.join(".git"), "gitdir: ../../.git/modules/lib\n");
    assert_eq!(project_key(&module), module);
    // A folder that isn't there is kept as it is.
    assert_eq!(
        project_key(Path::new("/nowhere/at/all")),
        Path::new("/nowhere/at/all")
    );
}

#[cfg(unix)]
#[test]
fn the_key_resolves_symlinks() {
    let root = Scratch::new("link");
    let real = root.join("real");
    std::fs::create_dir_all(real.join(".git")).unwrap();
    std::os::unix::fs::symlink(&real, root.join("alias")).unwrap();
    assert_eq!(project_key(&root.join("alias")), real);
}

#[test]
fn a_projects_entry_is_read_under_the_repositorys_key_only() {
    let root = Scratch::new("entry");
    let main = root.join("repo");
    let worktree = root.join("wt");
    repo_with_worktree(&main, &worktree);
    let file = root.join(".claude.json");
    // A missing file is an empty entry.
    assert_eq!(
        project_settings(&file, &main),
        Ok(ProjectSettings::default())
    );
    write(
        &file,
        &json!({ "projects": {
            main.display().to_string(): {
                "disabledMcpServers": ["railway", "plugin:copy:railway"],
                "hasTrustDialogAccepted": true,
                "enabledMcpjsonServers": ["team"],
                "disabledMcpjsonServers": ["other"]
            },
            worktree.display().to_string(): { "disabledMcpServers": ["ignored"] }
        }})
        .to_string(),
    );
    let want = ProjectSettings {
        off: vec!["railway".to_owned(), "plugin:copy:railway".to_owned()],
        trusted: true,
        approved: vec!["team".to_owned()],
        denied: vec!["other".to_owned()],
    };
    assert_eq!(project_settings(&file, &main).unwrap(), want);
    // The worktree reads its repository's entry: its own key is ignored, as Claude Code does.
    assert_eq!(project_settings(&file, &worktree).unwrap(), want);
    // An unreadable file is an error, never an empty entry.
    write(&file, "{ not json");
    assert!(
        project_settings(&file, &main)
            .unwrap_err()
            .starts_with("Brigadier couldn't read")
    );
}

#[test]
fn a_team_server_needs_trust_and_approval_and_a_denial_wins() {
    let root = Scratch::new("approval");
    let claude_dir = root.join(".claude");
    let project = root.join("repo");
    std::fs::create_dir_all(&project).unwrap();
    let mut settings = ProjectSettings::default();
    let approval =
        |settings: &ProjectSettings| team_approval(settings, &claude_dir, &project, "team");
    assert_eq!(approval(&settings), Approval::Pending);
    settings.approved.push("team".to_owned());
    // Approved but not trusted: still pending (checked with 2.1.310).
    assert_eq!(approval(&settings), Approval::Pending);
    settings.trusted = true;
    assert_eq!(approval(&settings), Approval::Approved);
    settings.approved.clear();
    write(
        &project.join(".claude/settings.local.json"),
        r#"{"enableAllProjectMcpServers": true}"#,
    );
    assert_eq!(approval(&settings), Approval::Approved);
    write(
        &claude_dir.join("settings.json"),
        r#"{"disabledMcpjsonServers": ["team"]}"#,
    );
    assert_eq!(approval(&settings), Approval::Denied);
    std::fs::remove_file(claude_dir.join("settings.json")).unwrap();
    settings.denied.push("team".to_owned());
    assert_eq!(approval(&settings), Approval::Denied);
}

/// A config file as Claude Code writes it, with values a careless rewrite would change.
fn claude_json(project: &Path) -> String {
    format!(
        r#"{{
  "numStartups": 12,
  "tipsHistory": {{
    "esc\"aped": 1.50,
    "unicode": "café ☕"
  }},
  "mcpServers": {{
    "railway": {{
      "command": "railway",
      "args": [
        "mcp"
      ]
    }}
  }},
  "projects": {{
    "{}": {{
      "allowedTools": [],
      "mcpServers": {{}},
      "disabledMcpServers": [
        "kept"
      ],
      "hasTrustDialogAccepted": true
    }}
  }},
  "zLast": [1, 2, 3]
}}
"#,
        project.display()
    )
}

#[test]
fn a_flag_changes_only_its_list_and_every_other_byte_stays() {
    let root = Scratch::new("splice");
    let project = root.join("repo");
    std::fs::create_dir_all(&project).unwrap();
    let file = root.join(".claude.json");
    let before = claude_json(&project);
    write(&file, &before);

    assert!(set_off_flags(&file, &[flag(&project, "railway", true)], NOW).unwrap());
    let after = std::fs::read_to_string(&file).unwrap();
    assert_eq!(
        after,
        before.replace(
            "\"kept\"\n      ]",
            "\"kept\",\n        \"railway\"\n      ]"
        )
    );
    // Again: nothing to do, and the file isn't written.
    let modified = std::fs::metadata(&file).unwrap().modified().unwrap();
    assert!(!set_off_flags(&file, &[flag(&project, "railway", true)], NOW).unwrap());
    assert_eq!(
        std::fs::metadata(&file).unwrap().modified().unwrap(),
        modified
    );
    // Taken back, the file is as it was.
    assert!(set_off_flags(&file, &[flag(&project, "railway", false)], NOW).unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
    // Taking back a flag that isn't there leaves the file alone.
    assert!(!set_off_flags(&file, &[flag(&project, "absent", false)], NOW).unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before);
}

#[test]
fn a_project_without_an_entry_gets_claude_codes_shape() {
    let root = Scratch::new("new");
    let known = root.join("repo");
    let fresh = root.join("fresh");
    std::fs::create_dir_all(&known).unwrap();
    std::fs::create_dir_all(&fresh).unwrap();
    let file = root.join(".claude.json");
    let before = claude_json(&known);
    write(&file, &before);

    assert!(set_off_flags(&file, &[flag(&fresh, "railway", true)], NOW).unwrap());
    let after = std::fs::read_to_string(&file).unwrap();
    // Everything before the new entry is as it was.
    let tail = before.find("\n  },\n  \"zLast\"").unwrap();
    assert!(after.starts_with(&before[..tail]));
    assert!(after.ends_with("  \"zLast\": [1, 2, 3]\n}\n"));
    let entry = &read(&file)["projects"][fresh.display().to_string()];
    assert_eq!(entry["disabledMcpServers"], json!(["railway"]));
    assert_eq!(entry["hasTrustDialogAccepted"], json!(false));
    assert_eq!(entry["mcpServers"], json!({}));
    // In Claude Code's order.
    let at = |key: &str| after.rfind(&format!("\"{key}\"")).unwrap();
    assert!(at("allowedTools") < at("mcpContextUris"));
    assert!(at("hasClaudeMdExternalIncludesWarningShown") < at("disabledMcpServers"));
    // Taking back a flag of a project with no entry adds none.
    let other = root.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let now = std::fs::read_to_string(&file).unwrap();
    assert!(!set_off_flags(&file, &[flag(&other, "railway", false)], NOW).unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), now);
}

#[test]
fn a_one_line_file_a_missing_file_and_a_worktree_key() {
    let root = Scratch::new("forms");
    let main = root.join("repo");
    let worktree = root.join("wt");
    repo_with_worktree(&main, &worktree);
    let file = root.join(".claude.json");
    write(&file, r#"{"a":1,"projects":{}}"#);
    // A flag for the worktree goes under the repository's key.
    set_off_flags(&file, &[flag(&worktree, "x", true)], NOW).unwrap();
    let got = read(&file);
    assert_eq!(got["a"], json!(1));
    assert_eq!(
        got["projects"][main.display().to_string()]["disabledMcpServers"],
        json!(["x"])
    );
    assert!(!std::fs::read_to_string(&file).unwrap().contains('\n'));

    let missing = root.join("none/.claude.json");
    std::fs::create_dir_all(missing.parent().unwrap()).unwrap();
    set_off_flags(&missing, &[flag(&main, "x", true)], NOW).unwrap();
    assert_eq!(
        read(&missing)["projects"][main.display().to_string()]["disabledMcpServers"],
        json!(["x"])
    );
    // Several flags in one write.
    set_off_flags(
        &file,
        &[
            flag(&main, "y", true),
            flag(&main, "x", false),
            flag(&root.join("elsewhere"), "z", true),
        ],
        NOW,
    )
    .unwrap();
    let got = read(&file);
    assert_eq!(
        got["projects"][main.display().to_string()]["disabledMcpServers"],
        json!(["y"])
    );
    assert_eq!(
        got["projects"][root.join("elsewhere").display().to_string()]["disabledMcpServers"],
        json!(["z"])
    );
}

#[test]
fn a_malformed_file_is_an_error_and_left_alone() {
    let root = Scratch::new("bad");
    let file = root.join(".claude.json");
    for text in [r#"{"projects": {"/x": 3}}"#, "{ broken", "[1, 2]"] {
        write(&file, text);
        assert!(set_off_flags(&file, &[flag(Path::new("/x"), "s", true)], NOW).is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), text);
    }
}

#[test]
fn it_waits_for_claude_codes_lock_and_takes_one_left_behind() {
    let root = Scratch::new("lock");
    let project = root.join("repo");
    std::fs::create_dir_all(&project).unwrap();
    let file = root.join(".claude.json");
    write(&file, &claude_json(&project));
    let lock = root.join(".claude.json.lock");

    // Held by Claude Code for a moment: Brigadier waits, then writes.
    std::fs::create_dir(&lock).unwrap();
    let release = {
        let lock = lock.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            std::fs::remove_dir(&lock).unwrap();
        })
    };
    let start = std::time::Instant::now();
    set_off_flags(&file, &[flag(&project, "a", true)], NOW).unwrap();
    assert!(start.elapsed() >= Duration::from_millis(250));
    release.join().unwrap();
    assert!(!lock.exists());

    // Left behind long ago: taken over.
    std::fs::create_dir(&lock).unwrap();
    std::fs::File::open(&lock)
        .unwrap()
        .set_modified(SystemTime::now() - Duration::from_secs(60))
        .unwrap();
    set_off_flags(&file, &[flag(&project, "b", true)], NOW).unwrap();
    assert!(!lock.exists());
    assert_eq!(
        read(&file)["projects"][project.display().to_string()]["disabledMcpServers"],
        json!(["kept", "a", "b"])
    );
}

#[test]
fn a_writer_that_ignores_the_lock_keeps_its_edits_and_the_flag_stays() {
    let root = Scratch::new("race");
    let project = root.join("repo");
    std::fs::create_dir_all(&project).unwrap();
    let file = root.join(".claude.json");
    write(&file, &claude_json(&project));
    // Like Claude Code's unlocked writes: read the file, change one value, write it whole.
    let writer = {
        let file = file.clone();
        std::thread::spawn(move || {
            // About a hundred writes a second, far more than Claude Code makes.
            for n in 0..60 {
                let mut config = read(&file);
                config["numStartups"] = json!(n);
                let temp = file.with_extension("claude-tmp");
                std::fs::write(&temp, serde_json::to_string_pretty(&config).unwrap()).unwrap();
                std::fs::rename(&temp, &file).unwrap();
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    };
    for name in ["one", "two", "three"] {
        set_off_flags(
            &file,
            &[flag(&project, name, true)],
            Duration::from_millis(20),
        )
        .unwrap();
    }
    writer.join().unwrap();
    let got = read(&file);
    assert_eq!(got["numStartups"], json!(59));
    assert_eq!(
        got["projects"][project.display().to_string()]["disabledMcpServers"],
        json!(["kept", "one", "two", "three"])
    );
}

#[cfg(unix)]
#[test]
fn it_writes_through_a_symlink_and_keeps_the_files_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let root = Scratch::new("perm");
    let project = root.join("repo");
    std::fs::create_dir_all(&project).unwrap();
    let real = root.join("dotfiles/claude.json");
    write(&real, &claude_json(&project));
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    let file = root.join(".claude.json");
    std::os::unix::fs::symlink(&real, &file).unwrap();

    set_off_flags(&file, &[flag(&project, "railway", true)], NOW).unwrap();
    assert!(
        std::fs::symlink_metadata(&file)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        read(&real)["projects"][project.display().to_string()]["disabledMcpServers"],
        json!(["kept", "railway"])
    );
    assert_eq!(
        std::fs::metadata(&real).unwrap().permissions().mode() & 0o777,
        0o600
    );
    // No temporary file is left beside it.
    let left: Vec<_> = std::fs::read_dir(root.join("dotfiles"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(left.len(), 1);
}

/// The installed Claude Code, in a throwaway home: a flag Brigadier writes for a linked
/// worktree switches the server off in the repository, its subfolder and the worktree, and
/// nowhere else.
#[test]
#[ignore = "runs the installed claude"]
fn live_claude_honours_the_flags_brigadier_writes() {
    let root = Scratch::new("live");
    let home = root.join("home");
    let repo = root.join("repo");
    let other = root.join("other");
    std::fs::create_dir_all(repo.join("sub")).unwrap();
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    let path = std::env::var("PATH").unwrap();
    let run = |program: &str, dir: &Path, args: &[&str]| -> String {
        let out = std::process::Command::new(program)
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", &path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr)
    };
    run("git", &repo, &["init", "-q"]);
    run(
        "git",
        &repo,
        &[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "i",
        ],
    );
    let worktree = root.join("wt");
    run(
        "git",
        &repo,
        &["worktree", "add", "-q", &worktree.display().to_string()],
    );
    let file = home.join(".claude.json");
    write(
        &file,
        r#"{"hasCompletedOnboarding": true, "mcpServers": {"probe": {"command": "/usr/bin/true"}}}"#,
    );
    set_off_flags(
        &file,
        &[flag(&worktree, "probe", true)],
        Duration::from_millis(50),
    )
    .unwrap();
    for (dir, off) in [
        (&repo, true),
        (&repo.join("sub"), true),
        (&worktree, true),
        (&other, false),
    ] {
        let listed = run("claude", dir, &["mcp", "list"]);
        let line = listed
            .lines()
            .find(|line| line.starts_with("probe:"))
            .unwrap_or_else(|| panic!("no probe in {}: {listed}", dir.display()));
        assert_eq!(
            line.contains("Disabled for this project"),
            off,
            "{}: {line}",
            dir.display()
        );
    }
}

#[cfg(unix)]
#[test]
fn a_replaced_lock_stops_the_writer_without_removing_someone_elses_lock() {
    let root = Scratch::new("compromised-lock");
    let file = root.join(".claude.json");
    write(&file, "{\"keep\":true}");
    let lock = root.join(".claude.json.lock");
    let error = rewrite_locked(&file, |_| {
        std::fs::rename(&lock, root.join("old-lock")).unwrap();
        std::fs::create_dir(&lock).unwrap();
        Ok(Some("{\"keep\":false}".to_owned()))
    })
    .unwrap_err();
    assert!(error.contains("lock changed"));
    assert!(lock.is_dir());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "{\"keep\":true}");
}

#[test]
fn an_empty_existing_file_is_an_error_never_a_lost_flag_or_a_fresh_config() {
    let root = Scratch::new("empty-config");
    let file = root.join(".claude.json");
    write(&file, "");
    assert!(project_settings(&file, &root).is_err());
    assert!(set_off_flags(&file, &[flag(&root, "kept", true)], NOW).is_err());
    assert_eq!(std::fs::read_to_string(file).unwrap(), "");
}
