//! The agent cursor's contract (§4.5): where each worker's cursor goes, told by the engine just
//! before an action is delivered, and drawn by the helper's overlay. Telling it never waits:
//! the cursor glides beside the action, it never holds one back.
//!
//! [`CursorScene`] is what the overlay draws, kept apart from the drawing so its rules (fade,
//! end, stop) can be tested anywhere.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::geom::{Point, Rect};

/// What the cursor shows at its target.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "gesture", rename_all = "snake_case")]
pub enum Gesture {
    /// A pointer click: a pulse where it lands.
    Click,
    /// An element action (press, set a value, select, perform): the element's frame outlined.
    Press,
    /// Text going into an element.
    Type,
    Scroll,
    /// A drag from the cursor's target to `to` (global points).
    Drag {
        to: Point,
    },
}

/// Where a worker's cursor goes: global points, top left of the main display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Aim {
    pub worker: String,
    pub at: Point,
    /// The element's frame, when the action named one.
    pub element: Option<Rect>,
    pub gesture: Gesture,
}

/// Receives the engine's aims and the helper's session news. Implementations hand them to
/// another thread and return at once.
pub trait CursorSink: Send + Sync {
    fn aim(&self, aim: Aim);
    /// The worker's name, for its pill; told before each of its requests.
    fn label(&self, _worker: &str, _label: &str) {}
    /// The worker's session ended: its cursor goes.
    fn end(&self, _worker: &str) {}
    /// The user stopped computer use: every cursor goes.
    fn clear(&self) {}
}

/// The engine's cursor, none by default (the bench and tests run without one).
pub type Cursor = Option<Arc<dyn CursorSink>>;

/// A worker's cursor colour, the same as its avatar's in Brigadier's window: the app's
/// `glyphFor` (`components/glyphs/worker-glyphs.tsx`) on the worker's task id picks one of
/// [`GLYPHS`] shapes, and the shape's colour is one of [`COLORS`]. Two workers whose avatars
/// share a colour share it here too; their name pills tell them apart.
pub fn color_of(worker: &str) -> (u8, u8, u8) {
    let mut hash: u64 = 0;
    for c in worker.chars() {
        hash = (hash * 31 + c as u64) % 2_147_483_647;
    }
    COLORS[(hash % GLYPHS as u64) as usize % COLORS.len()]
}

/// How many glyph shapes the app has (`SHAPES` in `worker-glyphs.tsx`).
pub const GLYPHS: usize = 27;

/// The app's `--glyph-1` … `--glyph-8` (`styles/tokens.css`, oklch) in sRGB.
pub const COLORS: [(u8, u8, u8); 8] = [
    (0xb5, 0x8b, 0xf9),
    (0x2a, 0xc3, 0xbb),
    (0xf0, 0x72, 0xb3),
    (0x68, 0xcb, 0x6e),
    (0xf0, 0xb1, 0x35),
    (0x4b, 0xa3, 0xf7),
    (0xf8, 0x79, 0x66),
    (0xc1, 0xcf, 0x51),
];

/// How long a cursor stays after its last aim before it fades.
pub const IDLE: Duration = Duration::from_secs(5);
/// How long the fade takes; the cursor is gone after it.
pub const FADE: Duration = Duration::from_millis(400);
/// The most characters a pill shows; a longer name ends in an ellipsis.
pub const LABEL_CHARS: usize = 24;

/// What the scene is told, in the order it was told.
#[derive(Debug, Clone, PartialEq)]
pub enum CursorMsg {
    Aim(Aim),
    Label {
        worker: String,
        label: String,
    },
    End(String),
    Clear,
    /// Time passed: idle cursors fade, faded ones go.
    Tick,
}

/// Whether a cursor is fully shown or on its way out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Phase {
    Shown,
    Fading { since: Instant },
}

