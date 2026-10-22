//! `open_in_app` (Q90): the daemon checks the target is the session's and names it, the window
//! opens it in front or behind, and the agent hears where it went in plain words. What isn't
//! there never reaches the window; a closed window says so.

use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, git};
use crate::app_call::{AppAnswer, AppOp, DiffScope, OpenTarget};
use crate::tools::{AppToolCall, OpenDiffScope, OpenInApp, OpenInAppTarget, ToolReply};
use crate::work::ReviewScope;

fn quiet() -> Script {
    Arc::new(|_| Box::pin(async { Reply::text("") }))
}

/// The app's window: it opens every target in front when asked to show it (the user is on
/// the session), and keeps what it was asked.
fn window(flow: &Flow) -> Arc<Mutex<Vec<AppOp>>> {
    let heard: Arc<Mutex<Vec<AppOp>>> = Arc::default();
    let mut calls = flow.manager.subscribe_app();
    let manager = flow.manager.clone();
    let log = heard.clone();
    tokio::spawn(async move {
        while let Ok(call) = calls.recv().await {
            let AppOp::Open { target, show } = &call.op else {
                continue;
            };
            let tab = match target {
                OpenTarget::File { path, .. } => Some(format!("file:{path}")),
                OpenTarget::Diff { .. } => Some("review".into()),
                OpenTarget::Progress => Some("panel:reviews".into()),
                _ => None,
            };
            let answer = AppAnswer::Opened {
                tab,
                front: *show,
                kept: false,
            };
            log.lock().unwrap().push(call.op.clone());
            manager.app_answer(&call.id, answer);
        }
    });
    heard
}

async fn open(flow: &Flow, target: OpenInAppTarget, show: bool) -> ToolReply {
    flow.manager
        .app_tool(
            flow.conversation.clone(),
            AppToolCall::Open(OpenInApp { target, show }),
        )
        .await
}

#[tokio::test]
async fn open_in_app_sends_a_checked_target_and_says_where_it_opened() {
    let flow = Flow::start("open-in-app", Options::default(), quiet()).await;
    let heard = window(&flow);
    let checkout = flow.manager.checkout_dir(&flow.conversation).unwrap();

    let reply = open(
        &flow,
        OpenInAppTarget::File {
            path: "README.md".into(),
            line: Some(3),
        },
        true,
    )
    .await;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(
        reply.text,
        "Opened README.md at line 3 (tab file:README.md) in front."
    );
    let absolute = std::path::Path::new(&checkout)
        .canonicalize()
        .unwrap()
        .join("README.md");
    let reply = open(
        &flow,
        OpenInAppTarget::File {
            path: absolute.to_string_lossy().into_owned(),
            line: None,
        },
        false,
    )
    .await;
    assert_eq!(
        reply.text,
        "Opened README.md (tab file:README.md) behind the user's tab, marked new."
    );
    let reply = open(
        &flow,
        OpenInAppTarget::Diff {
            scope: OpenDiffScope::Uncommitted,
        },
        false,
    )
    .await;
    assert!(
        reply.text.starts_with("Opened the uncommitted changes"),
        "{}",
        reply.text
    );
    let reply = open(&flow, OpenInAppTarget::Progress, true).await;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(
        *heard.lock().unwrap(),
        vec![
            AppOp::Open {
                target: OpenTarget::File {
                    path: "README.md".into(),
                    line: Some(3)
                },
                show: true
            },
            AppOp::Open {
                target: OpenTarget::File {
                    path: "README.md".into(),
                    line: None
                },
                show: false
            },
            AppOp::Open {
                target: OpenTarget::Diff {
                    scope: DiffScope::Uncommitted
                },
                show: false
            },
            AppOp::Open {
                target: OpenTarget::Progress,
                show: true
            },
        ]
    );
    flow.stop().await;
}

