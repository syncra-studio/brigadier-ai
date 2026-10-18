//! The user's own CLI setup (day-one hardening, item 8): a thread and a Claude worker run with
//! the project's own setup (its hooks, MCP servers, skills: [`SessionSpec::project`]) from the
//! checkout they work in, only while the user trusts the folder; reviewers and other one-shot
//! runs never get it.

use serde_json::json;

use brigadier_providers::{ProviderKind, SessionSpec, ToolSet};

use super::auto_merge_tests::script;
use super::{Flow, Options, Reply};

fn flow_script() -> super::Script {
    script(|turn| async move {
        if turn.is_orchestrator() && turn.input.contains("Add the file.") {
            let reply = turn
                .call(
                    "delegate_task",
                    json!({"effort": "high", "title": "Add the file", "kind": "implement",
                           "spec": "Create file.txt.", "provider": "claude"}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            return Reply::text("Delegated.");
        }
        Reply::text("Done.")
    })
}

fn of(flow: &Flow, tools: ToolSet) -> Vec<SessionSpec> {
    flow.specs()
        .into_iter()
        .filter(|(provider, spec)| *provider == ProviderKind::Claude && spec.tools == tools)
        .map(|(_, spec)| spec)
        .collect()
}

#[tokio::test]
async fn a_thread_and_its_worker_get_the_projects_setup_in_a_trusted_folder() {
    let flow = Flow::start("user-setup-trusted", Options::default(), flow_script()).await;
    flow.say("Add the file.").await;
    flow.settled().await;
    let threads = of(&flow, ToolSet::Thread);
    assert!(!threads.is_empty());
    for thread in &threads {
        let workspace = thread.add_dirs.first().expect("the thread's workspace");
        assert_eq!(thread.project.as_ref(), Some(workspace), "{thread:#?}");
        assert_ne!(thread.cwd, *workspace, "it works from its own folder");
    }
    let workers = of(&flow, ToolSet::Lean);
    assert!(!workers.is_empty(), "a worker ran");
    for worker in &workers {
        let worktree = worker.add_dirs.first().expect("the worker's worktree");
        assert_eq!(worker.project.as_ref(), Some(worktree), "{worker:#?}");
    }
    // Every other run keeps the sealed setup.
    assert!(
        flow.specs()
            .iter()
            .filter(|(_, spec)| !spec.tools.user_setup())
            .all(|(_, spec)| spec.project.is_none())
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_folder_the_user_does_not_trust_loads_no_project_setup() {
    let flow = Flow::start(
        "user-setup-untrusted",
        Options {
            trusted: Some(false),
            ..Options::default()
        },
        flow_script(),
    )
    .await;
    flow.say("Add the file.").await;
    flow.settled().await;
    let specs = flow.specs();
    assert!(specs.iter().any(|(_, spec)| spec.tools == ToolSet::Thread));
    assert!(
        specs.iter().all(|(_, spec)| spec.project.is_none()),
        "{specs:#?}"
    );
    flow.stop().await;
}
