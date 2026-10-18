//! Merging is the user's act (BUILD-PLAN A3, A4): only their click on "Merge into main" or
//! their own words merge. Merge cards are retired: an old card's answer never authorises a
//! merge (live, or stored across a restart), and an upgrade closes open merge cards and cancels a merge held on one. The
//! session keeps its workspace through a merge, and an old post-merge removal is taken back.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use super::thread_tests::{commit_in_workspace, on_main};
use super::{Flow, Options, Reply, Script, Turn, click, git};
use crate::manager::landing::pending_merge;
use crate::manager::prompts::{MERGE_BY_USER_REVISION, revision_note};
use crate::model::{CardId, DomainEvent, Environment, Lifecycle, Setup};
use crate::now_ms;
use crate::work::{
    AnswerInput, MergeCondition, OrchestratorStepKind, Question, QuestionKind, RunState,
};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

type Log = Arc<Mutex<Vec<String>>>;

/// A session whose thread commits NOTES.md on "Add notes.", says once that it is ready, merges
/// on [merge] without words and on "just merge it" with them. Every thread input and every
/// reply it got are logged.
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
                    let reply = turn.call("finish_session", json!({})).await;
                    log(format!("finish_session: {} {}", reply.is_error, reply.text));
                    return Reply::text("Merged.");
                }
                if turn.input.contains("just merge it") {
                    let reply = turn
                        .call("finish_session", json!({ "user_words": "just merge it" }))
                        .await;
                    log(format!("finish_session: {} {}", reply.is_error, reply.text));
                    return Reply::text("Merged.");
                }
                if turn.input.contains("Add more notes.") {
                    let head = commit_in_workspace(&turn, "MORE.md", "more\n", "Add more");
                    log(format!("worked in: {}", turn.add_dirs[0].display()));
                    log(format!("head: {head}"));
                    return Reply::text("More notes are in.");
                }
                if turn.input.contains("Run the long one.") {
                    let reply = turn
                        .call("run", json!({ "command": "sleep 60; echo late" }))
                        .await;
                    log(format!("run: {} {}", reply.is_error, reply.text));
                    return Reply::text("It runs.");
                }
                if turn.input.contains("Draft it.") {
                    std::fs::write(turn.add_dirs[0].join("DRAFT.md"), "draft\n").unwrap();
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

/// Waits until the session's full checks have ended and the thread is idle again, its scratch
/// swept: the checks run in the background (a merge no longer waits for them, #241), and a
/// Close workspace during the sweep that follows a request is refused as working.
async fn quiet(flow: &Flow) -> crate::board::Board {
    flow.until("the full checks to end", |board| {
        board
            .integrated_checks
            .values()
            .all(|check| check.state.is_final())
    })
    .await;
    let board = flow.settled().await;
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    super::eventually_async("the thread to be idle, its scratch swept", || async {
        !conv.scratch_used.load(std::sync::atomic::Ordering::SeqCst) && conv.scratch_idle().await
    })
    .await;
    board
}

fn merged(board: &crate::board::Board) -> usize {
    board
        .orchestrator_steps
        .iter()
        .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        .count()
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

/// #233: the local `main` fell behind GitHub's, and the session merged GitHub's `main` in. Its
/// size is what it changed since that merge base, its own notes only, not every file
/// GitHub's `main` gained.
#[tokio::test]
async fn the_session_diff_counts_from_the_upstream_it_merged() {
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
    flow.stop().await;
}

/// The merge's upstream as its Merged row records it.
fn merged_upstream(board: &crate::board::Board) -> Vec<Option<crate::work::MergedUpstream>> {
    board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::Merged { upstream, .. } => Some(upstream.clone()),
            _ => None,
        })
        .collect()
}

