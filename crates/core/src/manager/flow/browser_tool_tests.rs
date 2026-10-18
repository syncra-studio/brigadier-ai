//! The browser tools (Q74): the thread's and its workers' calls reach the app's window as ops
//! on a Browser tab of their session, and the app's answers come back as text and pictures.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::browser::{
    BrowserCall, BrowserGuard, BrowserOp, BrowserPageRead, BrowserResult, BrowserSensitive,
};
use crate::tools::{BrowserNavigate, BrowserOpen, BrowserTabRef, BrowserToolCall, ToolReply};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn quiet() -> Script {
    script(|_| async { Reply::text("[quiet]") })
}

/// A 1×1 PNG.
const PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgGAWjR9awAAAABJRU5ErkJggg==";

fn page(tab: &str, text: &str) -> BrowserResult {
    BrowserResult::Page {
        tab: tab.into(),
        page: BrowserPageRead {
            url: "http://127.0.0.1:9/form".into(),
            title: "Order form".into(),
            generation: 1,
            loading: false,
            text: text.into(),
        },
    }
}

/// The app's window: hears every call, keeps it and answers as a page would.
fn window(flow: &Flow) -> Arc<Mutex<Vec<BrowserCall>>> {
    let heard: Arc<Mutex<Vec<BrowserCall>>> = Arc::default();
    let mut calls = flow.manager.subscribe_browser();
    let manager = flow.manager.clone();
    let log = heard.clone();
    tokio::spawn(async move {
        while let Ok(call) = calls.recv().await {
            let result = match &call.op {
                BrowserOp::Open { .. } => page("browser:agent-1", "[e1] button \"Order\""),
                BrowserOp::Screenshot { tab } => BrowserResult::Shot {
                    tab: tab.clone(),
                    url: "http://127.0.0.1:9/form".into(),
                    png: PNG.into(),
                },
                BrowserOp::Close { tab } => BrowserResult::Closed { tab: tab.clone() },
                BrowserOp::Click {
                    tab,
                    guard:
                        BrowserGuard {
                            declared: Some(sensitive),
                            ..
                        },
                    target,
                } => BrowserResult::NeedsApproval {
                    tab: tab.clone(),
                    url: "http://127.0.0.1:9/form".into(),
                    generation: 1,
                    sensitive: *sensitive,
                    what: format!("{target} Order"),
                },
                other => page(other.tab().unwrap_or_default(), "Thanks"),
            };
            log.lock().unwrap().push(call.clone());
            manager.browser_answer(&call.id, result);
        }
    });
    heard
}

#[tokio::test]
async fn a_worker_opens_reads_and_pictures_a_page_in_its_sessions_browser() {
    let replies: Arc<Mutex<Vec<ToolReply>>> = Arc::default();
    let log = replies.clone();
    let flow = Flow::start(
        "browser-tool-worker",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Order it.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                json!({"effort": "medium", "title": "Check the order form",
                                       "kind": "implement", "spec": "Look at the form.",
                                       "provider": "claude"}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                if turn.task_number().is_some() && turn.earlier == 0 {
                    for (name, args) in [
                        ("browser_open", json!({"url": "http://127.0.0.1:9/form"})),
                        ("browser_read", json!({"tab": "browser:agent-1"})),
                        (
                            "browser_click",
                            json!({"tab": "browser:agent-1", "target": "e1", "asks": "pay"}),
                        ),
                        ("browser_screenshot", json!({"tab": "browser:agent-1"})),
                        ("browser_close", json!({"tab": "browser:agent-1"})),
                    ] {
                        let reply = turn.call(name, args).await;
                        log.lock().unwrap().push(reply);
                    }
                    let reply = turn
                        .call(
                            "submit_report",
                            json!({"summary": "Looked.", "changes": []}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                }
                Reply::text("Done.")
            }
        }),
    )
    .await;
    let heard = window(&flow);
    flow.say("Order it.").await;
    flow.settled().await;

    let heard = heard.lock().unwrap().clone();
    let ops: Vec<_> = heard.iter().map(|call| call.op.clone()).collect();
    assert_eq!(ops.len(), 5, "{ops:#?}");
    assert_eq!(
        ops[0],
        BrowserOp::Open {
            url: "http://127.0.0.1:9/form".into()
        }
    );
    assert!(matches!(
        &ops[2],
        BrowserOp::Click {
            guard: BrowserGuard {
                declared: Some(BrowserSensitive::Pay),
                approved: None
            },
            ..
        }
    ));
    for call in &heard {
        assert_eq!(call.conversation_id, flow.conversation);
        assert!(call.task_id.is_some(), "{call:?}");
        assert_eq!(call.label, "Check the order form");
    }

    let replies = replies.lock().unwrap().clone();
    assert!(!replies[0].is_error, "{}", replies[0].text);
    assert!(
        replies[0].text.starts_with(
            "browser:agent-1 · Order form — http://127.0.0.1:9/form\n\n[e1] button \"Order\""
        ),
        "{}",
        replies[0].text
    );
    assert!(replies[2].is_error, "{}", replies[2].text);
    assert!(
        replies[2].text.contains("Paying (\"e1 Order\")"),
        "{}",
        replies[2].text
    );
    assert!(
        replies[2].text.contains("nothing was done"),
        "{}",
        replies[2].text
    );

    let shot = &replies[3];
    assert!(!shot.is_error, "{}", shot.text);
    assert_eq!(shot.images.len(), 1);
    assert_eq!(shot.images[0].mime, "image/png");
    let path = shot
        .text
        .split("Saved as ")
        .nth(1)
        .and_then(|rest| rest.split(" (attach_proof").next())
        .unwrap();
    assert!(path.ends_with(".png"), "{path}");
    assert_eq!(std::fs::read(path).unwrap(), shot.images[0].data);
    assert_eq!(replies[4].text, "Closed browser:agent-1.");
    flow.stop().await;
}

