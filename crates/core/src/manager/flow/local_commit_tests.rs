//! On the user's own checkout the thread doesn't commit unless asked (UI audit M6): it leaves
//! its change uncommitted and says so, and a commit the user asks for carries no Brigadier
//! trailer; the store records that the thread wrote it.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{
    ConversationKind, DomainEvent, EnvironmentRequest, ModelChoice, PermissionLevel, SetupRequest,
};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A session told the instructions before this change, on the user's own checkout: it hears
/// the note once, its edit stays uncommitted, and the commit the user then asks for has no
/// trailer and is recorded as the thread's. Brigadier's own commit, the one that adds the
/// trailer (`run_check`'s `commit`), commits nothing there, so the thread commits with git.
#[tokio::test]
async fn on_the_users_checkout_the_thread_commits_only_when_asked_and_without_a_trailer() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let checked: Arc<Mutex<Option<String>>> = Arc::default();
    let (heard, check) = (inputs.clone(), checked.clone());
    let mut flow = Flow::start(
        "local-commit",
        Options::default(),
        script(move |turn| {
            let (heard, check) = (heard.clone(), check.clone());
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("");
                }
                heard.lock().unwrap().push(turn.input.clone());
                let workspace = turn.add_dirs[0].clone();
                if turn.input.contains("Fix the typo.") {
                    std::fs::write(workspace.join("README.md"), "fixed\n").unwrap();
                    return Reply::text("Fixed the typo.\nChanged 1 file, not committed.");
                }
                if turn.input.contains("Commit it.") {
                    let reply = turn
                        .call(
                            "run_check",
                            json!({"command": "true", "commit": "Fix the typo in the README"}),
                        )
                        .await;
                    *check.lock().unwrap() = Some(reply.text);
                    // Left uncommitted by Brigadier: the thread commits with git.
                    if !git(&workspace, &["status", "--porcelain"]).is_empty() {
                        commit_in_workspace(
                            &turn,
                            "README.md",
                            "fixed\n",
                            "Fix the typo in the README",
                        );
                    }
                    return Reply::text("Committed.");
                }
                Reply::text("Fine.")
            }
        }),
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
    flow.say("Look around.").await;
    flow.settled().await;
    // It was told the instructions of the revision before.
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
    flow.restart().await;
    let start = git(&flow.repo, &["rev-parse", "HEAD"]);
    flow.say("Fix the typo.").await;
    flow.settled().await;
    // Nothing was committed on its own: the change waits in the user's checkout.
    assert_eq!(git(&flow.repo, &["rev-parse", "HEAD"]), start);
    assert_eq!(git(&flow.repo, &["status", "--porcelain"]), "M README.md");
    flow.say("Commit it.").await;
    flow.settled().await;
    let reply = checked.lock().unwrap().clone().unwrap();
    assert!(
        reply.contains("Not committed: this is the user's own checkout"),
        "{reply}"
    );
    // One commit, the thread's own, with no trailer.
    let tip = git(&flow.repo, &["rev-parse", "HEAD"]);
    assert_eq!(
        git(
            &flow.repo,
            &["log", "--format=%B", &format!("{start}..{tip}")]
        ),
        "Fix the typo in the README"
    );
    // The store says who wrote it.
    let authored: Vec<(String, String)> = flow
        .events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::ThreadCommitsSeen {
                tip,
                authored_from: Some(from),
                ..
            } => Some((from, tip)),
            _ => None,
        })
        .collect();
    assert_eq!(authored, vec![(start, tip)]);
    flow.stop().await;
}

/// On the user's own checkout the self-edit metric counts the thread's uncommitted change, and
/// the commit the user asks for, which has no trailer, as the thread's.
#[tokio::test]
async fn the_self_edit_metric_counts_the_threads_uncommitted_and_trailerless_changes() {
    let mut flow = Flow::start(
        "local-self-edits",
        Options::default(),
        script(|turn| async move {
            if !turn.is_orchestrator() {
                return Reply::text("");
            }
            let workspace = turn.add_dirs[0].clone();
            if turn.input.contains("Fix the typo.") {
                std::fs::write(workspace.join("README.md"), "fixed\nand more\n").unwrap();
                return Reply::text("Fixed the typo.\nChanged 1 file, not committed.");
            }
            if turn.input.contains("Commit it.") {
                commit_in_workspace(
                    &turn,
                    "README.md",
                    "fixed\nand more\n",
                    "Fix the typo in the README",
                );
                return Reply::text("Committed.");
            }
            Reply::text("Fine.")
        }),
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
    flow.say("Look around.").await;
    flow.settled().await;
    let before = std::fs::read_to_string(flow.repo.join("README.md")).unwrap();
    let removed = i64::try_from(before.lines().count()).unwrap();
    // Uncommitted before the request: the user's, not the thread's.
    std::fs::write(flow.repo.join("draft.txt"), "draft\n").unwrap();
    flow.say("Fix the typo.").await;
    flow.settled().await;
    // The user's own changes after it, there and in its file, aren't the thread's.
    std::fs::write(flow.repo.join("notes.txt"), "mine\nmine\nmine\n").unwrap();
    let readme = flow.repo.join("README.md");
    std::fs::write(&readme, "fixed\nand more\nmine\n").unwrap();
    let edits = flow
        .manager
        .count_thread_edits(&flow.conversation)
        .await
        .unwrap();
    assert_eq!(
        (edits.commits, edits.added, edits.removed),
        (0, 2, removed),
        "{edits:?}"
    );
    // The user commits a file of their own: the thread's change still waits.
    git(&flow.repo, &["add", "notes.txt"]);
    git(&flow.repo, &["commit", "-q", "-m", "My notes"]);
    let edits = flow
        .manager
        .count_thread_edits(&flow.conversation)
        .await
        .unwrap();
    assert_eq!(
        (edits.commits, edits.added, edits.removed),
        (0, 2, removed),
        "{edits:?}"
    );
    std::fs::write(&readme, "fixed\nand more\n").unwrap();
    flow.say("Commit it.").await;
    flow.settled().await;
    assert!(!git(&flow.repo, &["log", "-1", "--format=%B"]).contains("Brigadier-"));
    let edits = flow
        .manager
        .count_thread_edits(&flow.conversation)
        .await
        .unwrap();
    assert_eq!(
        (edits.commits, edits.added, edits.removed),
        (1, 2, removed),
        "{edits:?}"
    );
    // Once committed, the user's later edit of the same file isn't the thread's either.
    std::fs::write(&readme, "fixed\nand more\nmine\n").unwrap();
    let edits = flow
        .manager
        .count_thread_edits(&flow.conversation)
        .await
        .unwrap();
    assert_eq!(
        (edits.commits, edits.added, edits.removed),
        (1, 2, removed),
        "{edits:?}"
    );
    flow.stop().await;
}
