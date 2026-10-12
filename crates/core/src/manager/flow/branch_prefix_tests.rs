//! Settings › Git's branch prefix: new session and task branches take it (a project's own
//! prefix first; one git would refuse gives way, with a notice); Brigadier deletes the branches
//! its records show it made, whatever their prefix, and never a branch of the user's that only
//! looks like one of its own.

use std::path::Path;
use std::sync::Arc;

use brigadier_providers::ProviderKind;
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{
    ConversationId, ConversationKind, DomainEvent, Environment, EnvironmentRequest, KeptBranch,
    ModelChoice, PermissionLevel, ProjectPatch, Setup, SetupRequest, streams,
};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A thread that delegates one implement task when asked to add a greeting, and a worker that
/// writes hello.txt and reports.
fn delegating() -> Script {
    script(|turn| async move {
        if turn.is_orchestrator() {
            if turn.input.contains("Please add a greeting") {
                let reply = turn
                    .call(
                        "delegate_task",
                        json!({"effort": "high", "title": "Add a greeting", "kind": "implement",
                               "spec": "Create hello.txt."}),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("[quiet]");
            }
            return Reply::text("Done.");
        }
        turn.write("hello.txt", "hello\n");
        let reply = turn
            .call(
                "submit_report",
                json!({"summary": "Added hello.txt.", "changes": ["hello.txt"]}),
            )
            .await;
        assert!(!reply.is_error, "{}", reply.text);
        Reply::text("Reported.")
    })
}

async fn set_prefix(flow: &Flow, prefix: &str) {
    let mut settings = flow.core.settings();
    settings.branch_prefix = prefix.into();
    flow.core.update_settings(settings).await.unwrap();
}

fn project(flow: &Flow) -> crate::model::ProjectId {
    flow.core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id
        .unwrap()
}

/// A second session in the flow's project, on a worktree of its own.
async fn another_session(flow: &Flow) -> ConversationId {
    flow.manager
        .create_conversation(
            ConversationKind::Session,
            Some(project(flow)),
            Some("Another".into()),
            Some(SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: EnvironmentRequest::NewWorktree {
                    base: "main".into(),
                    branch: None,
                },
                permission: PermissionLevel::default(),
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
        .id
}

/// The session branch `id` was set up with.
fn session_branch(flow: &Flow, id: &ConversationId) -> String {
    match flow.core.conversation(id).unwrap().setup {
        Some(Setup::Session {
            environment: Environment::NewWorktree { branch, .. },
            ..
        }) => branch,
        other => panic!("a session with a worktree, not {other:?}"),
    }
}

fn branch_exists(repo: &Path, branch: &str) -> bool {
    !git(repo, &["branch", "--list", branch]).trim().is_empty()
}

fn owns(flow: &Flow, branch: &str) -> bool {
    flow.manager
        .runtime
        .ledger()
        .branch_ownership()
        .owns(&flow.repo, branch)
}

async fn notices(flow: &Flow) -> Vec<String> {
    flow.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::ConversationNotice { notice, .. } => Some(notice.text),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_new_session_branch_takes_the_prefix_and_archiving_deletes_it_as_recorded() {
    let flow = Flow::start("prefix-session", Options::default(), delegating()).await;
    set_prefix(&flow, "sg-").await;
    let id = another_session(&flow).await;
    let branch = session_branch(&flow, &id);
    assert_eq!(branch, format!("sg-{}/session", id.short()));
    // The flow's own session, made before, keeps its name.
    assert!(session_branch(&flow, &flow.conversation).starts_with("brigadier/"));

    flow.manager
        .send_message(
            id.clone(),
            "Hi.".into(),
            Vec::new(),
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    super::eventually("the session's branch, recorded as Brigadier's", || {
        branch_exists(&flow.repo, &branch) && owns(&flow, &branch)
    })
    .await;
    super::eventually_async("the turn to end", || async {
        !flow.manager.conversation_running(&id).await
    })
    .await;

    // Archived, its worktree goes, and so does its branch, merged and recorded as Brigadier's
    // though it isn't named brigadier/….
    flow.manager.archive(id).await.unwrap();
    super::eventually("the archived session's branch to go", || {
        !branch_exists(&flow.repo, &branch)
    })
    .await;
    flow.stop().await;
}

#[tokio::test]
async fn a_project_prefix_wins_and_one_git_refuses_gives_way_with_a_notice() {
    let flow = Flow::start("prefix-project", Options::default(), delegating()).await;
    set_prefix(&flow, "sg-").await;
    let mut prefs = flow.core.project(&project(&flow)).unwrap().prefs;
    prefs.branch_prefix = Some("team/".into());
    flow.core
        .update_project(
            project(&flow),
            ProjectPatch {
                name: None,
                repos: None,
                prefs: Some(prefs.clone()),
            },
        )
        .await
        .unwrap();
    let id = another_session(&flow).await;
    assert_eq!(
        session_branch(&flow, &id),
        format!("team/{}/session", id.short())
    );

    // A prefix git refuses isn't saved…
    let mut bad = prefs.clone();
    bad.branch_prefix = Some("team..x/".into());
    let refused = flow
        .core
        .update_project(
            project(&flow),
            ProjectPatch {
                name: None,
                repos: None,
                prefs: Some(bad.clone()),
            },
        )
        .await;
    assert!(refused.is_err(), "{refused:?}");
    // …but one stored before prefixes were checked gives way to the user's, with a notice.
    let mut stored = flow.core.project(&project(&flow)).unwrap();
    stored.prefs = bad;
    flow.core
        .record(vec![(
            streams::CATALOG.into(),
            DomainEvent::ProjectUpdated { project: stored },
        )])
        .await
        .unwrap();
    flow.say("Hello").await;
    flow.settled().await;
    flow.say("Please add a greeting").await;
    let board = flow.settled().await;
    let task = Flow::task(&board, 1);
    let branch = task
        .workspace
        .as_ref()
        .and_then(|workspace| workspace.branch.clone())
        .expect("a task branch");
    assert_eq!(
        branch,
        format!("sg-{}/task-1-add-a-greeting", flow.conversation.short())
    );
    assert!(branch_exists(&flow.repo, &branch));
    let notices = notices(&flow).await;
    assert!(
        notices
            .iter()
            .any(|text| text.contains("“team..x/” can't be used") && text.contains("“sg-”")),
        "{notices:?}"
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_pre_warm_takes_the_prefix_set_after_it_was_made() {
    let flow = Flow::start("prefix-prewarm", Options::default(), delegating()).await;
    flow.say("Hello").await;
    flow.settled().await;
    flow.manager
        .prewarm_made(&flow.conversation)
        .await
        .expect("a pre-warm");
    // The prefix changes between warming and adoption: the task's branch is named when the
    // pre-warm is adopted, and the task records the branch its worktree is really on.
    set_prefix(&flow, "sg/").await;
    flow.say("Please add a greeting").await;
    let board = flow.settled().await;
    let task = Flow::task(&board, 1);
    let workspace = task.workspace.as_ref().expect("a workspace");
    let branch = workspace.branch.clone().expect("a branch");
    assert_eq!(
        branch,
        format!("sg/{}/task-1-add-a-greeting", flow.conversation.short())
    );
    let worktree = workspace.worktree.as_deref().expect("a worktree");
    assert_eq!(
        git(Path::new(worktree), &["branch", "--show-current"]).trim(),
        branch
    );
    assert!(owns(&flow, &branch));
    flow.stop().await;
}

#[tokio::test]
async fn storage_never_deletes_a_users_branch_that_only_looks_like_brigadiers() {
    let flow = Flow::start("prefix-users", Options::default(), delegating()).await;
    set_prefix(&flow, "feature/").await;
    let id = another_session(&flow).await;
    let ours = session_branch(&flow, &id);
    assert!(ours.starts_with("feature/"), "{ours}");
    flow.manager
        .send_message(
            id.clone(),
            "Hi.".into(),
            Vec::new(),
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    super::eventually("the session's branch, recorded as Brigadier's", || {
        branch_exists(&flow.repo, &ours) && owns(&flow, &ours)
    })
    .await;

    // The user's own branch under the same prefix.
    git(&flow.repo, &["branch", "feature/x", "main"]);
    assert!(!owns(&flow, "feature/x"));
    let tip = git(&flow.repo, &["rev-parse", "feature/x"])
        .trim()
        .to_owned();
    let refused = flow
        .manager
        .clean_storage(
            crate::manager::disk::Action::DeleteBranch {
                repo: flow.repo.clone(),
                branch: KeptBranch {
                    name: "feature/x".into(),
                    target: "main".into(),
                    tip,
                },
                merged: true,
            },
            crate::manager::disk::ScanContext::default(),
        )
        .await;
    assert!(
        refused.is_err_and(|why| why.contains("not one of Brigadier's branches")),
        "the user's branch is refused"
    );
    assert!(branch_exists(&flow.repo, "feature/x"));
    super::eventually_async("the turn to end", || async {
        !flow.manager.conversation_running(&id).await
    })
    .await;
    flow.stop().await;
}
