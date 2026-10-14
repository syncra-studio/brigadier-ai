//! Which app and window an `element_token` belongs to.
//!
//! A call may name its target only by an element token. The guard has to
//! check that target before the tool runs, so it remembers the process and
//! window of every snapshot a guarded `get_window_state` returned (snapshot
//! ids are unique for the life of the helper).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use cua_driver_core::protocol::ToolResult;
use serde_json::Value;

/// How many snapshots are remembered. The driver keeps far fewer current.
const CAPACITY: usize = 4096;

#[derive(Default)]
struct Known {
    owners: HashMap<u32, (i64, Option<u64>)>,
    order: VecDeque<u32>,
}

static KNOWN: Mutex<Option<Known>> = Mutex::new(None);

/// Remember the snapshot a result announces (`snapshot_id`, `pid`,
/// `window_id` in its structured content).
pub fn record(result: &ToolResult) {
    let Some(structured) = result.structured_content.as_ref() else {
        return;
    };
    let Some(snapshot) = structured
        .get("snapshot_id")
        .and_then(Value::as_str)
        .and_then(cua_driver_core::element_token::parse_snapshot_handle)
    else {
        return;
    };
    let Some(pid) = structured.get("pid").and_then(Value::as_i64) else {
        return;
    };
    let window = structured.get("window_id").and_then(Value::as_u64);
    let mut known = KNOWN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let known = known.get_or_insert_with(Known::default);
    if known.owners.insert(snapshot, (pid, window)).is_none() {
        known.order.push_back(snapshot);
    }
    while known.order.len() > CAPACITY {
        if let Some(oldest) = known.order.pop_front() {
            known.owners.remove(&oldest);
        }
    }
}

/// The process and window of the snapshot `token` was minted from, if a
/// guarded read announced it.
pub fn owner(token: &str) -> Option<(i64, Option<u64>)> {
    let (snapshot, _) = cua_driver_core::element_token::parse_token(token)?;
    let known = KNOWN
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    known.as_ref()?.owners.get(&snapshot).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_token_names_the_window_its_snapshot_was_read_from() {
        let read = ToolResult::text("tree").with_structured(json!({
            "snapshot_id": "s7fff0001", "pid": 4242, "window_id": 77,
        }));
        record(&read);
        assert_eq!(owner("s7fff0001:12"), Some((4242, Some(77))));
        assert_eq!(owner("s7fff0002:1"), None);
        assert_eq!(owner("not-a-token"), None);
    }
}
