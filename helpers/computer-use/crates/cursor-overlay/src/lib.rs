//! cursor-overlay — shared types and math for the brigadier-cu cursor overlay.
//!
//! Platform renderers (macOS, Windows, Linux) depend on this crate for:
//! - `CursorConfig` — parsed from CLI args (`--cursor-icon`, `--cursor-id`, etc.)
//! - `Palette` — 9 named colour palettes matching the reference implementations
//! - `MotionConfig` — glide duration, spring, dwell, idle-hide timings
//! - `CubicBezier` + `PathPlanner` — Bezier path math (ported 1:1 from C#)
//! - `CursorShape` — loaded and rasterised custom SVG / ICO / PNG asset
//! - `OverlayCommand` — messages sent from MCP tools to the overlay thread

pub mod bezier;
pub mod capture_utils;
pub mod motion;
pub mod palette;
pub mod path_planner;
pub mod render_state;
pub mod shape;
pub mod util;
pub mod z_order;

pub use bezier::CubicBezier;
pub use motion::{MotionConfig, Spring};
pub use palette::Palette;
pub use path_planner::{PathPlanner, PathState, PlannedPath};
pub use render_state::{
    draw_default_arrow, paint_cursor, render_frame, FocusRect, MotionTickResult, RenderStateCore,
};
pub use shape::{resolve_cursor_icon, BuiltinShape, CursorIconResolution, CursorShape};
pub use z_order::ZOrderEnforcer;

/// Configuration assembled from CLI arguments and passed to every
/// platform backend when it initialises the overlay window.
#[derive(Debug, Clone)]
pub struct CursorConfig {
    /// Multi-cursor instance identifier; affects palette selection.
    /// Defaults to `"default"`.
    pub cursor_id: String,

    /// Custom cursor shape loaded from `--cursor-icon <path>`. Takes
    /// precedence over `builtin_shape` when set. `None` means use the
    /// built-in selected by `builtin_shape`.
    pub shape: Option<CursorShape>,

    /// Which built-in silhouette to render when no custom `shape` is set.
    /// Defaults to [`BuiltinShape::Teardrop`]. Embedding hosts can select the
    /// branded [`BuiltinShape::Kite`] Sky kite with `--cursor-shape kite`.
    pub builtin_shape: BuiltinShape,

    /// Initial motion config (can be updated at runtime via MCP tool).
    pub motion: MotionConfig,

    /// Whether the overlay is visible at startup.
    /// Pass `--no-overlay` to disable.
    pub enabled: bool,

    /// Launch-time gradient default from `BRIGADIER_CU_CURSOR_GRADIENT`.
    pub gradient_colors: Vec<[u8; 4]>,

    /// Launch-time halo default from `BRIGADIER_CU_CURSOR_BLOOM`.
    pub bloom_color: Option<[u8; 4]>,

    /// Launch-time per-instance label default from `BRIGADIER_CU_CURSOR_LABEL`.
    pub cursor_label: Option<String>,
}

impl Default for CursorConfig {
    fn default() -> Self {
        Self {
            cursor_id: "default".into(),
            shape: None,
            builtin_shape: BuiltinShape::default(),
            motion: MotionConfig::default(),
            enabled: true,
            gradient_colors: Vec::new(),
            bloom_color: None,
            cursor_label: None,
        }
    }
}

/// Cursor-branding defaults injected by an embedding host.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentCursorDefaults {
    pub gradient_colors: Vec<[u8; 4]>,
    pub bloom_color: Option<[u8; 4]>,
    pub cursor_label: Option<String>,
}

impl AgentCursorDefaults {
    /// Parse environment-shaped values without touching process-global env.
    /// The returned strings are warnings for invalid values that were ignored.
    pub fn parse_values(
        gradient: Option<&str>,
        bloom: Option<&str>,
        label: Option<&str>,
    ) -> (Self, Vec<String>) {
        let mut defaults = Self::default();
        let mut warnings = Vec::new();

        if let Some(value) = gradient {
            let stops: Option<Vec<[u8; 4]>> = (!value.trim().is_empty())
                .then(|| {
                    value
                        .split(',')
                        .map(str::trim)
                        .map(parse_hex_color)
                        .collect::<Option<Vec<_>>>()
                })
                .flatten()
                .filter(|stops| !stops.is_empty());
            match stops {
                Some(stops) => defaults.gradient_colors = stops,
                None => warnings.push(format!(
                    "invalid BRIGADIER_CU_CURSOR_GRADIENT={value:?}; expected comma-separated #RGB/#RRGGBB stops"
                )),
            }
        }

        if let Some(value) = bloom {
            match (!value.trim().is_empty())
                .then(|| parse_hex_color(value.trim()))
                .flatten()
            {
                Some(color) => defaults.bloom_color = Some(color),
                None => warnings.push(format!(
                    "invalid BRIGADIER_CU_CURSOR_BLOOM={value:?}; expected #RGB or #RRGGBB"
                )),
            }
        }

        if let Some(value) = label {
            let value = value.trim();
            if value.is_empty() {
                warnings
                    .push("invalid BRIGADIER_CU_CURSOR_LABEL: label must not be empty".to_owned());
            } else {
                defaults.cursor_label = Some(value.to_owned());
            }
        }

        (defaults, warnings)
    }