#[tokio::test]
async fn the_thread_s_calls_are_its_own_and_bad_calls_never_reach_the_app() {
    let flow = Flow::start("browser-tool-thread", Options::default(), quiet()).await;
    let heard = window(&flow);
    let call = |call| {
        flow.manager
            .browser_tool(flow.conversation.clone(), None, call)
    };

    let opened = call(BrowserToolCall::Open(BrowserOpen {
        url: " https://example.com/ ".into(),
    }))
    .await;
    assert!(!opened.is_error, "{}", opened.text);

    let not_web = call(BrowserToolCall::Open(BrowserOpen {
        url: "file:///etc/passwd".into(),
    }))
    .await;
    assert!(not_web.is_error);
    assert!(
        not_web.text.contains("isn't a web address"),
        "{}",
        not_web.text
    );

    for (url, go) in [
        (None, None),
        (
            Some("https://a.example".into()),
            Some(crate::tools::BrowserStep::Back),
        ),
    ] {
        let reply = call(BrowserToolCall::Navigate(BrowserNavigate {
            tab: "browser:agent-1".into(),
            url,
            go,
        }))
        .await;
        assert!(reply.is_error);
        assert!(
            reply.text.contains("either `url` or `go`"),
            "{}",
            reply.text
        );
    }

    let back = call(BrowserToolCall::Navigate(BrowserNavigate {
        tab: "browser:agent-1".into(),
        url: None,
        go: Some(crate::tools::BrowserStep::Back),
    }))
    .await;
    assert!(!back.is_error, "{}", back.text);

    let heard = heard.lock().unwrap().clone();
    let ops: Vec<_> = heard.iter().map(|call| call.op.clone()).collect();
    assert_eq!(
        ops,
        [
            BrowserOp::Open {
                url: "https://example.com/".into()
            },
            BrowserOp::Go {
                tab: "browser:agent-1".into(),
                action: crate::browser::BrowserGo::Back
            }
        ]
    );
    assert!(heard.iter().all(|call| call.task_id.is_none()));
    flow.stop().await;
}

#[tokio::test]
async fn with_the_browser_off_or_no_window_open_a_call_says_so() {
    let flow = Flow::start("browser-tool-off", Options::default(), quiet()).await;
    let read = || {
        BrowserToolCall::Read(BrowserTabRef {
            tab: "browser:agent-1".into(),
        })
    };
    let reply = flow
        .manager
        .browser_tool(flow.conversation.clone(), None, read())
        .await;
    assert!(reply.is_error);
    assert!(reply.text.contains("window isn't open"), "{}", reply.text);

    let mut settings = flow.core.settings();
    settings.browser_enabled = false;
    flow.core.update_settings(settings).await.unwrap();
    let heard = window(&flow);
    let reply = flow
        .manager
        .browser_tool(flow.conversation.clone(), None, read())
        .await;
    assert!(reply.is_error);
    assert!(
        reply.text.contains("built-in browser is off"),
        "{}",
        reply.text
    );
    assert!(heard.lock().unwrap().is_empty());
    flow.stop().await;
}
