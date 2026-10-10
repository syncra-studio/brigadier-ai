//! Free up space, end to end on a real session and repository: what a scan offers, what it
//! keeps and why, and that cleaning removes only what it listed, still as it was listed.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use brigadier_providers::{Artifact, ProviderKind};
use brigadier_sandbox::removal;
use serde_json::json;

use super::{Action, AgentHome, Cleaned, ScanContext, ScanItem, ScanUsage};
use crate::manager::flow::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{Environment, Lifecycle, Setup};
use crate::storage::CleanCategory;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A session whose first turn made its work folder; the folder and its branch.
async fn session(name: &str) -> (Flow, PathBuf, String) {
    let flow = Flow::start(
        name,
        Options::default(),
        script(|_| async { Reply::text("Looked.") }),
    )
    .await;
    flow.say("Look around.").await;
    flow.settled().await;
    let (worktree, branch) = match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path),
                    branch,
                    ..
                },
            ..
        }) => (PathBuf::from(path), branch),
        other => panic!("no session worktree: {other:?}"),
    };
    (flow, worktree, branch)
}

/// Archives the session without its cleanup, as if its work folder had been left behind.
async fn leave_behind(flow: &Flow, worktree: &Path) {
    flow.core
        .set_lifecycle(flow.conversation.clone(), Lifecycle::Archived)
        .await
        .unwrap();
    let ledger = flow.manager.runtime.ledger();
    for (owner, artifacts, _) in ledger.owners() {
        for artifact in artifacts {
            if matches!(&artifact, Artifact::Worktree { path, .. } if Path::new(path) == worktree) {
                ledger.unrecord(&owner, artifact).await.unwrap();
            }
        }
    }
}

/// A scan that never looks in the real CLI homes.
fn context() -> ScanContext {
    ScanContext {
        agent_homes: Some(Vec::new()),
        ..ScanContext::default()
    }
}

async fn scan(flow: &Flow, context: ScanContext) -> (Vec<ScanItem>, ScanUsage) {
    flow.manager.scan_storage(context).await.unwrap()
}

fn labels(items: &[ScanItem]) -> Vec<String> {
    items.iter().map(|item| item.item.label.clone()).collect()
}

fn named<'a>(items: &'a [ScanItem], label: &str) -> &'a ScanItem {
    items
        .iter()
        .find(|item| item.item.label.contains(label))
        .unwrap_or_else(|| panic!("no item “{label}” in {:?}", labels(items)))
}

/// Cleans every item the sweep takes, as Free up space's one button does.
async fn sweep(flow: &Flow, items: &[ScanItem], context: ScanContext) -> Vec<Cleaned> {
    let mut done = Vec::new();
    for item in items
        .iter()
        .filter(|item| item.item.checked && item.item.selectable)
    {
        done.push(
            flow.manager
                .clean_storage(item.action.clone(), context.clone())
                .await
                .unwrap_or_else(|err| panic!("{}: {err}", item.item.label)),
        );
    }
    done
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    !git(repo, &["branch", "--list", branch]).trim().is_empty()
}

/// A folder outside every allowed root, removed when dropped.
struct Outside(PathBuf);

impl Outside {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-disk-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }
}

impl Drop for Outside {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Sets `path`'s, and everything in it's, modification time back by `by`.
fn age(path: &Path, by: Duration) {
    let at = SystemTime::now() - by;
    if std::fs::symlink_metadata(path).is_ok_and(|meta| meta.is_dir()) {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            age(&entry.path(), by);
        }
    }
    std::fs::File::open(path).unwrap().set_modified(at).unwrap();
}

/// An open session's work folder is never offered: it is counted as kept.
#[tokio::test]
async fn an_open_sessions_work_folder_is_kept_and_counted() {
    let (flow, worktree, _) = session("disk-open").await;
    let (items, usage) = scan(&flow, context()).await;
    assert!(
        items
            .iter()
            .all(|item| item.item.path.as_deref() != Some(worktree.to_str().unwrap())),
        "{:?}",
        labels(&items)
    );
    let kept = usage
        .kept
        .iter()
        .find(|line| line.category == CleanCategory::FinishedWork)
        .expect("a counted line for open sessions");
    // The session's work folder, and the one pre-made for its next worker.
    assert!(kept.label.contains("of an open session") || kept.label.contains("of open sessions"));
    flow.stop().await;
}

