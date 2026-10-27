//! The user's own CLI setup (day-one hardening, item 8): a thread and a Claude worker run with
//! the project's own setup (its hooks, MCP servers, skills: [`SessionSpec::project`]) from the
//! checkout they work in, only while the user trusts the folder; reviewers and other one-shot
//! runs never get it.

use brigadier_providers::{ProviderKind, SessionSpec, ToolSet};

use super::auto_merge_tests::script;
use super::{Flow, Options, Reply};

fn flow_script() -> super::Script {
    script(|turn| async move {
        if turn.is_orchestrator() && turn.input.contains("Add the file.") {
            turn.start_worker("Add the file", "Create file.txt.").await;
            return Reply::text("Started a worker.");
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
        assert_eq!(thread.project.as_ref(), Some(&thread.cwd), "{thread:#?}");
        assert_eq!(
            thread.add_dirs,
            vec![
                flow.manager.session_dir(&flow.conversation).record(),
                flow.manager
                    .session_dir(&flow.conversation)
                    .root()
                    .join("overlay")
            ],
            "it works in its workspace and writes its ledger"
        );
        assert!(
            thread.add_dirs[1]
                .join(".claude/skills/brigade/SKILL.md")
                .is_file()
        );
    }
    let workers = of(&flow, ToolSet::Lean);
    assert!(!workers.is_empty(), "a worker ran");
    for worker in &workers {
        // It starts in its worktree, as a CLI started there would.
        assert_eq!(worker.project.as_ref(), Some(&worker.cwd), "{worker:#?}");
        // It may write its own ledger, which sits outside its worktree (S17).
        let ledger = worker
            .env
            .iter()
            .find(|(key, _)| key == "BRIGADIER_LEDGER")
            .map(|(_, value)| std::path::PathBuf::from(value))
            .expect("its ledger");
        assert_eq!(worker.add_dirs, vec![ledger.parent().unwrap().to_owned()]);
        assert!(worker.add_dirs[0].is_dir());
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
