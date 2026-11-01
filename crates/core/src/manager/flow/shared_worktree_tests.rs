//! Several sessions in one folder (goal 9, D123): a session started on an existing branch works
//! where it is checked out, beside the session there, with its own context; the folder, its
//! branch, its uncommitted work and what runs in it outlive the other session's archive or
//! delete, and only the last Brigadier session using a Brigadier-made worktree lets it go.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{
    Conversation, ConversationKind, Environment, EnvironmentRequest, Setup, SetupRequest,
};

/// Each turn's input and folder, for the assertions.
type Heard = Arc<Mutex<Vec<(String, PathBuf)>>>;

fn script(heard: Heard) -> Script {
    Arc::new(move |turn: Turn| {
        let heard = heard.clone();
        Box::pin(async move {
            if turn.is_orchestrator() {
                heard
                    .lock()
                    .unwrap()
                    .push((turn.input.clone(), turn.cwd.clone()));
            }
            Reply::text("Done.")
        })
    })
}

async fn start(flow: &Flow, title: &str, branch: &str) -> crate::Result<Conversation> {
    start_in(
        flow,
        title,
        EnvironmentRequest::Existing {
            branch: branch.into(),
        },
    )
    .await
}

async fn start_in(
    flow: &Flow,
    title: &str,
    environment: EnvironmentRequest,
) -> crate::Result<Conversation> {
    let first = flow.core.conversation(&flow.conversation).unwrap();
    let Some(Setup::Session {
        repo,
        permission,
        orchestrator,
        ..
    }) = first.setup
    else {
        unreachable!()
    };
    flow.manager
        .create_conversation(
            ConversationKind::Session,
            first.project_id,
            Some(title.into()),
            Some(SetupRequest::Session {
                repo,
                environment,
                permission,
                orchestrator,
                plan_mode: false,
            }),
        )
        .await
}

fn worktree_of(conversation: &Conversation) -> (String, String, PathBuf) {
    match &conversation.setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    base,
                    branch,
                    path: Some(path),
                    ..
                },
            ..
        }) => (base.clone(), branch.clone(), PathBuf::from(path)),
        other => panic!("no worktree: {other:?}"),
    }
}

fn heard_in(heard: &Heard, marker: &str) -> (String, PathBuf) {
    heard
        .lock()
        .unwrap()
        .iter()
        .find(|(input, _)| input.contains(marker))
        .cloned()
        .unwrap_or_else(|| panic!("no turn heard {marker}"))
}

async fn archived(flow: &Flow, conversation: &Conversation) {
    flow.manager.archive(conversation.id.clone()).await.unwrap();
    flow.manager.cleanup_finished(&conversation.id).await;
    super::eventually("the archive's cleanup", || {
        !flow
            .core
            .conversation(&conversation.id)
            .unwrap()
            .cleanup_pending
    })
    .await;
}

async fn deleted(flow: &Flow, conversation: &Conversation) {
    flow.manager.delete(conversation.id.clone()).await.unwrap();
    super::eventually("the delete", || {
        flow.core.conversation(&conversation.id).is_err()
    })
    .await;
    flow.manager.cleanup_finished(&conversation.id).await;
}

