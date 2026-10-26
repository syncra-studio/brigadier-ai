//! Merging is the user's act (BUILD-PLAN A3, A4): their click on "Merge into main" or their own
//! words, as the thread judges them; Brigadier checks no consent. Merge cards are retired: an
//! upgrade closes open merge cards. The session keeps its workspace through a merge, and an old
//! post-merge removal is taken back.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use super::thread_tests::{commit_in_workspace, merge_with_git, on_main};
use super::{Flow, Options, Reply, Script, Turn, click, git};
use crate::model::{CardId, DomainEvent, Environment, Lifecycle, Setup};
use crate::now_ms;
use crate::work::{AnswerInput, OrchestratorStepKind, Question, QuestionKind, RunState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

type Log = Arc<Mutex<Vec<String>>>;

/// A session whose thread commits NOTES.md on "Add notes.", says once that it is ready, and
/// merges with plain git on [merge]. Every thread input and what it did are logged.
async fn session(name: &str) -> (Flow, Log) {
    let log: Log = Arc::default();
    let said = log.clone();
    let flow = Flow::start(
        name,
        Options::default(),
        script(move |turn| {
            let said = said.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                said.lock().unwrap().push(turn.input.clone());
                let log = |text: String| said.lock().unwrap().push(text);
                if turn.input.contains("Ask about the merge.") {
                    return Reply::text("Ready to merge when you are.");
                }
                if turn.input.contains("[merge] The user clicked") {
                    log(format!("merged {}", merge_with_git(&turn)));
                    return Reply::text("Merged.");
                }
                if turn.input.contains("Add more notes.") {
                    let head = commit_in_workspace(&turn, "MORE.md", "more\n", "Add more");
                    log(format!("worked in: {}", turn.cwd.display()));
                    log(format!("head: {head}"));
                    return Reply::text("More notes are in.");
                }
                if turn.input.contains("Draft it.") {
                    std::fs::write(turn.cwd.join("DRAFT.md"), "draft\n").unwrap();
                    return Reply::text("Drafted, not committed.");
                }
                // Last: a resumed thread's input may carry the earlier messages too.
                if turn.input.contains("Add notes.") {
                    commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                    return Reply::text("Notes are in, ready to merge when you are.");
                }
                Reply::text("Noted.")
            }
        }),
    )
    .await;
    (flow, log)
}

fn logged(log: &Log, what: &str) -> Vec<String> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|line| line.contains(what))
        .cloned()
        .collect()
}

/// The session's worktree and branch, as the conversation records them.
fn workspace(flow: &Flow) -> (Option<PathBuf>, String) {
    match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session {
            environment: Environment::NewWorktree { path, branch, .. },
            ..
        }) => (path.map(PathBuf::from), branch),
        other => panic!("not a worktree session: {other:?}"),
    }
}

/// A merge card as the earlier version stored it: open, or answered with `answer`.
async fn plant_merge_card(flow: &Flow, answer: Option<&str>) -> Question {
    let (_, branch) = workspace(flow);
    let now = now_ms();
    let question = Question {
        id: CardId::generate(),
        conversation_id: flow.conversation.clone(),
        task_id: None,
        request_id: flow.board().await.latest_request().map(|r| r.id.clone()),
        position: 0,
        kind: QuestionKind::Merge {
            branch: branch.clone(),
            base: "main".into(),
            conflicted: false,
            conflicts: Vec::new(),
        },
        text: format!("Merge `{branch}` into `main`?"),
        options: vec!["Merge into main".into(), "Not yet".into()],
        recommended: None,
        items: Vec::new(),
        answer: answer.map(str::to_owned),
        answers: answer.map(|a| vec![a.to_owned()]).unwrap_or_default(),
        title: None,
        created_at_ms: now,
        answered_at_ms: answer.map(|_| now),
        words_id: None,
        answered_by: None,
    };
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![DomainEvent::QuestionUpdated {
                question: question.clone(),
            }],
        )
        .await
        .unwrap();
    question
}