    pub fn from_env() -> Self {
        let gradient = std::env::var("BRIGADIER_CU_CURSOR_GRADIENT").ok();
        let bloom = std::env::var("BRIGADIER_CU_CURSOR_BLOOM").ok();
        let label = std::env::var("BRIGADIER_CU_CURSOR_LABEL").ok();
        let (defaults, warnings) =
            Self::parse_values(gradient.as_deref(), bloom.as_deref(), label.as_deref());
        for warning in warnings {
            eprintln!("[brigadier-cu] warning: {warning}");
        }
        defaults
    }
}

/// Parse `#RRGGBB` or `#RGB` to opaque RGBA. This intentionally matches the
/// accepted set_agent_cursor_style colour vocabulary.
pub fn parse_hex_color(hex: &str) -> Option<[u8; 4]> {
    let value = hex.strip_prefix('#').unwrap_or(hex);
    match value.len() {
        6 => Some([
            u8::from_str_radix(&value[0..2], 16).ok()?,
            u8::from_str_radix(&value[2..4], 16).ok()?,
            u8::from_str_radix(&value[4..6], 16).ok()?,
            255,
        ]),
        3 => Some([
            u8::from_str_radix(&value[0..1].repeat(2), 16).ok()?,
            u8::from_str_radix(&value[1..2].repeat(2), 16).ok()?,
            u8::from_str_radix(&value[2..3].repeat(2), 16).ok()?,
            255,
        ]),
        _ => None,
    }
}

impl CursorConfig {
    /// Parse from `std::env::args()`.
    ///
    /// Recognised flags:
    /// ```text
    /// --cursor-icon  <path.svg|path.ico|path.png>
    /// --cursor-id    <id>
    /// --cursor-shape <arrow|teardrop|sky|kite>  (selects a built-in silhouette;
    ///                                            default: teardrop)
    /// --cursor-palette <name>     (selects a named Palette)
    /// --no-overlay                (start with overlay disabled)
    /// --glide-ms     <f64>        (glideDurationMs override)
    /// --dwell-ms     <f64>        (dwellAfterClickMs override)
    /// --idle-hide-ms <f64>        (idleHideMs override)
    /// --cursor-speed <f64>        (glide speed multiplier, clamped to [0.25, 8])
    /// ```
    pub fn from_args() -> Self {
        let args: Vec<String> = std::env::args().collect();
        Self::parse(&args[1..])
    }

    pub fn parse(args: &[String]) -> Self {
        let mut cfg = CursorConfig::default();
        let defaults = AgentCursorDefaults::from_env();
        cfg.gradient_colors = defaults.gradient_colors;
        cfg.bloom_color = defaults.bloom_color;
        cfg.cursor_label = defaults.cursor_label;
        let mut i = 0usize;
        while i < args.len() {
            match args[i].as_str() {
                "--cursor-icon" => {
                    if let Some(path) = args.get(i + 1) {
                        match CursorShape::load(path) {
                            Ok(s) => cfg.shape = Some(s),
                            Err(e) => tracing::warn!("--cursor-icon {path}: {e}"),
                        }
                        i += 1;
                    }
                }
                "--cursor-id" => {
                    if let Some(id) = args.get(i + 1) {
                        cfg.cursor_id = id.clone();
                        i += 1;
                    }
                }
                "--cursor-palette" => {
                    if let Some(name) = args.get(i + 1) {
                        // Palette is resolved inside the platform backend using the id;
                        // store the name as the id so ForInstance logic picks it up.
                        cfg.cursor_id = name.clone();
                        i += 1;
                    }
                }
                "--cursor-shape" => {
                    if let Some(name) = args.get(i + 1) {
                        match BuiltinShape::parse(name) {
                            Some(s) => cfg.builtin_shape = s,
                            None => tracing::warn!(
                                "--cursor-shape {name}: unknown shape (expected {}); falling back to default",
                                BuiltinShape::names_help()
                            ),
                        }
                        i += 1;
                    }
                }
                "--no-overlay" => cfg.enabled = false,
                "--glide-ms" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        cfg.motion.glide_duration_ms = v;
                        i += 1;
                    }
                }
                "--dwell-ms" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        cfg.motion.dwell_after_click_ms = v;
                        i += 1;
                    }
                }
                "--idle-hide-ms" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        cfg.motion.idle_hide_ms = v;
                        i += 1;
                    }
                }
                "--cursor-speed" => {
                    if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                        cfg.motion = cfg.motion.scaled_by(v);
                        i += 1;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        cfg
    }

    /// Return the `Palette` for this config (by cursor_id).
    pub fn palette(&self) -> Palette {
        Palette::for_instance(&self.cursor_id)
    }
}

