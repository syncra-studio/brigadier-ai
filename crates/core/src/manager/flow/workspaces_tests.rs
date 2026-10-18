//! Workspaces (WORKSPACES-PLAN.md phase 2): a new chat in a project starts a workspace, more
//! task tabs join it, and each tab is a session of its own on its own worktree, so tabs of one
//! workspace work at once without sharing a file.

use std::path::PathBuf;
use std::sync::Arc;

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{ConversationId, ConversationKind, Environment, Setup, WorkspaceId};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A thread that commits `<name>.txt` on "Write <name>." in its own worktree. With `meet`,
/// the two "Write" turns wait for each other there, so they provably run at once.
fn writing(meet: Option<Arc<tokio::sync::Barrier>>) -> Script {
    script(move |turn| {
        let meet = meet.clone();
        async move {
            if !turn.is_orchestrator() {
                return Reply::text("Done.");
            }
            let Some(name) = turn
                .input
                .split("Write ")
                .nth(1)
                .and_then(|rest| rest.split('.').next())
                .map(str::to_owned)
            else {
                return Reply::text("Noted.");
            };
            if let Some(meet) = &meet {
                meet.wait().await;
            }
            commit_in_workspace(
                &turn,
                &format!("{name}.txt"),
                "text\n",
                &format!("Add {name}"),
            );
            Reply::text(format!("{name} is in."))
        }
    })
}

fn workspace_of(flow: &Flow, id: &ConversationId) -> WorkspaceId {
    flow.core
        .conversation(id)
        .unwrap()
        .workspace_id
        .expect("a tab has a workspace")
}

fn worktree(flow: &Flow, id: &ConversationId) -> (Option<PathBuf>, String, String) {
    match flow.core.conversation(id).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path, base, branch, ..
                },
            ..
        }) => (path.map(PathBuf::from), base, branch),
        other => panic!("not a worktree session: {other:?}"),
    }
}

/// A new chat in a project starts a workspace titled after it; a second tab joins it on a new
/// worktree from the same base; the two tabs work at once, each in its own folder on its own
/// branch, and neither sees the other's file.
#[tokio::test]
async fn two_tabs_of_one_workspace_work_at_once_on_their_own_worktrees() {
    let meet = Arc::new(tokio::sync::Barrier::new(2));
    let flow = Flow::start(
        "workspaces-two-tabs",
        Options::default(),
        writing(Some(meet)),
    )
    .await;
    let first = flow.conversation.clone();
    let workspace = workspace_of(&flow, &first);
    let catalog = flow.core.catalog();
    assert_eq!(catalog.workspaces.len(), 1);
    assert_eq!(catalog.workspaces[0].id, workspace);
    assert_eq!(catalog.workspaces[0].title, "Flow", "the first tab's title");
    assert_eq!(catalog.workspaces[0].base, "main");
    assert_eq!(
        catalog.workspaces[0].repos,
        vec![flow.repo.display().to_string()]
    );

    let second = flow.manager.create_tab(&workspace, None).await.unwrap();
    assert_eq!(second.workspace_id.as_ref(), Some(&workspace));
    assert_eq!(second.kind, ConversationKind::Session);
    let (_, base, branch) = worktree(&flow, &second.id);
    assert_eq!(base, "main", "from the workspace's base");
    assert_ne!(branch, worktree(&flow, &first).2, "a branch of its own");

    let (one, two) = tokio::join!(
        flow.say_to(&first, "Write alpha."),
        flow.say_to(&second.id, "Write beta.")
    );
    let ((), ()) = (one, two);
    flow.settled_in(&first).await;
    flow.settled_in(&second.id).await;

    let (Some(alpha), _, alpha_branch) = worktree(&flow, &first) else {
        panic!("the first tab's worktree");
    };
    let (Some(beta), _, beta_branch) = worktree(&flow, &second.id) else {
        panic!("the second tab's worktree");
    };
    assert_ne!(alpha, beta, "two folders");
    assert!(alpha.join("alpha.txt").exists() && !alpha.join("beta.txt").exists());
    assert!(beta.join("beta.txt").exists() && !beta.join("alpha.txt").exists());
    let files = |branch: &str| git(&flow.repo, &["ls-tree", "-r", "--name-only", branch]);
    assert!(!files(&alpha_branch).contains("beta.txt"));
    assert!(!files(&beta_branch).contains("alpha.txt"));

    let catalog = flow.core.catalog();
    assert_eq!(catalog.workspaces.len(), 1, "still one workspace");
    flow.stop().await;
}