/// One worker's cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct WorkerCursor {
    /// Where its tip is, global points.
    pub at: Point,
    /// The element its last aim outlined, global points.
    pub element: Option<Rect>,
    pub color: (u8, u8, u8),
    /// The pill's text, already cut to [`LABEL_CHARS`].
    pub label: Option<String>,
    pub last_aim: Instant,
    pub phase: Phase,
}

/// What a message changed, for the drawing to follow.
#[derive(Debug, Clone, PartialEq)]
pub enum Change {
    /// The cursor goes to the aim. `appeared`: it wasn't there, so it shows up at the point
    /// instead of gliding from somewhere.
    Moved {
        aim: Aim,
        appeared: bool,
    },
    /// The pill's text changed.
    Labelled {
        worker: String,
    },
    Fade {
        worker: String,
    },
    Removed {
        worker: String,
    },
}

/// Every worker's cursor, by worker id, and the names told for workers without one yet.
#[derive(Debug, Default)]
pub struct CursorScene {
    cursors: BTreeMap<String, WorkerCursor>,
    labels: HashMap<String, String>,
}

impl CursorScene {
    pub fn get(&self, worker: &str) -> Option<&WorkerCursor> {
        self.cursors.get(worker)
    }

    pub fn cursors(&self) -> impl Iterator<Item = (&str, &WorkerCursor)> {
        self.cursors.iter().map(|(w, c)| (w.as_str(), c))
    }

    /// Applies one message at `now` and says what changed. A removed cursor comes back only
    /// with a new aim: a late tick or name never brings it back.
    pub fn apply(&mut self, now: Instant, msg: CursorMsg) -> Vec<Change> {
        match msg {
            CursorMsg::Aim(aim) => {
                let label = self.labels.get(&aim.worker).cloned();
                let appeared = !self.cursors.contains_key(&aim.worker);
                let c = self
                    .cursors
                    .entry(aim.worker.clone())
                    .or_insert_with(|| WorkerCursor {
                        at: aim.at,
                        element: None,
                        color: color_of(&aim.worker),
                        label,
                        last_aim: now,
                        phase: Phase::Shown,
                    });
                c.at = match aim.gesture {
                    Gesture::Drag { to } => to,
                    _ => aim.at,
                };
                c.element = aim.element;
                c.last_aim = now;
                c.phase = Phase::Shown;
                vec![Change::Moved { aim, appeared }]
            }
            CursorMsg::Label { worker, label } => {
                let label = pill_text(&label);
                let changed = match self.cursors.get_mut(&worker) {
                    Some(c) if c.label.as_deref() != Some(label.as_str()) => {
                        c.label = Some(label.clone());
                        true
                    }
                    _ => false,
                };
                self.labels.insert(worker.clone(), label);
                if changed {
                    vec![Change::Labelled { worker }]
                } else {
                    Vec::new()
                }
            }
            CursorMsg::End(worker) => {
                self.labels.remove(&worker);
                match self.cursors.remove(&worker) {
                    Some(_) => vec![Change::Removed { worker }],
                    None => Vec::new(),
                }
            }
            CursorMsg::Clear => {
                self.labels.clear();
                std::mem::take(&mut self.cursors)
                    .into_keys()
                    .map(|worker| Change::Removed { worker })
                    .collect()
            }
            CursorMsg::Tick => {
                let mut out = Vec::new();
                self.cursors.retain(|worker, c| match c.phase {
                    Phase::Shown if now.duration_since(c.last_aim) >= IDLE => {
                        c.phase = Phase::Fading { since: now };
                        out.push(Change::Fade {
                            worker: worker.clone(),
                        });
                        true
                    }
                    Phase::Fading { since } if now.duration_since(since) >= FADE => {
                        out.push(Change::Removed {
                            worker: worker.clone(),
                        });
                        false
                    }
                    _ => true,
                });
                out
            }
        }
    }
}