/// An ended session's clean work folder is in the sweep and goes through git; its branch,
/// merged, goes too, but only while it is where it was listed.
#[tokio::test]
async fn an_ended_clean_work_folder_goes_through_git_and_a_moved_branch_stays() {
    let (flow, worktree, branch) = session("disk-ended").await;
    leave_behind(&flow, &worktree).await;
    let (items, _) = scan(&flow, context()).await;
    let folder = named(&items, "Work folder of “Flow”");
    assert_eq!(folder.item.category, CleanCategory::FinishedWork);
    assert!(folder.item.checked && folder.item.selectable);
    assert!(matches!(folder.action, Action::RemoveWorktree { .. }));
    let cleaned = flow
        .manager
        .clean_storage(folder.action.clone(), context())
        .await
        .unwrap();
    assert!(cleaned.failures.is_empty());
    assert!(!worktree.exists());
    // git removing the folder never takes its branch.
    assert!(branch_exists(&flow.repo, &branch));

    // Now the branch, merged, is offered; it moves before the clean, so it stays.
    let (items, _) = scan(&flow, context()).await;
    let listed = named(&items, &format!("Branch {branch}"));
    assert!(listed.item.checked && listed.item.selectable);
    std::fs::write(flow.repo.join("NOTES.md"), "more\n").unwrap();
    git(&flow.repo, &["add", "NOTES.md"]);
    git(&flow.repo, &["commit", "-q", "-m", "More"]);
    git(&flow.repo, &["branch", "-f", &branch, "main"]);
    let refused = flow
        .manager
        .clean_storage(listed.action.clone(), context())
        .await;
    assert!(
        refused
            .as_ref()
            .is_err_and(|err| err.contains("changed since")),
        "{refused:?}"
    );
    assert!(branch_exists(&flow.repo, &branch));
    flow.stop().await;
}

/// A work folder with unsaved changes is never in the sweep, and the sweep leaves it byte
/// for byte; picked by hand, its changes are first kept as a commit on its branch, which stays.
#[tokio::test]
async fn unsaved_changes_are_kept_by_the_sweep_and_committed_before_a_removal_by_hand() {
    let (flow, worktree, branch) = session("disk-dirty").await;
    std::fs::write(worktree.join("draft.txt"), "half a thought\n").unwrap();
    std::fs::write(worktree.join("README.md"), "# Flow, edited\n").unwrap();
    leave_behind(&flow, &worktree).await;
    let status = git(&worktree, &["status", "--porcelain"]);
    let (items, _) = scan(&flow, context()).await;
    let folder = named(&items, "Work folder of “Flow”");
    assert!(!folder.item.checked, "a dirty folder is not in the sweep");
    assert!(folder.item.selectable);
    assert!(
        folder.item.reason.contains(&branch),
        "{}",
        folder.item.reason
    );

    sweep(&flow, &items, context()).await;
    assert_eq!(
        std::fs::read_to_string(worktree.join("draft.txt")).unwrap(),
        "half a thought\n"
    );
    assert_eq!(
        std::fs::read_to_string(worktree.join("README.md")).unwrap(),
        "# Flow, edited\n"
    );
    assert_eq!(git(&worktree, &["status", "--porcelain"]), status);

    let (items, _) = scan(&flow, context()).await;
    let folder = named(&items, "Work folder of “Flow”");
    flow.manager
        .clean_storage(folder.action.clone(), context())
        .await
        .unwrap();
    assert!(!worktree.exists());
    assert!(branch_exists(&flow.repo, &branch));
    assert_eq!(
        git(&flow.repo, &["show", &format!("{branch}:draft.txt")]).trim(),
        "half a thought"
    );
    flow.stop().await;
}

/// A branch with commits its target doesn't have is kept, never offered.
#[tokio::test]
async fn an_unmerged_branch_is_kept_with_why() {
    let (flow, worktree, branch) = session("disk-unmerged").await;
    std::fs::write(worktree.join("work.txt"), "done\n").unwrap();
    git(&worktree, &["add", "work.txt"]);
    git(&worktree, &["commit", "-q", "-m", "Work"]);
    leave_behind(&flow, &worktree).await;
    git(
        &flow.repo,
        &["worktree", "remove", worktree.to_str().unwrap()],
    );
    let (items, _) = scan(&flow, context()).await;
    let kept = named(&items, &format!("Branch {branch}"));
    assert!(!kept.item.selectable && !kept.item.checked);
    assert_eq!(kept.item.reason, "Has 1 commit not merged into main.");
    // Cleaning it anyway (an id it isn't offered under) is refused by the daemon's pick; the
    // session manager can't remove what has no removal.
    assert!(matches!(kept.action, Action::External(_)));
    sweep(&flow, &items, context()).await;
    assert!(branch_exists(&flow.repo, &branch));
    flow.stop().await;
}

