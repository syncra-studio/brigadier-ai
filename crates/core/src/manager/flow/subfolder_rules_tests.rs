//! Load only what applies (BUILD-PLAN D1.6): a Claude thread starts with the root's rules whole
//! and a subfolder's listed; its output hook gives it a subfolder's rules once per CLI session,
//! with the first file it touches there.

use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::HOOK_GRANT_ENV;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

const WEB_RULE: &str = "Web components stay under 200 lines.";

/// A repository with a root rule and a sub-package's rule.
fn monorepo() -> Options {
    Options {
        files: vec![
            ("AGENTS.md", "Use pnpm.\n"),
            (
                "apps/web/CLAUDE.md",
                "Web components stay under 200 lines.\n",
            ),
        ],
        ..Options::default()
    }
}

/// The session's workspace, once its thread started.
fn workspace(flow: &Flow) -> std::path::PathBuf {
    flow.manager
        .recorded_workspace(&flow.conversation)
        .expect("the session's workspace")
        .path
}

/// The newest thread session's instructions and its hook's grant.
fn newest_thread(flow: &Flow) -> (String, String) {
    let (_, spec) = flow
        .thread_specs()
        .last()
        .expect("the thread started")
        .clone();
    let hook = spec
        .output_hook
        .as_ref()
        .and_then(|hook| hook.env.iter().find(|(key, _)| key == HOOK_GRANT_ENV))
        .map(|(_, grant)| grant.clone())
        .expect("a thread's hook grant");
    (spec.append_system_prompt.unwrap_or_default(), hook)
}

