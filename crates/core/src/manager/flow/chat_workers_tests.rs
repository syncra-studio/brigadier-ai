//! STRIP-DOWN-PLAN S28: a Chat is a brigade. Its `brigadier start` makes a worker of the Chat,
//! shown in its view and in `brigadier workers`: in the repository `--repo` names (a worktree of
//! it, or its checkout with `--local`), or without one in a folder of its own with no git. A
//! worker still starts no workers of its own.

use std::path::PathBuf;
use std::sync::Arc;

use brigadier_cli::protocol::{CliRequest, GRANT_ENV, StartRequest};
use serde_json::{Value, json};

use super::chat_thread_tests::{answered, new_chat};
use super::cli_tests::{Answers, brigadier, id_of, start, wait_for};
use super::{Flow, Options, Reply, Script};
use crate::tools::Role;

fn started(repo: Option<&str>, local: bool, base: Option<&str>, prompt: &str) -> CliRequest {
    CliRequest::Start(Box::new(StartRequest {
        repo: repo.map(Into::into),
        local,
        base: base.map(Into::into),
        ..start(prompt)
    }))
}

/// The Chat starts three workers in the repository its message names and waits for them;
/// each worker tries to start one of its own.
fn chat_script(answers: Answers) -> Script {
    Arc::new(move |turn| {
        let answers = answers.clone();
        Box::pin(async move {
            let keep = |name: &str, value: Value| {
                answers.lock().unwrap().insert(name.into(), value);
            };
            let worker = turn
                .env(GRANT_ENV)
                .and_then(|grant| turn.host.grants().resolve(grant));
            if let Some(Role::Worker { task_id, .. }) = worker {
                let own = brigadier(&turn, started(None, false, None, "More hands.")).await;
                keep(&format!("worker start {}", task_id.0), own);
                keep(&format!("worker cwd {}", task_id.0), json!(turn.cwd));
                return Reply::text("Done.");
            }
            let Some(repo) = turn
                .input
                .split_once("Work in ")
                .map(|(_, repo)| repo.trim())
            else {
                return Reply::text("ok");
            };
            keep(
                "local alone",
                brigadier(&turn, started(None, true, None, "Go.")).await,
            );
            keep(
                "base alone",
                brigadier(&turn, started(None, false, Some("main"), "Go.")).await,
            );
            keep(
                "not a repo",
                brigadier(&turn, started(Some("/no/such/repo"), false, None, "Go.")).await,
            );
            let mut ids = Vec::new();
            for (name, request) in [
                ("folder", started(None, false, None, "Write notes.")),
                ("worktree", started(Some(repo), false, None, "Fix a bug.")),
                ("local", started(Some(repo), true, None, "Read the code.")),
            ] {
                let answer = brigadier(&turn, request).await;
                ids.push(id_of(&answer));
                keep(name, answer);
            }
            let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
            keep("events", json!(wait_for(&turn, &ids, 3).await));
            keep("workers", brigadier(&turn, CliRequest::Workers).await);
            Reply::text("Started three.")
        })
    })
}