/// Binding refuses `..`, paths outside the root and links below it; a link in the data
/// directory is never followed, and a folder swapped for a link after the scan stays.
#[tokio::test]
async fn links_and_paths_outside_the_data_directory_are_never_followed() {
    let (flow, _, _) = session("disk-links").await;
    let data = flow.manager.data_dir.clone();
    let outside = Outside::new("target");
    std::fs::write(outside.0.join("precious.txt"), "keep\n").unwrap();

    assert!(removal::bind(&data, &data.join("scratch").join("..").join("..")).is_err());
    assert!(removal::bind(&data, &outside.0).is_err());
    let scratch = data.join("scratch");
    std::fs::create_dir_all(&scratch).unwrap();
    let link = scratch.join("old-task-link");
    std::os::unix::fs::symlink(&outside.0, &link).unwrap();
    assert!(removal::bind(&data, &link.join("precious.txt")).is_err());

    // A working folder no record claims, left for a while: offered.
    let left = scratch.join("old-task");
    std::fs::create_dir_all(&left).unwrap();
    std::fs::write(left.join("out.txt"), "x\n").unwrap();
    age(&left, Duration::from_secs(2 * 60 * 60));
    let (items, _) = scan(&flow, context()).await;
    let folders = named(&items, "working folder");
    assert_eq!(folders.item.category, CleanCategory::Temporary);
    assert!(
        !labels(&items).iter().any(|label| label.contains("link")),
        "{:?}",
        labels(&items)
    );
    // Swapped for a link to the outside before the clean: it stays, and so does the target.
    std::fs::remove_dir_all(&left).unwrap();
    std::os::unix::fs::symlink(&outside.0, &left).unwrap();
    let cleaned = flow
        .manager
        .clean_storage(folders.action.clone(), context())
        .await
        .unwrap();
    assert!(!cleaned.failures.is_empty());
    assert_eq!(
        std::fs::read_to_string(outside.0.join("precious.txt")).unwrap(),
        "keep\n"
    );
    assert!(link.exists() && left.exists());
    std::fs::remove_file(&link).unwrap();
    std::fs::remove_file(&left).unwrap();
    flow.stop().await;
}

/// Build output git ignores goes from an idle session's work folder; tracked folders, small
/// ones, a recently used session, a running process, and output git tracks by the time of
/// the clean all stay.
#[tokio::test]
async fn idle_sessions_build_files_go_only_while_git_ignores_them() {
    let (flow, worktree, _) = session("disk-build").await;
    let exclude = flow.repo.join(".git").join("info").join("exclude");
    std::fs::write(&exclude, "node_modules/\ntarget/\n").unwrap();
    let big = vec![7u8; 1_500_000];
    let put = |rel: &str| {
        let path = worktree.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &big).unwrap();
    };
    put("node_modules/pkg/index.bin");
    put("target/debug/app.bin");
    std::fs::write(
        worktree.join("target").join("CACHEDIR.TAG"),
        "Signature: 8a477f597d28d172789f06886806bc55\n",
    )
    .unwrap();
    put("vendor/node_modules/kept.bin");
    git(&worktree, &["add", "-f", "vendor/node_modules/kept.bin"]);
    git(&worktree, &["commit", "-q", "-m", "Vendor"]);
    std::fs::create_dir_all(worktree.join("web").join("node_modules")).unwrap();
    std::fs::write(worktree.join("web/node_modules/tiny.txt"), "x").unwrap();

    let idle = ScanContext {
        build_idle: Some(Duration::ZERO),
        ..context()
    };
    // Used recently: counted as kept.
    let recent = ScanContext {
        build_idle: Some(Duration::from_secs(24 * 60 * 60)),
        ..context()
    };
    let (items, usage) = scan(&flow, recent).await;
    assert!(
        items
            .iter()
            .all(|item| item.item.category != CleanCategory::BuildFiles)
    );
    assert!(
        usage
            .kept
            .iter()
            .any(|line| line.category == CleanCategory::BuildFiles)
    );

    // Something running in the work folder: not offered.
    let mut running = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&worktree)
        .spawn()
        .unwrap();
    let (items, _) = scan(&flow, idle.clone()).await;
    running.kill().unwrap();
    running.wait().unwrap();
    assert!(
        items
            .iter()
            .all(|item| item.item.category != CleanCategory::BuildFiles),
        "{:?}",
        labels(&items)
    );

    let (items, usage) = scan(&flow, idle.clone()).await;
    let build = named(&items, "Build files in “Flow”");
    assert!(build.item.checked && build.item.selectable);
    // The open session's work folder is kept, without counting its offered build files again.
    let open = usage
        .kept
        .iter()
        .find(|line| line.label.contains("open session"))
        .expect("a counted line for open sessions");
    let whole = removal::allocated_size(&worktree);
    assert!(
        open.bytes <= whole - build.item.bytes + 512 * 1024,
        "kept {} of {whole}, build files {}",
        open.bytes,
        build.item.bytes
    );
    assert!(
        build.item.reason.contains("(node_modules, target)"),
        "{}",
        build.item.reason
    );
    // Tracked after the scan: that folder stays, the other goes.
    git(&worktree, &["add", "-f", "target/debug/app.bin"]);
    let cleaned = flow
        .manager
        .clean_storage(build.action.clone(), idle)
        .await
        .unwrap();
    assert_eq!(cleaned.failures.len(), 1, "{:?}", cleaned.failures);
    assert!(
        cleaned.failures[0].starts_with("target: "),
        "{:?}",
        cleaned.failures
    );
    assert!(!worktree.join("node_modules").exists());
    assert!(worktree.join("target/debug/app.bin").exists());
    assert!(worktree.join("vendor/node_modules/kept.bin").exists());
    assert!(worktree.join("web/node_modules/tiny.txt").exists());
    flow.stop().await;
}