#[tokio::test]
async fn what_the_session_doesnt_have_never_reaches_the_window() {
    let flow = Flow::start("open-in-app-refused", Options::default(), quiet()).await;
    let heard = window(&flow);
    let file = |path: &str| OpenInAppTarget::File {
        path: path.into(),
        line: None,
    };
    for (target, said) in [
        (
            file("missing.rs"),
            "isn't a file in this session's checkout",
        ),
        (
            file("../outside.rs"),
            "isn't a file in this session's checkout",
        ),
        (
            file("/etc/hosts"),
            "isn't a file in this session's checkout",
        ),
        (
            OpenInAppTarget::Output {
                id: "preview-9".into(),
            },
            "there is no preview preview-9",
        ),
        (
            OpenInAppTarget::Worker {
                task: "nobody".into(),
            },
            "there is no worker nobody",
        ),
        (
            OpenInAppTarget::Plan {
                id: "card-9".into(),
            },
            "there is no plan or outline card-9",
        ),
        (
            OpenInAppTarget::Url {
                url: "file:///etc/hosts".into(),
            },
            "isn't a web address",
        ),
    ] {
        let reply = open(&flow, target, true).await;
        assert!(reply.is_error, "{}", reply.text);
        assert!(reply.text.contains(said), "{said}: {}", reply.text);
    }
    let mut settings = flow.core.settings();
    settings.browser_enabled = false;
    flow.core.update_settings(settings).await.unwrap();
    let reply = open(
        &flow,
        OpenInAppTarget::Url {
            url: "http://127.0.0.1:5173/".into(),
        },
        true,
    )
    .await;
    assert!(
        reply.text.contains("built-in browser is off"),
        "{}",
        reply.text
    );
    assert!(heard.lock().unwrap().is_empty());
    flow.stop().await;
}

#[tokio::test]
async fn with_no_window_open_in_app_fails_in_plain_words() {
    let flow = Flow::start("open-in-app-no-window", Options::default(), quiet()).await;
    let reply = open(&flow, OpenInAppTarget::Progress, true).await;
    assert!(reply.is_error);
    assert_eq!(
        reply.text,
        "Brigadier's window isn't open, so its tabs can't be used now."
    );
    flow.stop().await;
}

/// The diff `uncommitted` opens shows staged and unstaged edits and new files together.
#[tokio::test]
async fn the_uncommitted_diff_holds_staged_unstaged_and_new_files() {
    let flow = Flow::start("open-in-app-uncommitted", Options::default(), quiet()).await;
    let checkout = std::path::PathBuf::from(flow.manager.checkout_dir(&flow.conversation).unwrap());
    std::fs::write(checkout.join("README.md"), "# Flow\nstaged line\n").unwrap();
    git(&checkout, &["add", "README.md"]);
    std::fs::write(
        checkout.join("README.md"),
        "# Flow\nstaged line\nunstaged line\n",
    )
    .unwrap();
    std::fs::write(checkout.join("new.txt"), "a new file\n").unwrap();
    let diff = flow
        .manager
        .review_diff(&flow.conversation, ReviewScope::Uncommitted, false, false)
        .await
        .unwrap();
    for line in ["+staged line", "+unstaged line", "+a new file"] {
        assert!(diff.patch.contains(line), "{line}: {}", diff.patch);
    }
    flow.stop().await;
}

async fn show(flow: &Flow, id: &str) -> ToolReply {
    flow.manager
        .app_tool(
            flow.conversation.clone(),
            AppToolCall::ShowSession(crate::tools::ShowSession { id: id.into() }),
        )
        .await
}

#[tokio::test]
async fn show_session_asks_the_window_for_another_session_and_says_so_plainly() {
    let flow = Flow::start("show-session", Options::default(), quiet()).await;
    let closed = show(&flow, "anything").await;
    assert!(closed.is_error);
    assert!(
        closed.text.contains("No session with that id"),
        "{}",
        closed.text
    );
    let other = flow
        .add_session("API work", None, brigadier_providers::ProviderKind::Claude)
        .await;
    let no_window = show(&flow, &other.0).await;
    assert!(no_window.is_error);
    assert!(
        no_window.text.contains("window isn't open"),
        "{}",
        no_window.text
    );

    let heard: Arc<Mutex<Vec<AppOp>>> = Arc::default();
    let mut calls = flow.manager.subscribe_app();
    let (manager, log) = (flow.manager.clone(), heard.clone());
    tokio::spawn(async move {
        while let Ok(call) = calls.recv().await {
            log.lock().unwrap().push(call.op.clone());
            manager.app_answer(&call.id, AppAnswer::SessionShown { visible: true });
        }
    });
    let shown = show(&flow, &other.0).await;
    assert!(!shown.is_error, "{}", shown.text);
    assert_eq!(shown.text, "Brigadier shows the session \"API work\" now.");
    let own = show(&flow, &flow.conversation.0).await;
    assert!(own.is_error);
    assert!(own.text.contains("That is this session"), "{}", own.text);
    assert_eq!(
        *heard.lock().unwrap(),
        [AppOp::ShowSession { session: other }],
        "only the real other session reached the window"
    );
    flow.stop().await;
}
