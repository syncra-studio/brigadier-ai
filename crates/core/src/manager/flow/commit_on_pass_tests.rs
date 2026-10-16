//! The thread may commit in the same call as its last check (BUILD-PLAN C9, Q50): `run_check`'s
//! `commit` commits everything it changed, with its trailer, only when the check passes. It is
//! the thread's decision: nothing commits on its own, a failing check commits nothing, and on
//! the user's own checkout it commits nothing. Rule R: a session from before hears it once.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::manager::prompts::{COMMIT_ON_PASS_REVISION, THREAD_TRAILER, revision_note};
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

/// What the thread's scripted turns write and call: a failing check with a commit, then a
/// passing one. Each run_check reply goes into `replies`, each turn's input into `inputs`.
fn edits(inputs: Log, replies: Log) -> Script {
    script(move |turn| {
        let (inputs, replies) = (inputs.clone(), replies.clone());
        async move {
            if !turn.is_orchestrator() {
                return Reply::text("");
            }
            inputs.lock().unwrap().push(turn.input.clone());
            if !turn.input.contains("Add the notes file.") {
                return Reply::text("Fine.");
            }
            std::fs::write(turn.add_dirs[0].join("NOTES.md"), "notes\n").unwrap();
            for command in ["false", "true"] {
                let reply = turn
                    .call(
                        "run_check",
                        json!({"command": command, "commit": "Add the notes file"}),
                    )
                    .await;
                replies.lock().unwrap().push(reply.text);
            }
            Reply::text("Added NOTES.md.")
        }
    })
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
                        told: Some(crate::work::Told {
                            revision: Some(COMMIT_ON_PASS_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
}

/// On the session's own worktree, a session from before hears the note once. Its failing
/// check commits nothing and its passing one commits the change with its message and the
/// trailer, leaving nothing uncommitted.
#[tokio::test]
async fn a_passing_check_commits_the_threads_change_and_a_failing_one_nothing() {
    let (inputs, replies): (Log, Log) = (Arc::default(), Arc::default());
    let mut flow = Flow::start(
        "commit-on-pass",
        Options::default(),
        edits(inputs.clone(), replies.clone()),
    )
    .await;
    flow.say("Look around.").await;
    flow.settled().await;
    told_before(&flow).await;
    flow.restart().await;
    flow.say("Add the notes file.").await;
    flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert_eq!(replies.len(), 2, "{replies:#?}");
    assert!(
        replies[0].contains("Not committed: the check didn't pass."),
        "{}",
        replies[0]
    );
    assert!(
        replies[1].contains(
            "\"Add the notes file\" with your trailer: 1 file changed, 1 insertion(+) (NOTES.md). Nothing is left uncommitted."
        ),
        "{}",
        replies[1]
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
    let note = revision_note(COMMIT_ON_PASS_REVISION - 1, false).unwrap();
    assert!(note.contains("give your last run_check `commit"), "{note}");
    let inputs = inputs.lock().unwrap().clone();
    let told: usize = inputs
        .iter()
        .map(|input| input.matches(&note).count())
        .sum();
    assert_eq!(told, 1, "{inputs:#?}");
    flow.stop().await;
}

/// On the user's own checkout, `commit` commits nothing even on a pass, and says why.
#[tokio::test]
async fn on_the_users_checkout_a_check_commits_nothing() {
    let (inputs, replies): (Log, Log) = (Arc::default(), Arc::default());
    let mut flow = Flow::start(
        "commit-on-pass-local",
        Options::default(),
        edits(inputs, replies.clone()),
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
    let replies = replies.lock().unwrap().clone();
    assert!(
        replies[1].contains("Not committed: this is the user's own checkout"),
        "{}",
        replies[1]
    );
    assert_eq!(git(&flow.repo, &["rev-parse", "HEAD"]), start);
    assert_eq!(git(&flow.repo, &["status", "--porcelain"]), "?? NOTES.md");
    flow.stop().await;
}

/// A file that changed while the check ran (here the check itself writes it, as an edit made
/// while a check runs in the background would) isn't committed under a check that didn't see
/// it: nothing is, and the reply says which file. The check run again commits it all.
#[tokio::test]
async fn a_file_changed_while_the_check_ran_is_not_committed_under_it() {
    let replies: Log = Arc::default();
    let seen = replies.clone();
    let flow = Flow::start(
        "commit-on-pass-changed",
        Options::default(),
        script(move |turn| {
            let replies = seen.clone();
            async move {
                if !turn.is_orchestrator() || !turn.input.contains("Add the notes file.") {
                    return Reply::text("Fine.");
                }
                std::fs::write(turn.add_dirs[0].join("NOTES.md"), "notes\n").unwrap();
                for command in ["echo later > LATER.md", "true"] {
                    let reply = turn
                        .call(
                            "run_check",
                            json!({"command": command, "commit": "Add the notes file"}),
                        )
                        .await;
                    replies.lock().unwrap().push(reply.text);
                }
                Reply::text("Added NOTES.md.")
            }
        }),
    )
    .await;
    flow.say("Add the notes file.").await;
    flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert_eq!(replies.len(), 2, "{replies:#?}");
    assert!(
        replies[0].contains(
            "Not committed: LATER.md changed while the check ran, so it didn't check them."
        ),
        "{}",
        replies[0]
    );
    assert!(
        replies[1].contains("(LATER.md, NOTES.md). Nothing is left uncommitted."),
        "{}",
        replies[1]
    );
    flow.stop().await;
}