// ── Shared cursor instance registry ──────────────────────────────────────────

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Per-instance cursor configuration (icon, color, label, size, opacity).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorInstanceConfig {
    pub cursor_id: String,
    pub cursor_icon: Option<String>,
    pub cursor_color: Option<String>,
    pub cursor_label: Option<String>,
    pub cursor_size: Option<f64>,
    pub cursor_opacity: Option<f64>,
    pub enabled: bool,
}

impl Default for CursorInstanceConfig {
    fn default() -> Self {
        Self {
            cursor_id: "default".into(),
            cursor_icon: None,
            cursor_color: Some("#00FFFF".into()),
            cursor_label: None,
            cursor_size: Some(16.0),
            cursor_opacity: Some(0.85),
            enabled: true,
        }
    }
}

/// Runtime state for a cursor instance (config + last known position).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorInstanceState {
    pub config: CursorInstanceConfig,
    pub x: Option<f64>,
    pub y: Option<f64>,
}

/// Global registry of cursor instances, keyed by `cursor_id`.
pub struct CursorRegistry {
    inner: Mutex<HashMap<String, CursorInstanceState>>,
    default_config: CursorInstanceConfig,
}

impl CursorRegistry {
    pub fn new() -> Self {
        let defaults = AgentCursorDefaults::from_env();
        let mut default_config = CursorInstanceConfig::default();
        default_config.cursor_label = defaults.cursor_label;
        let mut map = HashMap::new();
        map.insert(
            "default".into(),
            CursorInstanceState {
                config: default_config.clone(),
                x: None,
                y: None,
            },
        );
        Self {
            inner: Mutex::new(map),
            default_config,
        }
    }

    fn config_for(&self, cursor_id: &str) -> CursorInstanceConfig {
        CursorInstanceConfig {
            cursor_id: cursor_id.to_owned(),
            ..self.default_config.clone()
        }
    }

    pub fn get_or_create(&self, cursor_id: &str) -> CursorInstanceState {
        let config = self.config_for(cursor_id);
        let mut inner = self.inner.lock().unwrap();
        inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorInstanceState {
                config,
                x: None,
                y: None,
            })
            .clone()
    }

    pub fn update_position(&self, cursor_id: &str, x: f64, y: f64) {
        let config = self.config_for(cursor_id);
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorInstanceState {
                config,
                x: None,
                y: None,
            });
        state.x = Some(x);
        state.y = Some(y);
    }

    pub fn set_enabled(&self, cursor_id: &str, enabled: bool) {
        let config = self.config_for(cursor_id);
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorInstanceState {
                config,
                x: None,
                y: None,
            });
        state.config.enabled = enabled;
    }

    pub fn update_config(&self, cursor_id: &str, f: impl FnOnce(&mut CursorInstanceConfig)) {
        let config = self.config_for(cursor_id);
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorInstanceState {
                config,
                x: None,
                y: None,
            });
        f(&mut state.config);
    }

    pub fn all_states(&self) -> Vec<CursorInstanceState> {
        self.inner.lock().unwrap().values().cloned().collect()
    }

    /// Drop a session's cursor metadata entry (fired from the `session_end`
    /// hook). The `"default"` key backs the anonymous / one-shot path and is
    /// guarded against removal; an empty or absent key is a harmless no-op.
    pub fn remove(&self, cursor_id: &str) {
        if cursor_id.is_empty() || cursor_id == "default" {
            return;
        }
        self.inner.lock().unwrap().remove(cursor_id);
    }
}

impl Default for CursorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Identifier for one owned cursor in the keyed render collection.
///
/// Resolved by the macOS tool layer (see `resolve_cursor_key`) with the
/// precedence: explicit `cursor_id` arg > injected `_session_id` > `"default"`.
/// The render side treats it as an opaque insertion-ordered map key; the
/// `"default"` key is special-cased (never removed) so the anonymous /
/// one-shot `brigadier-cu call` path is backward compatible.
pub type CursorKey = String;

/// A render command tagged with the cursor it targets. Wrapping the key
/// here (rather than inside [`OverlayCommand`]) keeps `OverlayCommand` and
/// the shared `apply_command_base` / `render_frame` API untouched, so the
/// Windows and Linux overlays — which never see a key — keep compiling
/// and behaving exactly as before.
#[derive(Debug, Clone)]
pub struct KeyedOverlayCommand {
    pub key: CursorKey,
    pub cmd: OverlayCommand,
}

