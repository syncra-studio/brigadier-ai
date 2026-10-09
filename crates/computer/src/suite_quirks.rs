//! The suite's hard-surface tasks (Phase 5, stream B): an Electron page, a SwiftUI window, a Mac
//! Catalyst window, a save panel shown as a sheet, and a minimised window. Each runs on a fixture
//! of its own (`fixtures/electron-pad`, `quirk-pad`, `catalyst-pad`) and is judged, like the
//! Phase 4 tasks, by what reached the app: the fixture's log and state snapshot, files on disk,
//! and the broker's records on the task's own window.

use std::collections::BTreeMap;

use crate::action::Status;
use crate::record::ActionRecord;
use crate::suite::{Event, Prepared, Setup, Task, Verdict};

/// The fixtures these tasks run on.
pub const ELECTRON: &str = "electron-pad";
pub const QUIRK: &str = "quirk-pad";
pub const CATALYST: &str = "catalyst-pad";

/// The file the save-panel task saves, in the trial's own save folder.
pub const SAVED: &str = "report.txt";
/// What quirk-pad writes into it.
pub const SAVED_TEXT: &str = "exported by quirk-pad\n";

pub const QUIRKS: [Task; 5] = [
    Task {
        id: "electron-signup",
        setup: Setup::Quirk {
            fixture: ELECTRON,
            window: "Electron Pad",
        },
        goal: "In the Electron Pad window, enter Ada Lovelace as the full name, tick Subscribe, then press Submit once.",
        end_state: "The form was submitted once with the name Ada Lovelace and Subscribe ticked.",
    },
    Task {
        id: "swiftui-item",
        setup: Setup::Quirk {
            fixture: QUIRK,
            window: "SwiftUI Pad",
        },
        goal: "In the SwiftUI Pad window, set Title to Quarterly report, turn Starred on, choose the Large size, then press Save Item once.",
        end_state: "Save Item was pressed once with the title Quarterly report, Starred on and the size Large.",
    },
    Task {
        id: "catalyst-order",
        setup: Setup::Quirk {
            fixture: CATALYST,
            window: "Catalyst Pad",
        },
        goal: "In the Catalyst Pad window, set the quantity to 4, enter gift wrap in the Note field, then press Place Order once.",
        end_state: "One order was placed with the quantity 4 and the note gift wrap.",
    },
    Task {
        id: "save-panel",
        setup: Setup::Quirk {
            fixture: QUIRK,
            window: "AppKit Pad",
        },
        goal: "In the AppKit Pad window, press Export…, then in the save panel that opens save the file as report.txt in the folder the panel shows.",
        end_state: "report.txt was saved by the app into the folder the save panel opened in.",
    },
    Task {
        id: "minimised-code",
        setup: Setup::Quirk {
            fixture: QUIRK,
            window: "Minimised Pad",
        },
        goal: "The Minimised Pad window is minimised in the Dock. Leave it minimised: set its Code field to 4711 and press its Apply button once.",
        end_state: "Apply was pressed once with the code 4711, and the window is still minimised.",
    },
];

/// The controls each task may touch; an event on any other control is a wrong target.
fn allowed(task: &str) -> &'static [&'static str] {
    match task {
        "electron-signup" => &["name", "subscribe", "submit"],
        "swiftui-item" => &[
            "swiftui-title",
            "swiftui-starred",
            "swiftui-size",
            "swiftui-save",
        ],
        "catalyst-order" => &["quantity", "note", "place-order"],
        "save-panel" => &["export"],
        "minimised-code" => &["mini-code", "mini-apply"],
        _ => &[],
    }
}

/// Fixture log ids that are notices, not controls.
fn notice(id: &str) -> bool {
    matches!(
        id,
        "app" | "state" | "window" | "mini-window" | "out-window" | "notes-view"
    )
}

/// The press events on `id`, with their values.
fn presses<'a>(events: &'a [Event], id: &str) -> Vec<&'a Event> {
    events
        .iter()
        .filter(|e| e.id == id && e.ev == "press")
        .collect()
}

fn acted(r: &ActionRecord) -> bool {
    r.status == Status::Done
}

fn names(r: &ActionRecord, words: &str) -> bool {
    r.target
        .as_deref()
        .is_some_and(|t| t.to_lowercase().contains(&words.to_lowercase()))
}

