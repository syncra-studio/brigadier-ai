//! Environment names and reserved arguments the helper shares with the host
//! app and with its own child processes.

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const RESPONSIBILITY_DISCLAIMED_ENV: &str = "BRIGADIER_CU_RESPONSIBILITY_DISCLAIMED";

/// Embedded mode (`BRIGADIER_CU_EMBEDDED=1` / `--embedded`): the driver runs
/// as a direct child of a host app and stays in its TCC responsibility
/// chain — no disclaim re-exec, no daemon relaunch, no permission prompts.
///
/// Caller-controlled, which is safe only because embedded mode strictly
/// REMOVES capability claims; it must never feed into the `driver-daemon`
/// attribution decision.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const EMBEDDED_ENV: &str = "BRIGADIER_CU_EMBEDDED";

/// Stable cursor/session identity for an embedded stdio driver process.
/// Embedding hosts may set this before launching the driver so their own run
/// identity appears in cursor ownership and diagnostics.
pub const DEFAULT_SESSION_ENV: &str = "BRIGADIER_CU_DEFAULT_SESSION";

/// Who the MCP `instructions` are for. `brigadier` (set by Brigadier for its agents): they get
/// the `computer-use` skill and their own brief, so the server says only where to look. Anything
/// else: the full standalone workflow.
pub const AUDIENCE_ENV: &str = "BRIGADIER_CU_AUDIENCE";

/// Reserved per-call argument carrying a host-managed session identity that is
/// stable across short-lived MCP proxy generations. The daemon's lifecycle
/// session remains generation-scoped for recording/config cleanup, while the
/// cursor and activity state use this durable host scope.
pub const HOST_SESSION_ARG: &str = "_host_session";

/// Bundle id of the app hosting the helper (`--host-bundle-id`): the
/// registry builder passes it to the driver and agents may not target it.
pub const HOST_BUNDLE_ID_ENV: &str = cu_layer::HOST_BUNDLE_ID_ENV;

/// Only the exact value `1` counts — fail-safe for anything else.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn embedded_mode() -> bool {
    std::env::var_os(EMBEDDED_ENV).is_some_and(|v| v == "1")
}
