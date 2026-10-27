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
//!
//! Windows and Linux: the helper is the `brigadier-cu` program beside brigadierd (a Tauri
//! sidecar). It runs from a copy in a folder of its own build, `<data dir>/computer/bin/<build
//! id>/` ([`versions`]): Windows can't replace a program while any process runs it, and every
//! running agent's MCP server does. An update places the new build beside the old one, so new
//! agents use it at once; the old build is marked retired, its daemon stopped, and its agents'
//! servers say plainly to start a new session. Folders no process runs from any more are
//! removed the next time the helper is placed. There is no bundle, signature check or
//! permission to grant.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use brigadier_providers::{McpServer, McpTransport};

use super::SessionManager;

/// The MCP server's name: tools reach the model as `mcp__computer__<tool>`.
pub(crate) const SERVER_NAME: &str = "computer";
/// The helper app's bundle name, in Brigadier.app and in the data folder.
#[cfg(target_os = "macos")]
pub(super) const HELPER_APP: &str = "Brigadier Computer Use.app";
/// Windows and Linux: the helper program, beside brigadierd and in `<home>/bin`.
#[cfg(not(target_os = "macos"))]
pub(super) const HELPER_APP: &str = if cfg!(windows) {
    "brigadier-cu.exe"
} else {
    "brigadier-cu"
};
/// The helper's executable inside its bundle.
pub(super) const HELPER_BIN: &str = "brigadier-cu";
/// The permissions the helper last read, which it keeps in its home.
pub(super) const PERMISSIONS_FILE: &str = "permissions.json";
/// Tells the helper that setup runs in Brigadier's window: it shows no panel, and agents'
/// calls wait until that window confirmed screen capture.
pub(super) const EXTERNAL_SETUP_ENV: &str = "BRIGADIER_CU_EXTERNAL_PERMISSION_FLOW";
/// Overrides where the helper app is found (a development checkout's staged bundle; on
/// Windows and Linux, the program).
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
pub(super) struct Placed {
    /// The bundled helper it was copied from, and that bundle's executable's change time: a
    /// different one (an update) is placed again.
    source: PathBuf,
    stamp: Option<SystemTime>,
    /// The placed helper: its app bundle on macOS, its program elsewhere.
    app: PathBuf,
    home: PathBuf,
    plugin: PathBuf,
    token: String,
}

/// What an agent gets for computer use: the driver's own server and skill, as a server entry
/// of its own beside the user's (every session, Chat and worker while the setting is on, S35).
pub(crate) struct ComputerUseKit {
    pub server: McpServer,
    /// The Brigadier-owned plugin folder holding the `computer-use` skill.
    pub plugin: PathBuf,
    /// The socket token, for the session's redactor.
    pub token: String,
}

impl SessionManager {
    /// The `computer` MCP server and the skill, while computer use is on (macOS only).
    /// `owner` names the agent (its task or conversation id): every call of its MCP server runs
    /// in that agent's own helper session, which ends when the agent's server exits.
    /// `always_load`: its tools are in the model's list from the start (a GUI job) instead of
    /// behind Claude's tool search. `None` when it is off or the helper can't be placed; the
    /// agent then simply works without it.
    pub(crate) async fn computer_use_kit(
        &self,
        owner: &str,
        always_load: bool,
    ) -> Option<ComputerUseKit> {
        if !self.core.settings().computer_use {
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
                // B: launch fills gateway metadata for resolved remote servers.
                gateway: None,
                name: SERVER_NAME.into(),
                transport: McpTransport::Stdio {
                    command: helper_program(&placed.app),
                    args: vec!["mcp".into()],
                    cwd: None,
                },
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
                    // Its agents have the skill: the server's own instructions stay one line, not
                    // a pitch that pulls them toward working on their own.
                    ("BRIGADIER_CU_AUDIENCE".into(), "brigadier".into()),
                    // Setup runs in Brigadier's own window, not a panel of the helper's.
                    (EXTERNAL_SETUP_ENV.into(), "1".into()),
                    // The session the helper keeps for this agent: it overrides the session
                    // names the agent passes, so the cursor, recordings and state end with it.
                    (
                        "BRIGADIER_CU_DEFAULT_SESSION".into(),
                        format!("brigadier-{owner}"),
                    ),
                ],
                tool_timeout_secs: Some(TOOL_TIMEOUT_SECS),
                trusted: true,
                always_load,
            },
            plugin: placed.plugin,
            token: placed.token,
        })
    }
}

