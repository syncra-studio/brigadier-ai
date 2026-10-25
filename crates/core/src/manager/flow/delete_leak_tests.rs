//! Delete leaves nothing of a session behind (D1-D8): its folder, worktrees, branches, refs and
//! their config, its CLIs' files (through the real Claude and Codex removers, under a test
//! home), its temp folder and its records, as a leak scan proves. Only the work it was asked
//! to do stays.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use brigadier_cli::protocol::CliRequest;
use brigadier_providers::cli::CliEnv;
use brigadier_providers::{Artifact, Provider, ProviderKind};

use super::{FakeBehavior, Flow, Options, Reply, Script, git};
use crate::model::{
    ConversationId, ConversationKind, Environment, EnvironmentRequest, ModelChoice,
    PermissionLevel, Setup, SetupRequest,
};
use crate::work::{Task, TaskState};

/// The thread starts two workers on "Start workers"; a Local checkout's thread leaves files
/// in its checkout on "Make litter".
fn script() -> Script {
    Arc::new(|turn| {
        Box::pin(async move {
            if turn.is_orchestrator() && turn.input.contains("Start workers") {
                for prompt in ["Read the repository.", "Read it again."] {
                    let value = super::cli_tests::brigadier(
                        &turn,
                        CliRequest::Start(Box::new(super::cli_tests::start(prompt))),
                    )
                    .await;
                    assert!(value.get("error").is_none(), "{value}");
                }
            }
            if turn.is_orchestrator() && turn.input.contains("Make litter") {
                std::fs::create_dir_all(turn.cwd.join("scratch")).unwrap();
                std::fs::write(turn.cwd.join("scratch/notes.md"), "scratch\n").unwrap();
                std::fs::write(turn.cwd.join("shot.png"), "png").unwrap();
                std::fs::write(turn.cwd.join("changed.txt"), "first\n").unwrap();
                std::fs::write(turn.cwd.join("feature.rs"), "fn main() {}\n").unwrap();
            }
            Reply::text("Done.")
        })
    })
}

/// A home for the real CLIs' files, with the files they keep of one session each.
struct CliHomes {
    root: PathBuf,
    claude: PathBuf,
    codex: PathBuf,
    /// A fake `codex` whose app-server deletes a thread's rollout, as Codex's does.
    bin: PathBuf,
    claude_session: String,
    codex_thread: String,
    temp: PathBuf,
}

impl CliHomes {
    fn new(flow: &Flow, short: &str) -> Self {
        let root = flow.scratch().join("home");
        let (claude, codex, bin) = (root.join(".claude"), root.join(".codex"), root.join("bin"));
        for dir in [&claude, &codex, &bin] {
            std::fs::create_dir_all(dir).unwrap();
        }
        write_fake_codex(&bin.join("codex"));
        let temp = PathBuf::from("/tmp").join(format!(
            "brigadier-{short}-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..4]
        ));
        Self {
            root,
            claude,
            codex,
            bin,
            claude_session: uuid::Uuid::new_v4().to_string(),
            codex_thread: uuid::Uuid::now_v7().to_string(),
            temp,
        }
    }

    /// The real adapters, removing from these homes: the ledger's removers from now on.
    fn install(&self, flow: &Flow) {
        let platform = flow.manager.runtime.platform().clone();
        let env = |vars: &[(&str, &Path)]| {
            Arc::new(CliEnv::from_vars(
                vars.iter()
                    .map(|(key, value)| (key.into(), value.as_os_str().to_owned()))
                    .chain([("HOME".into(), self.root.as_os_str().to_owned())]),
            ))
        };
        let claude: Arc<dyn Provider> = Arc::new(brigadier_providers::claude::Claude::new(
            platform.clone(),
            env(&[("CLAUDE_CONFIG_DIR", &self.claude)]),
        ));
        let codex: Arc<dyn Provider> = Arc::new(brigadier_providers::codex::Codex::new(
            platform,
            env(&[("CODEX_HOME", &self.codex), ("PATH", &self.bin)]),
        ));
        flow.manager
            .runtime
            .ledger()
            .set_test_providers([claude, codex]);
    }

