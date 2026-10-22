//! The thread may commit in the same call as its last check (BUILD-PLAN C9, Q50): `run_check`'s
//! `commit` commits everything it changed, with its trailer, only when the check passes. It is
//! the thread's decision: nothing commits on its own, a failing check commits nothing, and on
//! the user's own checkout it commits nothing.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::manager::prompts::THREAD_TRAILER;
use crate::model::{
    ConversationKind, DomainEvent, EnvironmentRequest, ModelChoice, PermissionLevel, SetupRequest,
};

/// What a scripted turn writes down as it goes.
type Log = Arc<Mutex<Vec<String>>>;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// What a run_check reply or a later turn's input says when it went to the background: its
/// result then comes as a `[run-N finished …]` message, which `seen` gets too.
fn waits(text: &str) -> bool {
    text.contains("goes on in the background")
}

/// The thread's scripted turns: on `ask`, write `file` and run `command` with a commit, one
/// check a turn. Every run_check reply and every later turn's input goes into `seen`, so a
/// check's result is there whether it came in the call or from the background.
fn checks(asks: &'static [(&'static str, &'static str, &'static str)], seen: Log) -> Script {
    script(move |turn| {
        let seen = seen.clone();
        async move {
            if !turn.is_orchestrator() {
                return Reply::text("");
            }
            seen.lock().unwrap().push(turn.input.clone());
            let Some((_, file, command)) = asks.iter().find(|(ask, ..)| turn.input.contains(ask))
            else {
                return Reply::text("Fine.");
            };
            std::fs::write(turn.add_dirs[0].join(file), "notes\n").unwrap();
            let reply = turn
                .call(
                    "run_check",
                    json!({"command": command, "commit": "Add the notes file"}),
                )
                .await;
            let quiet = waits(&reply.text);
            seen.lock().unwrap().push(reply.text);
            Reply::text(if quiet { "" } else { "Checked." })
        }
    })
}

/// A failing check with a commit, then a passing one.
const FAIL_THEN_PASS: &[(&str, &str, &str)] = &[
    ("Add the notes file.", "NOTES.md", "false"),
    ("Check it again.", "NOTES.md", "true"),
];

/// The one entry of `seen` that holds `text`.
fn said(seen: &Log, text: &str) -> String {
    let seen = seen.lock().unwrap().clone();
    seen.iter()
        .find(|said| said.contains(text))
        .cloned()
        .unwrap_or_else(|| panic!("nothing said {text:?}: {seen:#?}"))
}

/// Marks the session as told the instructions before this change.
async fn told_before(flow: &Flow) {
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
                    },
                },
            },
        )])
        .await
        .unwrap();
}

/// On the session's own worktree, a failing check commits nothing and its passing one commits the change with its message and the
/// trailer, leaving nothing uncommitted.
#[tokio::test]
async fn a_passing_check_commits_the_threads_change_and_a_failing_one_nothing() {
    let seen: Log = Arc::default();
    let mut flow = Flow::start(
        "commit-on-pass",
        Options::default(),
        checks(FAIL_THEN_PASS, seen.clone()),
    )
    .await;
    flow.say("Look around.").await;
    flow.settled().await;
    told_before(&flow).await;
    flow.restart().await;
    flow.say("Add the notes file.").await;
    flow.settled().await;
    said(&seen, "Not committed: the check didn't pass.");
    flow.say("Check it again.").await;
    flow.settled().await;
    said(
        &seen,
        "\"Add the notes file\" with your trailer: 1 file changed, 1 insertion(+) (NOTES.md). Nothing is left uncommitted.",
    );
    // The reply says nothing more runs once the work is done (BUILD-PLAN O3).
    said(
        &seen,
        &format!(
            "Nothing is left uncommitted. {}",
            crate::manager::checks::AFTER_COMMIT
        ),
    );
    let workspace = flow
        .thread_specs()
        .last()
        .unwrap()
        .1
        .add_dirs
        .first()
        .cloned()
        .unwrap();
    let message = git(&workspace, &["log", "-1", "--format=%B"]);
    assert!(message.starts_with("Add the notes file"), "{message}");
    assert!(message.contains(THREAD_TRAILER), "{message}");
    assert_eq!(git(&workspace, &["status", "--porcelain"]), "");
    assert_eq!(
        git(&workspace, &["show", "--format=", "--name-only", "HEAD"]),
        "NOTES.md"
    );
    flow.stop().await;
}