impl ComputerUse {
    /// The placed helper, placing it first when this is the first use or the bundle changed.
    #[cfg(target_os = "macos")]
    pub(super) async fn ready(&self, daemon_exe: &Path, data_dir: &Path) -> Result<Placed, String> {
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

    /// Windows and Linux: the placed copy of the helper program, placed first when this is
    /// the first use or the installed program changed.
    #[cfg(not(target_os = "macos"))]
    pub(super) async fn ready(&self, daemon_exe: &Path, data_dir: &Path) -> Result<Placed, String> {
        let source = bundled_helper(daemon_exe)
            .ok_or_else(|| "the computer-use helper is missing from this install".to_owned())?;
        let stamp = std::fs::metadata(&source)
            .and_then(|meta| meta.modified())
            .ok();
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

/// The program agents' MCP server runs: inside the app bundle on macOS, the placed program
/// itself elsewhere.
pub(super) fn helper_program(app: &Path) -> PathBuf {
    if cfg!(target_os = "macos") {
        app.join("Contents/MacOS").join(HELPER_BIN)
    } else {
        app.to_path_buf()
    }
}

/// Windows and Linux: the helper program beside the daemon (Tauri puts its sidecars next to
/// the app's own program, in a release and in `tauri dev`), or where [`HELPER_APP_ENV`] says.
#[cfg(not(target_os = "macos"))]
pub(super) fn bundled_helper(daemon_exe: &Path) -> Option<PathBuf> {
    if let Some(program) = std::env::var_os(HELPER_APP_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(program)).filter(|program| program.is_file());
    }
    Some(daemon_exe.parent()?.join(HELPER_APP)).filter(|program| program.is_file())
}

/// The helper bundled with this install: beside the daemon in Brigadier.app
/// (`Contents/MacOS/brigadierd` → `Contents/Helpers/…`), or where [`HELPER_APP_ENV`] says.
/// A development daemon run from the checkout (`target/debug/brigadierd`) uses the bundle the
/// desktop build staged there (`apps/desktop/src-tauri/binaries/…`).
#[cfg(target_os = "macos")]
pub(super) fn bundled_helper(daemon_exe: &Path) -> Option<PathBuf> {
    if let Some(app) = std::env::var_os(HELPER_APP_ENV).filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(app)).filter(|app| app.exists());
    }
    let dir = daemon_exe.parent()?;
    let installed = dir
        .parent()
        .map(|contents| contents.join("Helpers").join(HELPER_APP));
    let staged = cfg!(debug_assertions)
        .then(|| dir.parent()?.parent())
        .flatten()
        .map(|checkout| {
            checkout
                .join("apps/desktop/src-tauri/binaries")
                .join(HELPER_APP)
        });
    installed.into_iter().chain(staged).find(|app| app.exists())
}

#[cfg(target_os = "macos")]
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

#[cfg(unix)]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// Windows: the data folder lies in the user's profile, whose ACL already keeps other users
/// out.
#[cfg(not(unix))]
fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

