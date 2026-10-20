//! The Source panel's branch and stash menu (#257): Create Branch (From), Rename, Delete,
//! Rebase and Stash on a project's folder; a session never switches or renames its branch,
//! Brigadier's branches are never deleted, a rebase of a session's branch survives the ref
//! check (one that stops on conflicts is resolved commit by commit, #283), and Git Output
//! lists what the panel ran, failures too.

use super::{Flow, Options, Reply, git};
use crate::manager::refcheck::Caller;
use crate::model::{Environment, Setup, TaskId};
use crate::work::{OrchestratorStepKind, SourceAction, WorkspaceTarget};

#[tokio::test]
async fn the_source_panel_makes_renames_deletes_rebases_and_stashes() {
    let flow = Flow::start(
        "source-branches",
        Options::default(),
        std::sync::Arc::new(|_turn| Box::pin(async { Reply::text("Done.") })),
    )
    .await;
    let manager = &flow.manager;
    let repo = &flow.repo;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .expect("a session has a project");
    let target = WorkspaceTarget::Project {
        project_id: project,
    };
    let act = |action: SourceAction| manager.source_action(&target, action);

    // Create Branch… from what is checked out, Create Branch From… another; each switches.
    let state = act(SourceAction::CreateBranch {
        name: "topic".into(),
        from: None,
    })
    .await
    .unwrap();
    assert_eq!(state.branch.as_deref(), Some("topic"));
    std::fs::write(repo.join("topic.txt"), "topic\n").unwrap();
    git(repo, &["add", "topic.txt"]);
    git(repo, &["commit", "-qm", "Topic"]);
    let state = act(SourceAction::CreateBranch {
        name: "spare".into(),
        from: Some("main".into()),
    })
    .await
    .unwrap();
    assert_eq!(state.branch.as_deref(), Some("spare"));
    assert!(!repo.join("topic.txt").exists(), "made from main");
    let refused = act(SourceAction::CreateBranch {
        name: "topic".into(),
        from: None,
    })
    .await;
    assert!(refused.is_err(), "an existing branch is never replaced");

    // Rename Branch… renames the checked-out one; Delete Branch… another.
    let state = act(SourceAction::RenameBranch {
        name: "spare-2".into(),
    })
    .await
    .unwrap();
    assert_eq!(state.branch.as_deref(), Some("spare-2"));
    git(repo, &["checkout", "-q", "topic"]);
    act(SourceAction::DeleteBranch {
        name: "spare-2".into(),
        force: false,
    })
    .await
    .unwrap();
    assert!(
        git(repo, &["branch", "--list", "spare-2"])
            .trim()
            .is_empty()
    );

    // Rebase Branch… replays topic onto main once main moved on.
    git(repo, &["checkout", "-q", "main"]);
    std::fs::write(repo.join("main.txt"), "main\n").unwrap();
    git(repo, &["add", "main.txt"]);
    git(repo, &["commit", "-qm", "Main moved"]);
    git(repo, &["checkout", "-q", "topic"]);
    act(SourceAction::Rebase {
        branch: "main".into(),
    })
    .await
    .unwrap();
    assert_eq!(
        git(repo, &["rev-parse", "topic~1"]),
        git(repo, &["rev-parse", "main"])
    );

    // Stash, then Pop Latest Stash; Stash again and Drop Stash….
    std::fs::write(repo.join("README.md"), "# Edited\n").unwrap();
    let state = act(SourceAction::Stash { untracked: false }).await.unwrap();
    assert!(state.changes.is_empty(), "{:?}", state.changes);
    let state = act(SourceAction::ApplyStash { pop: true }).await.unwrap();
    assert!(state.changes.iter().any(|file| file.path == "README.md"));
    act(SourceAction::Stash { untracked: true }).await.unwrap();
    act(SourceAction::DropStash).await.unwrap();
    assert!(git(repo, &["stash", "list"]).trim().is_empty());
    let refused = act(SourceAction::DropStash).await;
    assert!(refused.is_err(), "no stash left");

    // Git Output lists what the panel ran, in order, a refusal too.
    let runs = manager.source_git_output(&target).await.unwrap();
    let commands: Vec<&str> = runs.iter().map(|run| run.command.as_str()).collect();
    let at = |command: &str| {
        commands
            .iter()
            .position(|run| *run == command)
            .unwrap_or_else(|| panic!("{command} in {commands:?}"))
    };
    assert!(
        at("git switch --quiet -c topic")
            < at("git rebase --merge --no-autostash --no-update-refs main")
    );
    assert!(runs.iter().any(|run| {
        run.command == "git switch --quiet -c topic" && !run.ok && run.output.contains("exists")
    }));
    assert!(
        runs.iter()
            .all(|run| !run.command.starts_with("git status"))
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_sessions_branch_stays_its_own_and_its_rebase_is_kept() {
    let flow = Flow::start(
        "source-session-branch",
        Options::default(),
        std::sync::Arc::new(|_turn| Box::pin(async { Reply::text("Done.") })),
    )
    .await;
    flow.say("Hi.").await;
    flow.settled().await;
    let manager = &flow.manager;
    let conversation = flow.core.conversation(&flow.conversation).unwrap();
    let (checkout, branch) = match conversation.setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path),
                    branch,
                    ..
                },
            ..
        }) => (std::path::PathBuf::from(path), branch),
        other => panic!("a session with a worktree, not {other:?}"),
    };
    let session = WorkspaceTarget::Conversation {
        conversation_id: flow.conversation.clone(),
    };
    let project = WorkspaceTarget::Project {
        project_id: conversation.project_id.expect("a project"),
    };

    // A session never switches to or makes another branch, nor renames its own.
    for action in [
        SourceAction::CreateBranch {
            name: "elsewhere".into(),
            from: None,
        },
        SourceAction::RenameBranch {
            name: "elsewhere".into(),
        },
    ] {
        assert!(manager.source_action(&session, action).await.is_err());
    }
    // Its branch is never deleted from the project's folder either.
    let refused = manager
        .source_action(
            &project,
            SourceAction::DeleteBranch {
                name: branch.clone(),
                force: true,
            },
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("a session works on"), "{refused}");
    let refused = manager
        .source_action(
            &project,
            SourceAction::DeleteBranch {
                name: branch.to_uppercase(),
                force: true,
            },
        )
        .await;
    assert!(
        refused.is_err(),
        "a case-blind disk takes its other spelling for it"
    );
    // Nor the base it merges into renamed.
    git(&flow.repo, &["checkout", "-q", "main"]);
    let refused = manager
        .source_action(
            &project,
            SourceAction::RenameBranch {
                name: "trunk".into(),
            },
        )
        .await;
    assert!(refused.is_err(), "main is the session's base");

    // The user's rebase of the session's branch onto a newer main is no damage to put back.
    std::fs::write(checkout.join("work.txt"), "work\n").unwrap();
    git(&checkout, &["add", "work.txt"]);
    git(&checkout, &["commit", "-qm", "Work"]);
    manager.record_session_refs(&flow.conversation, &checkout);
    // The check records the forward move: the branch's tip is Work now.
    manager
        .ref_check(&checkout, Caller::Task(TaskId("other".into())))
        .await;
    let repo = &flow.repo;
    std::fs::write(repo.join("main.txt"), "main\n").unwrap();
    git(repo, &["add", "main.txt"]);
    git(repo, &["commit", "-qm", "Main moved"]);
    manager
        .source_action(
            &session,
            SourceAction::Rebase {
                branch: "main".into(),
            },
        )
        .await
        .unwrap();
    let rebased = git(&checkout, &["rev-parse", "HEAD"]);
    manager
        .ref_check(&checkout, Caller::Task(TaskId("other".into())))
        .await;
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), rebased);
    assert_eq!(
        git(&checkout, &["rev-parse", "HEAD~1"]),
        git(repo, &["rev-parse", "main"])
    );
    flow.stop().await;
}

