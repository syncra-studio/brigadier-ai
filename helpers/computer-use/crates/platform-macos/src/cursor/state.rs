//! Cursor configuration and runtime state.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// Per-instance cursor configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorConfig {
    /// Instance identifier for multi-cursor use cases. Default: "default".
    pub cursor_id: String,
    /// Built-in icon name ("arrow", "teardrop", "Brigadier") or path to PNG/SVG.
    pub cursor_icon: Option<String>,
    /// Hex color (e.g. "#00FFFF") or CSS name for the glow/indicator.
    pub cursor_color: Option<String>,
    /// Short label rendered near the cursor dot.
    pub cursor_label: Option<String>,
    /// Dot radius in points. Default: 16.
    pub cursor_size: Option<f64>,
    /// Opacity 0.0–1.0. Default: 0.85.
    pub cursor_opacity: Option<f64>,
    /// Whether the overlay is currently visible.
    pub enabled: bool,
}

impl Default for CursorConfig {
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

/// Runtime position of the agent cursor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorPosition {
    pub x: f64,
    pub y: f64,
}

/// Full state for a cursor instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorState {
    pub config: CursorConfig,
    pub position: Option<CursorPosition>,
}

/// Global registry of cursor instances, keyed by `cursor_id`.
pub struct CursorRegistry {
    inner: Mutex<HashMap<String, CursorState>>,
    default_config: CursorConfig,
}

impl CursorRegistry {
    pub fn new() -> Self {
        let mut default_config = CursorConfig::default();
        default_config.cursor_label = cursor_overlay::AgentCursorDefaults::from_env().cursor_label;
        Self::with_default_config(default_config)
    }

    pub(crate) fn with_default_config(default_config: CursorConfig) -> Self {
        let mut map = HashMap::new();
        let default = CursorState {
            config: default_config.clone(),
            position: None,
        };
        map.insert("default".into(), default);
        Self {
            inner: Mutex::new(map),
            default_config,
        }
    }

    fn config_for(&self, cursor_id: &str) -> CursorConfig {
        CursorConfig {
            cursor_id: cursor_id.to_owned(),
            ..self.default_config.clone()
        }
    }

