//! The browser tool's way to the app (Q74): an agent's call goes to the app's window over the
//! live feed, the app runs it on the tab's page and answers it with its id. A call nobody
//! answers in time, or made with no window open, fails in plain words; a call dropped (the
//! agent stopped, its request ended) is forgotten, and a late answer to it goes nowhere.
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{broadcast, oneshot};

use base64::Engine as _;

use super::SessionManager;
use crate::browser::{
    BrowserCall, BrowserGo, BrowserGuard, BrowserOp, BrowserPageRead, BrowserResult,
    BrowserSensitive,
};
use crate::tools::{BrowserAsks, BrowserStep, BrowserToolCall, ToolReply};
use crate::{ConversationId, TaskId};

/// How long the app may take over a call: a page load it waits for, at most.
pub(crate) const CALL_TIMEOUT: Duration = Duration::from_secs(45);

/// Calls waiting for the app's answer, and the feed the app's windows hear them on.
pub(crate) struct BrowserHub {
    calls: broadcast::Sender<BrowserCall>,
    pending: Mutex<HashMap<String, oneshot::Sender<BrowserResult>>>,
    next: AtomicU64,
}

impl Default for BrowserHub {
    fn default() -> Self {
        Self {
            calls: broadcast::channel(64).0,
            pending: Mutex::default(),
            next: AtomicU64::new(1),
        }
    }
}

/// Forgets a call when its wait ends, however it ends.
struct Waiting<'a> {
    hub: &'a BrowserHub,
    id: String,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.hub.pending().remove(&self.id);
    }
}

impl BrowserHub {
    fn pending(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<BrowserResult>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<BrowserCall> {
        self.calls.subscribe()
    }

    /// Sends `op` to the app and waits up to `timeout` for its answer.
    pub(crate) async fn call(
        &self,
        conversation_id: &ConversationId,
        task_id: Option<&TaskId>,
        label: &str,
        op: BrowserOp,
        timeout: Duration,
    ) -> BrowserResult {
        let id = format!("browser-call-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (answer, answered) = oneshot::channel();
        self.pending().insert(id.clone(), answer);
        let _waiting = Waiting {
            hub: self,
            id: id.clone(),
        };
        let call = BrowserCall {
            id,
            conversation_id: conversation_id.clone(),
            task_id: task_id.cloned(),
            label: label.to_owned(),
            op,
        };
        if self.calls.send(call).is_err() {
            return failed("Brigadier's window isn't open, so its browser can't be used now.");
        }
        match tokio::time::timeout(timeout, answered).await {
            Ok(Ok(result)) => result,
            // The window closed with the call under way.
            Ok(Err(_)) => failed("Brigadier's window closed before the browser answered."),
            Err(_) => failed("The browser didn't answer in time."),
        }
    }

    /// The app's answer to call `id`; false when nothing waits for it any more.
    pub(crate) fn answer(&self, id: &str, result: BrowserResult) -> bool {
        match self.pending().remove(id) {
            Some(waiting) => waiting.send(result).is_ok(),
            None => false,
        }
    }

    /// The window that ran calls went away: what waits on it fails at once.
    pub(crate) fn window_gone(&self) {
        if self.calls.receiver_count() == 0 {
            self.pending().clear();
        }
    }
}

/// The op a tool call asks of the app; a call that says too little or too much is refused.
fn browser_op(call: BrowserToolCall) -> Result<BrowserOp, String> {
    Ok(match call {
        BrowserToolCall::Open(open) => BrowserOp::Open {
            url: web_address(&open.url)?,
        },
        BrowserToolCall::Navigate(navigate) => match (navigate.url, navigate.go) {
            (Some(url), None) => BrowserOp::Navigate {
                tab: navigate.tab,
                url: web_address(&url)?,
            },
            (None, Some(step)) => BrowserOp::Go {
                tab: navigate.tab,
                action: match step {
                    BrowserStep::Back => BrowserGo::Back,
                    BrowserStep::Forward => BrowserGo::Forward,
                    BrowserStep::Reload => BrowserGo::Reload,
                },
            },
            _ => return Err("browser_navigate takes either `url` or `go`, one of them.".to_owned()),
        },
        BrowserToolCall::Read(tab) => BrowserOp::Read { tab: tab.tab },
        BrowserToolCall::Click(click) => BrowserOp::Click {
            tab: click.tab,
            target: click.target,
            guard: declared(click.asks),
        },
        BrowserToolCall::Type(typing) => BrowserOp::Type {
            tab: typing.tab,
            target: typing.target,
            text: typing.text,
            submit: typing.submit,
            guard: declared(typing.asks),
        },
        BrowserToolCall::Screenshot(tab) => BrowserOp::Screenshot { tab: tab.tab },
        BrowserToolCall::Close(tab) => BrowserOp::Close { tab: tab.tab },
    })
}

/// `url` when it is a web address the tab may load.
fn web_address(url: &str) -> Result<String, String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        Ok(url.to_owned())
    } else {
        Err(format!(
            "{url} isn't a web address: the browser tools open http and https pages only."
        ))
    }
}