/// Issue #283: a session's rebase that stops on conflicts stays in progress, the panel's state
/// and conflict tab naming the commit it replays; Continue goes on to the next commit's
/// conflicts and finishes, kept by the ref check; Abort puts the branch back.
#[tokio::test]
async fn a_sessions_rebase_that_conflicts_is_resolved_commit_by_commit() {
    let flow = Flow::start(
        "source-rebase-conflicts",
        Options::default(),
        std::sync::Arc::new(|_turn| Box::pin(async { Reply::text("Done.") })),
    )
    .await;
    flow.say("Hi.").await;
    flow.settled().await;
    let manager = &flow.manager;
    let conversation = flow.core.conversation(&flow.conversation).unwrap();
    let (checkout, branch) = match conversation.setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path),
                    branch,
                    ..
                },
            ..
        }) => (std::path::PathBuf::from(path), branch),
        other => panic!("a session with a worktree, not {other:?}"),
    };
    let session = WorkspaceTarget::Conversation {
        conversation_id: flow.conversation.clone(),
    };
    let act = |action: SourceAction| manager.source_action(&session, action);
    // Two commits on the session's branch, each adding a file main adds its own way.
    for (file, subject) in [("a.txt", "Work a"), ("b.txt", "Work b")] {
        std::fs::write(checkout.join(file), "work\n").unwrap();
        git(&checkout, &["add", file]);
        git(&checkout, &["commit", "-qm", subject]);
    }
    manager.record_session_refs(&flow.conversation, &checkout);
    manager
        .ref_check(&checkout, Caller::Task(TaskId("other".into())))
        .await;
    let repo = &flow.repo;
    for file in ["a.txt", "b.txt"] {
        std::fs::write(repo.join(file), "main\n").unwrap();
    }
    git(repo, &["add", "a.txt", "b.txt"]);
    git(repo, &["commit", "-qm", "Main moved"]);
    let tip = git(&checkout, &["rev-parse", &branch]);
    let rebase = || {
        act(SourceAction::Rebase {
            branch: "main".into(),
        })
    };
    let dir = checkout.as_path();
    let resolve = |file: &'static str| async move {
        let text = std::fs::read_to_string(dir.join(file)).unwrap();
        act(SourceAction::Resolve {
            path: file.into(),
            expected: Some(text),
            text: "main\nwork\n".into(),
            stage: true,
        })
        .await
        .unwrap()
    };

    let state = rebase().await.unwrap();
    let rebasing = state.rebasing.clone().expect("a rebase in progress");
    assert_eq!(rebasing.branch.as_deref(), Some(branch.as_str()));
    assert_eq!(rebasing.onto, "main");
    assert_eq!((rebasing.step, rebasing.total), (1, 2));
    assert_eq!(rebasing.message, "Work a");
    assert_eq!(state.branch, None, "HEAD is detached meanwhile");
    assert!(state.changes.iter().any(|file| file.path == "a.txt"));
    assert_eq!(
        git(&checkout, &["rev-parse", &branch]),
        tip,
        "not moved yet"
    );
    let file = manager
        .conflict_file(&session, "a.txt".into())
        .await
        .unwrap();
    assert_eq!(file.rebasing, Some(rebasing.clone()));
    assert!(
        file.current_label.ends_with("• main"),
        "{}",
        file.current_label
    );
    assert_eq!(
        file.incoming_label,
        format!("{} • {branch}", rebasing.commit)
    );
    let refused = act(SourceAction::ContinueRebase).await.unwrap_err();
    assert!(refused.to_string().contains("resolve the rebase conflicts"));
    // Nothing else runs into the half-done rebase: a commit says to finish it first.
    let refused = manager
        .commit_changes(&flow.conversation, Some("x".into()), false, false, false)
        .await
        .unwrap_err();
    assert!(refused.to_string().contains("continue or abort the rebase"));

    // Resolved, Continue stops on the next commit's conflicts; Abort puts the branch back.
    resolve("a.txt").await;
    let state = act(SourceAction::ContinueRebase).await.unwrap();
    let rebasing = state.rebasing.expect("stopped again");
    assert_eq!((rebasing.step, rebasing.message.as_str()), (2, "Work b"));
    let state = act(SourceAction::AbortRebase).await.unwrap();
    assert_eq!(state.rebasing, None);
    assert_eq!(state.branch.as_deref(), Some(branch.as_str()));
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), tip);

    // Each stop resolved, the last Continue finishes; the ref check keeps the rebase.
    rebase().await.unwrap();
    resolve("a.txt").await;
    act(SourceAction::ContinueRebase).await.unwrap();
    resolve("b.txt").await;
    let state = act(SourceAction::ContinueRebase).await.unwrap();
    assert_eq!(state.rebasing, None);
    assert_eq!(state.branch.as_deref(), Some(branch.as_str()));
    let rebased = git(&checkout, &["rev-parse", "HEAD"]);
    manager
        .ref_check(&checkout, Caller::Task(TaskId("other".into())))
        .await;
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), rebased);
    assert_eq!(
        git(&checkout, &["rev-parse", "HEAD~2"]),
        git(repo, &["rev-parse", "main"])
    );
    flow.stop().await;
}

