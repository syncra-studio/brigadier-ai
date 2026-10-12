//! MCP JSON-RPC 2.0 server over stdio — platform-independent core.
//!
//! Implements the Model Context Protocol (MCP) 2024-11-05 over stdio,
//! matching the interface of `libs/brigadier-cu` (Swift/macOS) and
//! `BrigadierCu.Win` (.NET/Windows).
//!
//! # Protocol
//! - Line-delimited JSON-RPC 2.0 on stdin/stdout
//! - Methods: `initialize`, `notifications/initialized`, `tools/list`, `tools/call`
//! - Each request has `jsonrpc: "2.0"`, `id` (any), `method`, optional `params`
//! - Notifications (no `id`) are silently ignored

pub const RESPONSIBILITY_DISCLAIMED_ENV: &str = "BRIGADIER_CU_RESPONSIBILITY_DISCLAIMED";

/// Embedded mode (`BRIGADIER_CU_EMBEDDED=1` / `--embedded`): the driver runs
/// as a direct child of a host app and stays in its TCC responsibility
/// chain — no disclaim re-exec, no daemon relaunch, no permission prompts.
/// See `Skills/brigadier-cu/EMBEDDING.md`.
///
/// Caller-controlled, which is safe only because embedded mode strictly
/// REMOVES capability claims; it must never feed into the `driver-daemon`
/// attribution decision (`permission_source` in platform-macos).
pub const EMBEDDED_ENV: &str = "BRIGADIER_CU_EMBEDDED";

/// Stable cursor/session identity for an embedded stdio driver process.
/// Embedding hosts may set this before launching the driver so their own run
/// identity appears in cursor ownership and diagnostics. When it is absent (or
/// empty), the driver mints a process-local `embedded-<pid>` id.
pub const DEFAULT_SESSION_ENV: &str = "BRIGADIER_CU_DEFAULT_SESSION";

/// Reserved per-call argument carrying a host-managed session identity that is
/// stable across short-lived MCP proxy generations. The daemon's lifecycle
/// session remains generation-scoped for recording/config cleanup, while the
/// cursor and activity state use this durable host scope.
pub const HOST_SESSION_ARG: &str = "_host_session";

/// Return the stable embedding-host session carried by a short-lived MCP
/// proxy generation. Hosted proxy ids are formatted as
/// `<host-session>-mcp-<pid>-<start-token>`; standalone sessions have no
/// stable host prefix and therefore return `None`.
pub fn host_session_id_from_proxy(session_id: &str) -> Option<&str> {
    session_id
        .rfind("-mcp-")
        .map(|marker| &session_id[..marker])
        .filter(|value| !value.is_empty())
}

/// Advisory label for the embedding host's bundle id, echoed in
/// `check_permissions` output. NOT a trust signal — trust comes from the
/// OS responsibility chain.
pub const HOST_BUNDLE_ID_ENV: &str = "BRIGADIER_CU_HOST_BUNDLE_ID";

/// Explicit host opt-in for watchable automation that leaves driven apps in
/// the foreground. Embedding alone never changes background delivery or
/// launch placement; hosts that want this visible-demo behavior must request
/// it separately.
pub const WATCHABLE_FRONT_ENV: &str = "BRIGADIER_CU_WATCHABLE_FRONT";

/// Only the exact value `1` counts — fail-safe for anything else.
pub fn embedded_mode() -> bool {
    std::env::var_os(EMBEDDED_ENV).is_some_and(|v| v == "1")
}

/// Whether the host explicitly opted into foregrounding driven applications.
/// Only the exact value `1` enables this delivery-contract override.
pub fn watchable_front_mode() -> bool {
    std::env::var_os(WATCHABLE_FRONT_ENV).is_some_and(|v| v == "1")
}

/// Return the default session owned by this embedded driver process.
///
/// The id is resolved once so every argument-less tool call in the stdio MCP
/// session shares one cursor. Non-embedded processes deliberately return
/// `None`: daemon/serve and one-shot CLI calls retain their explicit-session
/// cursor semantics.
pub fn embedded_default_session_id() -> Option<&'static str> {
    if !embedded_mode() {
        return None;
    }

    static ID: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    Some(
        ID.get_or_init(|| {
            default_session_id_from_env(
                std::env::var(DEFAULT_SESSION_ENV).ok().as_deref(),
                std::process::id(),
            )
        })
        .as_str(),
    )
}

fn default_session_id_from_env(value: Option<&str>, pid: u32) -> String {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("embedded-{pid}"))
}

pub mod activity;
pub mod capture_mode;
pub mod cdp;
pub mod cursor_feed;
mod cursor_feed_writer;
pub mod cursor_sampler;
pub mod display_placement;
pub mod element_cache;
pub mod element_token;
pub mod health_report;
pub mod image_utils;
pub mod page;
pub mod pip_hook;
pub mod protocol;
pub mod recording;
pub mod recording_loader;
pub mod recording_render;
pub mod recording_tools;
pub mod recording_zoom;
pub mod server;
pub mod session;
pub mod session_state;
pub mod session_tools;
pub mod socket_io;
pub mod target_policy;
pub mod text_sanitize;
pub mod tool;
pub mod tool_args;
pub mod tool_schema;
pub mod video;
pub mod video_ffmpeg;

pub use recording::RecordingSession;

#[cfg(test)]
mod embedded_session_tests {
    use super::{default_session_id_from_env, host_session_id_from_proxy};

    #[test]
    fn hosted_proxy_generation_resolves_to_stable_host_identity() {
        assert_eq!(
            host_session_id_from_proxy("brigadier-surface-a-mcp-4242-99"),
            Some("brigadier-surface-a")
        );
        assert_eq!(host_session_id_from_proxy("standalone-session"), None);
        assert_eq!(host_session_id_from_proxy("-mcp-4242-99"), None);
    }

    #[test]
    fn default_session_uses_host_value_when_provided() {
        assert_eq!(
            default_session_id_from_env(Some("brigadier-codex-42"), 123),
            "brigadier-codex-42"
        );
        assert_eq!(
            default_session_id_from_env(Some("  brigadier-codex-42  "), 123),
            "brigadier-codex-42"
        );
    }

    #[test]
    fn default_session_falls_back_to_stable_process_id() {
        assert_eq!(default_session_id_from_env(None, 123), "embedded-123");
        assert_eq!(default_session_id_from_env(Some(""), 123), "embedded-123");
        assert_eq!(
            default_session_id_from_env(Some("   "), 123),
            "embedded-123"
        );
    }
}