/// Another clone of `origin` pushes a commit adding `file` to its `main`.
fn push_upstream(flow: &Flow, origin: &std::path::Path, file: &str) -> String {
    let other = flow.repo.with_file_name(format!("merge-upstream-{file}"));
    git(
        &flow.repo,
        &[
            "clone",
            "-q",
            origin.to_str().unwrap(),
            other.to_str().unwrap(),
        ],
    );
    std::fs::write(other.join(file), "upstream\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "Upstream work"]);
    git(&other, &["push", "-q", "origin", "HEAD:main"]);
    git(&other, &["rev-parse", "HEAD"])
}

/// The merge fetches the base's upstream first and fast-forwards a local `main` that is only
/// behind, so it merges onto what others pushed; its result and its Merged row say it is on
/// this Mac only, not pushed, and say so when local `main` has diverged from it (#224).
#[tokio::test]
async fn a_merge_brings_main_up_to_date_and_says_it_isnt_pushed() {
    let (flow, log) = session("merge-upstream").await;
    let origin = flow.repo.with_file_name("merge-upstream-origin.git");
    git(
        &flow.repo,
        &["clone", "-q", "--bare", ".", origin.to_str().unwrap()],
    );
    git(
        &flow.repo,
        &["remote", "add", "origin", origin.to_str().unwrap()],
    );
    git(&flow.repo, &["fetch", "-q", "origin"]);
    git(
        &flow.repo,
        &["branch", "--set-upstream-to=origin/main", "main"],
    );
    flow.say("Add notes.").await;
    flow.settled().await;
    // Someone pushes to origin's main: the local main is behind, and doesn't know it yet.
    let pushed = push_upstream(&flow, &origin, "UPSTREAM.md");
    flow.say("Yes, just merge it.").await;
    flow.until("the merge", |board| merged(board) == 1).await;
    let board = flow.settled().await;
    assert!(on_main(&flow, "NOTES.md") && on_main(&flow, "UPSTREAM.md"));
    assert!(
        std::process::Command::new("git")
            .args(["merge-base", "--is-ancestor", &pushed, "main"])
            .current_dir(&flow.repo)
            .status()
            .unwrap()
            .success(),
        "merged onto origin's main"
    );
    let replies = logged(&log, "finish_session: false");
    assert_eq!(replies.len(), 1, "{replies:?}");
    assert!(
        replies[0].contains("It is merged on this Mac only, not pushed to `origin/main`."),
        "{}",
        replies[0]
    );
    assert!(!replies[0].contains("diverged"), "{}", replies[0]);
    let only_here = crate::work::MergedUpstream {
        name: "origin/main".into(),
        behind: 0,
    };
    assert_eq!(merged_upstream(&board), [Some(only_here.clone())]);
    // Another push while local main holds the unpushed merge: they have diverged.
    flow.say("Add more notes.").await;
    flow.settled().await;
    push_upstream(&flow, &origin, "UPSTREAM-2.md");
    flow.say("Now just merge it again.").await;
    flow.until("the second merge", |board| merged(board) == 2)
        .await;
    let board = flow.settled().await;
    let replies = logged(&log, "finish_session: false");
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert!(
        replies[1].contains(
            "Local `main` has diverged from `origin/main`: 1 commit there is not in it, so pushing needs them merged in first."
        ),
        "{}",
        replies[1]
    );
    assert!(
        !on_main(&flow, "UPSTREAM-2.md"),
        "diverged: no fast-forward"
    );
    assert_eq!(
        merged_upstream(&board),
        [
            Some(only_here),
            Some(crate::work::MergedUpstream {
                name: "origin/main".into(),
                behind: 1,
            })
        ]
    );
    flow.stop().await;
}

/// The click merges, once: it is recorded with where it came from, the thread merges on its
/// [merge], and a second finish_session on the same click is refused. Nothing was ever put on a
/// card, and the workspace stays with its branch at the merge.
#[tokio::test]
async fn a_click_merges_once_and_keeps_the_workspace() {
    let (flow, log) = session("merge-click").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    // Without the click or the user's words, there is no merge.
    assert!(
        flow.manager
            .finish_session(&flow.conversation, "", None)
            .await
            .is_err()
    );
    let reply = flow
        .manager
        .merge_session(&flow.conversation, click())
        .await
        .unwrap();
    assert!(reply.contains("Merging"), "{reply}");
    flow.until("the merge", |board| merged(board) == 1).await;
    let board = flow.settled().await;
    assert!(on_main(&flow, "NOTES.md"));
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
    assert_eq!(logged(&log, "finish_session: false [finished]").len(), 1);
    // The click is spent: no second merge on it.
    assert!(
        flow.manager
            .finish_session(&flow.conversation, "", None)
            .await
            .is_err()
    );
    // And with nothing left to merge, the button's request is refused.
    let nothing = flow
        .manager
        .merge_session(&flow.conversation, click())
        .await
        .unwrap_err()
        .to_string();
    assert!(nothing.contains("Nothing to merge"), "{nothing}");
    // The workspace stays, its branch at the merge, and the next request works in it.
    let (path, branch) = workspace(&flow);
    let path = path.expect("the worktree stays");
    assert!(path.join(".git").exists());
    let main = git(&flow.repo, &["rev-parse", "main"]);
    assert_eq!(git(&flow.repo, &["rev-parse", &branch]), main);
    flow.say("Add more notes.").await;
    flow.settled().await;
    assert_eq!(
        logged(&log, "worked in: "),
        [format!("worked in: {}", path.display())]
    );
    assert!(path.join("MORE.md").exists());
    flow.stop().await;
}

/// A click after the user's words spends both: once it merged, the words before it don't
/// authorise another merge of what comes later.
#[tokio::test]
async fn a_spent_click_never_falls_back_to_the_words_before_it() {
    let (flow, _) = session("merge-spent-click").await;
    flow.say("Add notes. Then merge it into main.").await;
    flow.settled().await;
    flow.manager
        .merge_session(&flow.conversation, click())
        .await
        .unwrap();
    flow.until("the merge", |board| merged(board) == 1).await;
    flow.settled().await;
    let (path, _) = workspace(&flow);
    let path = path.expect("the worktree stays");
    std::fs::write(path.join("LATER.md"), "later\n").unwrap();
    git(&path, &["add", "LATER.md"]);
    git(&path, &["commit", "-qm", "Add later"]);
    let refused = flow
        .manager
        .finish_session(&flow.conversation, "merge it into main", None)
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("[not merged]"), "{refused}");
    assert_eq!(merged(&flow.board().await), 1);
    assert!(!on_main(&flow, "LATER.md"));
    flow.stop().await;
}

