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

/// What the thread was told, once it was told `what`.
async fn heard(inputs: &Arc<Mutex<Vec<String>>>, what: &str) -> Vec<String> {
    for _ in 0..200 {
        let told = inputs.lock().unwrap().clone();
        if told.iter().any(|input| input.contains(what)) {
            return told;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    inputs.lock().unwrap().clone()
}

/// A quiet thread that keeps what it was told.
fn listening() -> (Script, Arc<Mutex<Vec<String>>>) {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let script = script(move |turn| {
        let log = log.clone();
        async move {
            if turn.is_orchestrator() {
                log.lock().unwrap().push(turn.input.clone());
            }
            Reply::text("[quiet]")
        }
    });
    (script, inputs)
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
                            json!({"tab": "browser:agent-1", "target": "e1"}),
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
                declared: None,
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
    assert!(!replies[2].is_error, "{}", replies[2].text);
    assert!(
        replies[2].text.ends_with("\n\nThanks"),
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

/// A shop page that asks first unless the yes was given on the page it is on now; `moved`
/// loads it again (a new page) before the next call.
struct Shop {
    generation: std::sync::atomic::AtomicU64,
    heard: Mutex<Vec<BrowserOp>>,
}

fn shop(flow: &Flow) -> Arc<Shop> {
    let shop = Arc::new(Shop {
        generation: std::sync::atomic::AtomicU64::new(1),
        heard: Mutex::default(),
    });
    let mut calls = flow.manager.subscribe_browser();
    let (manager, page_of) = (flow.manager.clone(), shop.clone());
    tokio::spawn(async move {
        while let Ok(call) = calls.recv().await {
            let generation = page_of.generation.load(std::sync::atomic::Ordering::SeqCst);
            page_of.heard.lock().unwrap().push(call.op.clone());
            let url = "https://shop.example/checkout".to_owned();
            let result = match &call.op {
                BrowserOp::Click { tab, guard, target }
                | BrowserOp::Type {
                    tab, guard, target, ..
                } => {
                    let approved = guard
                        .approved
                        .as_ref()
                        .is_some_and(|yes| yes.generation == generation && yes.url == url);
                    let sensitive = match &call.op {
                        BrowserOp::Type { .. } => BrowserSensitive::Credentials,
                        _ => guard.declared.unwrap_or(BrowserSensitive::Pay),
                    };
                    if approved {
                        page(tab, "Thanks for your order")
                    } else {
                        BrowserResult::NeedsApproval {
                            tab: tab.clone(),
                            url: url.clone(),
                            generation,
                            sensitive,
                            what: format!("button “Pay $5” ({target})"),
                        }
                    }
                }
                other => page(other.tab().unwrap_or_default(), "Checkout"),
            };
            manager.browser_answer(&call.id, result);
        }
    });
    shop
}

async fn browser_cards(flow: &Flow) -> Vec<crate::work::Approval> {
    let board = flow.core.board(&flow.conversation).await.unwrap();
    let mut cards: Vec<_> = board
        .approvals
        .values()
        .filter(|card| matches!(card.subject, crate::work::ApprovalSubject::Browser { .. }))
        .cloned()
        .collect();
    cards.sort_by_key(|card| card.created_at_ms);
    cards
}

fn pay(tab: &str) -> BrowserToolCall {
    BrowserToolCall::Click(crate::tools::BrowserClick {
        tab: tab.into(),
        target: "e4".into(),
        asks: Some(crate::tools::BrowserAsks::Pay),
    })
}

/// Waits for the `count`th browser card.
async fn card_number(flow: &Flow, count: usize) -> crate::work::Approval {
    for _ in 0..200 {
        let cards = browser_cards(flow).await;
        if cards.len() >= count {
            return cards[count - 1].clone();
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    panic!("no browser card number {count}");
}

fn allow() -> brigadier_providers::ApprovalDecision {
    brigadier_providers::ApprovalDecision::Allow
}

fn approved_of(shop: &Shop) -> Vec<Option<crate::browser::BrowserApproved>> {
    shop.heard
        .lock()
        .unwrap()
        .iter()
        .map(|op| match op {
            BrowserOp::Click { guard, .. } | BrowserOp::Type { guard, .. } => {
                guard.approved.clone()
            }
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn paying_waits_for_the_user_and_their_yes_lets_that_call_through_once() {
    let (thread, inputs) = listening();
    let flow = Flow::start("browser-tool-pay", Options::default(), thread).await;
    let shop = shop(&flow);
    let manager = flow.manager.clone();
    let conversation = flow.conversation.clone();
    let paying = tokio::spawn(async move {
        manager
            .browser_tool(conversation, None, pay("browser:agent-1"))
            .await
    });

    let card = card_number(&flow, 1).await;
    assert_eq!(card.task_id, None);
    let crate::work::ApprovalSubject::Browser {
        question,
        details,
        generation,
        ..
    } = &card.subject
    else {
        unreachable!()
    };
    assert_eq!(question, "Pay on shop.example?");
    assert_eq!(
        details,
        "Clicks button “Pay $5” (e4).\nOn https://shop.example/checkout"
    );
    assert_eq!(*generation, 1);
    assert!(!paying.is_finished(), "the call waits for the user");

    flow.manager
        .answer_card(flow.conversation.clone(), card.id.clone(), allow())
        .await
        .unwrap();
    let done = paying.await.unwrap();
    assert!(!done.is_error, "{}", done.text);
    assert!(done.text.contains("Thanks for your order"), "{}", done.text);
    let yes = crate::browser::BrowserApproved {
        generation: 1,
        url: "https://shop.example/checkout".into(),
    };
    assert_eq!(approved_of(&shop), [None, Some(yes)]);
    // The call itself heard the yes: the thread isn't told it too.
    assert!(
        !inputs
            .lock()
            .unwrap()
            .iter()
            .any(|input| input.contains("[decision]"))
    );

    // The yes is spent: the same call asks again.
    flow.manager
        .set_question_timeout(std::time::Duration::from_millis(200));
    let again = flow
        .manager
        .browser_tool(flow.conversation.clone(), None, pay("browser:agent-1"))
        .await;
    assert!(
        again
            .text
            .starts_with("No answer yet: Pay on shop.example? Nothing was done."),
        "{}",
        again.text
    );
    assert_eq!(browser_cards(&flow).await.len(), 2);
    flow.stop().await;
}

#[tokio::test]
async fn an_answer_after_the_call_gave_up_comes_as_a_message_and_holds_on_that_page_only() {
    let (thread, inputs) = listening();
    let flow = Flow::start("browser-tool-late", Options::default(), thread).await;
    flow.manager
        .set_question_timeout(std::time::Duration::from_millis(200));
    let shop = shop(&flow);
    let call = |call| {
        flow.manager
            .browser_tool(flow.conversation.clone(), None, call)
    };

    let first = call(pay("browser:agent-1")).await;
    assert!(
        first
            .text
            .starts_with("No answer yet: Pay on shop.example?"),
        "{}",
        first.text
    );
    // Trying again before the user answered asks on the same card.
    call(pay("browser:agent-1")).await;
    let cards = browser_cards(&flow).await;
    assert_eq!(cards.len(), 1, "{cards:#?}");

    flow.manager
        .answer_card(flow.conversation.clone(), cards[0].id.clone(), allow())
        .await
        .unwrap();
    let told = heard(&inputs, "said yes").await;
    assert!(
        told.iter().any(|input| input.contains("[decision] The user said yes: Pay on shop.example? Make the same browser call on browser:agent-1 again now")),
        "{told:#?}"
    );

    // The page loaded again before the agent made its call again: the yes is void, it asks anew.
    shop.generation
        .store(2, std::sync::atomic::Ordering::SeqCst);
    let moved = call(pay("browser:agent-1")).await;
    assert!(moved.text.starts_with("No answer yet"), "{}", moved.text);
    let cards = browser_cards(&flow).await;
    assert_eq!(cards.len(), 2);
    assert!(matches!(
        &cards[1].subject,
        crate::work::ApprovalSubject::Browser { generation: 2, .. }
    ));
    assert_eq!(
        approved_of(&shop).last().cloned().flatten(),
        Some(crate::browser::BrowserApproved {
            generation: 1,
            url: "https://shop.example/checkout".into()
        })
    );

    flow.manager
        .answer_card(
            flow.conversation.clone(),
            cards[1].id.clone(),
            brigadier_providers::ApprovalDecision::Deny {
                message: "not that one".into(),
            },
        )
        .await
        .unwrap();
    let told = heard(&inputs, "said no").await;
    assert!(
        told.iter().any(|input| input.contains(
            "[decision] The user said no: Pay on shop.example? They said: not that one Don't do it"
        )),
        "{told:#?}"
    );
    // A no grants nothing.
    let after = call(pay("browser:agent-1")).await;
    assert!(after.text.starts_with("No answer yet"), "{}", after.text);
    assert_eq!(approved_of(&shop).last().cloned().flatten(), None);
    flow.stop().await;
}

#[tokio::test]
async fn a_no_while_the_call_waits_is_its_answer() {
    let flow = Flow::start("browser-tool-no", Options::default(), quiet()).await;
    let shop = shop(&flow);
    let manager = flow.manager.clone();
    let conversation = flow.conversation.clone();
    let sending = tokio::spawn(async move {
        let send = BrowserToolCall::Type(crate::tools::BrowserType {
            tab: "browser:agent-1".into(),
            target: "e7".into(),
            text: "See you at 6".into(),
            submit: true,
            asks: Some(crate::tools::BrowserAsks::Send),
        });
        manager.browser_tool(conversation, None, send).await
    });
    let card = card_number(&flow, 1).await;
    let crate::work::ApprovalSubject::Browser { details, .. } = &card.subject else {
        unreachable!()
    };
    // The shop fakes every typed field as a password; a real send shows its text.
    assert!(!details.contains("See you at 6"), "{details}");
    flow.manager
        .answer_card(
            flow.conversation.clone(),
            card.id.clone(),
            brigadier_providers::ApprovalDecision::Deny {
                message: String::new(),
            },
        )
        .await
        .unwrap();
    let reply = sending.await.unwrap();
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(
        reply.text,
        "The user said no: Enter your password on shop.example? Nothing was done. Don't do it, or anything else that does the same."
    );
    assert_eq!(approved_of(&shop), [None]);
    flow.stop().await;
}

#[test]
fn a_card_shows_what_is_sent_but_never_a_secret() {
    use crate::browser::BrowserSensitive as S;
    let typing = |text: &str| BrowserOp::Type {
        tab: "t".into(),
        target: "e7".into(),
        text: text.into(),
        submit: true,
        guard: BrowserGuard::default(),
    };
    let url = "https://mail.example/compose";
    assert_eq!(
        crate::manager::browser_tool::details(
            &typing("See you at 6"),
            S::Send,
            "textbox “Message”",
            url
        ),
        "Types “See you at 6” into textbox “Message”, then presses Enter.\nOn https://mail.example/compose"
    );
    for secret in [S::Credentials, S::Card, S::OneTimeCode] {
        let shown = crate::manager::browser_tool::details(
            &typing("4242 4242"),
            secret,
            "textbox “Card”",
            url,
        );
        assert!(!shown.contains("4242"), "{shown}");
    }
    assert_eq!(
        crate::manager::browser_tool::question(S::Send, url),
        "Send something in your name on mail.example?"
    );
    assert_eq!(
        crate::manager::browser_tool::question(S::SignIn, "https://user@id.example:8443/login?x=1"),
        "Sign in to id.example:8443?"
    );
}

#[tokio::test]
async fn a_worker_waits_for_the_user_s_answer_to_a_password() {
    let typed: Arc<Mutex<Vec<ToolReply>>> = Arc::default();
    let log = typed.clone();
    let flow = Flow::start(
        "browser-tool-password",
        Options::default(),
        script(move |turn| {
            let log = log.clone();
            async move {
                if turn.is_orchestrator() {
                    if turn.input.contains("Sign in.") {
                        turn.call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Sign in to the shop",
                                   "kind": "implement", "spec": "Sign in.", "provider": "claude"}),
                        )
                        .await;
                    }
                    return Reply::text("[quiet]");
                }
                if turn.task_number().is_none() || turn.earlier > 0 {
                    return Reply::text("ok");
                }
                let password = json!({"tab": "browser:agent-1", "target": "e9", "text": "hunter2", "submit": true});
                let reply = turn.call("browser_type", password).await;
                log.lock().unwrap().push(reply);
                turn.call("submit_report", json!({"summary": "Signed in.", "changes": []})).await;
                Reply::text("Done.")
            }
        }),
    )
    .await;
    let shop = shop(&flow);
    flow.say("Sign in.").await;
    let card = card_number(&flow, 1).await;
    let crate::work::ApprovalSubject::Browser {
        question, details, ..
    } = &card.subject
    else {
        unreachable!()
    };
    assert!(card.task_id.is_some());
    assert_eq!(question, "Enter your password on shop.example?");
    assert_eq!(
        details,
        "Types into button “Pay $5” (e9), then presses Enter.\nOn https://shop.example/checkout"
    );
    let board = flow
        .until("the worker waiting", |board| {
            board.tasks.values().any(|task| {
                task.blocked_reason.as_deref()
                    == Some("Waiting for your answer: Enter your password on shop.example?")
            })
        })
        .await;
    assert_eq!(board.tasks.len(), 1);

    flow.manager
        .answer_card(flow.conversation.clone(), card.id.clone(), allow())
        .await
        .unwrap();
    flow.settled().await;
    let typed = typed.lock().unwrap().clone();
    assert_eq!(typed.len(), 1, "{typed:#?}");
    assert!(
        typed[0].text.contains("Thanks for your order"),
        "{}",
        typed[0].text
    );
    assert!(matches!(approved_of(&shop).as_slice(), [None, Some(_)]));
    flow.stop().await;
}

/// Q82: with merging on its own, the thread proves a web change with the browser tools: the
/// preview's page opened in a background tab, the change used, its screenshot attached to the
/// merge message as proof.
#[tokio::test]
async fn the_thread_attaches_a_preview_s_screenshot_from_the_browser_tool_as_merge_proof() {
    let said: Arc<Mutex<Vec<ToolReply>>> = Arc::default();
    let log = said.clone();
    let flow = Flow::start(
        "browser-tool-proof",
        Options {
            auto_merge: true,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                if !turn.is_orchestrator() || !turn.input.contains("Ship the dark mode.") {
                    return Reply::text("Done.");
                }
                let mut replies = Vec::new();
                for (name, args) in [
                    (
                        "browser_open",
                        json!({"url": "http://127.0.0.1:5173/settings"}),
                    ),
                    (
                        "browser_click",
                        json!({"tab": "browser:agent-1", "target": "e1"}),
                    ),
                    ("browser_screenshot", json!({"tab": "browser:agent-1"})),
                ] {
                    replies.push(turn.call(name, args).await);
                }
                let path = replies[2]
                    .text
                    .split("Saved as ")
                    .nth(1)
                    .and_then(|rest| rest.split(" (attach_proof").next())
                    .unwrap_or_default()
                    .to_owned();
                replies.push(
                    turn.call(
                        "attach_proof",
                        json!({"path": path, "caption": "Dark mode on in Settings"}),
                    )
                    .await,
                );
                log.lock().unwrap().extend(replies);
                Reply::text("Merged: dark mode, with its proof.")
            }
        }),
    )
    .await;
    let heard = window(&flow);
    flow.say("Ship the dark mode.").await;
    let board = flow.settled().await;
    let said = said.lock().unwrap().clone();
    assert_eq!(said.len(), 4, "{said:#?}");
    for reply in &said {
        assert!(!reply.is_error, "{}", reply.text);
    }
    assert!(
        said[3].text.starts_with("Attached shot-"),
        "{}",
        said[3].text
    );
    let ops: Vec<_> = heard
        .lock()
        .unwrap()
        .iter()
        .map(|call| call.op.clone())
        .collect();
    assert!(
        matches!(
            ops.as_slice(),
            [
                BrowserOp::Open { .. },
                BrowserOp::Click { .. },
                BrowserOp::Screenshot { .. }
            ]
        ),
        "{ops:#?}"
    );
    let proofs: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            crate::work::OrchestratorStepKind::Proof {
                id,
                name,
                mime,
                caption,
                ..
            } => Some((id.clone(), name.clone(), mime.clone(), caption.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(proofs.len(), 1, "{proofs:?}");
    let (id, name, mime, caption) = proofs[0].clone();
    assert!(
        name.starts_with("shot-") && name.ends_with(".png"),
        "{name}"
    );
    assert_eq!(
        (mime.as_str(), caption.as_deref()),
        ("image/png", Some("Dark mode on in Settings"))
    );
    let (data, served) = flow
        .manager
        .read_thread_image(&flow.conversation, &id)
        .await
        .unwrap();
    assert_eq!(served, "image/png");
    assert_eq!(data, PNG);
    flow.stop().await;
}