    /// Writes what Claude Code and Codex keep of a session that ran in `cwd`, and records it
    /// as their adapters do: its files for `owner`, its temp folder for `temp_owner`.
    async fn write(&self, flow: &Flow, owner: &str, temp_owner: &str, cwd: &Path) {
        let id = &self.claude_session;
        let project = self.claude.join("projects").join(encoded(cwd));
        let files = [
            project.join(format!("{id}.jsonl")),
            project.join(id).join("subagents/agent-a1.jsonl"),
            project.join(id).join("tool-results/r1.txt"),
            self.claude.join("file-history").join(id).join("f1@v1"),
            self.claude.join("session-env").join(id).join("env"),
            self.claude.join("tasks").join(id).join("1.json"),
            self.claude
                .join("todos")
                .join(format!("{id}-agent-{id}.json")),
            self.claude.join("debug").join(format!("{id}.txt")),
        ];
        for file in &files {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, format!("{{\"cwd\":\"{}\"}}\n", cwd.display())).unwrap();
        }
        // The prompt history, another session's line beside it; a killed CLI's registration.
        std::fs::write(
            self.claude.join("history.jsonl"),
            format!(
                "{{\"display\":\"hi\",\"sessionId\":\"other\",\"project\":\"/elsewhere\"}}\n\
                 {{\"display\":\"go\",\"sessionId\":\"{id}\",\"project\":\"{}\"}}\n",
                cwd.display()
            ),
        )
        .unwrap();
        let dead = dead_pid();
        std::fs::create_dir_all(self.claude.join("sessions")).unwrap();
        std::fs::write(
            self.claude.join(format!("sessions/{dead}.json")),
            format!(
                "{{\"pid\":{dead},\"sessionId\":\"{id}\",\"cwd\":\"{}\"}}",
                cwd.display()
            ),
        )
        .unwrap();
        // Its temp folder, as Claude's adapter makes it.
        std::fs::create_dir_all(self.temp.join("claude-501")).unwrap();
        std::fs::write(self.temp.join("screenshot.png"), "png").unwrap();
        // Codex: the rollout, its index line, history line, shell snapshot and log rows.
        let thread = &self.codex_thread;
        let rollout = self.codex.join(format!(
            "sessions/2026/10/25/rollout-2026-10-25T12-00-00-{thread}.jsonl"
        ));
        std::fs::create_dir_all(rollout.parent().unwrap()).unwrap();
        std::fs::write(
            &rollout,
            format!("{{\"type\":\"session_meta\",\"payload\":{{\"id\":\"{thread}\",\"cwd\":\"{}\"}}}}\n", cwd.display()),
        )
        .unwrap();
        std::fs::write(
            self.codex.join("session_index.jsonl"),
            format!("{{\"id\":\"other\"}}\n{{\"id\":\"{thread}\",\"thread_name\":\"x\"}}\n"),
        )
        .unwrap();
        std::fs::write(
            self.codex.join("history.jsonl"),
            format!("{{\"session_id\":\"other\",\"text\":\"a\"}}\n{{\"session_id\":\"{thread}\",\"text\":\"b\"}}\n"),
        )
        .unwrap();
        std::fs::create_dir_all(self.codex.join("shell_snapshots")).unwrap();
        std::fs::write(
            self.codex
                .join(format!("shell_snapshots/{thread}.1761390000.sh")),
            "export A=1\n",
        )
        .unwrap();
        let logs = rusqlite::Connection::open(self.codex.join("logs_2.sqlite")).unwrap();
        logs.execute_batch(&format!(
            "PRAGMA secure_delete = ON;
             CREATE TABLE logs (id INTEGER PRIMARY KEY, thread_id TEXT, message TEXT);
             INSERT INTO logs (thread_id, message) VALUES ('other', 'kept');
             INSERT INTO logs (thread_id, message) VALUES ('{thread}', 'gone');"
        ))
        .unwrap();
        drop(logs);
        let ledger = flow.manager.runtime.ledger();
        let cwd_text = Some(cwd.display().to_string());
        for artifact in [
            Artifact::ClaudeProjectDir {
                path: project.display().to_string(),
            },
            Artifact::ClaudeSession {
                session_id: id.clone(),
                home: None,
                cwd: cwd_text.clone(),
            },
            Artifact::CodexThread {
                thread_id: thread.clone(),
                home: None,
                cwd: cwd_text,
            },
        ] {
            ledger.record(owner, artifact).await.unwrap();
        }
        ledger
            .record(
                temp_owner,
                Artifact::ClaudeTempDir {
                    path: self.temp.display().to_string(),
                },
            )
            .await
            .unwrap();
    }

    /// The rollout of its Codex thread.
    fn rollout(&self) -> PathBuf {
        self.codex.join(format!(
            "sessions/2026/10/25/rollout-2026-10-25T12-00-00-{}.jsonl",
            self.codex_thread
        ))
    }
}

