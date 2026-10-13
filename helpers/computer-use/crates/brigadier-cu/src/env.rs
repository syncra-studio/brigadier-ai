//! Environment names and reserved arguments the helper shares with the host
//! app and with its own child processes.

pub const RESPONSIBILITY_DISCLAIMED_ENV: &str = "BRIGADIER_CU_RESPONSIBILITY_DISCLAIMED";

/// Embedded mode (`BRIGADIER_CU_EMBEDDED=1` / `--embedded`): the driver runs
/// as a direct child of a host app and stays in its TCC responsibility
/// chain — no disclaim re-exec, no daemon relaunch, no permission prompts.
///
/// Caller-controlled, which is safe only because embedded mode strictly
/// REMOVES capability claims; it must never feed into the `driver-daemon`
/// attribution decision.
pub const EMBEDDED_ENV: &str = "BRIGADIER_CU_EMBEDDED";

/// Stable cursor/session identity for an embedded stdio driver process.
/// Embedding hosts may set this before launching the driver so their own run
/// identity appears in cursor ownership and diagnostics.
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

/// Only the exact value `1` counts — fail-safe for anything else.
pub fn embedded_mode() -> bool {
    std::env::var_os(EMBEDDED_ENV).is_some_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::host_session_id_from_proxy;

    #[test]
    fn hosted_proxy_generation_resolves_to_stable_host_identity() {
        assert_eq!(
            host_session_id_from_proxy("brigadier-surface-a-mcp-4242-99"),
            Some("brigadier-surface-a")
        );
        assert_eq!(host_session_id_from_proxy("standalone-session"), None);
        assert_eq!(host_session_id_from_proxy("-mcp-4242-99"), None);
    }
}
