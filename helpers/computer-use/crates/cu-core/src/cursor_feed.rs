//! Best-effort embedded-driver agent-cursor position feed.
//!
//! The embedded driver's own in-process cursor overlay renders nothing: a
//! driver spawned as a CLI grandchild of a terminal creates zero on-screen
//! `NSWindow`s (empirically confirmed via `CGWindowList`), so the branded
//! cursor never appears in real use. Instead of rendering in-process, the
//! embedded driver EMITS its cursor position/state to a companion file that the
//! Brigadier host app (a real GUI app) tails and renders.
//!
//! The file lives next to the round-3/4 process-state file at
//! `$BRIGADIER_CU_STATE_DIR/<driver_pid>.cursor.json` and is refreshed on every
//! cursor move (`move_cursor`, and the glide/target of
//! `click`/`double_click`/`right_click`/`drag`/`scroll`/`type_text`/`set_value`).
//! `x`/`y` are GLOBAL screen coordinates with a top-left origin — the same
//! `NSScreen` space AppKit uses — so the host can place the cursor directly.
//!
//! This mirrors [`crate::session_state::StateFile`]: same-directory temp file +
//! `rename` for atomicity, best-effort writes that never fail a tool call
//! (stderr warning only), and file removal on clean shutdown.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

/// Environment variables that carry the host's cursor branding. Already parsed
/// by `cursor_overlay::AgentCursorDefaults` for the (dead) in-process overlay;
/// re-parsed here so the emitted feed carries the same label/gradient/bloom the
/// host should paint.
const GRADIENT_ENV: &str = "BRIGADIER_CU_CURSOR_GRADIENT";
const BLOOM_ENV: &str = "BRIGADIER_CU_CURSOR_BLOOM";
const LABEL_ENV: &str = "BRIGADIER_CU_CURSOR_LABEL";

const SCHEMA_VERSION: u8 = 1;

/// Host cursor branding read once from `BRIGADIER_CU_CURSOR_*`. Colours are
/// normalised to `#RRGGBB` (uppercase); invalid values are dropped so a bad
/// env var never poisons the feed.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CursorBranding {
    pub label: Option<String>,
    pub gradient: Vec<String>,
    pub bloom: Option<String>,
}

impl CursorBranding {
    /// Parse env-shaped values without touching process-global env (testable).
    /// Uses the same `#RGB`/`#RRGGBB` vocabulary as the overlay's
    /// `AgentCursorDefaults`; unparseable entries are skipped, not errors.
    pub fn parse_values(gradient: Option<&str>, bloom: Option<&str>, label: Option<&str>) -> Self {
        let gradient = gradient
            .map(|value| {
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|entry| !entry.is_empty())
                    .filter_map(normalize_hex_color)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let bloom = bloom
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .and_then(normalize_hex_color);
        let label = label
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned);
        Self {
            label,
            gradient,
            bloom,
        }
    }

    pub fn from_env() -> Self {
        Self::parse_values(
            std::env::var(GRADIENT_ENV).ok().as_deref(),
            std::env::var(BLOOM_ENV).ok().as_deref(),
            std::env::var(LABEL_ENV).ok().as_deref(),
        )
    }
}

/// Normalise `#RGB` / `#RRGGBB` (or the same without the leading `#`) to
/// `#RRGGBB` uppercase. Returns `None` for anything that isn't a valid hex
/// colour. Matches `cursor_overlay::parse_hex_color`'s accepted set.
fn normalize_hex_color(hex: &str) -> Option<String> {
    let value = hex.strip_prefix('#').unwrap_or(hex);
    let expanded = match value.len() {
        6 => value.to_owned(),
        3 => value.chars().flat_map(|c| [c, c]).collect(),
        _ => return None,
    };
    if expanded.chars().all(|c| c.is_ascii_hexdigit()) {
        Some(format!("#{}", expanded.to_ascii_uppercase()))
    } else {
        None
    }
}

