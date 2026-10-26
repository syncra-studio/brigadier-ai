//! macOS bundle-context detection for the TCC auto-relaunch path.
//!
//! Mirrors `libs/brigadier-cu/Sources/BrigadierCuCLI/BundleHelpers.swift`'s
//! `isExecutableInsideBrigadierCuApp()` — the heuristic that decides
//! whether `brigadier-cu mcp` was spawned from an IDE terminal as a
//! bare CLI symlinked into our .app bundle. When true and the parent
//! isn't launchd, we re-launch the daemon via `open -n -g -a
//! BrigadierCu --args serve` so it picks up the bundle's TCC grants,
//! then proxy stdio MCP traffic through the daemon's Unix socket.
//!
//! Non-macOS targets compile to no-ops so the cross-platform call
//! sites stay tidy.

/// Returns `true` when the currently-running binary resolves into an
/// installed `Brigadier Computer Use.app` bundle (Rust port). The check is the
/// same shape as the Swift driver's `isExecutableInsideBrigadierCuApp`
/// (`/Brigadier Computer Use.app/Contents/MacOS/`) but keyed on the Rust port's
/// distinct bundle name so the two installs don't collide.
///
/// `false` for raw `cargo run` / `target/release/brigadier-cu` dev
/// invocations — there's no installed bundle to relaunch into, so the
/// caller should stay in-process.
///
/// Implementation:
///   1. Resolve `std::env::current_exe()` (preferred; absolute path
///      to the running image).
///   2. Walk symlinks via `std::fs::canonicalize` — the install layout
///      is `~/.local/bin/brigadier-cu` → `/Applications/Brigadier Computer Use.app/
///      Contents/MacOS/brigadier-cu`, so without the canonicalize step
///      we'd see the bare symlink path and miss the bundle.
///   3. Substring-match the canonical path for the bundle marker.
#[cfg(target_os = "macos")]
pub fn is_executable_inside_brigadier_cu_app() -> bool {
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return false,
    };
    let canonical = match std::fs::canonicalize(&exe) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let s = match canonical.to_str() {
        Some(s) => s,
        None => return false,
    };
    s.contains("/Brigadier Computer Use.app/Contents/MacOS/")
}

/// Returns `true` when the parent process is *not* `launchd` (pid 1).
/// Combined with [`is_executable_inside_brigadier_cu_app`], a `true`
/// here means the binary was spawned from a shell / IDE terminal that
/// inherits the wrong TCC responsibility — i.e. the case we want to
/// auto-relaunch from.
///
/// `ppid == 1` means launchd reparented us (we're already running as
/// the LaunchServices-spawned daemon). In that case we stay
/// in-process: TCC grants are already correct, and relaunching would
/// fork-bomb the daemon back into existence on every `mcp` startup.
///
/// Mirrors Swift's `if getppid() == 1 { return false }` gate in
/// `MCPCommand.shouldUseDaemonProxy()`.
#[cfg(unix)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn parent_is_not_launchd() -> bool {
    // SAFETY: `libc::getppid` is a thread-safe POSIX getter that
    // takes no args and returns the parent pid. No invariants to
    // uphold, no UB to risk.
    let ppid = unsafe { libc::getppid() };
    ppid != 1
}

/// Returns `true` when the env var is one of `1|true|yes|on`
/// (case-insensitive). Anything else, including unset, is falsy.
///
/// Mirrors Swift's `isEnvTruthy` helper on `MCPCommand`.
pub fn is_env_truthy(name: &str) -> bool {
    match std::env::var(name) {
        Ok(v) => matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        ),
        Err(_) => false,
    }
}

/// Returns whether the current image is the executable of Brigadier's branded
/// helper app. A copy under `Contents/Resources/bin` is only a proxy artifact;
/// allowing it to own TCC-protected work creates a second path-based Privacy &
/// Security identity beside `Brigadier Computer Use`.
#[cfg(target_os = "macos")]
pub fn is_executable_inside_brigadier_helper_app() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .as_deref()
        .is_some_and(is_executable_inside_brigadier_helper_app_path)
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn is_executable_inside_brigadier_helper_app_path(path: &std::path::Path) -> bool {
    path.ends_with(std::path::Path::new(
        "Brigadier Computer Use.app/Contents/MacOS/brigadier-cu",
    ))
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_run_is_not_inside_bundle() {
        // The unit-test runner image lives under `target/<config>/
        // deps/`, never inside a .app bundle. Should always return
        // false in CI / local dev, which is exactly the behavior we
        // want so `cargo run` callers stay in-process.
        assert!(!is_executable_inside_brigadier_cu_app());
    }

    #[test]
    fn unset_env_is_falsy() {
        // Use a deliberately unlikely name so we don't depend on the
        // surrounding shell environment.
        std::env::remove_var("BRIGADIER_CU_TEST_UNSET_NAME");
        assert!(!is_env_truthy("BRIGADIER_CU_TEST_UNSET_NAME"));
    }

    #[test]
    fn truthy_env_values_recognized() {
        let name = "BRIGADIER_CU_TEST_TRUTHY";
        for v in ["1", "true", "TRUE", "Yes", "on", " 1 "] {
            std::env::set_var(name, v);
            assert!(is_env_truthy(name), "expected truthy for {v:?}");
        }
        for v in ["0", "false", "no", "off", ""] {
            std::env::set_var(name, v);
            assert!(!is_env_truthy(name), "expected falsy for {v:?}");
        }
        std::env::remove_var(name);
    }

    #[test]
    #[cfg(unix)]
    fn parent_is_not_launchd_in_tests() {
        // The cargo test harness is reparented under whatever
        // launched it (cargo / IDE / shell), not directly under
        // launchd. The helper should report true.
        assert!(parent_is_not_launchd());
    }

    #[test]
    fn only_the_branded_helper_bundle_is_a_brigadier_permission_owner() {
        assert!(is_executable_inside_brigadier_helper_app_path(std::path::Path::new(
            "/Applications/brigadier.app/Contents/Library/Brigadier Computer Use.app/Contents/MacOS/brigadier-cu"
        )));
        assert!(is_executable_inside_brigadier_helper_app_path(std::path::Path::new(
            "/Users/me/Library/Application Support/ai.brigadier.app/computer/Brigadier Computer Use.app/Contents/MacOS/brigadier-cu"
        )));
        assert!(!is_executable_inside_brigadier_helper_app_path(
            std::path::Path::new("/Applications/brigadier.app/Contents/Resources/bin/brigadier-cu")
        ));
    }
}
