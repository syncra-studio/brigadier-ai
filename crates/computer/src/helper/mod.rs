//! The long-lived helper, `brigadier-computer serve` (§4.1, §4.2, Phase 2).
//!
//! It answers on a socket only the daemon that started it can use: a per-launch token in a
//! 0600 file, the daemon's pid when one is named, and in signed builds the same signing team.
//! The control service (permissions, cancel, stop) answers before any grant exists, so the
//! user can be walked through granting them. The engine starts on its own thread on the first
//! engine request that finds every grant in place. The main thread runs AppKit for the
//! menu-bar Stop item, the ⌃⌥⌘. hotkey and the agent cursor's overlay.
//!
//! The helper exits when its parent daemon goes away, and [`IDLE_EXIT`] after the last
//! session ended with nothing queued or running.

pub mod hub;
mod menu;
pub mod peer;

use std::io::{Read, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use objc2::MainThreadMarker;

use crate::wire::Permissions;
use hub::{Access, Hub, System};

/// How long the helper stays up with no session open and nothing running.
pub const IDLE_EXIT: Duration = Duration::from_secs(10 * 60);
/// Overrides [`IDLE_EXIT`], in seconds. For tests only: production never sets it.
pub const IDLE_EXIT_ENV: &str = "BRIGADIER_COMPUTER_IDLE_EXIT_SECS";
/// How often the parent and the idle time are checked.
const WATCH_EVERY: Duration = Duration::from_millis(250);

/// `serve`'s arguments.
#[derive(Debug, Clone)]
pub struct Options {
    pub socket: PathBuf,
    pub token_file: PathBuf,
    /// The daemon that started the helper: only it may connect, and the helper exits with it.
    pub parent: Option<i32>,
}

/// The real system's answers.
pub fn system() -> System {
    System {
        permissions: || {
            let (accessibility, screen_recording) = crate::macos::permissions();
            Permissions {
                accessibility,
                screen_recording,
            }
        },
        request_permission: crate::macos::request_permission,
        process_start_us: crate::macos::process_start_us,
    }
}

fn random_token() -> Result<String> {
    let mut b = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut b)?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

/// Writes the token where only this user can read it, from the first byte on.
fn write_token(path: &Path, token: &str) -> Result<()> {
    let _ = std::fs::remove_file(path);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("writing the token to {}", path.display()))?;
    f.write_all(token.as_bytes())?;
    Ok(())
}

/// Removes the socket and the token file and exits.
fn exit_clean(o: &Options, why: &str) -> ! {
    eprintln!("brigadier-computer: exiting: {why}");
    let _ = std::fs::remove_file(&o.socket);
    let _ = std::fs::remove_file(&o.token_file);
    std::process::exit(0)
}

/// Runs the helper. Call it from the process's main thread; it never returns.
pub fn serve(o: Options) -> Result<()> {
    let Some(mtm) = MainThreadMarker::new() else {
        bail!("the helper must run on the main thread");
    };
    // The parent's start time tells it apart from a later process that reuses its pid.
    let parent_started = match o.parent {
        Some(pid) => match crate::macos::process_start_us(pid) {
            Some(t) => Some((pid, t)),
            None => bail!("the parent process {pid} isn't running"),
        },
        None => None,
    };
    let dir = o.socket.parent().context("socket path")?;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    let _ = std::fs::remove_file(&o.socket);
    let token = random_token()?;
    let listener =
        UnixListener::bind(&o.socket).with_context(|| format!("binding {}", o.socket.display()))?;
    write_token(&o.token_file, &token)?;

    let team = peer::own_team();
    if team.is_none() {
        eprintln!("brigadier-computer: not signed by a team; peers are checked by token and pid");
    }
    let access = Access {
        token,
        parent: o.parent,
        team,
    };
    // The overlay draws on this thread once `menu::run` runs AppKit on it.
    let overlay: crate::cursor::Cursor = Some(crate::macos::overlay::Overlay::new());
    let hub = Hub::start(access, system(), overlay, crate::macos::MacDesktop::new)?;
    hub.accept(listener)?;
    eprintln!("brigadier-computer: serving on {}", o.socket.display());

    let idle_exit = std::env::var(IDLE_EXIT_ENV)
        .ok()
        .and_then(|s| s.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(IDLE_EXIT);
    let watched = hub.clone();
    let opts = o.clone();
    std::thread::Builder::new()
        .name("computer-lifecycle".into())
        .spawn(move || {
            loop {
                std::thread::sleep(WATCH_EVERY);
                if let Some((pid, started)) = parent_started
                    && crate::macos::process_start_us(pid) != Some(started)
                {
                    exit_clean(&opts, "the daemon that started it is gone");
                }
                if watched.idle_for().is_some_and(|d| d >= idle_exit) {
                    exit_clean(&opts, "no session for the idle time");
                }
            }
        })?;
    menu::run(mtm, hub)
}

#[cfg(test)]
mod tests;