fn declared(asks: Option<BrowserAsks>) -> BrowserGuard {
    BrowserGuard {
        declared: asks.map(|asks| match asks {
            BrowserAsks::Pay => BrowserSensitive::Pay,
            BrowserAsks::Send => BrowserSensitive::Send,
            BrowserAsks::SignIn => BrowserSensitive::SignIn,
        }),
        approved: None,
    }
}

/// A read page as the agent gets it.
fn page_text(tab: &str, page: &BrowserPageRead) -> String {
    let title = if page.title.trim().is_empty() {
        "(no title)"
    } else {
        page.title.trim()
    };
    let loading = if page.loading { " (loading)" } else { "" };
    format!("{tab} · {title} — {}{loading}\n\n{}", page.url, page.text)
}

/// What an action that asks first does, in plain words.
pub(crate) fn needs(sensitive: BrowserSensitive, what: &str) -> String {
    let doing = match sensitive {
        BrowserSensitive::Pay => "Paying",
        BrowserSensitive::Send => "Sending in the user's name",
        BrowserSensitive::SignIn => "Signing in",
        BrowserSensitive::Credentials => "Entering a password",
        BrowserSensitive::Card => "Entering a payment card",
        BrowserSensitive::OneTimeCode => "Entering a one-time code",
    };
    let what = what.trim();
    if what.is_empty() {
        doing.to_owned()
    } else {
        format!("{doing} (\"{what}\")")
    }
}

fn failed(message: &str) -> BrowserResult {
    BrowserResult::Failed {
        message: message.to_owned(),
    }
}

impl SessionManager {
    /// Whether the thread and its workers get the browser tools: the built-in browser is on,
    /// and agents can drive it on this platform (macOS for now).
    pub(crate) fn browser_tools_on(&self) -> bool {
        // Kept to development runs until the takeover and the guards are in.
        cfg!(target_os = "macos")
            && std::env::var_os("BRIGADIER_BROWSER_TOOL").is_some()
            && self.core.settings().browser_enabled
    }

    /// A browser tool's call from the thread (`task_id` none) or one of its workers.
    pub(crate) async fn browser_tool(
        &self,
        conversation_id: ConversationId,
        task_id: Option<TaskId>,
        call: BrowserToolCall,
    ) -> ToolReply {
        if !self.core.settings().browser_enabled {
            return ToolReply::error(
                "The built-in browser is off (Settings › Browser), so its tabs can't be used.",
            );
        }
        let op = match browser_op(call) {
            Ok(op) => op,
            Err(message) => return ToolReply::error(message),
        };
        let label = self.browser_label(&conversation_id, task_id.as_ref()).await;
        let result = self
            .browser
            .call(&conversation_id, task_id.as_ref(), &label, op, CALL_TIMEOUT)
            .await;
        self.browser_reply(&conversation_id, result)
    }

    /// The agent as the user knows it: its task's or thread's title.
    async fn browser_label(
        &self,
        conversation_id: &ConversationId,
        task: Option<&TaskId>,
    ) -> String {
        if let Some(task) = task {
            return self
                .task_by_id(conversation_id, task)
                .await
                .map(|task| task.title)
                .unwrap_or_else(|_| "A worker".to_owned());
        }
        match self.core.conversation(conversation_id) {
            Ok(found) if !found.title.trim().is_empty() => found.title,
            _ => "The thread".to_owned(),
        }
    }

