//! Which windows are kept drawn and when they get their setting back, with a
//! fake window server: no real window is touched.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use cu_layer::keep_drawn::{KeepDrawn, Occlusion};

#[derive(Default)]
struct World {
    web_pids: HashSet<i32>,
    visible: HashSet<u32>,
    windows: HashSet<u32>,
    /// Current notifications setting per window (absent = on).
    notifications: HashMap<u32, bool>,
    refuse: bool,
    calls: Vec<(u32, bool)>,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<World>>);

impl Occlusion for Fake {
    fn web_engine(&self, pid: i32) -> bool {
        self.0.lock().unwrap().web_pids.contains(&pid)
    }
    fn partly_visible(&self, window_id: u32) -> bool {
        self.0.lock().unwrap().visible.contains(&window_id)
    }
    fn exists(&self, window_id: u32) -> bool {
        self.0.lock().unwrap().windows.contains(&window_id)
    }
    fn set_notifications(&self, window_id: u32, enabled: bool) -> Option<bool> {
        let mut world = self.0.lock().unwrap();
        if world.refuse {
            return None;
        }
        world.calls.push((window_id, enabled));
        let previous = world
            .notifications
            .insert(window_id, enabled)
            .unwrap_or(true);
        Some(previous)
    }
}

fn setup() -> (Fake, KeepDrawn) {
    let fake = Fake::default();
    {
        let mut world = fake.0.lock().unwrap();
        world.web_pids.insert(10);
        world.windows.extend([1, 2, 3]);
        world.visible.extend([1, 2]);
    }
    let keep = KeepDrawn::new(Box::new(fake.clone()));
    (fake, keep)
}

fn notifications(fake: &Fake, window: u32) -> bool {
    *fake
        .0
        .lock()
        .unwrap()
        .notifications
        .get(&window)
        .unwrap_or(&true)
}

#[test]
fn a_partly_visible_web_window_is_kept_and_a_covered_one_is_not() {
    let (fake, keep) = setup();
    assert!(keep.keep(10, 1, "a"));
    assert!(!notifications(&fake, 1));
    // Already covered: pinning it would pin it hidden.
    assert!(!keep.keep(10, 3, "a"));
    assert!(notifications(&fake, 3));
    // Not a web app.
    assert!(!keep.keep(11, 2, "a"));
    assert!(notifications(&fake, 2));
}

#[test]
fn a_kept_window_stays_kept_once_covered_and_is_pinned_once() {
    let (fake, keep) = setup();
    assert!(keep.keep(10, 1, "a"));
    fake.0.lock().unwrap().visible.remove(&1);
    assert!(keep.keep(10, 1, "b"));
    assert_eq!(fake.0.lock().unwrap().calls, vec![(1, false)]);
}

#[test]
fn the_window_gets_its_setting_back_when_its_last_session_ends() {
    let (fake, keep) = setup();
    keep.keep(10, 1, "a");
    keep.keep(10, 1, "b");
    keep.release("a");
    assert!(keep.kept(1));
    assert!(!notifications(&fake, 1));
    keep.release("b");
    assert!(!keep.kept(1));
    assert!(notifications(&fake, 1));
}

#[test]
fn a_setting_another_owner_turned_off_stays_off() {
    let (fake, keep) = setup();
    fake.0.lock().unwrap().notifications.insert(1, false);
    keep.keep(10, 1, "a");
    keep.release("a");
    assert!(!notifications(&fake, 1));
}

#[test]
fn a_closed_window_is_dropped_without_a_call() {
    let (fake, keep) = setup();
    keep.keep(10, 1, "a");
    fake.0.lock().unwrap().windows.remove(&1);
    keep.keep(10, 2, "a");
    assert!(!keep.kept(1));
    keep.release("a");
    assert_eq!(
        fake.0.lock().unwrap().calls,
        vec![(1, false), (2, false), (2, true)]
    );
}

#[test]
fn a_refused_restore_is_tried_again_and_release_all_puts_everything_back() {
    let (fake, keep) = setup();
    keep.keep(10, 1, "a");
    keep.keep(10, 2, "b");
    fake.0.lock().unwrap().refuse = true;
    keep.release("a");
    assert!(keep.kept(1));
    fake.0.lock().unwrap().refuse = false;
    keep.release_all();
    assert!(!keep.kept(1) && !keep.kept(2));
    assert!(notifications(&fake, 1) && notifications(&fake, 2));
}

#[test]
fn nothing_is_kept_when_the_window_server_refuses() {
    let (fake, keep) = setup();
    fake.0.lock().unwrap().refuse = true;
    assert!(!keep.keep(10, 1, "a"));
    assert!(!keep.kept(1));
}

fn read(
    on_current_space: bool,
    driver_note: Option<&str>,
) -> cua_driver_core::protocol::ToolResult {
    let mut structured =
        serde_json::json!({"pid": 10, "window_id": 1, "on_current_space": on_current_space});
    let mut result = cua_driver_core::protocol::ToolResult::text("- [0] AXWindow");
    if let Some(note) = driver_note {
        structured["off_space_staleness"] = note.into();
        result
            .content
            .push(cua_driver_core::protocol::Content::text(note));
    }
    result.with_structured(structured)
}

fn texts(result: &cua_driver_core::protocol::ToolResult) -> Vec<String> {
    result
        .content
        .iter()
        .filter_map(|item| match item {
            cua_driver_core::protocol::Content::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_web_window_on_another_space_is_read_as_an_older_picture() {
    use cu_layer::keep_drawn::{note_older_picture, OLDER_PICTURE};
    let mut result = read(
        false,
        Some("confirm their effect after the window is shown"),
    );
    note_older_picture(&mut result, true);
    assert_eq!(texts(&result), ["- [0] AXWindow", OLDER_PICTURE]);
    assert_eq!(
        result.structured_content.unwrap()["off_space_staleness"],
        OLDER_PICTURE
    );

    for (on_current_space, web) in [(true, true), (false, false)] {
        let mut result = read(on_current_space, None);
        note_older_picture(&mut result, web);
        assert_eq!(
            texts(&result),
            ["- [0] AXWindow"],
            "{on_current_space} {web}"
        );
    }
}