/// Message carried over a platform overlay channel. Commands are keyed render
/// updates; removal and revival are ordered lifecycle events. Keeping lifecycle
/// outside [`OverlayCommand`] leaves the shared render-state match render-only.
#[derive(Debug, Clone)]
pub enum OverlayMsg {
    Cmd(KeyedOverlayCommand),
    Remove(CursorKey),
    /// Hide a live cursor while retaining its position/configuration for the
    /// next task on the same stable host session.
    HideReusable(CursorKey),
    /// Activate a retained cursor for a new task. This is the only lifecycle
    /// event that may make a reusable-hidden cursor visible again.
    Activate(CursorKey),
    /// An explicit `start_session` reused an ended id. Clear only that id's
    /// resurrection tombstone so commands after the revive are accepted while
    /// commands queued between `Remove` and `Revive` remain rejected.
    Revive(CursorKey),
}

/// Commands sent from MCP tool handlers to the overlay's render thread.
#[derive(Debug, Clone)]
pub enum OverlayCommand {
    /// Animate the cursor to a new screen position.
    MoveTo {
        x: f64,
        y: f64,
        end_heading_radians: f64,
    },
    /// Snap the cursor immediately to a screen position, optionally updating heading.
    SnapTo {
        x: f64,
        y: f64,
        heading_radians: Option<f64>,
    },
    /// Track a posted drag action point, preserving the selected shape's hotspot.
    DragTo { x: f64, y: f64, pressed: bool },
    /// Start the click-press visual.
    ClickPulse { x: f64, y: f64 },
    /// Toggle the held-button visual state.
    SetPressed(bool),
    /// Show or hide the overlay.
    SetEnabled(bool),
    /// Update the motion/timing config live.
    SetMotion(MotionConfig),
    /// Update the palette live.
    SetPalette(Palette),
    /// Pin the overlay above a specific window (by platform window id).
    PinAbove(u64),
    /// Replace the cursor shape at runtime with a custom image (or clear it).
    /// `None` clears the custom override so the configured `builtin_shape`
    /// shows again. Built-in silhouettes go through `SetBuiltinShape` instead.
    SetShape(Option<CursorShape>),
    /// Select the built-in silhouette at runtime
    /// (`arrow` / `teardrop` / `sky` / `kite`).
    /// Sets `builtin_shape` and clears any custom `SetShape` override, so
    /// either built-in is reachable regardless of which one is the default.
    SetBuiltinShape(BuiltinShape),
    /// Update the gradient/bloom colours used by the default arrow renderer.
    /// `gradient_colors`: ordered list of `#RRGGBB` hex strings.
    /// `bloom_color`: `#RRGGBB` hex string for the radial halo.
    SetGradient {
        gradient_colors: Vec<[u8; 4]>,
        bloom_color: Option<[u8; 4]>,
    },
    /// Show a focus-highlight rectangle around an AX-targeted element.
    /// `[x, y, width, height]` in screen coordinates (top-left origin).
    /// `None` clears the highlight.
    ShowFocusRect(Option<[f64; 4]>),
}

impl OverlayCommand {
    /// The overlay command that applies a resolved `cursor_icon` value: a
    /// built-in name selects the silhouette (`SetBuiltinShape`), a custom image
    /// becomes a one-off override (`SetShape`). Shared by every platform's MCP
    /// handler so they stay in lockstep.
    pub fn from_cursor_icon(resolution: CursorIconResolution) -> Self {
        match resolution {
            CursorIconResolution::Builtin(b) => OverlayCommand::SetBuiltinShape(b),
            CursorIconResolution::Image(s) => OverlayCommand::SetShape(Some(s)),
        }
    }
}

#[cfg(test)]
mod cursor_speed_arg_tests {
    use super::{CursorConfig, MotionConfig};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn cursor_speed_flag_scales_glide_speed() {
        let stock = MotionConfig::default();
        let cfg = CursorConfig::parse(&args(&["--cursor-speed", "1.75"]));
        assert_eq!(cfg.motion.peak_speed, stock.peak_speed * 1.75);
        assert_eq!(cfg.motion.min_end_speed, stock.min_end_speed * 1.75);
        // Click visuals keep their stock durations.
        assert_eq!(cfg.motion.press_duration_ms, stock.press_duration_ms);
        assert_eq!(cfg.motion.dwell_after_click_ms, stock.dwell_after_click_ms);
    }

    #[test]
    fn cursor_speed_flag_tolerates_garbage_and_missing_values() {
        let stock = MotionConfig::default();
        let garbage = CursorConfig::parse(&args(&["--cursor-speed", "fast"]));
        assert_eq!(garbage.motion.peak_speed, stock.peak_speed);
        let missing = CursorConfig::parse(&args(&["--cursor-speed"]));
        assert_eq!(missing.motion.peak_speed, stock.peak_speed);
    }
}