/// A worker's name as its pill shows it: at most [`LABEL_CHARS`] characters.
pub fn pill_text(label: &str) -> String {
    let label = label.trim();
    if label.chars().count() <= LABEL_CHARS {
        return label.to_owned();
    }
    let mut s: String = label.chars().take(LABEL_CHARS - 1).collect();
    s.truncate(s.trim_end().len());
    s.push('…');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aim(worker: &str, x: f64, y: f64) -> CursorMsg {
        CursorMsg::Aim(Aim {
            worker: worker.into(),
            at: Point::new(x, y),
            element: None,
            gesture: Gesture::Click,
        })
    }

    fn label(worker: &str, label: &str) -> CursorMsg {
        CursorMsg::Label {
            worker: worker.into(),
            label: label.into(),
        }
    }

    fn removed(worker: &str) -> Change {
        Change::Removed {
            worker: worker.into(),
        }
    }

    fn workers(s: &CursorScene) -> Vec<&str> {
        s.cursors().map(|(w, _)| w).collect()
    }

    #[test]
    fn an_aim_shows_the_cursor_with_its_colour_and_name_then_moves_it() {
        let t = Instant::now();
        let mut s = CursorScene::default();
        assert!(s.apply(t, label("a", "Fix the login page")).is_empty());
        let ch = s.apply(t, aim("a", 10.0, 20.0));
        assert!(matches!(ch[..], [Change::Moved { appeared: true, .. }]));
        let c = s.get("a").unwrap();
        assert_eq!(c.at, Point::new(10.0, 20.0));
        assert_eq!(c.color, color_of("a"));
        assert_eq!(c.label.as_deref(), Some("Fix the login page"));
        let ch = s.apply(t, aim("a", 30.0, 40.0));
        assert!(matches!(
            ch[..],
            [Change::Moved {
                appeared: false,
                ..
            }]
        ));
        // A drag ends where it lets go.
        let drag = CursorMsg::Aim(Aim {
            worker: "a".into(),
            at: Point::new(1.0, 1.0),
            element: None,
            gesture: Gesture::Drag {
                to: Point::new(50.0, 60.0),
            },
        });
        s.apply(t, drag);
        assert_eq!(s.get("a").unwrap().at, Point::new(50.0, 60.0));
        // A new name for a shown cursor changes its pill; the same name changes nothing.
        assert_eq!(
            s.apply(t, label("a", "Renamed")),
            vec![Change::Labelled { worker: "a".into() }]
        );
        assert!(s.apply(t, label("a", "Renamed")).is_empty());
    }

    #[test]
    fn a_cursor_fades_after_five_idle_seconds_and_then_goes() {
        let t = Instant::now();
        let mut s = CursorScene::default();
        s.apply(t, aim("a", 0.0, 0.0));
        assert!(s.apply(t + IDLE / 2, CursorMsg::Tick).is_empty());
        // An aim starts the idle time again.
        s.apply(t + IDLE / 2, aim("a", 1.0, 1.0));
        assert!(s.apply(t + IDLE, CursorMsg::Tick).is_empty());
        let faded = t + IDLE / 2 + IDLE;
        assert_eq!(
            s.apply(faded, CursorMsg::Tick),
            vec![Change::Fade { worker: "a".into() }]
        );
        assert!(matches!(s.get("a").unwrap().phase, Phase::Fading { .. }));
        // A second timer for the same fade does nothing until the fade is over.
        assert!(s.apply(faded + FADE / 2, CursorMsg::Tick).is_empty());
        assert_eq!(s.apply(faded + FADE, CursorMsg::Tick), vec![removed("a")]);
        assert!(s.get("a").is_none());
        assert!(s.apply(faded + FADE * 2, CursorMsg::Tick).is_empty());
    }

    #[test]
    fn an_aim_while_fading_shows_the_cursor_again() {
        let t = Instant::now();
        let mut s = CursorScene::default();
        s.apply(t, aim("a", 0.0, 0.0));
        s.apply(t + IDLE, CursorMsg::Tick);
        let ch = s.apply(t + IDLE, aim("a", 5.0, 5.0));
        assert!(matches!(
            ch[..],
            [Change::Moved {
                appeared: false,
                ..
            }]
        ));
        assert_eq!(s.get("a").unwrap().phase, Phase::Shown);
        // The fade's own timer finds it shown and leaves it.
        assert!(s.apply(t + IDLE + FADE, CursorMsg::Tick).is_empty());
    }

    #[test]
    fn end_removes_one_cursor_and_clear_removes_every_one() {
        let t = Instant::now();
        let mut s = CursorScene::default();
        s.apply(t, aim("a", 0.0, 0.0));
        s.apply(t, aim("b", 0.0, 0.0));
        s.apply(t, aim("c", 0.0, 0.0));
        assert_eq!(s.apply(t, CursorMsg::End("b".into())), vec![removed("b")]);
        assert_eq!(workers(&s), ["a", "c"]);
        assert!(s.apply(t, CursorMsg::End("b".into())).is_empty());
        assert_eq!(
            s.apply(t, CursorMsg::Clear),
            vec![removed("a"), removed("c")]
        );
        assert!(workers(&s).is_empty());
        assert!(s.apply(t, CursorMsg::Clear).is_empty());
    }

    #[test]
    fn updates_still_queued_when_end_or_clear_arrives_never_bring_a_cursor_back() {
        let t = Instant::now();
        let mut s = CursorScene::default();
        s.apply(t, label("a", "A"));
        s.apply(t, aim("a", 0.0, 0.0));
        s.apply(t, aim("b", 0.0, 0.0));
        // `b` is fading when the stop comes.
        s.apply(t + IDLE, aim("a", 1.0, 1.0));
        s.apply(t + IDLE, CursorMsg::Tick);
        assert!(matches!(s.get("b").unwrap().phase, Phase::Fading { .. }));
        // Queued behind the end and the stop: fade timers, a name, a late tick.
        let queue = [
            CursorMsg::End("a".into()),
            CursorMsg::Tick,
            label("a", "A again"),
            CursorMsg::Clear,
            CursorMsg::Tick,
            label("b", "B"),
        ];
        let mut changes = Vec::new();
        for (i, m) in queue.into_iter().enumerate() {
            changes.extend(s.apply(t + IDLE * 3 + FADE * i as u32, m));
        }
        assert_eq!(changes, vec![removed("a"), removed("b")]);
        assert!(workers(&s).is_empty());
        // A later aim resumes the worker: shown again, with the name told last.
        let ch = s.apply(t + IDLE * 4, aim("b", 2.0, 2.0));
        assert!(matches!(ch[..], [Change::Moved { appeared: true, .. }]));
        assert_eq!(s.get("b").unwrap().label.as_deref(), Some("B"));
        assert_eq!(s.get("b").unwrap().phase, Phase::Shown);
        // A name told before the stop is forgotten with the cursors.
        s.apply(t + IDLE * 4, aim("a", 2.0, 2.0));
        assert_eq!(s.get("a").unwrap().label, None);
    }

    #[test]
    fn a_long_name_is_cut_with_an_ellipsis() {
        assert_eq!(pill_text("  Short  "), "Short");
        let cut = pill_text("Refactor the billing service and its tests");
        assert_eq!(cut, "Refactor the billing se…");
        assert_eq!(cut.chars().count(), LABEL_CHARS);
        assert_eq!(pill_text(&"x".repeat(LABEL_CHARS)), "x".repeat(LABEL_CHARS));
    }

    #[test]
    fn a_workers_colour_is_its_avatars() {
        // What the app's `glyphFor` gives for these ids (shape index; colour = shape mod 8).
        for (id, shape) in [("task-12", 2), ("task-3", 8), ("task-1", 6), ("task-2", 7)] {
            assert_eq!(color_of(id), COLORS[shape % 8], "{id}");
        }
    }
}