/// The token every request on the helper's socket carries: `<home>/auth`, made once (0600).
/// The daemon reads the file (LaunchServices starts it without our environment); each shim
/// gets the token in its environment.
fn socket_token(home: &Path) -> std::io::Result<String> {
    use std::io::Write;
    #[cfg(unix)]
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
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(&partial)?;
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

/// Windows and Linux: the helper program placed in its build's folder under `home/bin`
/// ([`versions::place`]); older builds are retired, their daemons stopped through their own
/// `host stop`, and their folders removed once nothing runs from them.
#[cfg(not(target_os = "macos"))]
fn place_helper(program: &Path, home: &Path) -> std::io::Result<PathBuf> {
    versions::place(program, &home.join("bin"), &versions::runs_from, &|old| {
        stop_helper(old, home)
    })
}

/// The current build's placed program under `home`, as the last placing recorded it.
#[cfg(not(target_os = "macos"))]
pub(super) fn placed_program(home: &Path) -> Option<PathBuf> {
    versions::current(&home.join("bin"))
}

/// Windows and Linux: one folder per helper build under `<home>/bin`, named by the build's id
/// (the program's BLAKE3, 16 hex digits), and `<home>/bin/current` naming the one new agents
/// use. A folder holding `retired` belongs to a build an update replaced: its MCP servers
/// refuse new calls with a plain "start a new session" (brigadier-cu's supervisor), and it is
/// removed once no process runs from it. Written for every platform so its tests run anywhere;
/// only Windows and Linux use it.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(super) mod versions {
    use std::io;
    use std::path::{Path, PathBuf};

    /// The program's file name in each folder.
    pub(crate) const PROGRAM: &str = if cfg!(windows) {
        "brigadier-cu.exe"
    } else {
        "brigadier-cu"
    };
    /// The file naming the current build's folder.
    pub(crate) const CURRENT: &str = "current";
    /// The mark of a replaced build's folder (brigadier-cu checks for it beside itself).
    pub(crate) const RETIRED: &str = "retired";

    /// A build's id: the first 16 hex digits of the program's BLAKE3.
    pub(crate) fn build_id(program: &Path) -> io::Result<String> {
        let bytes = std::fs::read(program)?;
        Ok(blake3::hash(&bytes).to_hex()[..16].to_owned())
    }

    /// The current build's program, if the last placing recorded one that still exists.
    pub(crate) fn current(bin: &Path) -> Option<PathBuf> {
        let id = std::fs::read_to_string(bin.join(CURRENT)).ok()?;
        let id = id.trim();
        let valid = !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit());
        let program = bin.join(id).join(PROGRAM);
        (valid && program.is_file()).then_some(program)
    }

    /// Places `program` in its build's folder under `bin` and makes it current, then retires
    /// every other build: marks its folder, `stop`s its daemon (given that build's program),
    /// and removes the folder unless `in_use` says a process still runs from it or its program
    /// can't be deleted (Windows keeps a running program's file). A program from the layout
    /// before folders (`bin/brigadier-cu`) is stopped and removed the same way.
    pub(crate) fn place(
        program: &Path,
        bin: &Path,
        in_use: &dyn Fn(&Path) -> bool,
        stop: &dyn Fn(&Path),
    ) -> io::Result<PathBuf> {
        let id = build_id(program)?;
        let folder = bin.join(&id);
        let placed = folder.join(PROGRAM);
        if !super::same_file(program, &placed) {
            std::fs::create_dir_all(&folder)?;
            // A folder named for these bytes runs nothing else: an earlier, broken copy in it
            // can go.
            let _ = std::fs::remove_file(&placed);
            let fresh = folder.join(format!(".{PROGRAM}.new-{}", std::process::id()));
            let made = std::fs::copy(program, &fresh).and_then(|_| {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&fresh, std::fs::Permissions::from_mode(0o755))?;
                }
                std::fs::rename(&fresh, &placed)
            });
            if made.is_err() {
                let _ = std::fs::remove_file(&fresh);
            }
            made?;
        }
        // A folder an earlier update retired and this placing brings back is current again.
        let _ = std::fs::remove_file(folder.join(RETIRED));
        let pointer = bin.join(format!(".{CURRENT}.new-{}", std::process::id()));
        std::fs::write(&pointer, &id)?;
        std::fs::rename(&pointer, bin.join(CURRENT))?;
        retire_others(bin, &id, in_use, stop);
        Ok(placed)
    }

    /// Retires and, where nothing runs from them any more, removes every build but `keep`.
    fn retire_others(bin: &Path, keep: &str, in_use: &dyn Fn(&Path) -> bool, stop: &dyn Fn(&Path)) {
        let legacy = bin.join(PROGRAM);
        if legacy.is_file() {
            stop(&legacy);
            if !in_use(&legacy) {
                let _ = std::fs::remove_file(&legacy);
            }
        }
        let Ok(entries) = std::fs::read_dir(bin) else {
            return;
        };
        for entry in entries.flatten() {
            let folder = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !folder.is_dir() || name == keep || name.starts_with('.') {
                continue;
            }
            let program = folder.join(PROGRAM);
            if !folder.join(RETIRED).exists() {
                // Marked first, so none of its servers starts its daemon again meanwhile.
                if std::fs::write(folder.join(RETIRED), keep).is_err() {
                    continue;
                }
                if program.is_file() {
                    stop(&program);
                }
            }
            prune(&folder, &program, in_use);
        }
    }

    /// Removes a retired build's folder, unless something still runs from it.
    fn prune(folder: &Path, program: &Path, in_use: &dyn Fn(&Path) -> bool) {
        if in_use(folder) {
            return;
        }
        // Windows refuses to delete a program any process is running: that keeps the folder.
        if program.exists() && std::fs::remove_file(program).is_err() {
            return;
        }
        if let Err(error) = std::fs::remove_dir_all(folder) {
            tracing::debug!(folder = %folder.display(), %error, "couldn't remove a retired helper");
        }
    }

    /// Whether a running process's program lies at or under `path`: Linux reads each
    /// process's `/proc/<pid>/exe`. Windows answers through the program's file instead (it
    /// can't be deleted while it runs, see [`prune`]); elsewhere nothing runs it.
    pub(crate) fn runs_from(path: &Path) -> bool {
        #[cfg(target_os = "linux")]
        {
            let Ok(path) = std::fs::canonicalize(path) else {
                return false;
            };
            std::fs::read_dir("/proc")
                .into_iter()
                .flatten()
                .flatten()
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.bytes().all(|b| b.is_ascii_digit()))
                })
                .filter_map(|entry| std::fs::read_link(entry.path().join("exe")).ok())
                .any(|exe| exe.starts_with(&path))
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = path;
            false
        }
    }
}