impl Drop for CliHomes {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.temp);
    }
}

/// A Codex whose app-server answers every request and deletes a thread's rollout on
/// `thread/delete`.
fn write_fake_codex(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::write(
        path,
        r#"#!/usr/bin/python3
import glob, json, os, sys
home = os.environ.get("CODEX_HOME", "")
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    if m.get("method") == "thread/delete":
        thread = m["params"]["threadId"]
        for f in glob.glob(os.path.join(home, "sessions", "**", "*" + thread + ".jsonl"), recursive=True):
            os.remove(f)
    if m.get("method") == "initialize":
        result = {"userAgent": "codex/0.164.0", "codexHome": home, "platformFamily": "unix", "platformOs": "macos"}
    else:
        result = {}
    print(json.dumps({"id": m["id"], "result": result}), flush=True)
"#,
    )
    .unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A pid no process has.
fn dead_pid() -> u32 {
    let mut child = std::process::Command::new("/usr/bin/true").spawn().unwrap();
    let pid = child.id();
    child.wait().unwrap();
    pid
}

/// Claude's name for `cwd`'s project folder.
fn encoded(cwd: &Path) -> String {
    cwd.display()
        .to_string()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Everything that names a session: its ids, its folder's path (as Claude encodes it too),
/// its branches, its CLI sessions and its temp folder.
fn needles(flow: &Flow, id: &ConversationId, tasks: &[Task], extra: &[String]) -> Vec<String> {
    let folder = flow.manager.session_dir(id).root().to_path_buf();
    let mut needles: BTreeSet<String> = [
        id.0.clone(),
        id.short().to_owned(),
        folder.display().to_string(),
        encoded(&folder),
    ]
    .into();
    for task in tasks {
        needles.insert(task.id.0.clone());
        needles.extend(task.workspace.as_ref().and_then(|w| w.branch.clone()));
        needles.extend(task.native_session.clone());
    }
    if let Ok(conversation) = flow.core.conversation(id)
        && let Some(Setup::Session {
            environment: Environment::NewWorktree { branch, .. },
            ..
        }) = &conversation.setup
    {
        needles.insert(branch.clone());
    }
    // Every CLI session of this flow is the session's or its workers'.
    needles.extend(flow.manager.runtime.ledger().native_sessions().into_keys());
    needles.extend(extra.iter().cloned());
    needles.into_iter().filter(|n| !n.is_empty()).collect()
}

/// Where a needle shows: file names and contents under `roots` (the fake CLIs' own stores
/// aside: they stand in for the CLIs' files, which the test homes hold), the repository's
/// refs, worktrees and config, and `/tmp`.
fn leak_scan(flow: &Flow, repo: &Path, roots: &[&Path], needles: &[String]) -> Vec<String> {
    let data = flow.scratch().join("data");
    let skip = [
        data.join("transcripts"),
        data.join("own"),
        data.join("accounts"),
    ];
    let mut found = Vec::new();
    let mut check = |place: &str, text: &[u8]| {
        for needle in needles {
            if memchr_find(text, needle.as_bytes()) {
                found.push(format!("{place}: {needle}"));
            }
        }
    };
    fn walk(path: &Path, skip: &[PathBuf], files: &mut Vec<PathBuf>) {
        if skip.iter().any(|s| path == s) {
            return;
        }
        let Ok(meta) = std::fs::symlink_metadata(path) else {
            return;
        };
        if meta.is_dir() {
            if path.file_name().is_some_and(|name| name == "objects")
                && path.parent().is_some_and(|p| p.ends_with(".git"))
            {
                return;
            }
            for entry in std::fs::read_dir(path).into_iter().flatten().flatten() {
                walk(&entry.path(), skip, files);
            }
        } else {
            files.push(path.to_path_buf());
        }
    }
    let mut files = Vec::new();
    for root in roots {
        walk(root, &skip, &mut files);
    }
    for file in files {
        let name = file.display().to_string();
        check(&format!("path {name}"), name.as_bytes());
        if let Ok(bytes) = std::fs::read(&file) {
            check(&format!("in {name}"), &bytes);
        }
    }
    for args in [
        &["for-each-ref", "--format=%(refname)"][..],
        &["worktree", "list", "--porcelain"],
        &["config", "--local", "-l"],
    ] {
        check(
            &format!("git {}", args.join(" ")),
            git(repo, args).as_bytes(),
        );
    }
    for entry in std::fs::read_dir("/tmp").into_iter().flatten().flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("brigadier-") {
            check(&format!("/tmp/{name}"), name.as_bytes());
        }
    }
    found
}

fn memchr_find(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

async fn reported_workers(flow: &Flow, id: &ConversationId, count: usize) -> Vec<Task> {
    super::eventually_async("the workers to report", || async {
        let tasks = flow.core.tasks(id).await.unwrap();
        tasks.len() == count && tasks.iter().all(|t| t.state == TaskState::Reported)
    })
    .await;
    flow.core.tasks(id).await.unwrap()
}

fn options() -> Options {
    Options {
        behavior: Arc::new(FakeBehavior {
            cleanup: true,
            ..FakeBehavior::default()
        }),
        ..Options::default()
    }
}

/// A New worktree session with two workers, each on its own worktree and branch, its turn
/// snapshot refs, a screenshot in its `tmp/`, a branch and a worktree its CLI made in its
/// namespace, and its CLIs' files: once deleted, nothing names it.
#[tokio::test]
async fn a_deleted_worktree_session_leaves_no_trace() {
    let flow = Flow::start(
        "20261025-1221-cleanup-on-delete-w02-leak",
        options(),
        script(),
    )
    .await;
    let id = flow.conversation.clone();
    flow.say("Start workers").await;
    flow.settled().await;
    let tasks = reported_workers(&flow, &id, 2).await;
    let dir = flow.manager.session_dir(&id);
    let work = dir.work();
    assert!(work.join(".git").is_file());
    std::fs::create_dir_all(dir.tmp()).unwrap();
    std::fs::write(dir.tmp().join("screenshot.png"), "png").unwrap();
    std::fs::write(dir.tmp().join("mockup.html"), "<html>").unwrap();
    // What its CLI made in its namespace: a branch, and one in a worktree of its own.
    let space = format!("brigadier/{}/", id.short());
    git(&work, &["branch", &format!("{space}cli-made")]);
    git(
        &work,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            &format!("{space}cli-tree"),
            &dir.root().join("cli-tree").display().to_string(),
        ],
    );
    assert!(git(&flow.repo, &["for-each-ref", "refs/brigadier/s/"]).contains(&id.0));
    let homes = CliHomes::new(&flow, id.short());
    homes
        .write(
            &flow,
            &crate::ledger::lineage_owner(&id.0),
            &format!("orch:{id}"),
            &work,
        )
        .await;
    homes.install(&flow);
    let extra = vec![
        homes.claude_session.clone(),
        homes.codex_thread.clone(),
        homes.temp.display().to_string(),
        format!("{space}cli-made"),
        format!("{space}cli-tree"),
    ];
    let needles = needles(&flow, &id, &tasks, &extra);
    let before = leak_scan(&flow, &flow.repo, &[flow.scratch()], &needles);
    assert!(!before.is_empty(), "the scan sees the session before");
    flow.manager.delete(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    if flow.core.conversation(&id).is_ok() {
        let left = flow
            .manager
            .runtime
            .ledger()
            .dispose(&crate::ledger::lineage_owner(&id.0))
            .await;
        panic!("{left:#?}");
    }
    let leaks = leak_scan(&flow, &flow.repo, &[flow.scratch()], &needles);
    assert!(leaks.is_empty(), "{leaks:#?}");
    // Other sessions' lines stay.
    let history = std::fs::read_to_string(homes.claude.join("history.jsonl")).unwrap();
    assert!(history.contains("\"other\""), "{history}");
    assert_eq!(
        git(&flow.repo, &["branch", "--format=%(refname:short)"]),
        "main"
    );
    flow.stop().await;
}

/// A Local checkout session: the files its turns left uncommitted in the user's checkout go
/// (D5), the work committed stays, and nothing else names it.
#[tokio::test]
async fn a_deleted_local_checkout_session_takes_its_litter_and_leaves_its_work() {
    let mut flow = Flow::start(
        "20261025-1221-cleanup-on-delete-w02-local",
        options(),
        script(),
    )
    .await;
    let id = local_session(&mut flow).await;
    flow.say("Make litter").await;
    flow.settled().await;
    // The user commits the work they want, and changes a file the session made.
    git(&flow.repo, &["add", "feature.rs"]);
    git(&flow.repo, &["commit", "-q", "-m", "Feature"]);
    std::fs::write(flow.repo.join("changed.txt"), "the user's\n").unwrap();
    let tip = git(&flow.repo, &["rev-parse", "HEAD"]);
    let homes = CliHomes::new(&flow, id.short());
    homes
        .write(
            &flow,
            &crate::ledger::lineage_owner(&id.0),
            &format!("orch:{id}"),
            &flow.repo,
        )
        .await;
    homes.install(&flow);
    let files = flow
        .manager
        .preview_delete_files(std::slice::from_ref(&id))
        .await;
    assert_eq!(
        files.iter().collect::<BTreeSet<_>>(),
        ["scratch/notes.md", "shot.png"]
            .iter()
            .map(|f| f.to_string())
            .collect::<Vec<_>>()
            .iter()
            .collect()
    );
    let needles = needles(
        &flow,
        &id,
        &[],
        &[homes.claude_session.clone(), homes.codex_thread.clone()],
    );
    flow.manager.delete(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert!(flow.core.conversation(&id).is_err());
    for gone in ["shot.png", "scratch"] {
        assert!(!flow.repo.join(gone).exists(), "{gone}");
    }
    assert!(flow.repo.join("feature.rs").is_file());
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("changed.txt")).unwrap(),
        "the user's\n"
    );
    assert_eq!(git(&flow.repo, &["rev-parse", "HEAD"]), tip);
    assert_eq!(
        git(&flow.repo, &["status", "--porcelain"]),
        "?? changed.txt"
    );
    let leaks = leak_scan(&flow, &flow.repo, &[flow.scratch()], &needles);
    assert!(leaks.is_empty(), "{leaks:#?}");
    flow.stop().await;
}

/// Makes a Local checkout session on the flow's repository and makes it the flow's.
async fn local_session(flow: &mut Flow) -> ConversationId {
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id;
    flow.conversation = flow
        .manager
        .create_conversation(
            ConversationKind::Session,
            project,
            Some("Local".into()),
            Some(SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: EnvironmentRequest::LocalCheckout {
                    branch: "main".into(),
                    create_from: None,
                },
                permission: PermissionLevel::Full,
                orchestrator: ModelChoice {
                    provider: ProviderKind::Claude,
                    model: Some("claude-opus-5-5".into()),
                    effort: None,
                    fast: None,
                    account: None,
                },
                plan_mode: false,
            }),
        )
        .await
        .unwrap()
        .id;
    flow.conversation.clone()
}

