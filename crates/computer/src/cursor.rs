//! The agent cursor's contract (§4.5): where each worker's cursor goes, told by the engine just
//! before an action is delivered, and drawn by the helper's overlay. Telling it never waits:
//! the cursor glides beside the action, it never holds one back.

use std::sync::Arc;

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

/// Receives the engine's aims. Implementations hand them to another thread and return at once.
pub trait CursorSink: Send + Sync {
    fn aim(&self, aim: Aim);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_workers_colour_is_its_avatars() {
        // What the app's `glyphFor` gives for these ids (shape index; colour = shape mod 8).
        for (id, shape) in [("task-12", 2), ("task-3", 8), ("task-1", 6), ("task-2", 7)] {
            assert_eq!(color_of(id), COLORS[shape % 8], "{id}");
        }
    }
}
