//! The app call's way to the window (Q90; the browser tool's since Q74): an agent's call goes
//! to the app's window over the live feed, the app does it there and answers it with its id.
//! Brigadier has one main window, which shows every session, so the call goes there; the app
//! says whether the session is on screen. A call nobody answers in time, or made with no window
//! open, fails in plain words; a call dropped (the agent stopped, its request ended) is
//! forgotten, and a late answer to it goes nowhere.
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::{broadcast, oneshot};

use super::SessionManager;
use crate::app_call::{AppAnswer, AppCall, AppOp};
use crate::{ConversationId, TaskId};

/// Calls waiting for the app's answer, and the feed the app's windows hear them on.
pub(crate) struct AppHub {
    calls: broadcast::Sender<AppCall>,
    pending: Mutex<HashMap<String, oneshot::Sender<AppAnswer>>>,
    pub(crate) next: AtomicU64,
}

impl Default for AppHub {
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
    hub: &'a AppHub,
    id: String,
}

impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.hub.pending().remove(&self.id);
    }
}

impl AppHub {
    pub(crate) fn pending(
        &self,
    ) -> std::sync::MutexGuard<'_, HashMap<String, oneshot::Sender<AppAnswer>>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AppCall> {
        self.calls.subscribe()
    }

    /// Sends `op` to the app and waits up to `timeout` for its answer.
    pub(crate) async fn call(
        &self,
        conversation_id: &ConversationId,
        task_id: Option<&TaskId>,
        label: &str,
        op: AppOp,
        timeout: Duration,
    ) -> AppAnswer {
        let id = format!("app-call-{}", self.next.fetch_add(1, Ordering::Relaxed));
        let (answer, answered) = oneshot::channel();
        self.pending().insert(id.clone(), answer);
        let _waiting = Waiting {
            hub: self,
            id: id.clone(),
        };
        let what = op.what();
        let call = AppCall {
            id,
            conversation_id: conversation_id.clone(),
            task_id: task_id.cloned(),
            label: label.to_owned(),
            op,
        };
        if self.calls.send(call).is_err() {
            return failed(format!(
                "Brigadier's window isn't open, so {what} can't be used now."
            ));
        }
        match tokio::time::timeout(timeout, answered).await {
            Ok(Ok(result)) => result,
            // The window closed with the call under way.
            Ok(Err(_)) => failed("Brigadier's window closed before it answered.".to_owned()),
            Err(_) => failed("Brigadier's window didn't answer in time.".to_owned()),
        }
    }

    /// The app's answer to call `id`; false when nothing waits for it any more.
    pub(crate) fn answer(&self, id: &str, result: AppAnswer) -> bool {
        match self.pending().remove(id) {
            Some(waiting) => waiting.send(result).is_ok(),
            None => false,
        }
    }

    /// A window that ran calls went away: when it was the last, what waits on it fails at once,
    /// and true comes back.
    pub(crate) fn window_gone(&self) -> bool {
        if self.calls.receiver_count() == 0 {
            self.pending().clear();
            return true;
        }
        false
    }
}

fn failed(message: String) -> AppAnswer {
    AppAnswer::Failed { message }
}

impl SessionManager {
    /// The agents' app calls, for an app window to run.
    pub fn subscribe_app(&self) -> broadcast::Receiver<AppCall> {
        self.app.subscribe()
    }

    /// The app's answer to an app call; false when nothing waits for it any more.
    pub fn app_answer(&self, id: &str, result: AppAnswer) -> bool {
        self.app.answer(id, result)
    }

    /// An app window that ran app calls went away.
    pub fn app_window_gone(&self) {
        if self.app.window_gone() {
            // Its Browser tabs' pauses went with it.
            self.browser.clear_paused();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::{BrowserOp, BrowserResult};

    fn read(tab: &str) -> AppOp {
        AppOp::Browser {
            op: BrowserOp::Read { tab: tab.into() },
        }
    }

    fn closed(tab: &str) -> AppAnswer {
        AppAnswer::Browser {
            result: BrowserResult::Closed { tab: tab.into() },
        }
    }

    const WAIT: Duration = Duration::from_secs(5);

    #[tokio::test]
    async fn the_window_answers_a_call_by_its_id() {
        let c = ConversationId("c1".into());
        let hub = AppHub::default();
        let mut window = hub.subscribe();
        let answering = async {
            let call = window.recv().await.unwrap();
            assert_eq!(call.conversation_id, c);
            assert_eq!(call.label, "Fix the form");
            assert!(hub.answer(&call.id, closed("t1")));
        };
        let (answer, ()) = tokio::join!(
            hub.call(&c, None, "Fix the form", read("t1"), WAIT),
            answering
        );
        assert_eq!(answer, closed("t1"));
        assert!(hub.pending().is_empty());
    }

    #[tokio::test]
    async fn with_no_window_a_call_fails_at_once_in_plain_words() {
        let c = ConversationId("c1".into());
        let hub = AppHub::default();
        let answer = hub.call(&c, None, "t", read("t1"), WAIT).await;
        assert!(
            matches!(&answer, AppAnswer::Failed { message } if message == "Brigadier's window isn't open, so its browser can't be used now."),
            "{answer:?}"
        );
        assert!(hub.pending().is_empty());
    }

    #[tokio::test]
    async fn a_call_nobody_answers_times_out_and_a_late_answer_goes_nowhere() {
        let c = ConversationId("c1".into());
        let hub = AppHub::default();
        let mut window = hub.subscribe();
        let answer = hub
            .call(&c, None, "t", read("t1"), Duration::from_millis(50))
            .await;
        assert!(
            matches!(&answer, AppAnswer::Failed { message } if message.contains("didn't answer in time")),
            "{answer:?}"
        );
        let call = window.recv().await.unwrap();
        assert!(!hub.answer(&call.id, closed("t1")));
        assert!(hub.pending().is_empty());
    }

    #[tokio::test]
    async fn the_last_window_going_fails_what_waits_on_it() {
        let c = ConversationId("c1".into());
        let hub = std::sync::Arc::new(AppHub::default());
        let window = hub.subscribe();
        let waiting = {
            let hub = hub.clone();
            let c = c.clone();
            tokio::spawn(async move { hub.call(&c, None, "t", read("t1"), WAIT).await })
        };
        while hub.pending().is_empty() {
            tokio::task::yield_now().await;
        }
        drop(window);
        assert!(hub.window_gone());
        let answer = waiting.await.unwrap();
        assert!(
            matches!(&answer, AppAnswer::Failed { message } if message.contains("closed before")),
            "{answer:?}"
        );
    }
}