    /// What the agent hears of the app's answer.
    fn browser_reply(&self, conversation_id: &ConversationId, result: BrowserResult) -> ToolReply {
        match result {
            BrowserResult::Page { tab, page } => ToolReply::ok(page_text(&tab, &page)),
            BrowserResult::Shot { tab, url, png } => {
                let bytes = match base64::engine::general_purpose::STANDARD.decode(png.as_bytes()) {
                    Ok(bytes) => bytes,
                    Err(_) => {
                        return ToolReply::error(
                            "The browser's picture of the page was unreadable.",
                        );
                    }
                };
                let dir = self.owned_dir("orch", &conversation_id.0).join("browser");
                let path = dir.join(format!(
                    "shot-{}-{}.png",
                    crate::now_ms(),
                    self.browser.next.fetch_add(1, Ordering::Relaxed)
                ));
                let saved =
                    std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, &bytes));
                let text = match saved {
                    Ok(()) => format!(
                        "{tab} — {url}\nSaved as {} (attach_proof takes this path).",
                        path.display()
                    ),
                    Err(err) => format!("{tab} — {url}\nThe picture couldn't be saved: {err}."),
                };
                ToolReply::ok(text).with_image("image/png", bytes)
            }
            BrowserResult::Closed { tab } => ToolReply::ok(format!("Closed {tab}.")),
            BrowserResult::Paused { tab } => ToolReply::error(format!(
                "Still paused: the user is using {tab}. Nothing was done; try again once they press Continue."
            )),
            BrowserResult::NeedsApproval {
                tab,
                url,
                sensitive,
                what,
                ..
            } => ToolReply::error(format!(
                "{} on {tab} ({url}) needs the user's yes first, and they can't be asked from here, so nothing was done.",
                needs(sensitive, &what)
            )),
            BrowserResult::Failed { message } => ToolReply::error(message),
        }
    }

    /// The agents' browser calls, for an app window to run.
    pub fn subscribe_browser(&self) -> broadcast::Receiver<BrowserCall> {
        self.browser.subscribe()
    }

    /// The app's answer to a browser call; false when nothing waits for it any more.
    pub fn browser_answer(&self, id: &str, result: BrowserResult) -> bool {
        self.browser.answer(id, result)
    }

    /// An app window that ran browser calls went away.
    pub fn browser_window_gone(&self) {
        self.browser.window_gone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::BrowserPageRead;

    fn read(tab: &str) -> BrowserOp {
        BrowserOp::Read { tab: tab.into() }
    }

    fn conversation() -> ConversationId {
        ConversationId("c1".into())
    }

    fn page(tab: &str) -> BrowserResult {
        BrowserResult::Page {
            tab: tab.into(),
            page: BrowserPageRead {
                url: "http://127.0.0.1/".into(),
                title: "Test".into(),
                generation: 1,
                loading: false,
                text: "Hello".into(),
            },
        }
    }

    #[tokio::test]
    async fn the_app_answers_a_call() {
        let c = conversation();
        let hub = BrowserHub::default();
        let mut window = hub.subscribe();
        let answering = async {
            let call = window.recv().await.unwrap();
            assert_eq!(call.op, read("t1"));
            assert_eq!(call.label, "Fix the form");
            assert!(hub.answer(&call.id, page("t1")));
        };
        let (result, ()) = tokio::join!(
            hub.call(&c, None, "Fix the form", read("t1"), CALL_TIMEOUT),
            answering
        );
        assert_eq!(result, page("t1"));
        assert!(hub.pending().is_empty());
    }

    #[tokio::test]
    async fn with_no_window_a_call_fails_at_once() {
        let c = conversation();
        let hub = BrowserHub::default();
        let result = hub.call(&c, None, "t", read("t1"), CALL_TIMEOUT).await;
        assert!(
            matches!(&result, BrowserResult::Failed { message } if message.contains("window isn't open")),
            "{result:?}"
        );
        assert!(hub.pending().is_empty());
    }

    #[tokio::test]
    async fn a_call_nobody_answers_times_out_and_a_late_answer_goes_nowhere() {
        let c = conversation();
        let hub = BrowserHub::default();
        let mut window = hub.subscribe();
        let result = hub
            .call(&c, None, "t", read("t1"), Duration::from_secs(5))
            .await;
        assert!(
            matches!(&result, BrowserResult::Failed { message } if message.contains("in time")),
            "{result:?}"
        );
        let call = window.recv().await.unwrap();
        assert!(!hub.answer(&call.id, page("t1")));
        assert!(hub.pending().is_empty());
    }

    #[tokio::test]
    async fn a_dropped_call_is_forgotten_and_only_the_first_answer_counts() {
        let c = conversation();
        let hub = BrowserHub::default();
        let mut window = hub.subscribe();
        {
            let call = hub.call(&c, None, "t", read("t1"), CALL_TIMEOUT);
            tokio::pin!(call);
            // Started, then dropped: the agent stopped.
            assert!(
                tokio::time::timeout(Duration::from_millis(10), &mut call)
                    .await
                    .is_err()
            );
        }
        let dropped = window.recv().await.unwrap();
        assert!(!hub.answer(&dropped.id, page("t1")));

        let answering = async {
            let call = window.recv().await.unwrap();
            assert!(hub.answer(&call.id, page("t2")));
            assert!(!hub.answer(&call.id, page("t3")));
        };
        let (result, ()) =
            tokio::join!(hub.call(&c, None, "t", read("t2"), CALL_TIMEOUT), answering);
        assert_eq!(result, page("t2"));
    }

    #[tokio::test]
    async fn a_window_going_away_fails_what_waits_on_it() {
        let c = conversation();
        let hub = BrowserHub::default();
        let window = hub.subscribe();
        let call = hub.call(&c, None, "t", read("t1"), CALL_TIMEOUT);
        let closing = async {
            tokio::task::yield_now().await;
            drop(window);
            hub.window_gone();
        };
        let (result, ()) = tokio::join!(call, closing);
        assert!(
            matches!(&result, BrowserResult::Failed { message } if message.contains("closed")),
            "{result:?}"
        );
    }
}