/// Whether two files hold the same bytes.
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) if ma.len() == mb.len() => {
            matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y)
        }
        _ => false,
    }
}

/// Stops the daemon a placed helper runs, waiting at most a few seconds.
#[cfg(not(target_os = "macos"))]
fn stop_helper(placed: &Path, home: &Path) {
    let Ok(mut child) = std::process::Command::new(placed)
        .args(["host", "stop"])
        .env("BRIGADIER_CU_HOME", home)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        return;
    };
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::time::Instant::now() < until {
        if child.try_wait().ok().flatten().is_some() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let _ = child.kill();
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

#[cfg(all(test, target_os = "macos"))]
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

    /// #228: a run opens its first page once and navigates that same window and tab.
    #[test]
    fn the_skill_keeps_a_browser_run_in_one_window_and_tab() {
        assert!(SKILL.contains("**Use one window and one tab for the whole run.** Each `launch_app` with `urls` opens another window, so call it once."));
        assert!(SKILL.contains("**Close what you opened**"));
        assert!(!SKILL.contains("Prefer windows over tabs"));
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

    #[test]
    fn a_development_daemon_uses_the_bundle_staged_in_its_checkout() {
        let dir = Scratch::new();
        let exe = dir.path().join("target/debug/brigadierd");
        assert_eq!(bundled_helper(&exe), None);
        let staged = dir
            .path()
            .join("apps/desktop/src-tauri/binaries")
            .join(HELPER_APP);
        std::fs::create_dir_all(&staged).unwrap();
        assert_eq!(bundled_helper(&exe), Some(staged));
    }
}

/// Windows and Linux placement ([`versions`]), on any platform.
#[cfg(test)]
mod version_tests {
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};

    use super::versions::{self, CURRENT, PROGRAM, RETIRED};

    /// A fresh folder with a `bin` and two helper "builds", removed when dropped.
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            let dir = std::env::temp_dir()
                .join(format!("brigadier-cu-versions-{}", uuid::Uuid::now_v7()));
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            std::fs::write(dir.join("build-a"), b"helper build A").unwrap();
            std::fs::write(dir.join("build-b"), b"helper build B").unwrap();
            Self(dir)
        }
        fn bin(&self) -> PathBuf {
            self.0.join("bin")
        }
        fn build(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
        fn folder(&self, build: &str) -> PathBuf {
            self.bin()
                .join(versions::build_id(&self.build(build)).unwrap())
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            #[cfg(unix)]
            for build in ["build-a", "build-b"] {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(
                    self.folder(build),
                    std::fs::Permissions::from_mode(0o755),
                );
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Places `build`, with `held` folders still running and the daemons stopped recorded.
    fn place(
        scratch: &Scratch,
        build: &str,
        held: &[PathBuf],
        stopped: &RefCell<Vec<PathBuf>>,
    ) -> PathBuf {
        versions::place(
            &scratch.build(build),
            &scratch.bin(),
            &|path: &Path| held.iter().any(|h| path.starts_with(h)),
            &|program: &Path| stopped.borrow_mut().push(program.to_path_buf()),
        )
        .unwrap()
    }

    #[test]
    fn a_build_is_placed_in_a_folder_of_its_own_and_made_current() {
        let scratch = Scratch::new();
        let stopped = RefCell::new(Vec::new());
        let placed = place(&scratch, "build-a", &[], &stopped);
        assert_eq!(placed, scratch.folder("build-a").join(PROGRAM));
        assert_eq!(std::fs::read(&placed).unwrap(), b"helper build A");
        assert_eq!(versions::current(&scratch.bin()), Some(placed.clone()));
        // Placing the same build again changes nothing and stops nothing.
        assert_eq!(place(&scratch, "build-a", &[], &stopped), placed);
        assert!(stopped.borrow().is_empty());
        assert!(!scratch.folder("build-a").join(RETIRED).exists());
    }

    #[test]
    fn an_update_goes_beside_a_running_build_which_is_retired_then_pruned() {
        let scratch = Scratch::new();
        let stopped = RefCell::new(Vec::new());
        let old = place(&scratch, "build-a", &[], &stopped);
        // An agent's MCP server still runs build A: the update can't replace its program.
        let held = [scratch.folder("build-a")];
        let new = place(&scratch, "build-b", &held, &stopped);
        assert_eq!(versions::current(&scratch.bin()), Some(new.clone()));
        assert_eq!(std::fs::read(&new).unwrap(), b"helper build B");
        assert!(old.exists(), "a build in use stays");
        assert!(scratch.folder("build-a").join(RETIRED).exists());
        assert_eq!(
            *stopped.borrow(),
            std::slice::from_ref(&old),
            "the old build's daemon was stopped"
        );
        // The next start, once that server has exited: the retired build goes, and its
        // daemon isn't stopped twice.
        place(&scratch, "build-b", &[], &stopped);
        assert!(!scratch.folder("build-a").exists());
        assert_eq!(stopped.borrow().len(), 1);
        assert!(new.exists());
    }

    /// Windows keeps a running program's file: it can't be deleted, so its folder stays.
    #[cfg(unix)]
    #[test]
    fn a_program_that_cannot_be_deleted_keeps_its_folder_until_it_can() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new();
        let stopped = RefCell::new(Vec::new());
        let old = place(&scratch, "build-a", &[], &stopped);
        place(&scratch, "build-b", &[scratch.folder("build-a")], &stopped);
        // Held: a read-only folder refuses the delete, as Windows refuses it for a running image.
        std::fs::set_permissions(
            scratch.folder("build-a"),
            std::fs::Permissions::from_mode(0o555),
        )
        .unwrap();
        let new = place(&scratch, "build-b", &[], &stopped);
        assert!(old.exists() && new.exists());
        assert_eq!(versions::current(&scratch.bin()), Some(new));
        std::fs::set_permissions(
            scratch.folder("build-a"),
            std::fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        place(&scratch, "build-b", &[], &stopped);
        assert!(!scratch.folder("build-a").exists());
    }

    #[test]
    fn the_program_from_before_build_folders_is_stopped_and_removed() {
        let scratch = Scratch::new();
        let legacy = scratch.bin().join(PROGRAM);
        std::fs::write(&legacy, b"helper placed before folders").unwrap();
        let stopped = RefCell::new(Vec::new());
        // Still running: stopped, kept.
        place(&scratch, "build-a", std::slice::from_ref(&legacy), &stopped);
        assert!(legacy.exists());
        assert_eq!(*stopped.borrow(), std::slice::from_ref(&legacy));
        place(&scratch, "build-a", &[], &stopped);
        assert!(!legacy.exists());
    }

    #[test]
    fn current_names_only_a_placed_build() {
        let scratch = Scratch::new();
        assert_eq!(versions::current(&scratch.bin()), None);
        std::fs::write(scratch.bin().join(CURRENT), "../../etc").unwrap();
        assert_eq!(versions::current(&scratch.bin()), None);
        std::fs::write(scratch.bin().join(CURRENT), "0123456789abcdef").unwrap();
        assert_eq!(
            versions::current(&scratch.bin()),
            None,
            "no program in that folder"
        );
    }
}