/// The exact JSON shape written to `<driver_pid>.cursor.json`. Field order is
/// the serialised key order the Brigadier host reads.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CursorFeedState {
    pub driver_pid: u32,
    pub session: Option<String>,
    pub visible: bool,
    pub x: f64,
    pub y: f64,
    pub label: Option<String>,
    pub gradient: Vec<String>,
    pub bloom: Option<String>,
    pub updated_at: String,
    pub schema: u8,
}

/// One cursor feed file for the current driver process. Writes go through a
/// same-directory temp file followed by `rename`, exactly like
/// [`crate::session_state::StateFile`].
pub struct CursorFeed {
    dir: PathBuf,
    driver_pid: u32,
    branding: CursorBranding,
    temp_counter: AtomicU64,
    /// Last emitted `(session, x, y)`, so visibility changes can preserve the
    /// cursor's last on-screen position without waiting for another move.
    last: Mutex<Option<(Option<String>, f64, f64)>>,
}

impl CursorFeed {
    /// Build the feed whenever `BRIGADIER_CU_STATE_DIR` is set, regardless of
    /// embedded mode. Brigadier renders the agent cursor from this feed for both the
    /// embedded driver (which has no in-process overlay) and the standalone helper
    /// (non-embedded, so it can carry its own permission identity).
    pub fn from_env() -> Option<Self> {
        Self::from_parts(
            crate::embedded_mode(),
            std::env::var_os(crate::session_state::STATE_DIR_ENV).map(PathBuf::from),
            CursorBranding::from_env(),
        )
    }

    /// Env-free constructor used by `from_env` and by unit tests. Returns `None`
    /// unless a state dir is present; the feed is emitted in every driver mode so
    /// the Brigadier host can render the cursor from it.
    pub fn from_parts(
        _embedded: bool,
        dir: Option<PathBuf>,
        branding: CursorBranding,
    ) -> Option<Self> {
        let dir = dir?;
        Some(Self::new(dir, std::process::id(), branding))
    }

    pub fn new(dir: PathBuf, driver_pid: u32, branding: CursorBranding) -> Self {
        Self {
            dir,
            driver_pid,
            branding,
            temp_counter: AtomicU64::new(0),
            last: Mutex::new(None),
        }
    }

    pub fn path(&self) -> PathBuf {
        self.dir.join(format!("{}.cursor.json", self.driver_pid))
    }

    /// Emit a cursor move at GLOBAL screen coordinates (top-left origin). The
    /// visible flag is `true` — the cursor is on-screen and active.
    pub fn update(&self, session: Option<&str>, x: f64, y: f64) -> std::io::Result<()> {
        // Hold the ownership lock through the atomic file replacement. This
        // serializes a move with `hide_if_owned`, so a session cannot pass an
        // ownership check and then hide a newer session's update.
        let mut last = self.last.lock().unwrap();
        *last = Some((session.map(str::to_owned), x, y));
        self.write_state(session, true, x, y)
    }

    /// Unconditionally mark the cursor hidden at its last known position for a
    /// process-global shutdown/reset. Per-session teardown must use
    /// [`CursorFeed::hide_if_owned`]. If no position was ever emitted, writes
    /// `visible=false` at the origin with a null session.
    pub fn hide(&self) -> std::io::Result<()> {
        let last = self.last.lock().unwrap();
        let (session, x, y) = last.clone().unwrap_or((None, 0.0, 0.0));
        self.write_state(session.as_deref(), false, x, y)
    }

    /// Hide only when `session` still owns the process-global feed. Returns
    /// `true` when a hidden state was written and `false` when a newer/different
    /// session owns the feed. The ownership check and file replacement share
    /// the same lock as [`CursorFeed::update`], making the decision atomic with
    /// respect to concurrent cursor moves.
    pub fn hide_if_owned(&self, session: &str) -> std::io::Result<bool> {
        let last = self.last.lock().unwrap();
        let Some((owner, x, y)) = last.as_ref() else {
            return Ok(false);
        };
        if owner.as_deref() != Some(session) {
            return Ok(false);
        }
        self.write_state(owner.as_deref(), false, *x, *y)?;
        Ok(true)
    }