/// A pull request squash-merged on GitHub with its branch kept (#294): the session's branch
/// starts from the merge, so a Push of new work is no fast-forward of the kept copy. Push
/// replaces it only while it is still exactly the merged head; never with no merge recorded,
/// nor over anything pushed to it since.
#[tokio::test]
async fn push_replaces_the_kept_branch_of_a_squash_merged_pull_request_and_nothing_else() {
    let flow = Flow::start(
        "source-push-squashed",
        Options::default(),
        std::sync::Arc::new(|_turn| Box::pin(async { Reply::text("Done.") })),
    )
    .await;
    flow.say("Hi.").await;
    flow.settled().await;
    let manager = &flow.manager;
    let repo = &flow.repo;
    let (checkout, branch) = match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path),
                    branch,
                    ..
                },
            ..
        }) => (std::path::PathBuf::from(path), branch),
        other => panic!("a session with a worktree, not {other:?}"),
    };
    let session = WorkspaceTarget::Conversation {
        conversation_id: flow.conversation.clone(),
    };
    let remote = repo.parent().unwrap().join("origin.git");
    git(
        repo.parent().unwrap(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(repo, &["push", "-q", "-u", "origin", "main"]);

    // The session's work goes up as a pull request, which GitHub squash-merges into main.
    std::fs::write(checkout.join("work.txt"), "work\n").unwrap();
    git(&checkout, &["add", "work.txt"]);
    git(&checkout, &["commit", "-qm", "Work"]);
    git(&checkout, &["push", "-q", "-u", "origin", &branch]);
    let head = git(&checkout, &["rev-parse", "HEAD"]);
    git(repo, &["merge", "-q", "--squash", &branch]);
    git(repo, &["commit", "-qm", "Work (#7)"]);
    git(repo, &["push", "-q", "origin", "main"]);
    let squashed = git(repo, &["rev-parse", "HEAD"]);
    // The session's branch moves onto the merge, and new work follows.
    git(&checkout, &["reset", "-q", "--hard", &squashed]);
    std::fs::write(checkout.join("more.txt"), "more\n").unwrap();
    git(&checkout, &["add", "more.txt"]);
    git(&checkout, &["commit", "-qm", "More"]);
    manager.record_session_refs(&flow.conversation, &checkout);
    let more = git(&checkout, &["rev-parse", "HEAD"]);
    let on_github = || git(&remote, &["rev-parse", &format!("refs/heads/{branch}")]);

    // No merge recorded: the plain push's refusal stands.
    let push = || manager.source_action(&session, SourceAction::Push);
    assert!(push().await.is_err());
    assert_eq!(on_github(), head);

    manager
        .orchestrator_step_in(
            &flow.conversation,
            None,
            OrchestratorStepKind::Merged {
                branch: branch.clone(),
                base: "main".into(),
                commits: 1,
                asked_in: None,
                upstream: None,
                head: Some(head.clone()),
                merge_commit: Some(squashed.clone()),
                pr: Some(7),
                auto: false,
                requests: Vec::new(),
                record: None,
            },
        )
        .await;
    // Someone pushed to the kept branch since: that isn't replaced.
    let theirs = git(
        repo,
        &[
            "commit-tree",
            &format!("{head}^{{tree}}"),
            "-p",
            &head,
            "-m",
            "Theirs",
        ],
    );
    git(
        repo,
        &[
            "push",
            "-q",
            "origin",
            &format!("{theirs}:refs/heads/{branch}"),
        ],
    );
    assert!(push().await.is_err());
    assert_eq!(on_github(), theirs);

    // Still the merged head: Push replaces it with the new work.
    git(
        repo,
        &[
            "push",
            "-q",
            "-f",
            "origin",
            &format!("{head}:refs/heads/{branch}"),
        ],
    );
    push().await.unwrap();
    assert_eq!(on_github(), more);
    flow.stop().await;
}

/// Commit & Sync after a pull request GitHub squash-merged with its branch kept (#331): the
/// session's branch starts from the merge and the kept branch is still the merged head, so
/// Sync doesn't pull the merged commits back; its push replaces the kept branch.
#[tokio::test]
async fn sync_after_a_squash_merge_pulls_nothing_back_and_pushes() {
    let flow = Flow::start(
        "source-sync-squashed",
        Options::default(),
        std::sync::Arc::new(|_turn| Box::pin(async { Reply::text("Done.") })),
    )
    .await;
    flow.say("Hi.").await;
    flow.settled().await;
    let manager = &flow.manager;
    let repo = &flow.repo;
    let (checkout, branch) = match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path),
                    branch,
                    ..
                },
            ..
        }) => (std::path::PathBuf::from(path), branch),
        other => panic!("a session with a worktree, not {other:?}"),
    };
    let session = WorkspaceTarget::Conversation {
        conversation_id: flow.conversation.clone(),
    };
    let remote = repo.parent().unwrap().join("origin.git");
    git(
        repo.parent().unwrap(),
        &["init", "-q", "--bare", remote.to_str().unwrap()],
    );
    git(repo, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(repo, &["push", "-q", "-u", "origin", "main"]);

    std::fs::write(checkout.join("work.txt"), "work\n").unwrap();
    git(&checkout, &["add", "work.txt"]);
    git(&checkout, &["commit", "-qm", "Work"]);
    git(&checkout, &["push", "-q", "-u", "origin", &branch]);
    let head = git(&checkout, &["rev-parse", "HEAD"]);
    git(repo, &["merge", "-q", "--squash", &branch]);
    git(repo, &["commit", "-qm", "Work (#7)"]);
    git(repo, &["push", "-q", "origin", "main"]);
    let squashed = git(repo, &["rev-parse", "HEAD"]);
    git(&checkout, &["reset", "-q", "--hard", &squashed]);
    std::fs::write(checkout.join("more.txt"), "more\n").unwrap();
    git(&checkout, &["add", "more.txt"]);
    git(&checkout, &["commit", "-qm", "More"]);
    manager.record_session_refs(&flow.conversation, &checkout);
    let more = git(&checkout, &["rev-parse", "HEAD"]);
    manager
        .orchestrator_step_in(
            &flow.conversation,
            None,
            OrchestratorStepKind::Merged {
                branch: branch.clone(),
                base: "main".into(),
                commits: 1,
                asked_in: None,
                upstream: None,
                head: Some(head.clone()),
                merge_commit: Some(squashed.clone()),
                pr: Some(7),
                auto: false,
                requests: Vec::new(),
                record: None,
            },
        )
        .await;

    let state = manager
        .source_action(&session, SourceAction::Sync)
        .await
        .unwrap();
    assert!(state.merging.is_none(), "{state:?}");
    // Nothing pulled: the branch is the merge and the new work, no merge of the kept commits.
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), more);
    assert!(
        !git(&checkout, &["log", "--format=%H"]).contains(&head),
        "the merged commits came back"
    );
    assert_eq!(
        git(&remote, &["rev-parse", &format!("refs/heads/{branch}")]),
        more
    );
    flow.stop().await;
}

