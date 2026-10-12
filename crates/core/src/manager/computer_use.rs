//! Computer use: the bundled helper app, placed in the data folder, and what each agent gets
//! to use it: the `computer` MCP server and the `computer-use` skill.
//!
//! The helper ships as `Brigadier.app/Contents/Helpers/Brigadier Computer Use.app`. It runs
//! from a copy at `<data dir>/computer/Brigadier Computer Use.app`, because macOS answers
//! Screen Recording for the outermost app around a process: started inside Brigadier.app it
//! would be Brigadier there and never get an entry of its own. Its grants follow its bundle id
//! and signature, not its place.
//!
//! The MCP server each agent gets is the helper's own stdio shim (`brigadier-cu mcp`). The shim
//! starts the helper's daemon through LaunchServices on the first tool call and replaces a dead
//! or hung one, under a lock all shims share, so Brigadier itself starts nothing here.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use brigadier_providers::McpServer;

use super::SessionManager;

/// The MCP server's name: tools reach the model as `mcp__computer__<tool>`.
pub(crate) const SERVER_NAME: &str = "computer";
/// The helper app's bundle name, in Brigadier.app and in the data folder.
const HELPER_APP: &str = "Brigadier Computer Use.app";
/// The helper's executable inside its bundle.
const HELPER_BIN: &str = "brigadier-cu";
/// Overrides where the helper app is found (a development checkout's staged bundle).
const HELPER_APP_ENV: &str = "BRIGADIER_CU_APP";
/// The skill every agent gets, Brigadier's own workflow for the tools.
const SKILL: &str = include_str!("computer_use_skill.md");
/// A tool call may wait for the helper to start and for a permission the user is granting.
const TOOL_TIMEOUT_SECS: u64 = 120;

/// The helper's copy in the data folder, once placed.
#[derive(Default)]
pub(crate) struct ComputerUse {
    placed: tokio::sync::Mutex<Option<Placed>>,
}

#[derive(Clone)]
struct Placed {
    /// The bundled helper it was copied from, and that bundle's executable's change time: a
    /// different one (an update) is placed again.
    source: PathBuf,
    stamp: Option<SystemTime>,
    app: PathBuf,
    home: PathBuf,
    plugin: PathBuf,
    token: String,
}

/// What an agent gets for computer use.
pub(crate) struct ComputerUseKit {
    pub server: McpServer,
    /// The Brigadier-owned plugin folder holding the `computer-use` skill.
    pub plugin: PathBuf,
    /// The socket token, for the session's redactor.
    pub token: String,
}

impl SessionManager {
    /// The `computer` MCP server and the skill, while computer use is on (macOS only).
    /// `always_load`: its tools are in the model's list from the start (a GUI job) instead of
    /// behind Claude's tool search. `None` when it is off or the helper can't be placed; the
    /// agent then simply works without it.
    pub(crate) async fn computer_use_kit(&self, always_load: bool) -> Option<ComputerUseKit> {
        if !cfg!(target_os = "macos") || !self.core.settings().computer_use {
            return None;
        }
        let placed = match self
            .computer_use
            .ready(&self.config.daemon_exe, &self.data_dir)
            .await
        {
            Ok(placed) => placed,
            Err(error) => {
                tracing::warn!(%error, "computer use is unavailable");
                return None;
            }
        };
        Some(ComputerUseKit {
            server: McpServer {
                name: SERVER_NAME.into(),
                command: placed.app.join("Contents/MacOS").join(HELPER_BIN),
                args: vec!["mcp".into()],
                env: vec![
                    (
                        "BRIGADIER_CU_HOME".into(),
                        placed.home.to_string_lossy().into_owned(),
                    ),
                    (
                        "BRIGADIER_CU_DAEMON_APP".into(),
                        placed.app.to_string_lossy().into_owned(),
                    ),
                    (
                        "BRIGADIER_CU_SOCKET_AUTH_TOKEN".into(),
                        placed.token.clone(),
                    ),
                ],
                tool_timeout_secs: Some(TOOL_TIMEOUT_SECS),
                trusted: true,
                always_load,
                prompt_tools: Vec::new(),
            },
            plugin: placed.plugin,
            token: placed.token,
        })
    }
}