fn lines(values: &[serde_json::Value]) -> String {
    values.iter().map(|value| format!("{value}\n")).collect()
}

/// A Claude transcript in `home` of `id` run in `cwd`, started as `entrypoint`; `ours`
/// adds a call of Brigadier's tools.
fn transcript(home: &Path, cwd: &Path, id: &str, entrypoint: &str, ours: bool) -> PathBuf {
    let cwd = cwd.display().to_string();
    let folder = home.join("projects").join(
        cwd.chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>(),
    );
    std::fs::create_dir_all(&folder).unwrap();
    let mut records = vec![json!({"type": "user", "entrypoint": entrypoint, "cwd": cwd,
        "sessionId": id, "message": {"role": "user", "content": "Start the task."}})];
    if ours {
        records.push(json!({"type": "assistant", "message": {"content": [
            {"type": "tool_use", "name": "mcp__brigadier__report", "input": {}}]}}));
    }
    let path = folder.join(format!("{id}.jsonl"));
    std::fs::write(&path, lines(&records)).unwrap();
    for part in ["session-env", "file-history"] {
        std::fs::create_dir_all(home.join(part).join(id)).unwrap();
        std::fs::write(home.join(part).join(id).join("state"), "x").unwrap();
    }
    path
}

/// A Codex rollout in `home` of `id` run in `cwd`, started by `originator`.
fn rollout(home: &Path, cwd: &Path, id: &str, originator: &str) -> PathBuf {
    let day = home.join("sessions").join("2026").join("10").join("10");
    std::fs::create_dir_all(&day).unwrap();
    let path = day.join(format!("rollout-2026-10-10T00-00-00-{id}.jsonl"));
    let meta = json!({"type": "session_meta", "payload": {
        "id": id, "cwd": cwd.display().to_string(), "originator": originator}});
    std::fs::write(&path, lines(&[meta])).unwrap();
    path
}

