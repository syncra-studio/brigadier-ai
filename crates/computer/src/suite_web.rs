//! The suite's browser tasks (docs/COMPUTER-USE-PLAN.md §8, Phase 5, stream A). The web fixture
//! (`fixtures/web-range`) is served on loopback and opened in a browser the session launched,
//! driven through its debugging protocol, or in a Chrome without one and a WKWebView, driven
//! through their accessibility web area. The page logs every input event with whether the
//! browser marked it as the user's own (`trusted`); the server notes any request that isn't a
//! browser's, so a worker can't post the log itself.

use std::collections::BTreeSet;

use crate::action::Action;
use crate::record::ActionRecord;
use crate::suite::{BOARDS, Event, Prepared, Setup, Task, Verdict, WebTarget};

const FORM: &str = "On the Web Range page, fill in the Sign-up form: Name Ada Lovelace (replacing the name that is there), Email ada@example.com, Plan Team; tick Accept terms but not Send news, rate it 2 stars, then press Submit once.";
const AX_FORM: &str = "On the Web Range page, set Name to Ada Lovelace (replacing the name that is there) and Plan to Team, tick Accept terms, then press Submit once.";
const AX_FORM_END: &str =
    "The form was submitted once with Name Ada Lovelace, Plan Team and Accept terms ticked.";

pub const WEB_TASKS: [Task; 6] = [
    Task {
        id: "web-form",
        setup: Setup::Web {
            target: WebTarget::Cdp,
        },
        goal: FORM,
        end_state: "The form was submitted once with Name Ada Lovelace, Email ada@example.com, Plan Team, Accept terms ticked, Send news unticked and a 2 star rating.",
    },
    Task {
        id: "web-iframe",
        setup: Setup::Web {
            target: WebTarget::Cdp,
        },
        goal: "In the Frames section of the Web Range page, enter Paris as the City and press Save city. Then, in the frame from another site below it, enter the code 4321 and press Send code.",
        end_state: "Save city was pressed with City Paris, and Send code with Code 4321.",
    },
    Task {
        id: "web-dialog",
        setup: Setup::Web {
            target: WebTarget::Cdp,
        },
        goal: "On the Web Range page, rename the item to Summary.txt with Rename item (the page asks for the new name). Then press Delete item and, when the page asks, cancel: the item must not be deleted.",
        end_state: "The page took the name Summary.txt, and the deletion was cancelled, not confirmed.",
    },
    Task {
        id: "web-canvas",
        setup: Setup::Web {
            target: WebTarget::Cdp,
        },
        goal: "In the Dots picture on the Web Range page, click the blue dot, then the red dot, once each. The picture has no accessibility elements.",
        end_state: "The blue dot, then the red dot, were clicked once each, and nothing else in the picture.",
    },
    Task {
        id: "web-ax-form",
        setup: Setup::Web {
            target: WebTarget::Plain,
        },
        goal: AX_FORM,
        end_state: AX_FORM_END,
    },
    Task {
        id: "web-ax-form-webkit",
        setup: Setup::Web {
            target: WebTarget::WebView,
        },
        goal: AX_FORM,
        end_state: AX_FORM_END,
    },
];

/// The page's P3 boards (`fixtures/web-range/grounding.html`), one task per marker size: pixels
/// read off the page's screenshots, clicked through the browser's protocol.
pub const WEB_GROUNDING: [Task; 2] = [
    grounding(8, "Web grounding 8 px"),
    grounding(16, "Web grounding 16 px"),
];

const fn grounding(size: u32, id: &'static str) -> Task {
    Task {
        id,
        setup: Setup::WebGrounding { size },
        goal: "The Board picture on the Grounding boards page shows numbered markers 1 to 5 and lettered decoys A to E; it has no accessibility elements. On each board, click marker 1, then 2, 3, 4 and 5, once each, at its dot (not its label), then press Next board. Go on until the page says all boards are done. Don't click a marker twice or click to test.",
        end_state: "Every board's five markers were clicked once each, in order.",
    }
}

/// The page's controls each task may touch; an event on any other is a wrong target.
fn allowed(id: &str) -> &'static [&'static str] {
    match id {
        "web-form" => &["name", "email", "plan", "terms", "star-2", "submit", "form"],
        "web-iframe" => &["city", "save-city", "code", "send-code"],
        "web-dialog" => &["rename", "delete"],
        "web-canvas" => &["blue", "red"],
        _ => &["name", "plan", "terms", "submit", "form"],
    }
}