#[tokio::test]
async fn a_chats_start_makes_workers_of_the_chat_with_or_without_a_repository() {
    let answers = Answers::default();
    let flow = Flow::start(
        "chat-workers",
        Options::default(),
        chat_script(answers.clone()),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, &format!("Work in {}", flow.repo.display()))
        .await;
    answered(&flow, &chat).await;
    let answers = answers.lock().unwrap().clone();

    assert!(
        answers["local alone"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("give --repo"),
        "{:#}",
        answers["local alone"]
    );
    assert!(
        answers["base alone"]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("give --repo")
    );
    assert_eq!(answers["not a repo"]["error"]["code"], "not-found");

    let tasks = flow.core.tasks(&chat).await.unwrap();
    assert_eq!(tasks.len(), 3, "{tasks:#?}");
    let task = |name: &str| {
        let id = id_of(&answers[name]);
        tasks
            .iter()
            .find(|task| task.id.0.ends_with(&id))
            .unwrap_or_else(|| panic!("{name}: no task {id}"))
            .clone()
    };
    let cwd = |name: &str| {
        let task = task(name);
        PathBuf::from(
            answers[&format!("worker cwd {}", task.id.0)]
                .as_str()
                .unwrap(),
        )
    };
    let workspace = |name: &str| -> Value {
        let task = task(name);
        let file = flow
            .manager
            .session_dir(&chat)
            .worker(&task)
            .join("record/workspace.json");
        serde_json::from_str(&std::fs::read_to_string(file).unwrap()).unwrap()
    };

    // No repository: a folder of its own, with no git.
    let folder = flow
        .manager
        .session_dir(&chat)
        .worker(&task("folder"))
        .join("work");
    assert_eq!(
        cwd("folder").canonicalize().unwrap(),
        folder.canonicalize().unwrap()
    );
    assert!(!folder.join(".git").exists());
    assert_eq!(workspace("folder")["environment"], "folder");
    assert!(answers["folder"]["base"].is_null());

    // `--repo`: a worktree of it on its own branch, from the branch it has out.
    let worktree = task("worktree");
    let branch = worktree
        .workspace
        .as_ref()
        .and_then(|w| w.branch.clone())
        .expect("its own branch");
    let repo = flow.manager.git.open(&flow.repo).unwrap();
    assert!(repo.branch_tip(&branch).unwrap().is_some(), "{branch}");
    let current = repo.current_branch().unwrap().unwrap();
    assert_eq!(answers["worktree"]["base"], json!(current));
    assert_eq!(workspace("worktree")["environment"], "newWorktree");
    assert_ne!(
        cwd("worktree").canonicalize().unwrap(),
        flow.repo.canonicalize().unwrap()
    );

    // `--repo --local`: that checkout itself.
    assert_eq!(
        cwd("local").canonicalize().unwrap(),
        flow.repo.canonicalize().unwrap()
    );
    assert_eq!(workspace("local")["environment"], "localCheckout");
    assert_eq!(workspace("local")["branch"], json!(current));

    // Each is the Chat's worker: its view's rows and `brigadier workers` list them all.
    let listed = answers["workers"]["workers"].as_array().unwrap();
    assert_eq!(listed.len(), 3, "{:#}", answers["workers"]);
    let view = flow.manager.thread_view_with_workers(&chat).await.unwrap();
    assert_eq!(view.workers.len(), 3, "{:#?}", view.workers);
    // None of them is the session's.
    assert!(
        flow.core
            .tasks(&flow.conversation)
            .await
            .unwrap()
            .is_empty()
    );
    // Every turn ended, heard by the Chat's `wait`.
    assert_eq!(answers["events"].as_array().unwrap().len(), 3);

    // A worker starts no workers of its own.
    for task in &tasks {
        let own = &answers[&format!("worker start {}", task.id.0)];
        assert_eq!(own["error"]["code"], "refused", "{own:#}");
    }
    flow.stop().await;
}

/// A session's worker works in the session's own repository: `--repo` is a Chat's.
#[tokio::test]
async fn a_session_is_refused_repo() {
    let answers = Answers::default();
    let script: Script = {
        let answers = answers.clone();
        Arc::new(move |turn| {
            let answers = answers.clone();
            Box::pin(async move {
                let answer = brigadier(&turn, started(Some("/tmp"), false, None, "Go.")).await;
                answers.lock().unwrap().insert("repo".into(), answer);
                Reply::text("ok")
            })
        })
    };
    let flow = Flow::start("session-repo", Options::default(), script).await;
    flow.say("Start one elsewhere.").await;
    flow.settled().await;
    let answer = answers.lock().unwrap()["repo"].clone();
    assert_eq!(answer["error"]["code"], "refused", "{answer:#}");
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("--repo is for a Chat's worker")
    );
    flow.stop().await;
}

/// A Chat's CLI runs as a session's at the same level, on both CLIs: the same tools, access
/// (Claude's permission mode, Codex's sandbox and approval policy come from these) and user
/// setup, in the Chat's own folder, with `record/` beside it and `TMPDIR` in its `tmp/`.
#[tokio::test]
async fn a_chats_cli_runs_as_a_sessions_at_its_level() {
    use crate::model::{ConversationKind, ModelChoice, PermissionLevel, SetupRequest};
    use brigadier_providers::ProviderKind;

    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        for level in [
            PermissionLevel::Ask,
            PermissionLevel::AcceptEdits,
            PermissionLevel::Full,
        ] {
            let flow = Flow::start(
                "chat-level",
                Options {
                    permission: level,
                    thread: provider,
                    ..Options::default()
                },
                Arc::new(|_| Box::pin(async { Reply::text("ok") })),
            )
            .await;
            flow.say("Hello.").await;
            flow.settled().await;
            let chat = flow
                .manager
                .create_conversation(
                    ConversationKind::Chat,
                    None,
                    Some("Chat".into()),
                    Some(SetupRequest::Chat {
                        model: ModelChoice {
                            provider,
                            model: None,
                            effort: None,
                            fast: None,
                            account: None,
                        },
                        permission: level,
                    }),
                )
                .await
                .unwrap()
                .id;
            flow.say_to(&chat, "Hello too.").await;
            answered(&flow, &chat).await;
            let specs = flow.specs();
            let session = &flow.thread_specs()[0].1;
            let folder = flow.manager.session_dir(&chat);
            let (of, spec) = specs
                .iter()
                .find(|(_, spec)| spec.cwd == folder.work())
                .expect("the Chat started in its folder");
            assert_eq!(*of, provider);
            let label = format!("{provider:?} {level:?}");
            assert_eq!(spec.access, session.access, "{label}");
            assert_eq!(spec.access, level.access(), "{label}");
            assert_eq!(spec.tools, session.tools, "{label}");
            assert!(spec.keeps_detached && session.keeps_detached, "{label}");
            assert!(spec.tools.user_setup(), "{label}");
            assert_eq!(spec.add_dirs, vec![folder.record()], "{label}");
            let tmp = spec
                .env
                .iter()
                .find(|(key, _)| key == "TMPDIR")
                .map(|(_, value)| PathBuf::from(value));
            assert!(
                tmp.is_some_and(|tmp| tmp.starts_with(folder.tmp())),
                "{label}: {:?}",
                spec.env
            );
            flow.stop().await;
        }
    }
}