    /// Show only when `session` still owns the process-global feed. Returns
    /// `true` when a visible state was written and `false` when no cursor has
    /// moved yet or a newer/different session owns the feed. The prior position
    /// is reused so re-enabling does not require another cursor movement.
    pub fn show_if_owned(&self, session: &str) -> std::io::Result<bool> {
        let last = self.last.lock().unwrap();
        let Some((owner, x, y)) = last.as_ref() else {
            return Ok(false);
        };
        if owner.as_deref() != Some(session) {
            return Ok(false);
        }
        self.write_state(owner.as_deref(), true, *x, *y)?;
        Ok(true)
    }

    fn write_state(
        &self,
        session: Option<&str>,
        visible: bool,
        x: f64,
        y: f64,
    ) -> std::io::Result<()> {
        crate::session_state::ensure_private_dir(&self.dir)?;
        let state = CursorFeedState {
            driver_pid: self.driver_pid,
            session: session.map(str::to_owned),
            visible,
            x,
            y,
            label: self.branding.label.clone(),
            gradient: self.branding.gradient.clone(),
            bloom: self.branding.bloom.clone(),
            updated_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_owned()),
            schema: SCHEMA_VERSION,
        };
        let body = serde_json::to_vec(&state)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        let sequence = self.temp_counter.fetch_add(1, Ordering::Relaxed);
        let temp_path = self
            .dir
            .join(format!(".{}.cursor.json.tmp-{}", self.driver_pid, sequence));

        let result = (|| {
            use std::io::Write;
            let mut file = crate::session_state::create_private_temp_file(&temp_path)?;
            file.write_all(&body)?;
            file.sync_all()?;
            std::fs::rename(&temp_path, self.path())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temp_path);
        }
        result
    }

    pub fn remove(&self) -> std::io::Result<()> {
        match std::fs::remove_file(self.path()) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }
}

impl Drop for CursorFeed {
    fn drop(&mut self) {
        if let Err(error) = self.remove() {
            eprintln!("[brigadier-cu] warning: failed to remove cursor feed file: {error}");
        }
    }
}

// ── Process-global feed handle ───────────────────────────────────────────────
//
// One feed per driver process, initialised lazily from the environment. The
// platform cursor path emits through the free functions below so it never has
// to thread a handle through every tool.

fn global() -> Option<&'static crate::cursor_feed_writer::CursorFeedWriter> {
    use crate::cursor_feed_writer::CursorFeedWriter;
    static FEED: OnceLock<Option<CursorFeedWriter>> = OnceLock::new();
    FEED.get_or_init(|| {
        let feed = CursorFeed::from_env()?;
        Some(CursorFeedWriter::new(move |snapshot| match snapshot {
            Some(state) => {
                feed.write_state(state.session.as_deref(), state.visible, state.x, state.y)
            }
            None => feed.remove(),
        }))
    })
    .as_ref()
}

/// Best-effort emit of a cursor move at GLOBAL screen coordinates. No-op when
/// the feed is disabled (STATE_DIR unset or not embedded). Never fails a tool
/// call — a write error is logged to stderr only.
pub fn emit_move(session: Option<&str>, x: f64, y: f64) {
    if let Some(feed) = global() {
        feed.update(session, x, y, true);
    }
}

/// Queue the latest input sample without waiting for filesystem persistence.
/// The single writer coalesces superseded samples and retains the final one.
pub fn emit_move_coalesced(session: Option<&str>, x: f64, y: f64) {
    if let Some(feed) = global() {
        feed.update(session, x, y, false);
    }
}

/// Wait for queued feed changes only after native input has released capture.
pub fn flush() {
    if let Some(feed) = global() {
        feed.flush();
    }
}

/// Best-effort unconditional `visible=false` for process-global shutdown/reset.
/// Per-session teardown must use [`emit_hidden_if_owned`]. No-op when the feed
/// is disabled.
pub fn emit_hidden() {
    if let Some(feed) = global() {
        feed.visibility(None, false, true);
    }
}

/// Best-effort `visible=false` only when `session` still owns the last emitted
/// cursor. Used by per-session end/disable paths so one session cannot hide a
/// different session's active feed.
pub fn emit_hidden_if_owned(session: &str) {
    if let Some(feed) = global() {
        feed.visibility(Some(session), false, true);
    }
}