#[tokio::test]
async fn a_subfolders_rules_come_once_with_the_first_file_touched_there() {
    let flow = Flow::start(
        "subfolder-rules",
        monorepo(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("Hello.").await;
    flow.settled().await;
    let workspace = workspace(&flow);
    let (prompt, hook) = newest_thread(&flow);
    assert!(prompt.contains("Use pnpm."), "the root's rules come whole");
    assert!(!prompt.contains(WEB_RULE), "{prompt}");
    assert!(
        prompt.contains("apps/web/CLAUDE.md (files under apps/web/)"),
        "listed: {prompt}"
    );
    let page = workspace
        .join("apps/web/src/page.tsx")
        .display()
        .to_string();
    let first = flow
        .manager
        .hook_rules(&hook, &page)
        .await
        .expect("a hook grant")
        .expect("the folder's rules");
    assert!(first.contains(WEB_RULE), "{first}");
    assert!(first.contains("### apps/web/CLAUDE.md"), "{first}");
    // Once per session: a second file there brings nothing new.
    let other = workspace.join("apps/web/src/nav.tsx").display().to_string();
    assert_eq!(flow.manager.hook_rules(&hook, &other).await, Some(None));
    // A root file has no subfolder rules; a grant that isn't a hook's gets nothing.
    let readme = workspace.join("README.md").display().to_string();
    assert_eq!(flow.manager.hook_rules(&hook, &readme).await, Some(None));
    assert_eq!(
        flow.manager.hook_rules("brg_not_a_grant", &page).await,
        None
    );
    flow.stop().await;
}

/// A Codex thread gets the same (#121): its hook runs from a home of its own, and reads Codex's
/// events (`--codex`); without that home (a keyring login) it gets every rule up front.
#[tokio::test]
async fn a_codex_threads_subfolder_rules_come_with_the_first_file_touched_there() {
    let flow = Flow::start(
        "subfolder-rules-codex",
        Options {
            thread: ProviderKind::Codex,
            ..monorepo()
        },
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("Hello.").await;
    flow.settled().await;
    let workspace = workspace(&flow);
    let (_, spec) = flow
        .thread_specs()
        .last()
        .expect("the thread started")
        .clone();
    let prompt = spec.append_system_prompt.clone().unwrap_or_default();
    assert!(prompt.contains("Use pnpm."), "the root's rules come whole");
    if !brigadier_providers::codex::runs_thread_hook(flow.manager.runtime.cli_env()) {
        assert!(spec.output_hook.is_none() && prompt.contains(WEB_RULE));
        flow.stop().await;
        return;
    }
    assert!(!prompt.contains(WEB_RULE), "{prompt}");
    assert!(
        prompt.contains("apps/web/CLAUDE.md (files under apps/web/)"),
        "listed: {prompt}"
    );
    let hook = spec.output_hook.as_ref().expect("a Codex thread's hook");
    assert_eq!(hook.args[..2], ["hook", "post-tool-use"]);
    assert_eq!(hook.args.last().map(String::as_str), Some("--codex"));
    let (_, grant) = newest_thread(&flow);
    let page = workspace.join("apps/web/page.tsx").display().to_string();
    let first = flow
        .manager
        .hook_rules(&grant, &page)
        .await
        .expect("a hook grant")
        .expect("the folder's rules");
    assert!(first.contains(WEB_RULE), "{first}");
    // The folder a command lists brings them too, once.
    let folder = workspace.join("apps/web").display().to_string();
    assert_eq!(flow.manager.hook_rules(&grant, &folder).await, Some(None));
    flow.stop().await;
}

const API_RULE: &str = "Handlers return typed errors.";

/// A thread's `run` and `run_unsandboxed` calls bring a subfolder's rules too (#460): its
/// `workdir` there, or a folder its command `cd`s to or names, gives them with the call's
/// result, once, kept with what its hook gave.
async fn a_runs_subfolder_rules_come_with_its_result(provider: ProviderKind) {
    let replies: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        &format!("subfolder-rules-run-{provider:?}"),
        Options {
            thread: provider,
            files: vec![
                ("AGENTS.md", "Use pnpm.\n"),
                (
                    "apps/web/CLAUDE.md",
                    "Web components stay under 200 lines.\n",
                ),
                ("apps/web/page.tsx", "export default 1;\n"),
                ("apps/api/AGENTS.md", "Handlers return typed errors.\n"),
                ("apps/api/main.ts", "export {};\n"),
                ("apps/docs/AGENTS.md", "Docs use plain words.\n"),
            ],
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() {
                    return Reply::text("Done.");
                }
                for args in [
                    serde_json::json!({ "command": "ls", "workdir": "apps/web" }),
                    serde_json::json!({ "command": "cat page.tsx", "workdir": "apps/web" }),
                    serde_json::json!({ "command": "cd apps/api && ls" }),
                    serde_json::json!({ "command": "echo hello" }),
                ] {
                    let reply = turn.call("run", args).await;
                    assert!(!reply.is_error, "{}", reply.text);
                    log.lock().unwrap().push(reply.text);
                }
                Reply::text("Ran them.")
            }
        }),
    )
    .await;
    flow.say("Look at the web app.").await;
    flow.settled().await;
    let replies = replies.lock().unwrap().clone();
    assert_eq!(replies.len(), 4, "{replies:?}");
    let (_, spec) = flow
        .thread_specs()
        .last()
        .expect("the thread started")
        .clone();
    let prompt = spec.append_system_prompt.clone().unwrap_or_default();
    if spec.output_hook.is_none() {
        // A thread without the hook has every rule up front, so a run brings none.
        assert!(
            prompt.contains(WEB_RULE) && prompt.contains(API_RULE),
            "{prompt}"
        );
        assert!(replies.iter().all(|reply| !reply.contains("[Brigadier]")));
        flow.stop().await;
        return;
    }
    assert!(!prompt.contains(WEB_RULE), "{prompt}");
    let (ls, cat, cd, echo) = (&replies[0], &replies[1], &replies[2], &replies[3]);
    assert!(ls.starts_with("[exit 0]\n"), "{ls}");
    assert!(ls.contains("page.tsx"), "{ls}");
    assert!(ls.contains(WEB_RULE), "{ls}");
    assert!(ls.contains("### apps/web/CLAUDE.md"), "{ls}");
    assert!(!ls.contains(API_RULE), "{ls}");
    // Once: the same folder again brings nothing.
    assert!(!cat.contains(WEB_RULE), "{cat}");
    assert!(cd.contains(API_RULE), "{cd}");
    assert!(!cd.contains(WEB_RULE), "{cd}");
    assert!(!echo.contains("[Brigadier]"), "{echo}");
    assert!(
        replies
            .iter()
            .all(|reply| !reply.contains("Docs use plain words."))
    );
    // The hook's bookkeeping is the same: it doesn't give them again.
    let (_, hook) = newest_thread(&flow);
    let page = workspace(&flow)
        .join("apps/web/page.tsx")
        .display()
        .to_string();
    assert_eq!(flow.manager.hook_rules(&hook, &page).await, Some(None));
    flow.stop().await;
}

#[tokio::test]
async fn a_claude_threads_run_brings_a_subfolders_rules() {
    a_runs_subfolder_rules_come_with_its_result(ProviderKind::Claude).await;
}

#[tokio::test]
async fn a_codex_threads_run_brings_a_subfolders_rules() {
    a_runs_subfolder_rules_come_with_its_result(ProviderKind::Codex).await;
}