/// Issue #342: while the user's rebase stopped on conflicts in the session's checkout, the
/// thread is told so, its git commands are refused (its `run`, and a Claude thread's `Bash`
/// through its hook) and others run; once the rebase is aborted it hears git is its own again.
#[tokio::test]
async fn the_thread_runs_no_git_into_the_users_rebase() {
    use std::sync::{Arc, Mutex};
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let script: super::Script = {
        let seen = seen.clone();
        Arc::new(move |turn| {
            let seen = seen.clone();
            Box::pin(async move {
                let ran = if turn.input.contains("Commit it") {
                    let reply = turn
                        .call(
                            "run",
                            serde_json::json!({"command": "git commit --allow-empty -qm Mine"}),
                        )
                        .await;
                    format!(
                        "{}{}",
                        if reply.is_error { "error: " } else { "" },
                        reply.text
                    )
                } else {
                    String::new()
                };
                seen.lock().unwrap().push((turn.input.clone(), ran));
                Reply::text("Done.")
            })
        })
    };
    let flow = Flow::start("source-rebase-thread", Options::default(), script).await;
    flow.say("Hi.").await;
    flow.settled().await;
    let manager = &flow.manager;
    let conversation = flow.core.conversation(&flow.conversation).unwrap();
    let checkout = match conversation.setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path: Some(path), ..
                },
            ..
        }) => std::path::PathBuf::from(path),
        other => panic!("a session with a worktree, not {other:?}"),
    };
    let session = WorkspaceTarget::Conversation {
        conversation_id: flow.conversation.clone(),
    };
    std::fs::write(checkout.join("a.txt"), "work\n").unwrap();
    git(&checkout, &["add", "a.txt"]);
    git(&checkout, &["commit", "-qm", "Work a"]);
    manager.record_session_refs(&flow.conversation, &checkout);
    std::fs::write(flow.repo.join("a.txt"), "main\n").unwrap();
    git(&flow.repo, &["add", "a.txt"]);
    git(&flow.repo, &["commit", "-qm", "Main moved"]);
    let state = manager
        .source_action(
            &session,
            SourceAction::Rebase {
                branch: "main".into(),
            },
        )
        .await
        .unwrap();
    assert!(state.rebasing.is_some());
    let hook = flow
        .thread_specs()
        .first()
        .and_then(|(_, spec)| spec.output_hook.clone())
        .and_then(|hook| {
            hook.env
                .into_iter()
                .find(|(key, _)| key == crate::manager::HOOK_GRANT_ENV)
        })
        .map(|(_, grant)| grant)
        .expect("a Claude thread's hook grant");
    let shell = |command: &'static str| {
        let hook = hook.clone();
        async move {
            manager
                .hook_shell(&hook, false, "toolu_1", command, None, false)
                .await
        }
    };
    let refused = shell("git commit -am x").await.expect("refused");
    // Sent to the background, it is refused all the same.
    assert_eq!(
        manager
            .hook_shell(&hook, false, "toolu_2", "git add -A", None, true)
            .await
            .as_deref(),
        Some(refused.as_str())
    );
    assert!(
        refused.starts_with("Not run: the user is rebasing"),
        "{refused}"
    );
    assert_eq!(shell("cargo test").await, None);

    flow.say("Commit it").await;
    flow.settled().await;
    let (input, ran) = seen.lock().unwrap().last().cloned().unwrap();
    assert!(
        input.contains("[workspace] The user is rebasing `")
            && input.contains("` onto `main` in the Source panel"),
        "{input}"
    );
    assert!(
        ran.starts_with("error: Not run: the user is rebasing"),
        "{ran}"
    );
    assert!(git(&checkout, &["log", "-1", "--format=%s"]) != "Mine");

    // Brigadier started again: the rebase is still the user's, by its mark on disk.
    {
        let conv = manager.conv(&flow.conversation).unwrap();
        *conv.user_rebase.lock().unwrap() = None;
        conv.user_rebase_looked
            .store(false, std::sync::atomic::Ordering::SeqCst);
    }
    assert!(shell("git commit -am x").await.is_some());

    // Aborted in a terminal: the thread's next turn hears it ended, and its git runs.
    git(&checkout, &["rebase", "--abort"]);
    flow.say("Commit it").await;
    flow.settled().await;
    let (input, ran) = seen.lock().unwrap().last().cloned().unwrap();
    assert!(
        input.contains("in your checkout ended: git is yours to run there again."),
        "{input}"
    );
    assert!(!ran.starts_with("error:"), "{ran}");
    assert_eq!(git(&checkout, &["log", "-1", "--format=%s"]), "Mine");

    // Aborted in the Source panel, it says so; a rebase of the thread's own is never refused.
    git(&checkout, &["reset", "-q", "--hard", "HEAD~1"]);
    let rebase = || {
        manager.source_action(
            &session,
            SourceAction::Rebase {
                branch: "main".into(),
            },
        )
    };
    assert!(rebase().await.unwrap().rebasing.is_some());
    manager
        .source_action(&session, SourceAction::AbortRebase)
        .await
        .unwrap();
    assert_eq!(shell("git commit -am x").await, None);
    flow.say("Commit it").await;
    flow.settled().await;
    let (input, ran) = seen.lock().unwrap().last().cloned().unwrap();
    assert!(
        input.contains("in your checkout was aborted, so the branch is as it was before it: git is yours to run there again."),
        "{input}"
    );
    assert!(!ran.starts_with("error:"), "{ran}");
    git(&checkout, &["reset", "-q", "--hard", "HEAD~1"]);
    let own = crate::test_git_command()
        .args(["rebase", "main"])
        .current_dir(&checkout)
        .output()
        .unwrap();
    assert!(
        !own.status.success(),
        "the thread's own rebase stops on the conflict"
    );
    assert_eq!(shell("git rebase --abort").await, None);
    git(&checkout, &["rebase", "--abort"]);
    flow.stop().await;
}
