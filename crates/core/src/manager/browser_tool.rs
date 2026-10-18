//! The browser tool's way to the app (Q74): an agent's call goes to the app's window over the
//! live feed, the app runs it on the tab's page and answers it with its id. A call nobody
//! answers in time, or made with no window open, fails in plain words; a call dropped (the
//! agent stopped, its request ended) is forgotten, and a late answer to it goes nowhere.
// No tool makes calls yet: they are wired once the takeover and the guards are in.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{broadcast, oneshot};

use super::SessionManager;
use crate::browser::{BrowserCall, BrowserOp, BrowserResult};
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

fn failed(message: &str) -> BrowserResult {
    BrowserResult::Failed {
        message: message.to_owned(),
    }
}

impl SessionManager {
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