/// The CLIs' files of Brigadier's ended sessions go, proven by Brigadier's mark or its record
/// of starting them; the user's own sessions, those of an open conversation, fresh ones and
/// unmarked ones never show. Codex threads go through Codex's own delete.
#[tokio::test]
async fn agent_files_go_only_with_proof_once_what_they_served_is_gone() {
    let (flow, _, _) = session("disk-agents").await;
    let data = flow.manager.data_dir.clone();
    let homes = Outside::new("homes");
    let (claude, codex) = (homes.0.join(".claude"), homes.0.join(".codex"));
    let gone = data.join("orch").join("0000-deleted-conversation");
    let open = data.join("orch").join(&flow.conversation.0);
    let ids = |n: u8| format!("{n}{n}{n}{n}{n}{n}{n}{n}-1111-4111-8111-111111111111");
    let marked = transcript(&claude, &gone, &ids(1), "sdk-cli", true);
    let users = transcript(&claude, &gone.join("mine"), &ids(2), "cli", true);
    let of_open = transcript(&claude, &open, &ids(3), "sdk-cli", true);
    let unmarked = transcript(&claude, &gone.join("plain"), &ids(4), "sdk-cli", false);
    let recorded = transcript(&claude, &gone.join("recorded"), &ids(5), "sdk-cli", false);
    let other_dir = homes.0.join("removed-data").join("scratch").join("t1");
    let elsewhere = transcript(&claude, &other_dir, &ids(6), "sdk-cli", true);
    let ours = rollout(
        &codex,
        &data.join("scratch").join("t9"),
        "01a1-ours",
        "brigadier",
    );
    let theirs = rollout(
        &codex,
        &data.join("scratch").join("t9"),
        "01a1-user",
        "codex_exec",
    );
    age(&homes.0, Duration::from_secs(2 * 60 * 60));
    let fresh = transcript(&claude, &gone.join("fresh"), &ids(7), "sdk-cli", true);
    // Brigadier recorded starting this one; its record of the session is long gone.
    let ledger = flow.manager.runtime.ledger();
    let session = Artifact::ClaudeSession {
        session_id: ids(5),
        home: None,
        cwd: Some(gone.join("recorded").display().to_string()),
    };
    ledger.record("raw:old", session.clone()).await.unwrap();
    ledger.unrecord("raw:old", session).await.unwrap();

    let homes_context = ScanContext {
        agent_homes: Some(vec![
            AgentHome {
                kind: ProviderKind::Claude,
                history: claude.clone(),
                homes: vec![claude.clone()],
            },
            AgentHome {
                kind: ProviderKind::Codex,
                history: codex.clone(),
                homes: vec![codex.clone()],
            },
        ]),
        ..ScanContext::default()
    };
    let (items, usage) = scan(&flow, homes_context.clone()).await;
    let agent: Vec<&ScanItem> = items
        .iter()
        .filter(|item| item.item.category == CleanCategory::AgentFiles)
        .collect();
    let names: Vec<&str> = agent.iter().map(|item| item.item.label.as_str()).collect();
    assert!(
        names.contains(&"Claude Code files of 2 ended sessions"),
        "{names:?}"
    );
    assert!(
        names.contains(&"Codex files of 1 ended session"),
        "{names:?}"
    );
    assert!(
        names
            .iter()
            .any(|name| name.starts_with("Claude Code files of 1 session of a removed Brigadier")),
        "{names:?}"
    );
    assert_eq!(agent.len(), 3, "{names:?}");
    assert!(
        usage
            .kept
            .iter()
            .any(|line| line.label == "1 agent session of an open conversation")
    );

    let swept = sweep(&flow, &items, homes_context).await;
    assert!(
        swept.iter().all(|cleaned| cleaned.failures.is_empty()),
        "{swept:?}"
    );
    for gone in [&marked, &recorded, &elsewhere] {
        assert!(!gone.exists(), "{}", gone.display());
    }
    for id in [ids(1), ids(5)] {
        assert!(!claude.join("session-env").join(&id).exists());
        assert!(!claude.join("file-history").join(&id).exists());
    }
    for stays in [&users, &of_open, &unmarked, &fresh, &theirs] {
        assert!(stays.exists(), "{}", stays.display());
    }
    assert!(claude.join("session-env").join(ids(3)).exists());
    // The Codex thread went through Codex's own delete, by its id.
    let removals = flow.behavior.removals.lock().unwrap().clone();
    assert!(removals.iter().any(|(kind, _, artifacts)| {
        *kind == ProviderKind::Codex
            && artifacts.iter().any(|artifact| {
                matches!(artifact, Artifact::CodexThread { thread_id, .. } if thread_id == "01a1-ours")
            })
    }));
    assert!(
        !removals.iter().flat_map(|(_, _, artifacts)| artifacts).any(|artifact| {
            matches!(artifact, Artifact::CodexThread { thread_id, .. } if thread_id == "01a1-user")
        })
    );
    // Nothing is left in the ledger for the sweep.
    assert!(
        ledger
            .owners()
            .iter()
            .all(|(owner, _, _)| !owner.starts_with("sweep:"))
    );
    let _ = ours;
    flow.stop().await;
}
