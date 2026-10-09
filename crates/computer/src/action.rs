//! Requests and results of `observe`, `act` and `zoom` (§4.3, §4.4).

use serde::{Deserialize, Serialize};

use crate::desktop::Button;
use crate::error::CuError;
use crate::geom::Provider;

/// Where an action lands: an element by ref, or a pixel of a named image.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Target {
    #[serde(default, rename = "ref", skip_serializing_if = "Option::is_none")]
    pub r#ref: Option<String>,
    /// The image the point was read from (`i3`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub y: Option<f64>,
}

/// A predicate checked after an action, or waited for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "is", rename_all = "snake_case")]
pub enum Expect {
    ValueEquals {
        #[serde(rename = "ref")]
        r#ref: String,
        text: String,
    },
    ValueContains {
        #[serde(rename = "ref")]
        r#ref: String,
        text: String,
    },
    Checked {
        #[serde(rename = "ref")]
        r#ref: String,
        on: bool,
    },
    /// An element whose line contains `find` is present.
    Appears {
        find: String,
    },
    /// The element is gone.
    Gone {
        #[serde(rename = "ref")]
        r#ref: String,
    },
    TitleContains {
        text: String,
    },
    Focused {
        #[serde(rename = "ref")]
        r#ref: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "do", rename_all = "snake_case")]
pub enum Action {
    Click {
        #[serde(flatten)]
        target: Target,
        #[serde(default)]
        button: Button,
        #[serde(default = "one")]
        count: u8,
        #[serde(default)]
        modifiers: Vec<String>,
        #[serde(default)]
        expect: Option<Expect>,
    },
    SetValue {
        #[serde(rename = "ref")]
        r#ref: String,
        text: String,
        #[serde(default)]
        expect: Option<Expect>,
    },
    Type {
        text: String,
        #[serde(default, rename = "ref")]
        r#ref: Option<String>,
        #[serde(default)]
        expect: Option<Expect>,
    },
    Key {
        key: String,
        #[serde(default = "one_u32")]
        repeat: u32,
        #[serde(default)]
        expect: Option<Expect>,
    },
    Scroll {
        #[serde(flatten)]
        target: Target,
        #[serde(default)]
        dx: i32,
        #[serde(default)]
        dy: i32,
        #[serde(default)]
        expect: Option<Expect>,
    },
    Drag {
        from: Target,
        to: Target,
        #[serde(default)]
        expect: Option<Expect>,
    },
    Perform {
        #[serde(rename = "ref")]
        r#ref: String,
        action: String,
        #[serde(default)]
        expect: Option<Expect>,
    },
    Menu {
        path: Vec<String>,
        #[serde(default)]
        expect: Option<Expect>,
    },
    Wait {
        expect: Expect,
        #[serde(default = "wait_ms")]
        timeout_ms: u64,
    },
}

fn one() -> u8 {
    1
}
fn one_u32() -> u32 {
    1
}
fn wait_ms() -> u64 {
    5_000
}

impl Action {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Click { .. } => "click",
            Self::SetValue { .. } => "set_value",
            Self::Type { .. } => "type",
            Self::Key { .. } => "key",
            Self::Scroll { .. } => "scroll",
            Self::Drag { .. } => "drag",
            Self::Perform { .. } => "perform",
            Self::Menu { .. } => "menu",
            Self::Wait { .. } => "wait",
        }
    }

    pub fn expect(&self) -> Option<&Expect> {
        match self {
            Self::Click { expect, .. }
            | Self::SetValue { expect, .. }
            | Self::Type { expect, .. }
            | Self::Key { expect, .. }
            | Self::Scroll { expect, .. }
            | Self::Drag { expect, .. }
            | Self::Perform { expect, .. }
            | Self::Menu { expect, .. } => expect.as_ref(),
            Self::Wait { expect, .. } => Some(expect),
        }
    }

    /// Keyboard, menu and focus work: it needs the app's focus, so it is serialized per app.
    pub fn uses_app_focus(&self) -> bool {
        matches!(
            self,
            Self::Type { .. } | Self::Key { .. } | Self::Menu { .. }
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Screenshot {
    #[default]
    Auto,
    Always,
    Never,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserveRequest {
    pub window: u32,
    #[serde(default)]
    pub screenshot: Screenshot,
    /// The observation the diff is against; a diff needs one this worker received.
    #[serde(default)]
    pub since: Option<u64>,
    #[serde(default)]
    pub full: bool,
    #[serde(default, rename = "element")]
    pub element: Option<String>,
    #[serde(default)]
    pub find: Option<String>,
    /// A page of one element's full value: `{element, value_page}`.
    #[serde(default)]
    pub value_page: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActRequest {
    pub window: u32,
    pub actions: Vec<Action>,
    #[serde(default)]
    pub screenshot: Screenshot,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ZoomRequest {
    /// The image the region is read from.
    pub image: String,
    /// `[x0, y0, x1, y1]` in that image's pixels.
    pub region: [f64; 4],
}

/// The rung that delivered an action (§4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rung {
    /// An accessibility action or attribute: no events at all.
    Element,
    /// Events posted to the app, the user's focus untouched.
    Background,
    /// Events posted to the app inside synthetic activation, the user's focus untouched.
    BackgroundActivated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Confirmed,
    Unverified,
    NoChange,
    BackgroundUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Done,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Timings {
    /// The pre-action checks: window, block list, focus snapshot, before the action starts.
    pub checks_ms: f64,
    /// Until the action was delivered.
    pub dispatch_ms: f64,
    /// Until the change was seen (an `expect` held, or the element's state changed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect_ms: Option<f64>,
    /// Until the app went quiet (no notification for 50 ms), or the bound.
    pub settle_ms: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ActionResult {
    pub index: usize,
    pub action: String,
    pub status: Status,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivered: Option<Rung>,
    /// The app went quiet before the bound.
    pub settled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<CuError>,
    pub timings: Timings,
    /// Side effects worth knowing: a new window, another app in front.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

/// An image handed to the caller.
#[derive(Debug, Clone)]
pub struct ImageOut {
    pub id: String,
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub tokens: u32,
    pub provider: Provider,
}

/// What `observe`, `act` and `zoom` return: text first, then at most one image.
#[derive(Debug, Clone)]
pub struct Reply {
    pub text: String,
    pub image: Option<ImageOut>,
    /// Per-action results of an `act`, also rendered into `text`.
    pub results: Vec<ActionResult>,
}