/// Checks one of these tasks; `None` for any other task.
pub fn check(
    prep: &Prepared,
    events: &[Event],
    records: &[ActionRecord],
    files: &BTreeMap<String, String>,
    state: Option<&BTreeMap<String, String>>,
) -> Option<Verdict> {
    let t = QUIRKS.iter().find(|t| t.id == prep.task)?;
    let mut v = Verdict {
        task: prep.task.clone(),
        pass: true,
        ..Default::default()
    };
    let fail = |v: &mut Verdict, why: String| {
        v.pass = false;
        v.notes.push(why);
    };
    // The broker's records: done actions on this app, by rung; none on any other app.
    let mine: Vec<&ActionRecord> = records
        .iter()
        .filter(|r| r.pid == prep.pid && acted(r))
        .collect();
    for r in mine.iter().filter(|r| r.window == prep.window) {
        let rung = r
            .rung
            .and_then(|g| serde_json::to_value(g).ok())
            .and_then(|g| g.as_str().map(str::to_owned))
            .unwrap_or_else(|| "none".into());
        *v.rungs.entry(rung).or_default() += 1;
    }
    let others = records
        .iter()
        .filter(|r| r.pid != prep.pid && acted(r))
        .count();
    if others > 0 {
        fail(&mut v, format!("{others} actions landed on another app"));
    }
    let ok = allowed(t.id);
    let wrong: Vec<String> = events
        .iter()
        .filter(|e| !notice(&e.id) && !ok.contains(&e.id.as_str()))
        .map(|e| format!("{} {}", e.id, e.ev))
        .collect();
    if !wrong.is_empty() {
        v.wrong_target = wrong.len();
        fail(
            &mut v,
            format!("events on other controls: {}", wrong.join(", ")),
        );
    }
    let on_window = |words: &str| {
        mine.iter()
            .any(|r| r.window == prep.window && names(r, words))
    };
    let st = |k: &str| state.and_then(|s| s.get(k)).cloned();
    match t.id {
        "electron-signup" => {
            let p = presses(events, "submit");
            let got: Vec<_> = p.iter().map(|e| e.v.clone().unwrap_or_default()).collect();
            if got != ["Ada Lovelace|on"] {
                fail(&mut v, format!("Submit pressed with {got:?}"));
            }
            if !on_window("Submit") {
                fail(&mut v, "no done action on Submit in the records".into());
            }
        }
        "swiftui-item" => {
            let p = presses(events, "swiftui-save");
            let got: Vec<_> = p.iter().map(|e| e.v.clone().unwrap_or_default()).collect();
            if got != ["Quarterly report|on|Large"] {
                fail(&mut v, format!("Save Item pressed with {got:?}"));
            }
            if !on_window("Save Item") {
                fail(&mut v, "no done action on Save Item in the records".into());
            }
        }
        "catalyst-order" => {
            let p = presses(events, "place-order");
            let got: Vec<_> = p.iter().map(|e| e.v.clone().unwrap_or_default()).collect();
            if got != ["4|gift wrap"] {
                fail(&mut v, format!("Place Order pressed with {got:?}"));
            }
            if !on_window("Place Order") {
                fail(
                    &mut v,
                    "no done action on Place Order in the records".into(),
                );
            }
        }
        "save-panel" => {
            let saved: Vec<_> = events
                .iter()
                .filter(|e| e.id == "export" && e.ev == "saved")
                .filter_map(|e| e.v.clone())
                .collect();
            if saved != [SAVED] {
                fail(&mut v, format!("the app saved {saved:?}"));
            }
            if events
                .iter()
                .any(|e| e.id == "export" && e.ev == "rejected")
            {
                fail(&mut v, "a save outside the trial's folder was tried".into());
            }
            match files.get(SAVED) {
                Some(text) if text == SAVED_TEXT => {}
                Some(text) => fail(&mut v, format!("{SAVED} holds {text:?}")),
                None => fail(&mut v, format!("{SAVED} is not in the save folder")),
            }
            // The panel's Save is in the sheet, a window of its own.
            if !mine.iter().any(|r| names(r, "Export")) {
                fail(&mut v, "no done action on Export… in the records".into());
            }
        }
        "minimised-code" => {
            let p = presses(events, "mini-apply");
            let got: Vec<_> = p.iter().map(|e| e.v.clone().unwrap_or_default()).collect();
            if got != ["4711"] {
                fail(&mut v, format!("Apply pressed with {got:?}"));
            }
            if events
                .iter()
                .any(|e| e.id == "mini-window" && e.ev == "restored")
            {
                fail(&mut v, "the window was restored from the Dock".into());
            }
            match st("mini-window").as_deref() {
                Some("minimised") => {}
                None => v.notes.push("no state snapshot; read from events".into()),
                Some(other) => fail(&mut v, format!("the window is {other} at the end")),
            }
            if !on_window("Apply") {
                fail(&mut v, "no done action on Apply in the records".into());
            }
        }
        _ => {}
    }
    // Pixel work isn't wrong here, but a worker that only typed keys into the app's focus
    // without a record on the window didn't use the tools on it.
    if mine.is_empty() {
        fail(&mut v, "no done action on the app in the records".into());
    }
    Some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;
    use crate::record::ActionRecord;

    fn ev(id: &str, ev: &str, v: Option<&str>) -> Event {
        Event {
            t: 0.0,
            id: id.into(),
            ev: ev.into(),
            x: None,
            y: None,
            v: v.map(str::to_owned),
        }
    }

    fn rec(pid: i32, window: u32, target: &str, status: Status) -> ActionRecord {
        let mut r: ActionRecord = serde_json::from_value(serde_json::json!({
            "index": 0,
            "at_ms": 0,
            "worker": "w",
            "pid": pid,
            "window": window,
            "window_title": "",
            "user_focus_kept": true,
            "action": {"do": "click", "ref": "e1"},
            "status": "done",
            "timings": {"checks_ms": 0.0, "dispatch_ms": 0.0, "settle_ms": 0.0}
        }))
        .unwrap();
        r.target = Some(target.into());
        r.status = status;
        if status == Status::Failed {
            r.error = Some(ErrorCode::Failed);
        }
        r
    }

    fn prep(task: &str) -> Prepared {
        Prepared {
            task: task.into(),
            pid: 7,
            window: 70,
            ..Default::default()
        }
    }

    #[test]
    fn every_quirk_task_is_found_by_its_id() {
        for t in &QUIRKS {
            assert_eq!(crate::suite::task(t.id).map(|x| x.id), Some(t.id));
        }
    }

    #[test]
    fn the_electron_form_passes_only_when_submitted_once_with_both_values() {
        let p = prep("electron-signup");
        let recs = [rec(7, 70, "button \"Submit\"", Status::Done)];
        let good = [
            ev("name", "text", Some("Ada Lovelace")),
            ev("subscribe", "toggle", Some("on")),
            ev("submit", "press", Some("Ada Lovelace|on")),
        ];
        let v = check(&p, &good, &recs, &BTreeMap::new(), None).unwrap();
        assert!(v.pass, "{:?}", v.notes);
        let twice = [
            ev("submit", "press", Some("Ada Lovelace|on")),
            ev("submit", "press", Some("Ada Lovelace|on")),
        ];
        assert!(
            !check(&p, &twice, &recs, &BTreeMap::new(), None)
                .unwrap()
                .pass
        );
        let unticked = [ev("submit", "press", Some("Ada Lovelace|off"))];
        assert!(
            !check(&p, &unticked, &recs, &BTreeMap::new(), None)
                .unwrap()
                .pass
        );
        let tapped = [
            ev("submit", "press", Some("Ada Lovelace|on")),
            ev("tap", "press", Some("1")),
        ];
        let v = check(&p, &tapped, &recs, &BTreeMap::new(), None).unwrap();
        assert!(!v.pass);
        assert_eq!(v.wrong_target, 1);
    }

    #[test]
    fn a_task_done_without_the_tools_or_on_another_app_fails() {
        let p = prep("catalyst-order");
        let good = [ev("place-order", "press", Some("4|gift wrap"))];
        assert!(!check(&p, &good, &[], &BTreeMap::new(), None).unwrap().pass);
        let elsewhere = [
            rec(7, 70, "button \"Place Order\"", Status::Done),
            rec(8, 80, "button \"Delete\"", Status::Done),
        ];
        let v = check(&p, &good, &elsewhere, &BTreeMap::new(), None).unwrap();
        assert!(!v.pass);
        assert!(v.notes.iter().any(|n| n.contains("another app")));
    }

    #[test]
    fn the_save_panel_task_needs_the_apps_own_file_in_the_trial_folder() {
        let p = prep("save-panel");
        let recs = [rec(7, 70, "button \"Export…\"", Status::Done)];
        let events = [
            ev("export", "open", None),
            ev("export", "saved", Some(SAVED)),
        ];
        let mut files = BTreeMap::new();
        let v = check(&p, &events, &recs, &files, None).unwrap();
        assert!(!v.pass, "no file on disk");
        files.insert(SAVED.to_owned(), SAVED_TEXT.to_owned());
        let v = check(&p, &events, &recs, &files, None).unwrap();
        assert!(v.pass, "{:?}", v.notes);
        let mut rejected = events.to_vec();
        rejected.push(ev("export", "rejected", Some("/tmp/x.txt")));
        assert!(!check(&p, &rejected, &recs, &files, None).unwrap().pass);
        files.insert(SAVED.to_owned(), "written by someone else".to_owned());
        assert!(!check(&p, &events, &recs, &files, None).unwrap().pass);
    }

    #[test]
    fn the_minimised_window_must_stay_minimised() {
        let p = prep("minimised-code");
        let recs = [rec(7, 70, "button \"Apply\"", Status::Done)];
        let events = [
            ev("mini-code", "text", Some("4711")),
            ev("mini-apply", "press", Some("4711")),
        ];
        let mut st = BTreeMap::new();
        st.insert("mini-window".to_owned(), "minimised".to_owned());
        let v = check(&p, &events, &recs, &BTreeMap::new(), Some(&st)).unwrap();
        assert!(v.pass, "{:?}", v.notes);
        let mut restored = events.to_vec();
        restored.insert(0, ev("mini-window", "restored", None));
        assert!(
            !check(&p, &restored, &recs, &BTreeMap::new(), Some(&st))
                .unwrap()
                .pass
        );
        st.insert("mini-window".to_owned(), "restored".to_owned());
        assert!(
            !check(&p, &events, &recs, &BTreeMap::new(), Some(&st))
                .unwrap()
                .pass
        );
    }

    #[test]
    fn other_tasks_are_not_checked_here() {
        assert!(check(&prep("slider"), &[], &[], &BTreeMap::new(), None).is_none());
    }
}
