//! A new tab can run the app: its worktree gets copies of the checkout's gitignored `.env*`
//! files and of what `.worktreeinclude` names, and the thread hears so once. A tracked
//! `.brigadier/setup` is the thread's to run, as any of its commands, and only in a folder the
//! user trusts.

use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script};
use crate::manager::prompts::SETUP_LINE;

const FILES: &[(&str, &str)] = &[
    (
        ".gitignore",
        ".env*\n!.env.example\n*.log\nconfig/local.json\n",
    ),
    (".env.example", "API_KEY=\n"),
    (".env", "API_KEY=flow-secret-value-123\n"),
    ("apps/web/.env.local", "PORT=4321\n"),
    (".worktreeinclude", "config/local.json\n"),
    ("config/local.json", "{\"debug\": true}\n"),
    (".brigadier/setup", "#!/bin/sh\ntouch .setup-ran\n"),
];

/// The first turn's input and whether its worktree had each copy.
type Seen = Arc<Mutex<Vec<(String, Vec<bool>)>>>;

async fn first_turns(name: &str, trusted: bool) -> Vec<(String, Vec<bool>)> {
    let seen: Seen = Arc::default();
    let script: Script = {
        let seen = seen.clone();
        Arc::new(move |turn| {
            let seen = seen.clone();
            Box::pin(async move {
                let has = [".env", "apps/web/.env.local", "config/local.json"]
                    .map(|file| {
                        turn.cwd.join(file).is_file()
                            || turn.add_dirs.iter().any(|dir| dir.join(file).is_file())
                    })
                    .to_vec();
                seen.lock().unwrap().push((turn.input.clone(), has));
                Reply::text("Done.")
            })
        })
    };
    let options = Options {
        files: FILES.to_vec(),
        trusted: Some(trusted),
        ..Options::default()
    };
    let flow = Flow::start(name, options, script).await;
    flow.say("Start the app").await;
    flow.settled().await;
    flow.say("And again").await;
    flow.settled().await;
    // The checkout's own files are where they were, and none was committed.
    assert!(flow.repo.join(".env").is_file());
    let tracked = super::git(&flow.repo, &["ls-files"]);
    assert!(
        !tracked
            .lines()
            .any(|file| file == ".env" || file == "config/local.json"),
        "{tracked}"
    );
    flow.stop().await;
    seen.lock().unwrap().clone()
}

#[tokio::test]
async fn a_new_tab_gets_the_checkouts_env_files_and_its_setup_script_to_run() {
    let seen = first_turns("worktree-copies", true).await;
    assert_eq!(seen.len(), 2, "{seen:?}");
    let (input, has) = &seen[0];
    assert_eq!(has, &[true, true, true], "{input}");
    assert!(
        input.contains("[setup] Your worktree has copies of the user's gitignored files from their checkout, so the app runs here as it does there: `.env`, `apps/web/.env.local`, `config/local.json`."),
        "{input}"
    );
    assert!(input.contains(&format!("[setup] {SETUP_LINE}")), "{input}");
    // Once: the next message's worktree is the same one.
    assert!(!seen[1].0.contains("[setup]"), "{}", seen[1].0);
}

#[tokio::test]
async fn an_untrusted_folder_gets_the_copies_but_no_setup_script() {
    let seen = first_turns("worktree-copies-untrusted", false).await;
    let (input, has) = &seen[0];
    assert_eq!(has, &[true, true, true], "{input}");
    assert!(
        input.contains("[setup] Your worktree has copies"),
        "{input}"
    );
    assert!(!input.contains(SETUP_LINE), "{input}");
}

/// A writer's new worktree gets the setup line in its brief; a reader's never.
#[tokio::test]
async fn a_writing_worker_runs_the_setup_script_and_a_reader_doesnt() {
    let briefs: Arc<Mutex<Vec<(u32, bool)>>> = Arc::default();
    let script: Script = {
        let briefs = briefs.clone();
        Arc::new(move |turn| {
            let briefs = briefs.clone();
            Box::pin(async move {
                if let Some(number) = turn.task_number() {
                    let told = turn.prompt.contains(SETUP_LINE) || turn.input.contains(SETUP_LINE);
                    briefs.lock().unwrap().push((number, told));
                    let reply = turn
                        .call("submit_report", serde_json::json!({"summary": "Done."}))
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    return Reply::text("Reported.");
                }
                if !turn.is_orchestrator() || turn.input.contains("[report task-") {
                    return Reply::text("[quiet]");
                }
                for (kind, title) in [
                    ("implement", "Write the page"),
                    ("research", "Read the code"),
                ] {
                    let reply = turn
                        .call(
                            "delegate_task",
                            serde_json::json!({"effort": "medium", "title": title, "kind": kind, "spec": title}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("[quiet]")
            })
        })
    };
    let options = Options {
        files: FILES.to_vec(),
        ..Options::default()
    };
    let flow = Flow::start("worktree-copies-workers", options, script).await;
    let mut settings = flow.core.settings();
    settings.start_workers_on_own = true;
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Build the page").await;
    let ran = briefs.clone();
    flow.until("both workers ran", move |_| ran.lock().unwrap().len() >= 2)
        .await;
    flow.settled().await;
    flow.stop().await;
    let mut briefs = briefs.lock().unwrap().clone();
    briefs.sort();
    briefs.dedup_by_key(|(number, _)| *number);
    assert_eq!(briefs, [(1, true), (2, false)]);
}