/// B starts on A's branch: it works in A's worktree, merges where A does, hears once that it
/// shares the folder, and neither hears the other's messages. A command B runs there outlives
/// A's archive, the folder and branch stay for B, and B's archive, the last, removes them.
#[tokio::test]
async fn a_session_on_another_sessions_branch_works_beside_it() {
    let heard = Heard::default();
    let flow = Flow::start("shared-worktree", Options::default(), script(heard.clone())).await;
    flow.say("A: look around.").await;
    flow.settled().await;
    let a = flow.core.conversation(&flow.conversation).unwrap();
    let (base, branch, folder) = worktree_of(&a);

    let b = start(&flow, "B", &branch).await.unwrap();
    assert_eq!(
        worktree_of(&b),
        (base.clone(), branch.clone(), folder.clone())
    );
    flow.say_to(&b.id, "B: build it.").await;
    flow.settled_in(&b.id).await;
    let (input, cwd) = heard_in(&heard, "B: build it.");
    assert_eq!(cwd, folder);
    assert!(
        input.contains("Another session also works in this folder"),
        "{input}"
    );
    assert!(!input.contains("A: look around."), "{input}");
    let (a_input, _) = heard_in(&heard, "A: look around.");
    assert!(!a_input.contains("Another session"));

    // B's command runs in the folder while A is archived.
    let done = folder.join("done.txt");
    let mut command = std::process::Command::new("sh")
        .args(["-c", "sleep 2; echo done > done.txt"])
        .current_dir(&folder)
        .spawn()
        .unwrap();
    archived(&flow, &a).await;
    assert!(command.wait().unwrap().success(), "B's command was stopped");
    assert!(done.is_file());
    std::fs::remove_file(&done).unwrap();
    assert!(
        folder.join(".git").is_file(),
        "A's archive leaves B's folder"
    );
    assert!(!git(&flow.repo, &["branch", "--list", &branch]).is_empty());
    flow.say_to(&b.id, "B: again.").await;
    flow.settled_in(&b.id).await;
    assert_eq!(heard_in(&heard, "B: again.").1, folder);

    // B's archive, the last, stops what still runs there and lets the folder go.
    let mut left = std::process::Command::new("sleep")
        .arg("60")
        .current_dir(&folder)
        .spawn()
        .unwrap();
    archived(&flow, &b).await;
    assert!(
        left.try_wait().unwrap().is_some(),
        "a command outlived the folder"
    );
    assert!(!folder.exists(), "the last session using it lets it go");
    flow.stop().await;
}

/// Deleting A while B works in its worktree keeps the folder, its branch and B's uncommitted
/// change as they are: no WIP commit, nothing removed.
#[tokio::test]
async fn deleting_a_session_keeps_the_folder_and_work_of_the_one_beside_it() {
    let heard = Heard::default();
    let flow = Flow::start("shared-delete", Options::default(), script(heard)).await;
    flow.say("A: look around.").await;
    flow.settled().await;
    let a = flow.core.conversation(&flow.conversation).unwrap();
    let (_, branch, folder) = worktree_of(&a);
    let b = start(&flow, "B", &branch).await.unwrap();
    std::fs::write(folder.join("b.txt"), "B's work\n").unwrap();
    let head = git(&folder, &["rev-parse", "HEAD"]);

    flow.manager.delete(a.id.clone()).await.unwrap();
    super::eventually("A to go", || flow.core.conversation(&a.id).is_err()).await;
    flow.manager.cleanup_finished(&a.id).await;
    assert!(folder.join(".git").is_file());
    assert_eq!(git(&folder, &["rev-parse", "HEAD"]), head, "no WIP commit");
    assert!(git(&folder, &["status", "--porcelain"]).contains("b.txt"));
    assert_eq!(git(&folder, &["branch", "--show-current"]), branch);
    assert_eq!(
        worktree_of(&flow.core.conversation(&b.id).unwrap()).2,
        folder
    );
    // B commits its work: the next launch's orphan sweep never takes a clean folder B works in.
    git(&folder, &["add", "b.txt"]);
    git(&folder, &["commit", "-qm", "B's work"]);
    let swept = flow.manager.sweep_orphans().await;
    assert!(
        folder.join(".git").is_file(),
        "B's folder survives the sweep"
    );
    assert!(
        swept.kept_branches.is_empty(),
        "B's branch isn't a leftover: {swept:?}"
    );
    assert_eq!(git(&folder, &["branch", "--show-current"]), branch);
    // A's folder went to B: B's delete, the last, removes it.
    let root = flow.manager.session_dir(&a.id).root().to_path_buf();
    flow.manager.delete(b.id.clone()).await.unwrap();
    super::eventually("B to go", || flow.core.conversation(&b.id).is_err()).await;
    flow.manager.cleanup_finished(&b.id).await;
    assert!(
        !root.exists(),
        "A's folder goes with the last session in it"
    );
    flow.stop().await;
}