/// On the user's own checkout, `commit` commits nothing even on a pass, and says why.
#[tokio::test]
async fn on_the_users_checkout_a_check_commits_nothing() {
    let seen: Log = Arc::default();
    let mut flow = Flow::start(
        "commit-on-pass-local",
        Options::default(),
        checks(&[("Add the notes file.", "NOTES.md", "true")], seen.clone()),
    )
    .await;
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
                permission: PermissionLevel::FullAccess,
                orchestrator: ModelChoice {
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
    let start = git(&flow.repo, &["rev-parse", "HEAD"]);
    flow.say("Add the notes file.").await;
    flow.settled().await;
    said(&seen, "Not committed: this is the user's own checkout");
    assert_eq!(git(&flow.repo, &["rev-parse", "HEAD"]), start);
    assert_eq!(git(&flow.repo, &["status", "--porcelain"]), "?? NOTES.md");
    flow.stop().await;
}

/// A file the check started with that changed while it ran (here the check itself edits it,
/// as an edit made while a check runs in the background would) isn't committed under a check
/// that didn't see it: nothing is, and the reply says which file. The check run again commits
/// it all.
#[tokio::test]
async fn a_file_changed_while_the_check_ran_is_not_committed_under_it() {
    let seen: Log = Arc::default();
    let flow = Flow::start(
        "commit-on-pass-changed",
        Options::default(),
        checks(
            &[
                ("Add the notes file.", "NOTES.md", "echo later >> NOTES.md"),
                ("Check it again.", "LATER.md", "true"),
            ],
            seen.clone(),
        ),
    )
    .await;
    flow.say("Add the notes file.").await;
    flow.settled().await;
    said(
        &seen,
        "Not committed: NOTES.md changed while the check ran, so it didn't check them.",
    );
    flow.say("Check it again.").await;
    flow.settled().await;
    said(&seen, "(LATER.md, NOTES.md). Nothing is left uncommitted.");
    flow.stop().await;
}

/// A file new since the check started, here one the check itself writes (a test runner's
/// fresh snapshot), doesn't hold the commit back: it is committed with the change, and the
/// reply names it (#143).
#[tokio::test]
async fn a_file_the_check_itself_made_is_committed_with_it() {
    let seen: Log = Arc::default();
    let flow = Flow::start(
        "commit-on-pass-made",
        Options::default(),
        checks(
            &[("Add the notes file.", "NOTES.md", "echo snap > SNAP.md")],
            seen.clone(),
        ),
    )
    .await;
    flow.say("Add the notes file.").await;
    flow.settled().await;
    said(
        &seen,
        "(NOTES.md, SNAP.md). Nothing is left uncommitted. New while the check ran, so committed as it left them: SNAP.md.",
    );
    flow.stop().await;
}

/// A hook stopped after git made the commit (a post-commit hook past the commit's time limit)
/// leaves the commit made: the reply says it was, and what stopped after it.
#[tokio::test]
async fn a_commit_whose_hook_is_stopped_after_it_says_it_was_made() {
    let seen: Log = Arc::default();
    let flow = Flow::start(
        "commit-on-pass-hook",
        Options::default(),
        checks(&[("Add the notes file.", "NOTES.md", "true")], seen.clone()),
    )
    .await;
    let hook = flow.repo.join(".git/hooks/post-commit");
    std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
    std::fs::write(&hook, "#!/bin/sh\nsleep 30\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    flow.say("Add the notes file.").await;
    flow.settled().await;
    let line = said(&seen, "\"Add the notes file\" with your trailer");
    assert!(
        line.contains("Nothing is left uncommitted. Then git commit timed out after 5 s."),
        "{line}"
    );
    flow.stop().await;
}