    pub fn get_or_create(&self, cursor_id: &str) -> CursorState {
        let config = self.config_for(cursor_id);
        let mut inner = self.inner.lock().unwrap();
        // Write-boundary resurrection guard: a per-session cursor keys on the
        // session_id, so an in-flight call that lands AFTER session_end (a slow
        // AX click that passed the dispatch gate, then the proxy died and the
        // reaper removed this cursor) must NOT re-create the entry the reaper
        // just cleared — it would be invisible and never reaped again.
        // `fire_session_end` marks ENDED_SESSIONS *before* fanning out to the
        // remove hook, so this check is authoritative. The seeded "default" key
        // and any explicit non-session cursor_id are never in ENDED_SESSIONS, so
        // they pass through unchanged. Return the live entry if one survives,
        // else a transient default so callers get a sane value without a write.
        if cu_core::session::is_session_ended(cursor_id) {
            return inner
                .get(cursor_id)
                .cloned()
                .unwrap_or_else(|| CursorState {
                    config: config.clone(),
                    position: None,
                });
        }
        inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorState {
                config,
                position: None,
            })
            .clone()
    }

    /// Non-creating read of a single cursor's state. Returns None when the id
    /// has no entry (used by get_agent_cursor_state to scope its result to the
    /// caller's session without materialising a phantom cursor).
    pub fn get(&self, cursor_id: &str) -> Option<CursorState> {
        self.inner.lock().unwrap().get(cursor_id).cloned()
    }

    pub fn update_config(&self, config: CursorConfig) {
        // Write-boundary resurrection guard (see get_or_create): no-op for an
        // ended session id so a post-session_end set_config can't resurrect the
        // cursor metadata. "default" / non-session ids are never ended.
        if config.cursor_id.is_empty() || cu_core::session::is_session_ended(&config.cursor_id) {
            return;
        }
        let mut inner = self.inner.lock().unwrap();
        let entry = inner
            .entry(config.cursor_id.clone())
            .or_insert_with(|| CursorState {
                config: config.clone(),
                position: None,
            });
        entry.config = config;
    }

    pub fn update_position(&self, cursor_id: &str, x: f64, y: f64) {
        // Write-boundary resurrection guard (see get_or_create): no-op for an
        // ended session id so an in-flight move after session_end can't
        // re-create the cleared cursor. "default" / non-session ids never ended.
        if cursor_id.is_empty() || cu_core::session::is_session_ended(cursor_id) {
            return;
        }
        let config = self.config_for(cursor_id);
        let mut inner = self.inner.lock().unwrap();
        // Get-or-create so a per-session cursor that moves before any explicit
        // enable/style call still shows up in get_agent_cursor_state.
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorState {
                config,
                position: None,
            });
        state.position = Some(CursorPosition { x, y });
    }

    pub fn set_enabled(&self, cursor_id: &str, enabled: bool) {
        // Write-boundary resurrection guard (see get_or_create): no-op for an
        // ended session id so a post-session_end enable/disable can't resurrect
        // the cleared cursor. "default" / non-session ids are never ended.
        if cursor_id.is_empty() || cu_core::session::is_session_ended(cursor_id) {
            return;
        }
        let config = self.config_for(cursor_id);
        let mut inner = self.inner.lock().unwrap();
        let state = inner
            .entry(cursor_id.to_owned())
            .or_insert_with(|| CursorState {
                config,
                position: None,
            });
        state.config.enabled = enabled;
    }

    pub fn all_states(&self) -> Vec<CursorState> {
        self.inner.lock().unwrap().values().cloned().collect()
    }

    /// Remove a cursor instance's metadata (fired from the `session_end` hook
    /// when a session disconnects). The `"default"` cursor backs the
    /// anonymous / one-shot path and is never removed — guarding here AND in
    /// the overlay Remove arm keeps the backward-compatibility contract intact.
    pub fn remove(&self, cursor_id: &str) {
        if cursor_id == "default" {
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

#[cfg(test)]
mod tests {
    use super::*;
    use cu_core::session::fire_session_end;

    #[test]
    fn cursor_branding_env_values_parse_valid_invalid_empty_and_missing() {
        let (valid, warnings) = cursor_overlay::AgentCursorDefaults::parse_values(
            Some("#12c7f5,#2d8cff,#6c5cff"),
            Some("#abc"),
            Some("Codex"),
        );
        assert!(warnings.is_empty());
        assert_eq!(
            valid.gradient_colors,
            vec![
                [0x12, 0xc7, 0xf5, 255],
                [0x2d, 0x8c, 0xff, 255],
                [0x6c, 0x5c, 0xff, 255],
            ]
        );
        assert_eq!(valid.bloom_color, Some([0xaa, 0xbb, 0xcc, 255]));
        assert_eq!(valid.cursor_label.as_deref(), Some("Codex"));

        let (invalid, warnings) = cursor_overlay::AgentCursorDefaults::parse_values(
            Some("#12c7f5,#nothex"),
            Some("#xyzxyz"),
            Some(""),
        );
        assert_eq!(invalid, cursor_overlay::AgentCursorDefaults::default());
        assert_eq!(warnings.len(), 3);

        let (empty, warnings) =
            cursor_overlay::AgentCursorDefaults::parse_values(Some(""), Some(""), Some("   "));
        assert_eq!(empty, cursor_overlay::AgentCursorDefaults::default());
        assert_eq!(warnings.len(), 3);

        let (missing, warnings) =
            cursor_overlay::AgentCursorDefaults::parse_values(None, None, None);
        assert_eq!(missing, cursor_overlay::AgentCursorDefaults::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn env_defaults_seed_every_instance_and_render_style_overrides_win() {
        let defaults = cursor_overlay::AgentCursorDefaults::parse_values(
            Some("#112233,#445566"),
            Some("#778899"),
            Some("Brigadier"),
        )
        .0;
        let mut runtime_default = CursorConfig::default();
        runtime_default.cursor_label = defaults.cursor_label.clone();
        let registry = CursorRegistry::with_default_config(runtime_default);
        assert_eq!(
            registry
                .get_or_create("session-a")
                .config
                .cursor_label
                .as_deref(),
            Some("Brigadier"),
        );
        assert_eq!(
            registry
                .get_or_create("session-b")
                .config
                .cursor_label
                .as_deref(),
            Some("Brigadier"),
        );

        let mut launch = cursor_overlay::CursorConfig::default();
        launch.gradient_colors = defaults.gradient_colors;
        launch.bloom_color = defaults.bloom_color;
        let mut render = cursor_overlay::RenderStateCore::new(launch);
        assert_eq!(
            render.gradient_colors,
            vec![[0x11, 0x22, 0x33, 255], [0x44, 0x55, 0x66, 255]]
        );
        assert_eq!(render.bloom_override, Some([0x77, 0x88, 0x99, 255]));
        render.apply_command_base(
            cursor_overlay::OverlayCommand::SetGradient {
                gradient_colors: vec![[0xaa, 0xbb, 0xcc, 255]],
                bloom_color: Some([0xdd, 0xee, 0xff, 255]),
            },
            false,
            false,
        );
        assert_eq!(render.gradient_colors, vec![[0xaa, 0xbb, 0xcc, 255]]);
        assert_eq!(render.bloom_override, Some([0xdd, 0xee, 0xff, 255]));
    }

    // Each test uses a unique session id so the process-global ENDED_SESSIONS
    // set never collides across tests running in the same process.

    #[test]
    fn ended_session_id_is_not_resurrected_by_mutators() {
        // THE FIX: mark a session ended (as fire_session_end does before the
        // reaper's remove hook runs), then drive every guarded mutator with that
        // session id. None may create an entry — proving the in-flight write
        // that lands after session_end cannot resurrect the cleared cursor.
        let reg = CursorRegistry::new();
        let sid = "wb-cursor-ended-A1B2C3";
        fire_session_end(sid);
        assert!(cu_core::session::is_session_ended(sid));

        reg.set_enabled(sid, true);
        reg.update_position(sid, 10.0, 20.0);
        reg.update_config(CursorConfig {
            cursor_id: sid.to_owned(),
            ..Default::default()
        });
        let got = reg.get_or_create(sid);

        // get_or_create returns a transient default value, but must NOT persist.
        assert_eq!(got.config.cursor_id, sid);
        assert!(
            reg.get(sid).is_none(),
            "no entry may be created for an ended session"
        );
        assert!(
            !reg.all_states().iter().any(|s| s.config.cursor_id == sid),
            "ended session cursor must never appear in the registry"
        );
    }

    #[test]
    fn default_cursor_unaffected_by_guard() {
        // "default" is seeded and is never tombstoned — its mutators still work.
        let reg = CursorRegistry::new();
        assert!(!cu_core::session::is_session_ended("default"));
        reg.set_enabled("default", false);
        reg.update_position("default", 1.0, 2.0);
        let s = reg.get("default").expect("default cursor always present");
        assert!(!s.config.enabled);
        assert_eq!(s.position.as_ref().map(|p| (p.x, p.y)), Some((1.0, 2.0)));
    }

    #[test]
    fn explicit_non_session_cursor_id_unaffected_by_guard() {
        // A user-supplied cursor_id that is not a session id is never in
        // ENDED_SESSIONS, so the plain is_session_ended check leaves it working.
        let reg = CursorRegistry::new();
        let cid = "wb-explicit-user-cursor-XYZ";
        assert!(!cu_core::session::is_session_ended(cid));
        reg.set_enabled(cid, true);
        reg.update_position(cid, 5.0, 6.0);
        let s = reg.get(cid).expect("explicit cursor_id must materialise");
        assert!(s.config.enabled);
        assert_eq!(s.position.as_ref().map(|p| (p.x, p.y)), Some((5.0, 6.0)));
    }

    #[test]
    fn live_session_cursor_writes_still_take_effect() {
        // A live (not-ended) session id writes normally.
        let reg = CursorRegistry::new();
        let sid = "wb-cursor-live-LMNOP";
        assert!(!cu_core::session::is_session_ended(sid));
        reg.set_enabled(sid, true);
        reg.update_position(sid, 7.0, 8.0);
        let s = reg.get(sid).expect("live session cursor must exist");
        assert!(s.config.enabled);
        assert_eq!(s.position.as_ref().map(|p| (p.x, p.y)), Some((7.0, 8.0)));
    }
}