/// A branch checked out nowhere gets a worktree of its own on that branch (no new branch, its
/// git config untouched); the checkout's branch is worked on in the checkout; a folder the
/// user made is worked in, never removed, and a turn there is refused once it moved on.
#[tokio::test]
async fn a_free_branch_the_checkouts_branch_and_a_users_worktree() {
    let heard = Heard::default();
    let flow = Flow::start("shared-places", Options::default(), script(heard.clone())).await;
    git(&flow.repo, &["branch", "feature"]);
    let names = || git(&flow.repo, &["branch", "--format=%(refname:short)"]);
    let branches = names();
    let free = start(&flow, "Free", "feature").await.unwrap();
    let (base, branch, folder) = worktree_of(&free);
    assert_eq!((base.as_str(), branch.as_str()), ("feature", "feature"));
    assert_eq!(folder, flow.manager.session_worktree_path(&free));
    assert_eq!(git(&folder, &["branch", "--show-current"]), "feature");
    assert_eq!(names(), branches, "no branch made");
    let config = std::process::Command::new("git")
        .args(["config", "--get", "branch.feature.gh-merge-base"])
        .current_dir(&flow.repo)
        .output()
        .unwrap();
    assert!(
        config.stdout.is_empty(),
        "the branch's git config is left as it was"
    );

    let main = start(&flow, "Main", "main").await.unwrap();
    assert!(matches!(
        main.setup,
        Some(Setup::Session {
            environment: Environment::LocalCheckout { ref branch },
            ..
        }) if branch == "main"
    ));
    // A plain Local checkout session beside it hears it shares the folder; the first didn't.
    let beside = start_in(
        &flow,
        "Beside",
        EnvironmentRequest::LocalCheckout {
            branch: "main".into(),
            create_from: None,
        },
    )
    .await
    .unwrap();
    flow.say_to(&main.id, "M: hello.").await;
    flow.settled_in(&main.id).await;
    flow.say_to(&beside.id, "S: hello.").await;
    flow.settled_in(&beside.id).await;
    assert!(!heard_in(&heard, "M: hello.").0.contains("Another session"));
    let (input, cwd) = heard_in(&heard, "S: hello.");
    assert!(
        input.contains("Another session also works in this folder on `main`"),
        "{input}"
    );
    assert_eq!(cwd, flow.repo);

    let mine = flow.dir.join("mine");
    git(
        &flow.repo,
        &["worktree", "add", "-b", "mine", mine.to_str().unwrap()],
    );
    // As git names it.
    let mine = mine.canonicalize().unwrap();
    let users = start(&flow, "Users", "mine").await.unwrap();
    assert_eq!(worktree_of(&users).2, mine);
    flow.say_to(&users.id, "U: hello.").await;
    flow.settled_in(&users.id).await;
    assert_eq!(heard_in(&heard, "U: hello.").1, mine);
    git(&mine, &["switch", "-c", "elsewhere"]);
    let refused = flow
        .manager
        .effective_workspace(&users.id)
        .await
        .unwrap_err()
        .to_string();
    assert!(refused.contains("no longer on `mine`"), "{refused}");
    // Nor does a turn start there.
    flow.say_to(&users.id, "U: again.").await;
    flow.settled_in(&users.id).await;
    assert!(
        !heard
            .lock()
            .unwrap()
            .iter()
            .any(|(input, _)| input.contains("U: again.")),
        "a turn ran off the branch"
    );
    // Its delete commits nothing in the user's folder and leaves it.
    std::fs::write(mine.join("theirs.txt"), "the user's\n").unwrap();
    let head = git(&mine, &["rev-parse", "HEAD"]);
    flow.manager.delete(users.id.clone()).await.unwrap();
    super::eventually("Users to go", || flow.core.conversation(&users.id).is_err()).await;
    flow.manager.cleanup_finished(&users.id).await;
    assert!(mine.join(".git").is_file(), "a folder the user made stays");
    assert_eq!(git(&mine, &["rev-parse", "HEAD"]), head, "no WIP commit");

    let gone = start(&flow, "Gone", "no-such-branch").await.unwrap_err();
    assert!(
        gone.to_string().contains("isn't in this repository"),
        "{gone}"
    );
    flow.stop().await;
}

/// A start on A's branch racing A's archive never ends with B in a removed folder.
#[tokio::test]
async fn starting_beside_a_session_while_it_is_archived_never_loses_the_folder() {
    let heard = Heard::default();
    let flow = Flow::start("shared-race", Options::default(), script(heard)).await;
    flow.say("A: look around.").await;
    flow.settled().await;
    let a = flow.core.conversation(&flow.conversation).unwrap();
    let (_, branch, _) = worktree_of(&a);
    let (archive, b) = tokio::join!(
        flow.manager.archive(a.id.clone()),
        start(&flow, "B", &branch)
    );
    archive.unwrap();
    flow.manager.cleanup_finished(&a.id).await;
    if let Ok(b) = b {
        let folder = worktree_of(&flow.core.conversation(&b.id).unwrap()).2;
        assert!(
            Path::new(&folder).join(".git").is_file(),
            "B's folder is there"
        );
        assert_eq!(git(&folder, &["branch", "--show-current"]), branch);
    }
    flow.stop().await;
}