/// "Merge into main" reads the branch's commits, not its diff: commits that change nothing in
/// sum (a file added, then removed) are still work to merge, and none is none.
#[tokio::test]
async fn the_session_diff_counts_commits_whose_sum_changes_nothing() {
    let (flow, _log) = session("merge-diffless").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let (path, _) = workspace(&flow);
    let path = path.expect("the worktree");
    let (stat, commits) = flow
        .manager
        .session_diff_stat(&flow.conversation)
        .await
        .unwrap()
        .expect("a worktree session");
    assert_eq!((stat.files.len(), commits), (1, 1));
    git(&path, &["rm", "-q", "NOTES.md"]);
    git(&path, &["commit", "-q", "-m", "Remove the notes"]);
    let (stat, commits) = flow
        .manager
        .session_diff_stat(&flow.conversation)
        .await
        .unwrap()
        .expect("a worktree session");
    assert!(stat.files.is_empty(), "{stat:?}");
    assert_eq!(commits, 2);
    flow.stop().await;
}

/// A worktree session's diff counts what its worktree holds that its base lacks, uncommitted
/// and new files too, as its Changes card does: `brigadier sessions` reads it (#552).
#[tokio::test]
async fn the_session_diff_counts_uncommitted_work() {
    let (flow, _log) = session("merge-uncommitted").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let (path, _) = workspace(&flow);
    let path = path.expect("the worktree");
    let diff = || async {
        flow.manager
            .session_diff_stat(&flow.conversation)
            .await
            .unwrap()
            .expect("a worktree session")
    };
    let (stat, commits) = diff().await;
    let (files, insertions) = (stat.files.len(), stat.insertions);
    let notes = std::fs::read_to_string(path.join("NOTES.md")).unwrap();
    std::fs::write(path.join("NOTES.md"), format!("{notes}One line more.\n")).unwrap();
    std::fs::write(path.join("hello.txt"), "hello\n").unwrap();
    let (stat, after) = diff().await;
    assert_eq!(
        (stat.files.len(), stat.insertions, after),
        (files + 1, insertions + 2, commits),
        "{stat:?}"
    );
    flow.stop().await;
}