/// Requests the server got from something other than a browser (`web_fixture`), next to the
/// page's log.
pub fn foreign_requests(prep: &Prepared) -> Vec<String> {
    let Some(log) = prep.log.as_deref() else {
        return Vec::new();
    };
    std::fs::read_to_string(format!("{log}.foreign"))
        .map(|t| t.lines().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// Checks a browser task: the page's log, trusted input only, the broker's records, and no
/// request to the page's server that a browser didn't make.
pub fn check(t: &Task, prep: &Prepared, events: &[Event], done: &[&ActionRecord], v: &mut Verdict) {
    let (target, grounding) = match t.setup {
        Setup::Web { target } => (target, None),
        Setup::WebGrounding { size } => (WebTarget::Cdp, Some(size)),
        _ => return,
    };
    if done.is_empty() {
        v.fail("no done action on the page's window in the records");
    }
    // Through the protocol only in a browser the session launched; elsewhere accessibility.
    let page_rung = done
        .iter()
        .filter(|r| r.rung.and_then(|g| serde_json::to_value(g).ok()) == Some("page".into()))
        .count();
    match target {
        WebTarget::Cdp if page_rung == 0 => v.fail("no action went through the page's protocol"),
        WebTarget::Plain | WebTarget::WebView if page_rung > 0 => {
            v.fail("an action went through a protocol this browser doesn't offer");
        }
        _ => {}
    }
    let foreign = foreign_requests(prep);
    if !foreign.is_empty() {
        v.fail(format!(
            "the page's server got requests no browser made: {}",
            foreign.join("; ")
        ));
    }
    let untrusted: Vec<String> = events
        .iter()
        .filter(|e| e.trusted == Some(false))
        .map(|e| format!("{} {}", e.id, e.ev))
        .collect();
    if !untrusted.is_empty() {
        v.fail(format!(
            "input the browser didn't mark as the user's: {}",
            untrusted.join(", ")
        ));
    }
    if let Some(size) = grounding {
        // As the native boards: every board dealt and every click a hit; misses are its own count.
        let g = crate::suite::score_grounding(size, events);
        if g.boards < BOARDS {
            v.fail(format!("{} of {BOARDS} boards done", g.boards));
        }
        if g.hits < g.trials {
            v.fail(format!(
                "{} of {} hits ({} wrong, {} misses, {} missing)",
                g.hits, g.trials, g.wrong, g.misses, g.missing
            ));
        }
        v.grounding = Some(g);
        return;
    }
    let ok = allowed(t.id);
    let wrong: Vec<String> = events
        .iter()
        .filter(|e| e.id != "app" && !ok.contains(&e.id.as_str()))
        .map(|e| format!("{} {}", e.id, e.ev))
        .collect();
    v.wrong_target = wrong.len();
    if !wrong.is_empty() {
        v.fail(format!(
            "{} events on other controls: {}",
            wrong.len(),
            wrong.join(", ")
        ));
    }
    let of = |id: &str, ev: &str| -> Vec<&Event> {
        events.iter().filter(|e| e.id == id && e.ev == ev).collect()
    };
    let last_v = |id: &str, ev: &str| of(id, ev).last().and_then(|e| e.v.clone());
    match t.id {
        "web-form" | "web-ax-form" | "web-ax-form-webkit" => {
            let submits = of("form", "submit");
            if submits.len() != 1 {
                v.fail(format!("the form was submitted {} times", submits.len()));
            }
            let form: serde_json::Value = submits
                .last()
                .and_then(|e| e.v.as_deref())
                .and_then(|s| serde_json::from_str(s).ok())
                .unwrap_or_default();
            let mut want = vec![
                ("name", serde_json::json!("Ada Lovelace")),
                ("plan", serde_json::json!("Team")),
                ("terms", serde_json::json!(true)),
            ];
            if t.id == "web-form" {
                want.push(("email", serde_json::json!("ada@example.com")));
                want.push(("news", serde_json::json!(false)));
                want.push(("rating", serde_json::json!("2 stars")));
            }
            for (k, w) in want {
                if form[k] != w {
                    v.fail(format!("the submitted {k} is {}, not {w}", form[k]));
                }
            }
        }
        "web-iframe" => {
            if last_v("save-city", "click").as_deref() != Some("Paris") {
                v.fail("Save city wasn't pressed with City Paris");
            }
            if last_v("send-code", "click").as_deref() != Some("4321") {
                v.fail("Send code wasn't pressed with Code 4321");
            }
        }
        "web-dialog" => {
            if last_v("rename", "prompt").as_deref() != Some("Summary.txt") {
                v.fail(format!(
                    "the page took the name {:?}, not Summary.txt",
                    last_v("rename", "prompt")
                ));
            }
            let answers: BTreeSet<String> = of("delete", "confirm")
                .iter()
                .filter_map(|e| e.v.clone())
                .collect();
            if answers.is_empty() {
                v.fail("Delete item's question was never answered");
            }
            if answers.contains("accepted") {
                v.fail("the deletion was confirmed");
            }
        }
        "web-canvas" => {
            let hits: Vec<&str> = events
                .iter()
                .filter(|e| e.ev == "down")
                .map(|e| e.id.as_str())
                .collect();
            if hits != ["blue", "red"] {
                v.fail(format!(
                    "the picture got clicks on {hits:?}, not blue then red"
                ));
            }
            let pixels = done
                .iter()
                .filter(
                    |r| matches!(&r.action, Action::Click { target, .. } if target.r#ref.is_none()),
                )
                .count();
            if pixels < 2 {
                v.fail("the dots weren't clicked at points read from a screenshot");
            }
        }
        _ => v.fail(format!("no checker for {}", t.id)),
    }
}