/// The CLI sessions the ledger names for an owner of `id` (its own, its workers').
fn natives_of(flow: &Flow, id: &ConversationId, tasks: &[Task]) -> Vec<String> {
    let ids: Vec<&str> = std::iter::once(id.0.as_str())
        .chain(tasks.iter().map(|task| task.id.0.as_str()))
        .collect();
    flow.manager
        .runtime
        .ledger()
        .owners()
        .into_iter()
        .filter(|(owner, _, _)| ids.iter().any(|id| owner.contains(id)))
        .flat_map(|(_, artifacts, _)| artifacts)
        .filter_map(|artifact| match artifact {
            Artifact::ClaudeSession { session_id, .. } => Some(session_id),
            Artifact::CodexThread { thread_id, .. } => Some(thread_id),
            _ => None,
        })
        .collect()
}

/// A delete cut off by a failure (a CLI's files can't go yet) stays marked, hidden, and the
/// next start finishes it (D4): then nothing names the session.
#[tokio::test]
async fn an_interrupted_delete_is_finished_by_the_next_start() {
    let mut flow = Flow::start(
        "20261025-1221-cleanup-on-delete-w02-retry",
        options(),
        script(),
    )
    .await;
    let id = flow.conversation.clone();
    flow.say("Start workers").await;
    flow.settled().await;
    let tasks = reported_workers(&flow, &id, 2).await;
    let needles = needles(&flow, &id, &tasks, &[]);
    flow.behavior
        .fail_cleanup
        .store(true, std::sync::atomic::Ordering::SeqCst);
    flow.manager.delete(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert!(flow.core.conversation(&id).is_ok_and(|c| c.deleting));
    assert!(flow.manager.session_dir(&id).root().exists());
    flow.behavior
        .fail_cleanup
        .store(false, std::sync::atomic::Ordering::SeqCst);
    flow.crash().await;
    super::eventually("the delete to finish", || {
        flow.core.conversation(&id).is_err()
    })
    .await;
    flow.manager.cleanup_finished(&id).await;
    super::eventually("its records to go", || {
        flow.manager.runtime.ledger().disposing().is_empty()
    })
    .await;
    let leaks = leak_scan(&flow, &flow.repo, &[flow.scratch()], &needles);
    assert!(leaks.is_empty(), "{leaks:#?}");
    flow.stop().await;
}

/// A Local checkout's delete cut off before its files went: a file the user changes before
/// the retry stays, the rest go (D4, D5).
#[tokio::test]
async fn an_interrupted_local_checkout_delete_keeps_a_file_changed_since() {
    let mut flow = Flow::start(
        "20261025-1221-cleanup-on-delete-w02-local-retry",
        options(),
        script(),
    )
    .await;
    let id = local_session(&mut flow).await;
    flow.say("Make litter").await;
    flow.settled().await;
    flow.behavior
        .fail_cleanup
        .store(true, std::sync::atomic::Ordering::SeqCst);
    flow.manager.delete(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert!(flow.core.conversation(&id).is_ok_and(|c| c.deleting));
    assert!(flow.repo.join("shot.png").is_file(), "nothing went yet");
    std::fs::write(flow.repo.join("changed.txt"), "the user's\n").unwrap();
    flow.behavior
        .fail_cleanup
        .store(false, std::sync::atomic::Ordering::SeqCst);
    flow.crash().await;
    super::eventually("the delete to finish", || {
        flow.core.conversation(&id).is_err()
    })
    .await;
    for gone in ["shot.png", "scratch", "feature.rs"] {
        assert!(!flow.repo.join(gone).exists(), "{gone}");
    }
    assert_eq!(
        std::fs::read_to_string(flow.repo.join("changed.txt")).unwrap(),
        "the user's\n"
    );
    flow.stop().await;
}

/// The launch sweep removes what a session deleted before this fix left (its merged branches,
/// their config, its refs and folder), records its unmerged branch for Storage's ask, and
/// leaves an archived session and a branch Brigadier didn't make alone (D3).
#[tokio::test]
async fn the_orphan_sweep_takes_a_deleted_sessions_leftovers_only() {
    let mut flow = Flow::start(
        "20261025-1221-cleanup-on-delete-w02-orphans",
        options(),
        script(),
    )
    .await;
    let archived = flow.conversation.clone();
    flow.say("Hello").await;
    flow.settled().await;
    flow.manager.archive(archived.clone()).await.unwrap();
    flow.manager.cleanup_finished(&archived).await;
    // What Archive left of it (a merged, empty session branch goes with Archive itself).
    let archived_space = format!("brigadier/{}/*", archived.short());
    let archived_branches = git(&flow.repo, &["branch", "--list", &archived_space]);
    let old = flow.add_session("Old", None, ProviderKind::Claude).await;
    flow.say_to(&old, "Start workers").await;
    flow.settled().await;
    let tasks = reported_workers(&flow, &old, 2).await;
    let space = format!("brigadier/{}/", old.short());
    let session_branch = format!("{space}session");
    // Its CLI's branches: one landed on main, one with work that never did.
    git(&flow.repo, &["branch", &format!("{space}landed"), "main"]);
    let tree = git(&flow.repo, &["rev-parse", "HEAD^{tree}"]);
    let wip = git(
        &flow.repo,
        &["commit-tree", &tree, "-p", "HEAD", "-m", "wip"],
    );
    let unmerged = format!("{space}unmerged");
    git(&flow.repo, &["branch", &unmerged, &wip]);
    git(
        &flow.repo,
        &[
            "config",
            &format!("branch.{session_branch}.gh-merge-base"),
            "main",
        ],
    );
    git(&flow.repo, &["branch", "harness/w01", "main"]);
    let mut needles = vec![
        old.0.clone(),
        session_branch.clone(),
        format!("{space}landed"),
    ];
    needles.extend(tasks.iter().map(|t| t.id.0.clone()));
    needles.extend(natives_of(&flow, &old, &tasks));
    // Deleted as Brigadier did before: its record, branches, refs and CLI files left.
    flow.manager.archive(old.clone()).await.unwrap();
    flow.manager.cleanup_finished(&old).await;
    flow.core.forget_conversation(old.clone()).await.unwrap();
    let folder = flow.manager.session_dir(&old).root().to_path_buf();
    assert!(folder.exists());
    assert!(git(&flow.repo, &["for-each-ref", "refs/brigadier/s/"]).contains(&old.0));
    flow.crash().await;
    super::eventually("the sweep to take its leftovers", || !folder.exists()).await;
    super::eventually("its branches to go", || {
        git(&flow.repo, &["branch", "--list", &format!("{space}*")]).trim() == unmerged
    })
    .await;
    assert!(!git(&flow.repo, &["for-each-ref", "refs/brigadier/s/"]).contains(&old.0));
    assert!(!git(&flow.repo, &["config", "--local", "-l"]).contains(&session_branch));
    // Its unmerged work waits for Storage's "not merged" ask.
    let kept = flow.manager.kept_branches().await.unwrap();
    assert!(
        kept.iter().any(|(_, branch)| branch.name == unmerged),
        "{kept:?}"
    );
    // The archived session and the branch Brigadier didn't make are untouched.
    let archived_now = flow.core.conversation(&archived).unwrap();
    assert_eq!(archived_now.lifecycle, crate::model::Lifecycle::Archived);
    assert!(flow.manager.session_dir(&archived).record().is_dir());
    assert!(git(&flow.repo, &["for-each-ref", "refs/brigadier/s/"]).contains(&archived.0));
    assert!(!git(&flow.repo, &["branch", "--list", "harness/w01"]).is_empty());
    assert_eq!(
        git(&flow.repo, &["branch", "--list", &archived_space]),
        archived_branches
    );
    // Once the user lets the unmerged branch go, the next sweep finds nothing of it left.
    git(&flow.repo, &["branch", "-D", &unmerged]);
    let swept = flow.manager.sweep_orphans().await;
    assert!(swept.failures.is_empty(), "{swept:?}");
    super::eventually("its records to go", || {
        flow.manager.runtime.ledger().disposing().is_empty()
    })
    .await;
    needles.push(unmerged);
    let leaks = leak_scan(&flow, &flow.repo, &[flow.scratch()], &needles);
    assert!(leaks.is_empty(), "{leaks:#?}");
    flow.stop().await;
}

/// Archive keeps the session's record and its CLIs' transcripts and removes its tmp/, dev/,
/// logs/ and its CLI's temp folder (S8, S20).
#[tokio::test]
async fn archive_keeps_record_and_transcripts_and_removes_the_rest() {
    let flow = Flow::start(
        "20261025-1221-cleanup-on-delete-w02-archive",
        options(),
        script(),
    )
    .await;
    let id = flow.conversation.clone();
    flow.say("Hello").await;
    flow.settled().await;
    let dir = flow.manager.session_dir(&id);
    for part in [dir.tmp(), dir.dev(), dir.logs()] {
        std::fs::create_dir_all(&part).unwrap();
        std::fs::write(part.join("screenshot.png"), "png").unwrap();
    }
    let homes = CliHomes::new(&flow, id.short());
    homes
        .write(
            &flow,
            &crate::ledger::lineage_owner(&id.0),
            &format!("orch:{id}"),
            &dir.work(),
        )
        .await;
    homes.install(&flow);
    let transcript = homes
        .claude
        .join("projects")
        .join(encoded(&dir.work()))
        .join(format!("{}.jsonl", homes.claude_session));
    flow.manager.archive(id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&id).await;
    assert!(dir.record().is_dir());
    for part in [dir.tmp(), dir.dev(), dir.logs()] {
        assert!(!part.exists(), "{}", part.display());
    }
    assert!(!homes.temp.exists(), "its CLI's temp folder goes");
    assert!(transcript.is_file(), "its Claude transcript stays");
    assert!(homes.rollout().is_file(), "its Codex rollout stays");
    flow.stop().await;
}