/// ⌘⏎ starts a task in a new tab of the workspace: it runs there, on its own worktree.
#[tokio::test]
async fn a_background_tab_starts_on_its_message() {
    let flow = Flow::start("workspaces-start-tab", Options::default(), writing(None)).await;
    let workspace = workspace_of(&flow, &flow.conversation);
    let (tab, _) = flow
        .manager
        .start_tab(
            &workspace,
            None,
            "Write gamma.".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(tab.title, "Write gamma.", "titled by its message");
    flow.settled_in(&tab.id).await;
    let (Some(path), _, _) = worktree(&flow, &tab.id) else {
        panic!("the tab's worktree");
    };
    assert!(path.join("gamma.txt").exists());
    assert_ne!(Some(path), worktree(&flow, &flow.conversation).0);
    flow.stop().await;
}

/// A session from before workspaces is a workspace of one tab; a new tab in it records that
/// workspace under its implicit id, and both tabs list in it.
#[tokio::test]
async fn a_tab_joins_an_older_sessions_implicit_workspace() {
    let flow = Flow::start("workspaces-implicit", Options::default(), writing(None)).await;
    let setup = flow.core.conversation(&flow.conversation).unwrap().setup;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id;
    let older = flow
        .core
        .create_conversation(
            ConversationId::generate(),
            ConversationKind::Session,
            project,
            Some("Older".into()),
            setup,
            crate::sessions::Origin::default(),
        )
        .await
        .unwrap();
    let implicit = WorkspaceId::implicit(&older.id);
    assert_eq!(workspace_of(&flow, &older.id), implicit);

    let tab = flow.manager.create_tab(&implicit, None).await.unwrap();
    assert_eq!(tab.workspace_id.as_ref(), Some(&implicit));
    let catalog = flow.core.catalog();
    let workspace = catalog
        .workspaces
        .iter()
        .find(|workspace| workspace.id == implicit)
        .unwrap();
    assert_eq!(workspace.title, "Older");
    let tabs = catalog
        .conversations
        .iter()
        .filter(|conversation| conversation.workspace_id.as_ref() == Some(&implicit))
        .count();
    assert_eq!(tabs, 2);
    flow.stop().await;
}

/// A new chat's first message titles its workspace, and the workspace keeps that name when
/// the tab it came from is deleted.
#[tokio::test]
async fn a_workspace_keeps_its_first_tabs_title() {
    let flow = Flow::start("workspaces-title", Options::default(), writing(None)).await;
    let first = flow.core.conversation(&flow.conversation).unwrap();
    let chat = flow
        .manager
        .create_conversation(
            ConversationKind::Session,
            first.project_id.clone(),
            None,
            Some(crate::model::SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: crate::model::EnvironmentRequest::NewWorktree {
                    base: "main".into(),
                    branch: None,
                },
                permission: crate::model::PermissionLevel::ApproveForMe,
                orchestrator: match first.setup {
                    Some(Setup::Session { orchestrator, .. }) => orchestrator,
                    _ => unreachable!(),
                },
                plan_mode: false,
            }),
        )
        .await
        .unwrap();
    let workspace = workspace_of(&flow, &chat.id);
    flow.say_to(&chat.id, "Redesigning the login page").await;
    let title = flow.core.workspace(&workspace).unwrap().title;
    assert_eq!(title, flow.core.conversation(&chat.id).unwrap().title);
    assert_ne!(title, "New session");

    let tab = flow.manager.create_tab(&workspace, None).await.unwrap();
    flow.settled_in(&chat.id).await;
    flow.manager.delete(chat.id.clone()).await.unwrap();
    assert_eq!(flow.core.workspace(&workspace).unwrap().title, title);
    assert_eq!(
        flow.core.conversation(&tab.id).unwrap().workspace_id,
        Some(workspace)
    );
    flow.stop().await;
}

/// A background tab whose message can't go leaves no empty tab behind.
#[tokio::test]
async fn a_background_tab_whose_message_fails_goes() {
    let flow = Flow::start("workspaces-start-fails", Options::default(), writing(None)).await;
    let workspace = workspace_of(&flow, &flow.conversation);
    let before = flow.core.catalog().conversations.len();
    assert!(
        flow.manager
            .start_tab(&workspace, None, "  ".into(), Vec::new(), Vec::new())
            .await
            .is_err()
    );
    super::eventually("the empty tab gone", || {
        flow.core.catalog().conversations.len() == before
    })
    .await;
    flow.stop().await;
}
