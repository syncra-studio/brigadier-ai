//! The Source panel's branch and stash menu (#257): Create Branch (From), Rename, Delete,
//! Rebase and Stash on a project's folder; a session never switches or renames its branch,
//! Brigadier's branches are never deleted, a rebase of a session's branch survives the ref
//! check, and Git Output lists what the panel ran, failures too.

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
        at("git switch --quiet -c topic") < at("git rebase --no-autostash --no-update-refs main")
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