/// #573: deleting a Chat removes its `--repo` workers' CLI session files, as deleting a
/// session removes its workers' (G15).
#[tokio::test]
async fn deleting_a_chat_removes_its_repo_workers_session_files() {
    deleting_a_chat_removes_its_repo_workers_files(false).await;
}

/// The cleanup ledger survives a daemon restart even though a Chat has no project of its own.
#[tokio::test]
async fn deleting_a_chat_after_restart_removes_its_repo_workers_session_files() {
    deleting_a_chat_removes_its_repo_workers_files(true).await;
}

async fn deleting_a_chat_removes_its_repo_workers_files(restart: bool) {
    let answers = Answers::default();
    let script: Script = {
        let answers = answers.clone();
        Arc::new(move |turn| {
            let answers = answers.clone();
            Box::pin(async move {
                let worker = turn
                    .env(GRANT_ENV)
                    .and_then(|grant| turn.host.grants().resolve(grant));
                if matches!(worker, Some(Role::Worker { .. })) {
                    return Reply::text("Seven.");
                }
                let Some(repo) = turn.input.split_once("Count in ").map(|(_, r)| r.trim()) else {
                    return Reply::text("ok");
                };
                let mut ids = Vec::new();
                for local in [false, true] {
                    let answer = brigadier(&turn, started(Some(repo), local, None, "Count.")).await;
                    ids.push(id_of(&answer));
                }
                let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                let events = json!(wait_for(&turn, &ids, 2).await);
                answers.lock().unwrap().insert("events".into(), events);
                Reply::text("Counted.")
            })
        })
    };
    let mut flow = Flow::start(
        "chat-workers-delete",
        Options {
            behavior: Arc::new(super::FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            ..Options::default()
        },
        script,
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, &format!("Count in {}", flow.repo.display()))
        .await;
    answered(&flow, &chat).await;
    let tasks = flow.core.tasks(&chat).await.unwrap();
    assert_eq!(tasks.len(), 2, "{:#?}", answers.lock().unwrap());
    let own = flow.dir.join("data/own/claude");
    let files: Vec<PathBuf> = tasks
        .iter()
        .flat_map(|task| super::fake_files(&own, task.native_session.as_ref().unwrap()))
        .collect();
    assert!(files.iter().all(|file| file.exists()), "{files:#?}");
    if restart {
        flow.restart().await;
        assert!(
            files.iter().all(|file| file.exists()),
            "restart keeps transcripts"
        );
    }
    flow.manager.delete(chat.clone()).await.unwrap();
    flow.manager.cleanup_finished(&chat).await;
    for file in files {
        assert!(!file.exists(), "{} survived", file.display());
    }
    flow.stop().await;
}

/// A Chat's worker with no repository works in a folder of its own, which Archive removes: after
/// Restore it reads reported but is closed, never offered for more work in a folder that is gone.
#[tokio::test]
async fn a_chats_folder_worker_is_closed_after_archive_and_restore() {
    let script: Script = Arc::new(move |turn| {
        Box::pin(async move {
            let worker = turn
                .env(GRANT_ENV)
                .and_then(|grant| turn.host.grants().resolve(grant));
            if matches!(worker, Some(Role::Worker { .. })) {
                return Reply::text("Notes written.");
            }
            if !turn.input.contains("Write notes") {
                return Reply::text("ok");
            }
            let answer = brigadier(&turn, started(None, false, None, "Write notes.")).await;
            wait_for(&turn, &[id_of(&answer).as_str()], 1).await;
            Reply::text("Written.")
        })
    });
    let flow = Flow::start("chat-folder-worker-archive", Options::default(), script).await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Write notes").await;
    answered(&flow, &chat).await;
    let task = flow.core.tasks(&chat).await.unwrap().remove(0);
    assert_eq!(task.state, crate::work::TaskState::Reported);
    let folder = PathBuf::from(task.workspace.as_ref().unwrap().worktree.clone().unwrap());
    assert!(folder.exists());
    flow.manager.archive(chat.clone()).await.unwrap();
    flow.manager.cleanup_finished(&chat).await;
    assert!(!folder.exists(), "its folder goes with the archive");
    flow.manager.restore(chat.clone()).await.unwrap();
    let task = flow.core.tasks(&chat).await.unwrap().remove(0);
    assert_eq!(task.state, crate::work::TaskState::Reported);
    assert!(task.plain.as_ref().unwrap().closed, "{task:#?}");
    flow.stop().await;
}