/// A card's answer is the user's word (#240): the thread judges it and names the card in
/// `consent`, and it merges once. With nothing named, the answer alone merges nothing.
#[tokio::test]
async fn a_card_answer_merges_when_the_thread_names_it() {
    let (flow, _) = session("merge-card-answer").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let card = plant_merge_card(&flow, None).await;
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["Merge into main".into()],
            Some(click()),
        )
        .await
        .unwrap();
    flow.settled().await;
    assert_eq!(merged(&flow.board().await), 0);
    assert!(
        flow.manager
            .finish_session(&flow.conversation, "", None)
            .await
            .is_err()
    );
    let card = card.id.to_string();
    let merged_on = flow
        .manager
        .finish_session(&flow.conversation, &card, None)
        .await
        .unwrap();
    assert!(merged_on.contains("[finished]"), "{merged_on}");
    assert!(on_main(&flow, "NOTES.md"));
    let again = flow
        .manager
        .finish_session(&flow.conversation, &card, None)
        .await
        .unwrap_err()
        .to_string();
    assert!(again.contains("used for a merge already"), "{again}");
    assert_eq!(merged(&flow.board().await), 1);
    flow.stop().await;
}

/// The user's own words still merge, and close a merge card left open.
#[tokio::test]
async fn words_still_merge_and_close_an_old_open_card() {
    let (flow, log) = session("merge-words").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let card = plant_merge_card(&flow, None).await;
    flow.say("Yes, just merge it.").await;
    flow.until("the merge", |board| merged(board) == 1).await;
    let board = flow.settled().await;
    assert!(on_main(&flow, "NOTES.md"));
    assert_eq!(logged(&log, "finish_session: false").len(), 1);
    assert!(!board.questions[&card.id].is_open());
    flow.stop().await;
}