/// The composer's branch list marks the branch a running worker works on, from the worker's
/// own record (any branch prefix), and only while the worker is open.
#[tokio::test]
async fn a_running_workers_branch_is_marked_for_the_composer() {
    let release = Arc::new(tokio::sync::Notify::new());
    let hold = release.clone();
    let script: Script = Arc::new(move |turn: Turn| {
        let hold = hold.clone();
        Box::pin(async move {
            if turn.worker().is_some() {
                hold.notified().await;
            } else if turn.input.contains("Start a worker.") {
                turn.start_worker("Held", "Hold.").await;
            }
            Reply::text("Done.")
        })
    });
    let flow = Flow::start("shared-worker-branch", Options::default(), script).await;
    flow.say("Start a worker.").await;
    let repo = flow.repo.display().to_string();
    let marked = || async {
        flow.manager
            .repo_info(repo.clone())
            .await
            .unwrap()
            .branches
            .into_iter()
            .filter(|branch| branch.worker)
            .map(|branch| branch.name)
            .collect::<Vec<_>>()
    };
    super::eventually_async("the worker's branch marked", || async {
        marked().await.len() == 1
    })
    .await;
    let session = worktree_of(&flow.core.conversation(&flow.conversation).unwrap()).1;
    assert!(
        !marked().await.contains(&session),
        "the session's own branch is offered"
    );
    // Done with its turn, it is still open and holds its branch; stopped, it doesn't.
    release.notify_waiters();
    flow.settled().await;
    assert_eq!(marked().await.len(), 1);
    // Nor can a session be started there while it is.
    let held = marked().await.remove(0);
    let refused = start(&flow, "On the worker's", &held).await.unwrap_err();
    assert!(refused.to_string().contains("running worker"), "{refused}");
    let task = flow.board().await.tasks.keys().next().unwrap().clone();
    flow.manager.stop_task(task).await.unwrap();
    super::eventually_async("the branch offered again", || async {
        marked().await.is_empty()
    })
    .await;
    flow.stop().await;
}

/// A, archived beside B and restored, works in the folder again: B's archive leaves it to A.
#[tokio::test]
async fn a_restored_session_keeps_the_folder_it_shares() {
    let flow = Flow::start(
        "shared-restore",
        Options::default(),
        script(Heard::default()),
    )
    .await;
    flow.say("A: look around.").await;
    flow.settled().await;
    let a = flow.core.conversation(&flow.conversation).unwrap();
    let (_, branch, folder) = worktree_of(&a);
    let b = start(&flow, "B", &branch).await.unwrap();
    archived(&flow, &a).await;
    flow.manager.restore(a.id.clone()).await.unwrap();
    std::fs::write(folder.join("a.txt"), "A's work\n").unwrap();
    archived(&flow, &b).await;
    assert!(
        folder.join("a.txt").is_file(),
        "B's archive removed A's folder"
    );
    flow.stop().await;
}

/// A folder passed on by one delete passes on again: B, given A's, deleted while C works
/// there, never removes C's folder.
#[tokio::test]
async fn a_folder_passed_on_twice_stays_for_the_last_session() {
    let flow = Flow::start(
        "shared-inherit",
        Options::default(),
        script(Heard::default()),
    )
    .await;
    git(&flow.repo, &["branch", "feature"]);
    let a = start(&flow, "A", "feature").await.unwrap();
    let folder = worktree_of(&a).2;
    let b = start(&flow, "B", "feature").await.unwrap();
    for (one, says) in [(&a, "A: hello."), (&b, "B: hello.")] {
        flow.say_to(&one.id, says).await;
        flow.settled_in(&one.id).await;
    }
    deleted(&flow, &a).await;
    start(&flow, "C", "feature").await.unwrap();
    std::fs::write(folder.join("c.txt"), "C's work\n").unwrap();
    deleted(&flow, &b).await;
    assert!(
        folder.join("c.txt").is_file(),
        "B's delete removed C's folder"
    );
    flow.stop().await;
}
