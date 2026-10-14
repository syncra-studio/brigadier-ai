//! One agent types into an app at a time, and the others wait their turn.
//!
//! The driver refuses a `type_text` for an app while another one for the same
//! app is running ("text input is already active"), so that the two texts
//! can't mix. Two agents typing into two documents of one app then got that
//! refusal for about one call in forty. The daemon takes the app's turn here
//! before it hands such a call to the driver, so the call waits for the other
//! agent's text to finish instead of failing.
//!
//! A `run_steps` call takes the turns of the apps its typing steps name by
//! pid (or the call's default pid) for the whole batch. A typing step that
//! names its app another way still meets the driver's refusal.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use tokio::sync::OwnedMutexGuard;

/// The longest a call waits for its turns. After that it goes to the driver
/// anyway, which refuses it if the app is still busy.
const MAX_WAIT: Duration = Duration::from_secs(60);

fn turns() -> &'static Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>> {
    static TURNS: OnceLock<Mutex<HashMap<i64, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    TURNS.get_or_init(Default::default)
}

fn positive_pid(args: Option<&Value>) -> Option<i64> {
    args?.get("pid")?.as_i64().filter(|pid| *pid > 0)
}

/// The pids of the apps a call types into, in a fixed order.
pub fn typing_pids(tool: &str, args: &Value) -> BTreeSet<i64> {
    let mut pids = BTreeSet::new();
    match tool {
        "type_text" => pids.extend(positive_pid(Some(args))),
        "run_steps" => {
            let default = positive_pid(Some(args));
            for step in args
                .get("steps")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let step_args = if step.get("tool").and_then(Value::as_str) == Some("type_text") {
                    Some(step.get("args"))
                } else {
                    step.get("type_text").map(Some)
                };
                if let Some(step_args) = step_args {
                    pids.extend(positive_pid(step_args).or(default));
                }
            }
        }
        _ => {}
    }
    pids
}

/// Runs `call` once no other call is typing into the apps it types into.
pub fn in_turn<F: Future>(tool: &str, args: &Value, call: F) -> impl Future<Output = F::Output> {
    let pids = typing_pids(tool, args);
    async move {
        if pids.is_empty() {
            return call.await;
        }
        take_turns(pids, call).await
    }
}

async fn take_turns<F: Future>(pids: BTreeSet<i64>, call: F) -> F::Output {
    let locks: Vec<_> = {
        let mut turns = turns().lock().unwrap_or_else(|e| e.into_inner());
        pids.iter()
            .map(|pid| turns.entry(*pid).or_default().clone())
            .collect()
    };
    let held = tokio::time::timeout(MAX_WAIT, async {
        let mut held: Vec<OwnedMutexGuard<()>> = Vec::with_capacity(locks.len());
        // Taken in pid order, so two batches can't each hold what the other waits for.
        for lock in locks {
            held.push(lock.lock_owned().await);
        }
        held
    })
    .await
    .unwrap_or_default();
    let output = call.await;
    drop(held);
    let mut turns = turns().lock().unwrap_or_else(|e| e.into_inner());
    turns.retain(|_, lock| Arc::strong_count(lock) > 1);
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn typing_calls_name_their_apps() {
        assert_eq!(
            typing_pids("type_text", &json!({"pid": 7, "text": "x"})),
            BTreeSet::from([7])
        );
        assert!(typing_pids("type_text", &json!({"element_token": "s1:2"})).is_empty());
        assert!(typing_pids("click", &json!({"pid": 7})).is_empty());
        let batch = json!({"pid": 3, "steps": [
            {"tool": "type_text", "args": {"pid": 9, "text": "a"}},
            {"type_text": {"text": "b"}},
            {"tool": "click", "args": {"pid": 5}},
        ]});
        assert_eq!(typing_pids("run_steps", &batch), BTreeSet::from([3, 9]));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn calls_typing_into_one_app_take_turns() {
        let inside = Arc::new(AtomicUsize::new(0));
        let most = Arc::new(AtomicUsize::new(0));
        let calls = (0..4).map(|n| {
            let (inside, most) = (inside.clone(), most.clone());
            tokio::spawn(async move {
                in_turn("type_text", &json!({"pid": 4242, "text": n}), async {
                    let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                    most.fetch_max(now, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(20)).await;
                    inside.fetch_sub(1, Ordering::SeqCst);
                })
                .await
            })
        });
        for call in calls.collect::<Vec<_>>() {
            call.await.unwrap();
        }
        assert_eq!(most.load(Ordering::SeqCst), 1);
        assert!(!turns().lock().unwrap().contains_key(&4242));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn other_apps_and_other_tools_dont_wait() {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let typing = tokio::spawn(in_turn("type_text", &json!({"pid": 4343}), async move {
            let _ = rx.await;
        }));
        tokio::time::sleep(Duration::from_millis(20)).await;
        let other = in_turn("type_text", &json!({"pid": 4344}), async { 1 });
        let read = in_turn("get_window_state", &json!({"pid": 4343}), async { 2 });
        let done =
            tokio::time::timeout(Duration::from_secs(1), async { (other.await, read.await) }).await;
        assert_eq!(done.ok(), Some((1, 2)));
        tx.send(()).unwrap();
        typing.await.unwrap();
    }
}