/// At the upgrade, an open merge card is withdrawn and the thread hears of it once; a merge
/// held on a card's answer (#240) or on the user's words stays.
#[tokio::test]
async fn an_upgrade_closes_open_merge_cards_and_keeps_a_held_merge() {
    let (mut flow, log) = session("merge-upgrade").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let (_, branch) = workspace(&flow);
    let open = plant_merge_card(&flow, None).await;
    let answered = plant_merge_card(&flow, Some("Merge into main")).await;
    flow.manager
        .orchestrator_step(
            &flow.conversation,
            OrchestratorStepKind::MergeHeld {
                branch: branch.clone(),
                base: "main".into(),
                asked_in: answered.id.to_string(),
                consented_at_ms: now_ms(),
                condition: MergeCondition::WorkersFinish,
                waiting: Vec::new(),
                tip: String::new(),
            },
        )
        .await;
    flow.restart().await;
    let board = flow.settled().await;
    let withdrawn = &board.questions[&open.id];
    assert!(
        !withdrawn.is_open() && withdrawn.answer.is_none(),
        "{withdrawn:#?}"
    );
    assert!(
        !board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(&step.kind, OrchestratorStepKind::MergeCancelled { .. }))
    );
    assert_eq!(
        pending_merge(&board).map(|held| held.asked_in),
        Some(answered.id.to_string())
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

/// Rule R: a session told the instructions before this change hears the merge note once when
/// it resumes, and a click merges.
#[tokio::test]
async fn a_session_from_before_hears_the_note_once_and_merges_on_a_click() {
    let (mut flow, log) = session("merge-revision").await;
    flow.say("Add notes.").await;
    flow.settled().await;
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&flow.conversation),
            DomainEvent::OrchestratorLogged {
                conversation_id: flow.conversation.clone(),
                entry: crate::work::OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            revision: Some(MERGE_BY_USER_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    flow.say("Ask about the merge.").await;
    flow.settled().await;
    let note = revision_note(MERGE_BY_USER_REVISION - 1, false).unwrap();
    assert!(note.contains("there are no merge cards"), "{note}");
    assert_eq!(logged(&log, &note).len(), 1);
    flow.manager
        .merge_session(&flow.conversation, click())
        .await
        .unwrap();
    flow.until("the merge", |board| merged(board) == 1).await;
    flow.settled().await;
    assert!(on_main(&flow, "NOTES.md"));
    assert_eq!(logged(&log, &note).len(), 1);
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

/// A Close workspace whose removal failed leaves the `session:` owner disposing while the
/// session still works in its folder: the thread's clean_up never takes that folder (phase A
/// verify).
#[tokio::test]
async fn clean_up_keeps_a_workspace_whose_close_failed() {
    let (flow, _) = session("merge-close-failed-clean-up").await;
    flow.say("Add notes.").await;
    quiet(&flow).await;
    let (path, _) = workspace(&flow);
    let path = path.unwrap();
    // The folder can't be removed (a locked file, say), so the Close fails.
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
    let owner = format!("session:{}", flow.conversation);
    assert!(flow.manager.runtime.ledger().disposing().contains(&owner));
    assert_eq!(workspace(&flow).0.as_deref(), Some(path.as_path()));
    flow.manager.install_worktree_remover();
    // The user keeps working there, then asks the thread to clean up.
    std::fs::write(path.join("DRAFT.md"), "draft\n").unwrap();
    let text = flow.manager.clean_up(&flow.conversation).await.unwrap();
    assert!(path.join(".git").exists(), "{text}");
    assert!(path.join("DRAFT.md").exists(), "{text}");
    flow.stop().await;
}

/// A Close workspace whose removal failed is taken back at the next start, and the session's
/// notices say so: the workspace stays, and closing it again removes it (#48).
#[tokio::test]
async fn a_failed_close_workspace_says_so_at_the_next_start() {
    let (mut flow, _) = session("merge-close-failed-restart").await;
    flow.say("Add notes.").await;
    quiet(&flow).await;
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
                "Closing this session's workspace didn't finish, so its folder {} stays",
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
    quiet(&flow).await;
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

/// Close workspace right after a turn, while the sweep that follows it still holds the
/// thread, waits for the sweep and closes instead of saying the session is working (#180).
#[tokio::test]
async fn close_workspace_right_after_a_turn_waits_for_its_sweep() {
    let (flow, _) = session("merge-close-sweep").await;
    flow.say("Add notes.").await;
    quiet(&flow).await;
    let path = workspace(&flow).0.unwrap();
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    assert!(conv.begin_scratch_sweep().await, "the sweep after the turn");
    let closing = tokio::spawn({
        let manager = flow.manager.clone();
        let id = flow.conversation.clone();
        async move { manager.close_workspace(&id).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(
        !closing.is_finished(),
        "it waits for the sweep, not refuses"
    );
    conv.end_scratch_sweep().await;
    closing.await.unwrap().unwrap();
    assert!(!path.exists());
    flow.stop().await;
}

/// A command the thread left running in the background keeps its request working: Close
/// workspace stops it and closes, without waiting for it to end (Codex review of #180).
#[tokio::test]
async fn close_workspace_stops_a_background_run_and_closes() {
    let (flow, _) = session("merge-close-background").await;
    flow.say("Add notes.").await;
    quiet(&flow).await;
    let path = workspace(&flow).0.unwrap();
    flow.background_commands();
    flow.say("Run the long one.").await;
    flow.until("the run in the background, the turn over", |board| {
        board
            .background_runs
            .values()
            .any(|run| run.state == crate::work::BackgroundRunState::Running)
            && board.run == RunState::Idle
    })
    .await;
    let started = std::time::Instant::now();
    flow.manager
        .close_workspace(&flow.conversation)
        .await
        .unwrap();
    assert!(started.elapsed() < std::time::Duration::from_secs(10));
    assert!(!path.exists());
    flow.stop().await;
}

/// A message sent while Close workspace waits for the sweep may get its turn first: what that
/// turn leaves uncommitted is still kept on the branch, never removed with the folder (Codex
/// review of #180).
#[tokio::test]
async fn a_turn_that_overtakes_a_waiting_close_keeps_its_changes() {
    let (flow, _) = session("merge-close-sweep-turn").await;
    flow.say("Add notes.").await;
    quiet(&flow).await;
    let (path, branch) = workspace(&flow);
    let path = path.unwrap();
    let conv = flow.manager.conv(&flow.conversation).unwrap();
    assert!(conv.begin_scratch_sweep().await, "the sweep after the turn");
    let closing = tokio::spawn({
        let manager = flow.manager.clone();
        let id = flow.conversation.clone();
        async move { manager.close_workspace(&id).await }
    });
    flow.say("Draft it.").await;
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    conv.end_scratch_sweep().await;
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
    quiet(&flow).await;
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
    assert!(logged(&log, "[workspace closed]").len() == 1);
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
    flow.say("Now just merge it.").await;
    let board = quiet(&flow).await;
    assert_eq!(merged(&board), 1, "{:#?}", log.lock().unwrap());
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
    assert!(input.contains("[workspace closed]"), "{input}");
    assert!(!input.contains("[workspace] A fresh branch"), "{input}");
    flow.stop().await;
}

/// A message the user sends while Close workspace removes the folder waits for the close: its
/// turn runs after, in a fresh worktree, never in the folder going (#47).
#[tokio::test]
async fn a_message_during_close_workspace_waits_for_it() {
    let (flow, log) = session("merge-close-fence").await;
    flow.say("Add notes.").await;
    quiet(&flow).await;
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

/// Close workspace waits for the session's workers: while a scout runs, the folder stays and
/// the user hears why; once it has ended, the workspace closes (Codex review, phase A verify).
#[tokio::test]
async fn close_workspace_waits_for_a_running_scout() {
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let flow = Flow::start(
        "merge-close-scout",
        Options::default(),
        script(move |turn| {
            let held = held.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Add notes.") {
                        commit_in_workspace(&turn, "NOTES.md", "notes\n", "Add notes");
                        return Reply::text("Notes are in.");
                    }
                    if turn.input.contains("Look around.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "low", "title": "Look around",
                                       "kind": "scout", "spec": "Look around."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text(crate::manager::prompts::QUIET);
                }
                held.notified().await;
                let reply = turn
                    .call("submit_report", json!({"summary": "Looked around."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Looked around.")
            }
        }),
    )
    .await;
    flow.say("Add notes.").await;
    flow.settled().await;
    let path = workspace(&flow).0.unwrap();
    flow.say("Look around.").await;
    flow.until("the scout to run", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && !task.state.is_final())
    })
    .await;
    let refused = flow
        .manager
        .close_workspace(&flow.conversation)
        .await
        .unwrap_err()
        .to_string();
    assert!(
        refused.contains("still run") || refused.contains("working"),
        "{refused}"
    );
    assert!(path.join(".git").exists());
    release.notify_one();
    flow.until("the scout to end", |board| {
        board
            .tasks
            .values()
            .any(|task| task.number == 1 && task.state.is_final())
    })
    .await;
    flow.settled().await;
    flow.manager
        .close_workspace(&flow.conversation)
        .await
        .unwrap();
    assert!(!path.exists());
    flow.stop().await;
}