impl ComputerUse {
    /// The placed helper, placing it first when this is the first use or the bundle changed.
    async fn ready(&self, daemon_exe: &Path, data_dir: &Path) -> Result<Placed, String> {
        let source = bundled_helper(daemon_exe)
            .ok_or_else(|| "the helper app is missing from this install".to_owned())?;
        let stamp = executable_stamp(&source);
        let mut placed = self.placed.lock().await;
        if let Some(current) = placed.as_ref()
            && current.source == source
            && current.stamp == stamp
            && current.app.exists()
        {
            return Ok(current.clone());
        }
        let home = data_dir.join("computer");
        let fresh = tokio::task::spawn_blocking(move || place(&source, stamp, &home))
            .await
            .map_err(|e| e.to_string())??;
        *placed = Some(fresh.clone());
        Ok(fresh)
    }
}

/// The helper bundled with this install: beside the daemon in Brigadier.app
/// (`Contents/MacOS/brigadierd` → `Contents/Helpers/…`), or where [`HELPER_APP_ENV`] says.
fn bundled_helper(daemon_exe: &Path) -> Option<PathBuf> {
    if let Some(app) = std::env::var_os(HELPER_APP_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(app)).filter(|app| app.exists());
    }
    daemon_exe
        .parent()
        .and_then(Path::parent)
        .map(|contents| contents.join("Helpers").join(HELPER_APP))
        .filter(|app| app.exists())
}

fn executable_stamp(app: &Path) -> Option<SystemTime> {
    std::fs::metadata(app.join("Contents/MacOS").join(HELPER_BIN))
        .and_then(|meta| meta.modified())
        .ok()
}

/// Places the helper, its token and the skill plugin under `home`.
fn place(source: &Path, stamp: Option<SystemTime>, home: &Path) -> Result<Placed, String> {
    private_dir(home).map_err(|e| format!("couldn't make {}: {e}", home.display()))?;
    let app = place_helper(source, home).map_err(|e| e.to_string())?;
    let token =
        socket_token(home).map_err(|e| format!("couldn't write the helper's token: {e}"))?;
    let plugin = write_plugin(home).map_err(|e| format!("couldn't write the skill: {e}"))?;
    Ok(Placed {
        source: source.to_path_buf(),
        stamp,
        app,
        home: home.to_path_buf(),
        plugin,
        token,
    })
}

fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// The token every request on the helper's socket carries: `<home>/auth`, made once (0600).
/// The daemon reads the file (LaunchServices starts it without our environment); each shim
/// gets the token in its environment.
fn socket_token(home: &Path) -> std::io::Result<String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let path = home.join("auth");
    if let Ok(token) = std::fs::read_to_string(&path) {
        let token = token.trim();
        if token.len() >= 32 {
            return Ok(token.to_owned());
        }
    }
    let token = format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    );
    let partial = home.join(format!(".auth-{}", std::process::id()));
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o600)
        .open(&partial)?;
    file.write_all(token.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(&partial, &path)?;
    Ok(token)
}

/// Writes the skill plugin: `<home>/agent-plugin/.claude-plugin/plugin.json` and
/// `skills/computer-use/SKILL.md`. Claude loads the folder as a plugin; Codex reads its
/// `skills` folder as an extra skills root.
fn write_plugin(home: &Path) -> std::io::Result<PathBuf> {
    let plugin = home.join("agent-plugin");
    let skill = plugin.join("skills/computer-use");
    std::fs::create_dir_all(plugin.join(".claude-plugin"))?;
    std::fs::create_dir_all(&skill)?;
    let manifest = serde_json::json!({
        "name": "brigadier",
        "description": "Brigadier's skills for its agents.",
        "version": env!("CARGO_PKG_VERSION"),
    });
    write_if_changed(
        &plugin.join(".claude-plugin/plugin.json"),
        &serde_json::to_string_pretty(&manifest).unwrap_or_default(),
    )?;
    write_if_changed(&skill.join("SKILL.md"), SKILL)?;
    Ok(plugin)
}

fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    std::fs::write(path, contents)
}

/// The helper's copy in `home`, copied again whenever it differs from `bundle` (an update).
///
/// A new copy must pass its signature check before it replaces the old one, which happens in
/// one swap. A helper daemon still running from the old copy is then stopped, so the next tool
/// call starts the new one. The copy in place is checked again each time it is placed.
#[cfg(target_os = "macos")]
fn place_helper(bundle: &Path, home: &Path) -> std::io::Result<PathBuf> {
    let placed = home.join(HELPER_APP);
    if !same_tree(bundle, &placed) {
        let running = running_daemon(home, &placed);
        let fresh = home.join(format!(".helper-new-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&fresh);
        let made = copy_bundle(bundle, &fresh)
            .and_then(|()| verify_signature(&fresh))
            .and_then(|()| match std::fs::symlink_metadata(&placed) {
                Ok(_) => place::swap(&fresh, &placed),
                Err(_) => std::fs::rename(&fresh, &placed),
            });
        // After a swap this is the old copy; after a failure, the new one.
        let _ = std::fs::remove_dir_all(&fresh);
        made?;
        if let Some(pid) = running {
            place::stop(pid);
        }
    }
    verify_signature(&placed)?;
    Ok(placed)
}

#[cfg(not(target_os = "macos"))]
fn place_helper(_: &Path, _: &Path) -> std::io::Result<PathBuf> {
    Err(std::io::ErrorKind::Unsupported.into())
}

/// The helper daemon recorded in `<home>/cu.pid`, if it still runs from `placed`: a reused pid
/// belongs to another program, whose executable lies elsewhere.
#[cfg(target_os = "macos")]
fn running_daemon(home: &Path, placed: &Path) -> Option<i32> {
    let pid: i32 = std::fs::read_to_string(home.join("cu.pid"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let placed = std::fs::canonicalize(placed).ok()?;
    (pid > 1 && place::executable(pid)?.starts_with(&placed)).then_some(pid)
}

/// Copies an app bundle with ditto(1), which keeps it as signed. `--noqtn`: no quarantine on
/// the copy, which macOS already checked as part of Brigadier.app.
#[cfg(target_os = "macos")]
fn copy_bundle(from: &Path, to: &Path) -> std::io::Result<()> {
    let out = std::process::Command::new("/usr/bin/ditto")
        .arg("--noqtn")
        .arg(from)
        .arg(to)
        .stdin(std::process::Stdio::null())
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    Err(std::io::Error::other(format!(
        "couldn't copy Brigadier Computer Use to {}: {}",
        to.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

/// `codesign --verify --strict` (codesign(1): the signature, with its extra checks such as
/// symbolic links pointing only inside the bundle).
#[cfg(target_os = "macos")]
fn verify_signature(app: &Path) -> std::io::Result<()> {
    let out = std::process::Command::new("/usr/bin/codesign")
        .args(["--verify", "--strict"])
        .arg(app)
        .stdin(std::process::Stdio::null())
        .output()?;
    if out.status.success() {
        return Ok(());
    }
    Err(std::io::Error::other(format!(
        "Brigadier Computer Use at {} failed its signature check: {}",
        app.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    )))
}

/// The system calls placing the helper needs.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
mod place {
    use std::ffi::CString;
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use nix::libc;

    /// Swaps two paths in one step: renamex_np(2) with `RENAME_SWAP`. On a file system without
    /// it (`ENOTSUP`), two renames, with `b` missing for a moment between them.
    pub(super) fn swap(a: &Path, b: &Path) -> std::io::Result<()> {
        let c = |p: &Path| CString::new(p.as_os_str().as_bytes()).map_err(std::io::Error::other);
        let (ca, cb) = (c(a)?, c(b)?);
        // SAFETY: both are NUL-terminated paths that live across the call.
        if unsafe { libc::renamex_np(ca.as_ptr(), cb.as_ptr(), libc::RENAME_SWAP) } == 0 {
            return Ok(());
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::ENOTSUP) {
            return Err(e);
        }
        let aside = a.with_extension("swap");
        std::fs::rename(b, &aside)?;
        std::fs::rename(a, b)?;
        std::fs::rename(aside, a)
    }

    /// A process's executable, as proc_pidpath(3) reports it.
    pub(super) fn executable(pid: i32) -> Option<PathBuf> {
        let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        // SAFETY: the buffer is `PROC_PIDPATHINFO_MAXSIZE` bytes, its documented size.
        let len = unsafe { libc::proc_pidpath(pid, path.as_mut_ptr().cast(), path.len() as u32) };
        if len <= 0 {
            return None;
        }
        path.truncate(len as usize);
        Some(PathBuf::from(std::ffi::OsString::from_vec(path)))
    }

    /// SIGTERM, then SIGKILL if it hasn't gone within a second.
    pub(super) fn stop(pid: i32) {
        // SAFETY: plain signals to a pid checked just before.
        unsafe { libc::kill(pid, libc::SIGTERM) };
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(1) {
            // SAFETY: signal 0 only checks that the process exists.
            if unsafe { libc::kill(pid, 0) } != 0 {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: as above.
        unsafe { libc::kill(pid, libc::SIGKILL) };
    }
}

/// Whether two trees hold the same entries: names, file bytes and modes, link targets.
#[cfg(target_os = "macos")]
fn same_tree(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let (Ok(ma), Ok(mb)) = (std::fs::symlink_metadata(a), std::fs::symlink_metadata(b)) else {
        return false;
    };
    let (ta, tb) = (ma.file_type(), mb.file_type());
    if ta.is_symlink() || tb.is_symlink() {
        return ta.is_symlink()
            && tb.is_symlink()
            && matches!((std::fs::read_link(a), std::fs::read_link(b)), (Ok(x), Ok(y)) if x == y);
    }
    if ta.is_dir() && tb.is_dir() {
        let names = |d: &Path| -> Option<std::collections::BTreeSet<std::ffi::OsString>> {
            std::fs::read_dir(d)
                .ok()?
                .map(|e| e.ok().map(|e| e.file_name()))
                .collect()
        };
        return match (names(a), names(b)) {
            (Some(x), Some(y)) if x == y => x.iter().all(|n| same_tree(&a.join(n), &b.join(n))),
            _ => false,
        };
    }
    ta.is_file()
        && tb.is_file()
        && ma.len() == mb.len()
        && ma.permissions().mode() == mb.permissions().mode()
        && matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder, removed when dropped.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir().join(format!("brigadier-cu-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_token_is_made_once_and_kept_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = Scratch::new();
        let first = socket_token(dir.path()).unwrap();
        assert_eq!(first.len(), 64);
        assert_eq!(socket_token(dir.path()).unwrap(), first);
        let mode = std::fs::metadata(dir.path().join("auth"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn the_plugin_holds_the_skill_for_both_providers() {
        let dir = Scratch::new();
        let plugin = write_plugin(dir.path()).unwrap();
        let manifest: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(plugin.join(".claude-plugin/plugin.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(manifest["name"], "brigadier");
        let skill = std::fs::read_to_string(plugin.join("skills/computer-use/SKILL.md")).unwrap();
        assert!(skill.starts_with("---\nname: computer-use\n"));
    }

    #[test]
    fn a_missing_helper_is_no_helper() {
        let dir = Scratch::new();
        let exe = dir.path().join("Brigadier.app/Contents/MacOS/brigadierd");
        assert_eq!(bundled_helper(&exe), None);
        let helper = dir
            .path()
            .join("Brigadier.app/Contents/Helpers")
            .join(HELPER_APP);
        std::fs::create_dir_all(&helper).unwrap();
        assert_eq!(bundled_helper(&exe), Some(helper));
    }
}