/// Hide a disabled/ended input cursor without stalling the native event stream.
pub fn emit_hidden_if_owned_coalesced(session: &str) {
    if let Some(feed) = global() {
        feed.visibility(Some(session), false, false);
    }
}

/// Best-effort `visible=true` only when `session` still owns the last emitted
/// cursor. Used when re-enabling a session so the host-rendered cursor returns
/// at its prior position without allowing one session to reveal another's feed.
pub fn emit_visible_if_owned(session: &str) {
    if let Some(feed) = global() {
        feed.visibility(Some(session), true, true);
    }
}

/// Best-effort removal of the feed file (clean process shutdown). No-op when
/// the feed is disabled or the file is already gone.
pub fn remove() {
    if let Some(feed) = global() {
        feed.remove();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branding() -> CursorBranding {
        CursorBranding::parse_values(
            Some("#ff0000, #0f0"),
            Some("00f"),
            Some("  brigadier-codex  "),
        )
    }

    #[test]
    fn branding_parses_and_normalises_colors() {
        let b = branding();
        assert_eq!(b.gradient, vec!["#FF0000".to_owned(), "#00FF00".to_owned()]);
        assert_eq!(b.bloom.as_deref(), Some("#0000FF"));
        assert_eq!(b.label.as_deref(), Some("brigadier-codex"));

        // Invalid entries are dropped, not errors. ("nope"/"#gg"/"" are not
        // valid hex; "#123456" survives.)
        let messy =
            CursorBranding::parse_values(Some("#zzz, #123456, nope"), Some("#gg"), Some("   "));
        assert_eq!(messy.gradient, vec!["#123456".to_owned()]);
        assert_eq!(messy.bloom, None);
        assert_eq!(messy.label, None);
    }

    #[test]
    fn update_writes_the_exact_feed_shape() {
        let dir = tempfile::tempdir().unwrap();
        let feed = CursorFeed::new(dir.path().to_owned(), 4242, branding());
        feed.update(Some("embedded-42"), 1280.5, 96.25).unwrap();

        // Field-typed round-trip.
        let state: CursorFeedState =
            serde_json::from_slice(&std::fs::read(feed.path()).unwrap()).unwrap();
        assert_eq!(state.driver_pid, 4242);
        assert_eq!(state.session.as_deref(), Some("embedded-42"));
        assert!(state.visible);
        assert_eq!(state.x, 1280.5);
        assert_eq!(state.y, 96.25);
        assert_eq!(state.label.as_deref(), Some("brigadier-codex"));
        assert_eq!(
            state.gradient,
            vec!["#FF0000".to_owned(), "#00FF00".to_owned()]
        );
        assert_eq!(state.bloom.as_deref(), Some("#0000FF"));
        assert_eq!(state.schema, 1);
        assert!(time::OffsetDateTime::parse(
            &state.updated_at,
            &time::format_description::well_known::Rfc3339,
        )
        .is_ok());

        // Serialised key order is the host contract. `serde_json::Value` sorts
        // keys, so assert against the raw serialised bytes: each key must appear
        // in declaration order.
        let raw = std::fs::read_to_string(feed.path()).unwrap();
        let order = [
            "\"driver_pid\"",
            "\"session\"",
            "\"visible\"",
            "\"x\"",
            "\"y\"",
            "\"label\"",
            "\"gradient\"",
            "\"bloom\"",
            "\"updated_at\"",
            "\"schema\"",
        ];
        let positions: Vec<usize> = order
            .iter()
            .map(|key| {
                raw.find(key)
                    .unwrap_or_else(|| panic!("missing key {key} in {raw}"))
            })
            .collect();
        assert!(
            positions.windows(2).all(|w| w[0] < w[1]),
            "keys must serialise in declaration order: {raw}"
        );
    }

    #[test]
    fn path_uses_the_cursor_json_suffix_next_to_the_state_file() {
        let dir = tempfile::tempdir().unwrap();
        let feed = CursorFeed::new(dir.path().to_owned(), 7, CursorBranding::default());
        assert_eq!(feed.path(), dir.path().join("7.cursor.json"));
    }

    #[test]
    fn update_atomically_replaces_via_rename_with_no_tmp_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let feed = CursorFeed::new(dir.path().to_owned(), 4343, CursorBranding::default());
        feed.update(Some("s"), 1.0, 2.0).unwrap();
        feed.update(Some("s"), 3.0, 4.0).unwrap();

        let state: CursorFeedState =
            serde_json::from_slice(&std::fs::read(feed.path()).unwrap()).unwrap();
        assert_eq!((state.x, state.y), (3.0, 4.0));

        let tmp_count = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
            .count();
        assert_eq!(tmp_count, 0, "rename must leave no temporary file behind");
    }

    #[test]
    fn global_coord_conversion_from_window_local_point() {
        // A window-local screenshot pixel is converted to a GLOBAL screen point
        // by the platform tools as `window_origin + local_px / backing_scale`.
        // The feed emits that global point verbatim. Fixture: window at global
        // origin (300, 120), Retina 2×, click read at window-local pixel
        // (200, 80) → global (300 + 100, 120 + 40) = (400, 160).
        let (gx, gy) = window_local_to_global((300.0, 120.0), 2.0, (200.0, 80.0));
        assert_eq!((gx, gy), (400.0, 160.0));

        let dir = tempfile::tempdir().unwrap();
        let feed = CursorFeed::new(dir.path().to_owned(), 909, CursorBranding::default());
        feed.update(Some("embedded-909"), gx, gy).unwrap();
        let state: CursorFeedState =
            serde_json::from_slice(&std::fs::read(feed.path()).unwrap()).unwrap();
        assert_eq!((state.x, state.y), (400.0, 160.0));
    }

    #[test]
    fn unconditional_hide_marks_invisible_at_last_position_for_global_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let feed = CursorFeed::new(dir.path().to_owned(), 5151, branding());
        feed.update(Some("embedded-1"), 640.0, 400.0).unwrap();
        feed.hide().unwrap();

        let state: CursorFeedState =
            serde_json::from_slice(&std::fs::read(feed.path()).unwrap()).unwrap();
        assert!(!state.visible, "session end must clear visibility");
        assert_eq!(
            (state.x, state.y),
            (640.0, 400.0),
            "hide keeps last position"
        );
        assert_eq!(state.session.as_deref(), Some("embedded-1"));
    }

    #[test]
    fn non_owner_session_end_or_disable_does_not_hide_active_feed() {
        let dir = tempfile::tempdir().unwrap();
        let feed = CursorFeed::new(dir.path().to_owned(), 5152, branding());
        feed.update(Some("active-session"), 320.0, 240.0).unwrap();

        assert!(!feed.hide_if_owned("other-session").unwrap());
        let state: CursorFeedState =
            serde_json::from_slice(&std::fs::read(feed.path()).unwrap()).unwrap();
        assert!(state.visible, "a non-owner must not hide the active cursor");
        assert_eq!(state.session.as_deref(), Some("active-session"));
        assert_eq!((state.x, state.y), (320.0, 240.0));
    }

    #[test]
    fn owner_session_end_or_disable_hides_active_feed() {
        let dir = tempfile::tempdir().unwrap();
        let feed = CursorFeed::new(dir.path().to_owned(), 5153, branding());
        feed.update(Some("active-session"), 320.0, 240.0).unwrap();

        assert!(feed.hide_if_owned("active-session").unwrap());
        let state: CursorFeedState =
            serde_json::from_slice(&std::fs::read(feed.path()).unwrap()).unwrap();
        assert!(!state.visible, "the owner must hide its active cursor");
        assert_eq!(state.session.as_deref(), Some("active-session"));
        assert_eq!((state.x, state.y), (320.0, 240.0));
    }

    #[test]
    fn owner_reenable_restores_hidden_feed_without_another_move() {
        let dir = tempfile::tempdir().unwrap();
        let feed = CursorFeed::new(dir.path().to_owned(), 5154, branding());
        feed.update(Some("active-session"), 812.5, 417.25).unwrap();
        assert!(feed.hide_if_owned("active-session").unwrap());

        assert!(feed.show_if_owned("active-session").unwrap());
        let state: CursorFeedState =
            serde_json::from_slice(&std::fs::read(feed.path()).unwrap()).unwrap();
        assert!(
            state.visible,
            "re-enabling must restore the host-rendered cursor"
        );
        assert_eq!(state.session.as_deref(), Some("active-session"));
        assert_eq!(
            (state.x, state.y),
            (812.5, 417.25),
            "re-enabling must reuse the last position without requiring movement"
        );
    }

    #[test]
    fn non_owner_reenable_does_not_restore_another_sessions_feed() {
        let dir = tempfile::tempdir().unwrap();
        let feed = CursorFeed::new(dir.path().to_owned(), 5155, branding());
        feed.update(Some("active-session"), 75.0, 125.0).unwrap();
        assert!(feed.hide_if_owned("active-session").unwrap());

        assert!(!feed.show_if_owned("other-session").unwrap());
        let state: CursorFeedState =
            serde_json::from_slice(&std::fs::read(feed.path()).unwrap()).unwrap();
        assert!(
            !state.visible,
            "a non-owner must not restore the cursor feed"
        );
        assert_eq!(state.session.as_deref(), Some("active-session"));
        assert_eq!((state.x, state.y), (75.0, 125.0));
    }

    #[test]
    fn from_parts_gates_on_state_dir_only() {
        let dir = tempfile::tempdir().unwrap();
        // No state dir → no feed, in either mode.
        assert!(CursorFeed::from_parts(true, None, CursorBranding::default()).is_none());
        assert!(CursorFeed::from_parts(false, None, CursorBranding::default()).is_none());
        // State dir present → feed enabled, regardless of embedded mode.
        assert!(CursorFeed::from_parts(
            true,
            Some(dir.path().to_owned()),
            CursorBranding::default()
        )
        .is_some());
        assert!(CursorFeed::from_parts(
            false,
            Some(dir.path().to_owned()),
            CursorBranding::default()
        )
        .is_some());
    }

    #[test]
    fn drop_removes_the_feed_file_on_clean_shutdown() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("6161.cursor.json");
        {
            let feed = CursorFeed::new(dir.path().to_owned(), 6161, CursorBranding::default());
            feed.update(Some("s"), 5.0, 6.0).unwrap();
            assert!(path.exists());
        }
        assert!(!path.exists(), "Drop must remove the cursor feed file");
    }

    #[test]
    fn malformed_state_dir_is_an_error_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let not_a_dir = dir.path().join("plain-file");
        std::fs::write(&not_a_dir, b"occupied").unwrap();
        let feed = CursorFeed::new(not_a_dir, 5252, CursorBranding::default());
        assert!(feed.update(Some("s"), 1.0, 2.0).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn update_repairs_existing_directory_and_writes_private_cursor_file() {
        use std::os::unix::fs::PermissionsExt;

        let parent = tempfile::tempdir().unwrap();
        let state_dir = parent.path().join("state");
        std::fs::create_dir(&state_dir).unwrap();
        std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o755)).unwrap();

        let feed = CursorFeed::new(state_dir.clone(), 5353, CursorBranding::default());
        feed.update(Some("private"), 10.0, 20.0).unwrap();

        assert_eq!(
            std::fs::metadata(&state_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(feed.path()).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}

/// Convert a window-local screenshot-pixel point to a GLOBAL top-left-origin
/// screen point (AppKit `NSScreen` space). `window_origin` is the target
/// window's top-left in global screen points; `backing_scale` maps physical
/// screenshot pixels to logical points; `local_px` is the point read off the
/// window screenshot. This is the SAME conversion every pixel action applies
/// before driving the cursor, and therefore the coordinate space the cursor
/// feed emits in. Exposed here so the conversion is unit-testable without a
/// live macOS `WindowServer`.
pub fn window_local_to_global(
    window_origin: (f64, f64),
    backing_scale: f64,
    local_px: (f64, f64),
) -> (f64, f64) {
    let scale = if backing_scale > 0.0 {
        backing_scale
    } else {
        1.0
    };
    (
        window_origin.0 + local_px.0 / scale,
        window_origin.1 + local_px.1 / scale,
    )
}