/// A Local checkout session's diff is what git shows uncommitted in the checkout, new files too,
/// the user's own edits as well: `brigadier sessions` and the context card read it (#552).
#[tokio::test]
async fn a_local_checkout_sessions_diff_is_its_uncommitted_work() {
    let (flow, _log) = session("merge-local-uncommitted").await;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id;
    let local = flow
        .manager
        .create_conversation(
            crate::model::ConversationKind::Session,
            project,
            Some("Local".into()),
            Some(crate::model::SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: crate::model::EnvironmentRequest::LocalCheckout {
                    branch: "main".into(),
                    create_from: None,
                },
                permission: crate::model::PermissionLevel::Full,
                orchestrator: crate::model::ModelChoice {
                    provider: brigadier_providers::ProviderKind::Claude,
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
    let diff = || async {
        flow.manager
            .session_diff_stat(&local)
            .await
            .unwrap()
            .expect("a Local checkout session's diff")
    };
    let (stat, commits) = diff().await;
    assert_eq!((stat.files.len(), commits), (0, 0), "{stat:?}");
    std::fs::write(flow.repo.join("notes.md"), "One line.\n").unwrap();
    let (stat, commits) = diff().await;
    assert_eq!(
        (stat.files.len(), stat.insertions, stat.deletions, commits),
        (1, 1, 0, 0),
        "{stat:?}"
    );
    assert!(
        git(&flow.repo, &["status", "--porcelain"]).contains("?? notes.md"),
        "the user's index is left as it was"
    );
    flow.stop().await;
}

/// #233: the local `main` fell behind GitHub's, and the session merged GitHub's `main` in. Its
/// size is what it changed since that merge base, its own notes only, not every file
/// GitHub's `main` gained. If local `main` then diverges, its fork and commit counts win.
#[tokio::test]
async fn the_session_diff_uses_a_containing_upstream_but_keeps_a_diverged_local_base() {
    let (flow, _log) = session("merge-diff-upstream").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let (path, branch) = workspace(&flow);
    let path = path.expect("the worktree");
    // GitHub's `main` (as last fetched) gains three files the local `main` lacks.
    let main = git(&flow.repo, &["rev-parse", "main"]);
    git(
        &flow.repo,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/repo.git",
        ],
    );
    git(&flow.repo, &["branch", "upstream-work", &main]);
    git(&path, &["checkout", "-q", "upstream-work"]);
    for n in 1..=3 {
        std::fs::write(path.join(format!("base-{n}.txt")), "base work\n").unwrap();
    }
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-q", "-m", "Base work"]);
    let upstream = git(&path, &["rev-parse", "HEAD"]);
    git(&path, &["checkout", "-q", &branch]);
    git(
        &flow.repo,
        &["update-ref", "refs/remotes/origin/main", &upstream],
    );
    git(
        &flow.repo,
        &["branch", "--set-upstream-to=origin/main", "main"],
    );
    git(&path, &["merge", "-q", "--no-edit", "origin/main"]);
    let (stat, _) = flow
        .manager
        .session_diff_stat(&flow.conversation)
        .await
        .unwrap()
        .expect("a worktree session");
    let files: Vec<&str> = stat.files.iter().map(|f| f.path.as_str()).collect();
    assert_eq!(files, ["NOTES.md"], "{stat:?}");
    let review = flow
        .manager
        .review_diff(
            &flow.conversation,
            crate::work::ReviewScope::Branch,
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!((review.insertions, review.deletions), (1, 0));
    assert_eq!(review.files.len(), 1);
    assert_eq!(review.files[0].path, "NOTES.md");

    // Once local main diverges, its commits take precedence over the upstream.
    std::fs::write(flow.repo.join("local.txt"), "local work\n").unwrap();
    git(&flow.repo, &["add", "local.txt"]);
    git(&flow.repo, &["commit", "-q", "-m", "Local work"]);
    std::fs::write(flow.repo.join("local.txt"), "local work\nmore local work\n").unwrap();
    git(&flow.repo, &["add", "local.txt"]);
    git(&flow.repo, &["commit", "-q", "-m", "More local work"]);
    // The session has not merged local main's new commits.
    let (stat, commits) = flow
        .manager
        .session_diff_stat(&flow.conversation)
        .await
        .unwrap()
        .expect("a worktree session");
    assert_eq!((stat.insertions, stat.deletions, commits), (4, 0, 3));
    assert_eq!(
        stat.files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["NOTES.md", "base-1.txt", "base-2.txt", "base-3.txt"]
    );
    let review = flow
        .manager
        .review_diff(
            &flow.conversation,
            crate::work::ReviewScope::Branch,
            false,
            false,
        )
        .await
        .unwrap();
    assert_eq!((review.insertions, review.deletions), (4, 0));
    assert_eq!(
        review
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>(),
        ["NOTES.md", "base-1.txt", "base-2.txt", "base-3.txt"]
    );
    flow.stop().await;
}

/// The click is recorded with where it came from and the thread merges on its [merge], with
/// plain git; with nothing left to merge, the button is refused. Nothing was ever put on a
/// card, and the workspace stays.
#[tokio::test]
async fn a_click_asks_the_thread_to_merge_and_keeps_the_workspace() {
    let (flow, log) = session("merge-click").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let (_, branch) = workspace(&flow);
    let reply = flow
        .manager
        .merge_session(&flow.conversation, click())
        .await
        .unwrap();
    assert!(reply.contains("Merging"), "{reply}");
    let heard = log.clone();
    flow.until("the thread's merge", move |_| {
        !logged(&heard, "merged ").is_empty()
    })
    .await;
    let board = flow.settled().await;
    assert!(on_main(&flow, "NOTES.md"));
    assert_eq!(logged(&log, "merged "), [format!("merged {branch}")]);
    assert!(
        board.questions.is_empty(),
        "no card: {:#?}",
        board.questions
    );
    let origin = board
        .orchestrator_steps
        .iter()
        .find_map(|step| match &step.kind {
            OrchestratorStepKind::MergeRequested { origin, .. } => Some(origin.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(origin.input, Some(AnswerInput::Click));
    assert_eq!(origin.client, "Brigadier");
    // With nothing left to merge, the button's request is refused.
    let nothing = flow
        .manager
        .merge_session(&flow.conversation, click())
        .await
        .unwrap_err()
        .to_string();
    assert!(nothing.contains("Nothing to merge"), "{nothing}");
    // The workspace stays, and the next request works in it.
    let (path, _) = workspace(&flow);
    let path = path.expect("the worktree stays");
    assert!(path.join(".git").exists());
    flow.say("Add more notes.").await;
    flow.settled().await;
    assert_eq!(
        logged(&log, "worked in: "),
        [format!("worked in: {}", path.display())]
    );
    assert!(path.join("MORE.md").exists());
    flow.stop().await;
}

/// At the upgrade, an open merge card is withdrawn and the thread hears of it once.
#[tokio::test]
async fn an_upgrade_closes_open_merge_cards() {
    let (mut flow, log) = session("merge-upgrade").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let open = plant_merge_card(&flow, None).await;
    flow.restart().await;
    let board = flow.settled().await;
    let withdrawn = &board.questions[&open.id];
    assert!(
        !withdrawn.is_open() && withdrawn.answer.is_none(),
        "{withdrawn:#?}"
    );
    flow.say("Anything new?").await;
    // The note has no request of its own: its turn may follow the user's after that request
    // is done.
    let heard = log.clone();
    flow.until("the note's turn", move |board| {
        board.run == RunState::Idle && !logged(&heard, "[merge cards retired]").is_empty()
    })
    .await;
    flow.settled().await;
    assert_eq!(logged(&log, "[merge cards retired]").len(), 1);
    // Nothing open is left to retire: the next restart says nothing.
    flow.restart().await;
    flow.settled().await;
    assert_eq!(logged(&log, "[merge cards retired]").len(), 1);
    flow.stop().await;
}

/// Plants what an earlier version left when it couldn't finish removing a merged session's
/// worktree: the `session:` owner still disposing, and the session recording no worktree.
async fn plant_failed_removal(flow: &Flow) -> PathBuf {
    let (path, _) = workspace(flow);
    let path = path.unwrap();
    flow.core
        .record(vec![(
            crate::model::streams::CLEANUP.to_owned(),
            DomainEvent::CleanupRequested {
                owner: format!("session:{}", flow.conversation),
                close: false,
            },
        )])
        .await
        .unwrap();
    let Some(Setup::Session {
        repo,
        environment:
            Environment::NewWorktree {
                base,
                branch,
                start,
                ..
            },
        permission,
        orchestrator,
        workers_see_uncommitted,
        plan_mode,
    }) = flow.core.conversation(&flow.conversation).unwrap().setup
    else {
        panic!("a worktree session");
    };
    flow.core
        .set_setup(
            flow.conversation.clone(),
            Setup::Session {
                repo,
                environment: Environment::NewWorktree {
                    base,
                    branch,
                    path: None,
                    start,
                },
                permission,
                orchestrator,
                workers_see_uncommitted,
                plan_mode,
            },
        )
        .await
        .unwrap();
    path
}

/// An earlier version's failed post-merge removal, opened by this one: the startup sweep, the
/// next request and its workspace setup leave the worktree in place, and the session works in
/// it.
#[tokio::test]
async fn an_old_post_merge_removal_is_taken_back_at_the_upgrade() {
    let (mut flow, log) = session("merge-old-removal").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let path = plant_failed_removal(&flow).await;
    flow.restart().await;
    flow.settled().await;
    let owner = format!("session:{}", flow.conversation);
    assert!(!flow.manager.runtime.ledger().disposing().contains(&owner));
    assert!(path.join(".git").exists(), "the sweep left it");
    assert_eq!(workspace(&flow).0.as_deref(), Some(path.as_path()));
    flow.say("Add more notes.").await;
    flow.settled().await;
    assert_eq!(
        logged(&log, "worked in: "),
        [format!("worked in: {}", path.display())]
    );
    assert!(path.join("NOTES.md").exists() && path.join("MORE.md").exists());
    assert!(path.join(".git").exists());
    flow.stop().await;
}

/// The same leftover in an archived session is the archive's own cleanup: it still goes.
#[tokio::test]
async fn an_archived_sessions_unfinished_removal_still_goes() {
    let (mut flow, _) = session("merge-old-removal-archived").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let path = plant_failed_removal(&flow).await;
    flow.core
        .set_lifecycle(flow.conversation.clone(), Lifecycle::Archived)
        .await
        .unwrap();
    flow.restart().await;
    super::eventually("the archived session's worktree removed", || !path.exists()).await;
    flow.stop().await;
}

/// A Close workspace whose removal failed is taken back at the next start, and the session's
/// notices say so: the workspace stays, and closing it again removes it (#48).
#[tokio::test]
async fn a_failed_close_workspace_says_so_at_the_next_start() {
    let (mut flow, _) = session("merge-close-failed-restart").await;
    flow.say("Add notes.").await;
    flow.idle().await;
    let path = workspace(&flow).0.unwrap();
    flow.manager
        .runtime
        .ledger()
        .set_worktree_remover(std::sync::Arc::new(|_, _| {
            Box::pin(async { Err("a file in it is locked".to_owned()) })
        }));
    assert!(
        flow.manager
            .close_workspace(&flow.conversation)
            .await
            .is_err()
    );
    flow.restart().await;
    flow.settled().await;
    let owner = format!("session:{}", flow.conversation);
    assert!(!flow.manager.runtime.ledger().disposing().contains(&owner));
    assert!(path.join(".git").exists());
    let board = flow.board().await;
    assert!(
        board
            .notices
            .iter()
            .any(|notice| notice.text.contains(&format!(
                "Removing this session's worktree didn't finish, so its folder {} stays",
                path.display()
            ))),
        "{:#?}",
        board.notices
    );
    flow.stop().await;
}

/// An older removal that also left the session's folder recorded, taken back at a restart, was
/// no Close workspace: the session isn't told one didn't finish (#182).
#[tokio::test]
async fn an_older_removal_that_kept_its_folder_recorded_says_nothing_at_the_next_start() {
    let (mut flow, _) = session("merge-old-removal-path").await;
    flow.say("Add notes.").await;
    flow.idle().await;
    let path = workspace(&flow).0.unwrap();
    flow.core
        .record(vec![(
            crate::model::streams::CLEANUP.to_owned(),
            DomainEvent::CleanupRequested {
                owner: format!("session:{}", flow.conversation),
                close: false,
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.settled().await;
    let owner = format!("session:{}", flow.conversation);
    assert!(!flow.manager.runtime.ledger().disposing().contains(&owner));
    assert!(path.join(".git").exists());
    let board = flow.board().await;
    assert!(
        !board
            .notices
            .iter()
            .any(|notice| notice.text.contains("didn't finish")),
        "{:#?}",
        board.notices
    );
    flow.stop().await;
}

/// Concurrent Close workspace calls wait for the reservation already held by the first.
#[tokio::test]
async fn close_workspace_waits_for_an_existing_reservation() {
    let (flow, _) = session("merge-close-reserved").await;
    flow.say("Add notes.").await;
    flow.idle().await;
    let path = workspace(&flow).0.unwrap();
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    assert!(
        conv.reserve_workspace().await,
        "the first close holds the workspace"
    );
    let closing = tokio::spawn({
        let manager = flow.manager.clone();
        let id = flow.conversation.clone();
        async move { manager.close_workspace(&id).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !closing.is_finished(),
        "it waits for the existing reservation"
    );
    conv.release_workspace().await;
    closing.await.unwrap().unwrap();
    assert!(!path.exists());
    flow.stop().await;
}

/// A message sent while Close workspace waits for another reservation may get its turn first.
/// What it leaves uncommitted stays on the branch, never removed with the folder (#180).
#[tokio::test]
async fn a_turn_that_overtakes_a_waiting_close_keeps_its_changes() {
    let (flow, _) = session("merge-close-reserved-turn").await;
    flow.say("Add notes.").await;
    flow.idle().await;
    let (path, branch) = workspace(&flow);
    let path = path.unwrap();
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    assert!(
        conv.reserve_workspace().await,
        "the first close holds the workspace"
    );
    let closing = tokio::spawn({
        let manager = flow.manager.clone();
        let id = flow.conversation.clone();
        async move { manager.close_workspace(&id).await }
    });
    flow.say("Draft it.").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    conv.release_workspace().await;
    flow.manager.kick(&conv);
    closing.await.unwrap().unwrap();
    flow.settled().await;
    let files = git(&flow.repo, &["ls-tree", "-r", "--name-only", &branch]);
    assert!(
        files.contains("DRAFT.md") || path.join("DRAFT.md").exists(),
        "the draft went with the folder: {files}"
    );
    flow.stop().await;
}

/// "Close workspace" removes the session's worktree only when the user asks; its branch keeps
/// work the base lacks, and the next change makes a fresh worktree in the same folder.
#[tokio::test]
async fn close_workspace_removes_it_only_when_asked() {
    let (flow, log) = session("merge-close").await;
    flow.say("Add notes.").await;
    flow.idle().await;
    let (path, branch) = workspace(&flow);
    let path = path.unwrap();
    // Uncommitted work is kept on the branch.
    std::fs::write(path.join("DRAFT.md"), "draft\n").unwrap();
    assert!(path.join(".git").exists(), "nothing removes it on its own");
    let reply = flow
        .manager
        .close_workspace(&flow.conversation)
        .await
        .unwrap();
    assert!(
        reply.contains(&format!("Branch `{branch}` stays")),
        "{reply}"
    );
    assert!(!path.exists());
    assert_eq!(workspace(&flow).0, None);
    let files = git(&flow.repo, &["ls-tree", "-r", "--name-only", &branch]);
    assert!(
        files.contains("NOTES.md") && files.contains("DRAFT.md"),
        "{files}"
    );
    // Nothing more to close.
    assert!(
        flow.manager
            .close_workspace(&flow.conversation)
            .await
            .is_err()
    );
    flow.say("Add more notes.").await;
    flow.settled().await;
    assert_eq!(
        logged(&log, "worked in: "),
        [format!("worked in: {}", path.display())]
    );
    assert!(logged(&log, "[worktree removed]").len() == 1);
    assert!(
        path.join("DRAFT.md").exists(),
        "the fresh worktree is on the same branch"
    );
    flow.stop().await;
}

/// Close workspace after a merge takes the branch too (all its work is on the base); the next
/// change makes a fresh one in the same folder. The thread heard so with the close, so no
/// "[workspace] A fresh branch" note follows it (#39).
#[tokio::test]
async fn close_workspace_after_a_merge_tells_the_thread_once() {
    let (flow, log) = session("merge-close-fresh").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    flow.manager
        .merge_session(&flow.conversation, click())
        .await
        .unwrap();
    let heard = log.clone();
    flow.until("the thread's merge", move |_| {
        !logged(&heard, "merged ").is_empty()
    })
    .await;
    flow.idle().await;
    let (path, branch) = workspace(&flow);
    let path = path.unwrap();
    let reply = flow
        .manager
        .close_workspace(&flow.conversation)
        .await
        .unwrap();
    assert!(
        reply.contains(&format!("Branch `{branch}` went too")),
        "{reply}"
    );
    flow.say("Add more notes.").await;
    flow.settled().await;
    assert_eq!(
        logged(&log, "worked in: "),
        [format!("worked in: {}", path.display())]
    );
    let input = logged(&log, "Add more notes.").pop().unwrap();
    assert!(input.contains("[worktree removed]"), "{input}");
    assert!(!input.contains("[workspace] A fresh branch"), "{input}");
    flow.stop().await;
}

/// A message the user sends while Close workspace removes the folder waits for the close: its
/// turn runs after, in a fresh worktree, never in the folder going (#47).
#[tokio::test]
async fn a_message_during_close_workspace_waits_for_it() {
    let (flow, log) = session("merge-close-fence").await;
    flow.say("Add notes.").await;
    flow.idle().await;
    let path = workspace(&flow).0.unwrap();
    // The workspace's removal holds until released, so the message comes while it runs.
    let removing = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (seen, held, workspace_path) = (removing.clone(), release.clone(), path.clone());
    flow.manager
        .runtime
        .ledger()
        .set_worktree_remover(Arc::new(move |repo, path| {
            let (seen, held) = (seen.clone(), held.clone());
            let ours = path == workspace_path;
            Box::pin(async move {
                if ours {
                    seen.notify_one();
                    held.notified().await;
                }
                git(
                    &repo,
                    &["worktree", "remove", "--force", &path.to_string_lossy()],
                );
                Ok(())
            })
        }));
    let closing = tokio::spawn({
        let manager = flow.manager.clone();
        let id = flow.conversation.clone();
        async move { manager.close_workspace(&id).await }
    });
    removing.notified().await;
    flow.say("Add more notes.").await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert!(
        logged(&log, "Add more notes.").is_empty(),
        "a turn started in the folder being removed"
    );
    release.notify_one();
    closing.await.unwrap().unwrap();
    flow.manager.install_worktree_remover();
    flow.settled().await;
    assert_eq!(
        logged(&log, "worked in: "),
        [format!("worked in: {}", path.display())]
    );
    assert!(path.join(".git").exists(), "the turn made a fresh worktree");
    assert!(path.join("MORE.md").exists());
    flow.stop().await;
}
