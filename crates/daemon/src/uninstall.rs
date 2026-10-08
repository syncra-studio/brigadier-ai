//! Uninstall Brigadier…, the daemon's side. A preview comes first (`previewUninstall`): what
//! runs and will stop, the worktrees and branches, the per-app folders, the keep-awake rule, the
//! app. Then `uninstall` tears down in order:
//!
//! 1. every terminal and dictation stops, and the session manager winds every conversation
//!    down and disposes of everything the cleanup ledger records (worktrees through git, their
//!    uncommitted changes committed first; CLI session files, Codex threads and trust entries);
//! 2. the branches the user picked go (merged ones come picked; others only when ticked), each
//!    at the tip the preview showed; the rest are listed with the command that deletes them;
//! 3. sleep is restored, the lid-closed sudoers rule removed (one administrator prompt, and only
//!    by the installed app, `ai.brigadier.app`, whose rule it is), the microphone permission
//!    reset;
//! 4. a detached `brigadierd uninstall-finish` starts. Once the app and this daemon have quit it
//!    moves to the Trash what they held until then: the per-app folders, the data directory
//!    (unless kept), and the app bundle; it deletes this data directory's session temp folders
//!    and connection folder. Every entry is found by its exact name, bound and checked again
//!    (see [`brigadier_sandbox::removal`]); what fails is written to
//!    `~/Library/Logs/Brigadier-uninstall.log` and announced in a notification.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use brigadier_core::manager::disk::counted;
use brigadier_core::storage::{
    AppRemoval, BranchChoice, UninstallApp, UninstallItem, UninstallPlan, UninstallReport,
    UninstallStep,
};
use brigadier_providers::Artifact;
use brigadier_sandbox::removal::{self, Bound};
use brigadier_sandbox::{AppPaths, InstanceLock, SpawnSpec, footprint};

use crate::server::Daemon;
use crate::{awake, idle};

/// How long a preview's plan can be carried out.
const PLAN_LIFETIME: Duration = Duration::from_secs(15 * 60);
/// How long the finisher waits for the app and the daemon to quit.
const QUIT_WAIT: Duration = Duration::from_secs(60 * 60);
/// The installed app, whose keep-awake rule is the system's one.
const INSTALLED_IDENTIFIER: &str = "ai.brigadier.app";

struct Planned {
    at: Instant,
    app: UninstallApp,
    plan: UninstallPlan,
}

#[derive(Default)]
pub struct Uninstall {
    plans: Mutex<HashMap<String, Planned>>,
    started: AtomicBool,
}

/// Something the finisher removes once the app and the daemon quit.
struct Target {
    label: String,
    root: PathBuf,
    path: PathBuf,
    /// To the Trash (what the user may want back); otherwise deleted (rebuildable).
    trash: bool,
}

impl Uninstall {
    /// Uninstalling has begun: nothing housekeeps any more.
    pub fn started(&self) -> bool {
        self.started.load(Ordering::Acquire)
    }

    fn plans(&self) -> std::sync::MutexGuard<'_, HashMap<String, Planned>> {
        self.plans.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub async fn preview(
        &self,
        daemon: &Daemon,
        app: UninstallApp,
    ) -> Result<UninstallPlan, String> {
        if !footprint::is_brigadier_identifier(&app.identifier) {
            return Err(format!(
                "{} is not a Brigadier app, so nothing is uninstalled for it",
                app.identifier
            ));
        }
        let paths = daemon.runtime.platform().paths().clone();
        let data_dir = paths.data_dir.clone();
        removal::check_data_dir(&data_dir).map_err(|err| err.to_string())?;
        let running = idle::running(daemon).await;
        let (worktrees, branches) = daemon
            .sessions
            .uninstall_survey()
            .await
            .map_err(|err| err.to_string())?;
        let cli_files = daemon
            .runtime
            .ledger()
            .owners()
            .into_iter()
            .flat_map(|(_, artifacts, _)| artifacts)
            .filter(|artifact| {
                matches!(
                    artifact,
                    Artifact::ClaudeSession { .. }
                        | Artifact::ClaudeProjectDir { .. }
                        | Artifact::CodexThread { .. }
                        | Artifact::CodexGeneratedImages { .. }
                        | Artifact::CodexProjectTrust { .. }
                        | Artifact::CliTrust { .. }
                )
            })
            .count();
        let identifier = app.identifier.clone();
        let bundle = app.bundle_path.clone();
        let (data_bytes, mut items, app_removal) = tokio::task::spawn_blocking(move || {
            let items: Vec<UninstallItem> = targets(&paths, &identifier)
                .into_iter()
                .map(|target| UninstallItem {
                    bytes: removal::allocated_size(&target.path),
                    path: Some(target.path.display().to_string()),
                    label: target.label,
                    to_trash: target.trash,
                })
                .collect();
            (
                removal::allocated_size(&paths.data_dir),
                items,
                app_removal(bundle.as_deref(), &identifier),
            )
        })
        .await
        .map_err(|err| err.to_string())?;
        if !worktrees.is_empty() {
            let dirty = worktrees.iter().filter(|w| w.has_changes).count();
            items.insert(
                0,
                UninstallItem {
                    label: {
                        let count = counted(worktrees.len(), "worktree", "worktrees");
                        if dirty == 0 {
                            format!("{count}, removed through git")
                        } else {
                            format!(
                                "{count}, removed through git ({dirty} with uncommitted changes, \
                                 kept first as WIP commits on their branches)"
                            )
                        }
                    },
                    path: None,
                    bytes: worktrees.iter().map(|w| w.bytes).sum(),
                    to_trash: false,
                },
            );
        }
        if cli_files > 0 {
            items.push(UninstallItem {
                label: format!(
                    "{} Brigadier recorded",
                    counted(
                        cli_files,
                        "Claude or Codex session file, thread or trust entry",
                        "Claude and Codex session files, threads and trust entries",
                    )
                ),
                path: None,
                bytes: 0,
                to_trash: false,
            });
        }
        let plan = UninstallPlan {
            plan_id: uuid::Uuid::now_v7().to_string(),
            data_dir: data_dir.display().to_string(),
            data_bytes,
            running,
            items,
            branches,
            sudoers_rule: awake::lid_rule_installed()
                && (app.identifier == INSTALLED_IDENTIFIER || lid_dry_run()),
            microphone: cfg!(target_os = "macos"),
            app: app_removal,
        };
        let mut plans = self.plans();
        plans.retain(|_, planned| planned.at.elapsed() < PLAN_LIFETIME);
        plans.insert(
            plan.plan_id.clone(),
            Planned {
                at: Instant::now(),
                app,
                plan: plan.clone(),
            },
        );
        Ok(plan)
    }

    pub async fn run(
        &self,
        daemon: &Daemon,
        plan_id: &str,
        keep_data: bool,
        picked: Vec<BranchChoice>,
    ) -> Result<UninstallReport, String> {
        let Planned { app, plan, .. } = self
            .plans()
            .remove(plan_id)
            .filter(|planned| planned.at.elapsed() < PLAN_LIFETIME)
            .ok_or("This preview is too old; open Uninstall Brigadier… again.")?;
        if self.started.swap(true, Ordering::AcqRel) {
            return Err("Brigadier is already being uninstalled.".into());
        }
        tracing::info!(identifier = %app.identifier, keep_data, "uninstalling Brigadier");
        let mut report = UninstallReport::default();
        let steps = &mut report.steps;
        daemon.terminals.close_all();
        daemon.dictation.stop_all();
        let torn = daemon.sessions.tear_down().await;
        let mut problems = torn.failures.clone();
        problems.extend(
            torn.kept_worktrees
                .iter()
                .map(|kept| format!("kept the worktree {kept}")),
        );
        step(
            steps,
            "Stopped every session, chat, worker and terminal, and removed their worktrees, temp \
             files and CLI session files",
            if problems.is_empty() {
                Ok(None)
            } else {
                Err(problems.join("\n"))
            },
        );

        let listed = plan.branches.len();
        let (kept, failures) = daemon
            .sessions
            .settle_uninstall_branches(plan.branches, picked)
            .await;
        let deleted = listed - kept.len();
        step(
            steps,
            &format!(
                "Deleted {deleted} of Brigadier's branches; kept {}",
                kept.len()
            ),
            if failures.is_empty() {
                Ok(None)
            } else {
                Err(failures.join("\n"))
            },
        );
        report.kept_branches = kept;

        daemon.awake.shutdown().await;
        step(steps, "Stopped keeping the computer awake", Ok(None));
        if plan.sudoers_rule {
            step(
                steps,
                "Removed the rule for keeping awake with the lid closed",
                awake::remove_lid_rule().await.map(Some),
            );
        }
        if plan.microphone {
            step(
                steps,
                "Reset the microphone permission",
                reset_microphone(&app.identifier).await.map(|()| None),
            );
        }

        // What still holds changes stays where it is, with the folder that holds it.
        let keep_data = keep_data || !torn.kept_worktrees.is_empty();
        let paths = daemon.runtime.platform().paths().clone();
        let bundle = match &plan.app {
            AppRemoval::Bundle { path, .. } => Some(PathBuf::from(path)),
            _ => None,
        };
        let mut after = targets(&paths, &app.identifier);
        if !keep_data {
            after.push(data_target(&paths.data_dir));
        }
        if let Some(bundle) = &bundle {
            after.push(bundle_target(bundle));
        }
        report.after_quit = after
            .iter()
            .map(|target| target.path.display().to_string())
            .collect();
        let mut spec = SpawnSpec::new(std::env::current_exe().map_err(|err| err.to_string())?)
            .arg("uninstall-finish")
            .arg("--data-dir")
            .arg(paths.data_dir.as_os_str())
            .arg("--identifier")
            .arg(&app.identifier)
            .arg("--app-pid")
            .arg(app.pid.to_string())
            .arg("--daemon-pid")
            .arg(std::process::id().to_string());
        if keep_data {
            spec = spec.arg("--keep-data");
        }
        if let Some(bundle) = &bundle {
            spec = spec.arg("--bundle").arg(bundle.as_os_str());
        }
        let spawned = daemon
            .runtime
            .platform()
            .processes()
            .spawn_detached(&spec)
            .map(|child| {
                tracing::info!(
                    pid = child.pid(),
                    "the uninstall finisher waits for Brigadier to quit"
                )
            })
            .map_err(|err| err.to_string());
        step(
            steps,
            &if keep_data {
                "Moves the per-app folders and the app to the Trash once Brigadier quits; your data \
                 stays"
                    .to_owned()
            } else {
                "Moves the data folder, the per-app folders and the app to the Trash once Brigadier \
                 quits"
                    .to_owned()
            },
            spawned.map(|()| {
                (!torn.kept_worktrees.is_empty()).then(|| {
                    format!(
                        "The data folder {} stays: it still holds worktrees that git couldn't \
                         remove or whose changes couldn't be kept.",
                        paths.data_dir.display()
                    )
                })
            }),
        );
        Ok(report)
    }
}

/// Records how one step of the teardown went.
fn step(steps: &mut Vec<UninstallStep>, label: &str, result: Result<Option<String>, String>) {
    let (ok, detail) = match result {
        Ok(detail) => (true, detail),
        Err(detail) => (false, Some(detail)),
    };
    tracing::info!(
        step = label,
        ok,
        detail = detail.as_deref().unwrap_or(""),
        "uninstall step"
    );
    steps.push(UninstallStep {
        label: label.to_owned(),
        ok,
        detail,
    });
}

/// `BRIGADIER_LID_RULE_DRY_RUN=<file>` in a development build: the rule step looks at that
/// stand-in and only says what it would run (see `awake`).
fn lid_dry_run() -> bool {
    cfg!(debug_assertions)
        && std::env::var_os("BRIGADIER_LID_RULE_DRY_RUN").is_some_and(|value| !value.is_empty())
}

async fn reset_microphone(identifier: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let output = tokio::process::Command::new("/usr/bin/tccutil")
            .args(["reset", "Microphone", identifier])
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|err| err.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = identifier;
        Ok(())
    }
}

/// The per-app folders, this data directory's session temp folders and its connection folder,
/// as they are now.
fn targets(paths: &AppPaths, identifier: &str) -> Vec<Target> {
    let mut targets: Vec<Target> = footprint::app_folders(identifier)
        .into_iter()
        .filter(|folder| std::fs::symlink_metadata(&folder.path).is_ok())
        .map(|folder| Target {
            label: folder.path.display().to_string(),
            root: folder.root,
            path: folder.path,
            trash: true,
        })
        .collect();
    #[cfg(unix)]
    targets.extend(
        footprint::session_temp_folders()
            .into_iter()
            .filter(|folder| folder.made_by(&paths.instance))
            .map(|folder| Target {
                label: format!("Session temp folder {}", folder.path.display()),
                root: PathBuf::from(footprint::SESSION_TEMP_BASE),
                path: folder.path,
                trash: false,
            }),
    );
    if let Some(dir) = paths.socket_dir()
        && let Some(root) = dir.parent()
        && std::fs::symlink_metadata(dir).is_ok()
    {
        targets.push(Target {
            label: format!("Connection folder {}", dir.display()),
            root: root.to_owned(),
            path: dir.to_owned(),
            trash: false,
        });
    }
    targets
}

fn data_target(data_dir: &Path) -> Target {
    Target {
        label: format!("Data folder {}", data_dir.display()),
        root: data_dir.parent().unwrap_or(data_dir).to_owned(),
        path: data_dir.to_owned(),
        trash: true,
    }
}

fn bundle_target(bundle: &Path) -> Target {
    Target {
        label: format!("The app {}", bundle.display()),
        root: bundle.parent().unwrap_or(bundle).to_owned(),
        path: bundle.to_owned(),
        trash: true,
    }
}

/// Whether the app can go too: a bundle whose identifier is `identifier` and that holds this
/// very program (so it is the app this daemon came with).
fn app_removal(bundle: Option<&str>, identifier: &str) -> AppRemoval {
    if !cfg!(target_os = "macos") {
        return AppRemoval::Unsupported {
            how: "Remove the app the way you installed it (your package manager, or Apps & \
                  features on Windows)."
                .into(),
        };
    }
    let Some(bundle) = bundle else {
        return AppRemoval::NoBundle;
    };
    match check_bundle(Path::new(bundle), identifier) {
        Ok(()) => AppRemoval::Bundle {
            path: bundle.to_owned(),
            bytes: removal::allocated_size(Path::new(bundle)),
        },
        Err(why) => {
            tracing::warn!(bundle, why, "the app bundle is left alone");
            AppRemoval::NoBundle
        }
    }
}

/// The bundle at `bundle` is this app: an `.app` folder (not a link), its Info.plist says
/// `identifier`, and it holds the running program.
fn check_bundle(bundle: &Path, identifier: &str) -> Result<(), String> {
    if bundle.extension().is_none_or(|ext| ext != "app") {
        return Err("not an .app bundle".into());
    }
    let meta = std::fs::symlink_metadata(bundle).map_err(|err| err.to_string())?;
    if !meta.is_dir() {
        return Err("not a folder".into());
    }
    let exe = std::env::current_exe().map_err(|err| err.to_string())?;
    let (exe, bundle_real) = (
        exe.canonicalize().map_err(|err| err.to_string())?,
        bundle.canonicalize().map_err(|err| err.to_string())?,
    );
    if !exe.starts_with(&bundle_real) {
        return Err("it doesn't hold this program".into());
    }
    let output = std::process::Command::new("/usr/bin/plutil")
        .args(["-extract", "CFBundleIdentifier", "raw", "-o", "-"])
        .arg(bundle.join("Contents/Info.plist"))
        .output()
        .map_err(|err| err.to_string())?;
    let found = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if !output.status.success() || found != identifier {
        return Err(format!("its identifier is {found:?}, not {identifier}"));
    }
    Ok(())
}

/// `brigadierd uninstall-finish`: waits for the app and its daemon to quit, then removes what
/// they held. Exit code 0 when everything went, 1 otherwise (see the log).
pub fn finish_main(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let usage = "usage: brigadierd uninstall-finish --data-dir PATH --identifier ID --app-pid PID \
                 --daemon-pid PID [--keep-data] [--bundle PATH]";
    let (mut data_dir, mut identifier, mut app_pid, mut daemon_pid) = (None, None, None, None);
    let (mut keep_data, mut bundle) = (false, None);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--keep-data") => keep_data = true,
            Some(flag) => {
                let Some(value) = args.next() else {
                    eprintln!("{usage}");
                    return ExitCode::from(2);
                };
                match flag {
                    "--data-dir" => data_dir = Some(PathBuf::from(value)),
                    "--identifier" => identifier = value.into_string().ok(),
                    "--app-pid" => app_pid = value.to_str().and_then(|v| v.parse::<u32>().ok()),
                    "--daemon-pid" => {
                        daemon_pid = value.to_str().and_then(|v| v.parse::<u32>().ok());
                    }
                    "--bundle" => bundle = Some(PathBuf::from(value)),
                    _ => {
                        eprintln!("{usage}");
                        return ExitCode::from(2);
                    }
                }
            }
            None => {
                eprintln!("{usage}");
                return ExitCode::from(2);
            }
        }
    }
    let (Some(data_dir), Some(identifier), Some(app_pid), Some(daemon_pid)) =
        (data_dir, identifier, app_pid, daemon_pid)
    else {
        eprintln!("{usage}");
        return ExitCode::from(2);
    };
    let failures = finish(
        &data_dir,
        &identifier,
        &[app_pid, daemon_pid],
        keep_data,
        bundle,
    );
    if failures.is_empty() {
        return ExitCode::SUCCESS;
    }
    tell_failures(&identifier, &failures);
    ExitCode::FAILURE
}

fn finish(
    data_dir: &Path,
    identifier: &str,
    pids: &[u32],
    keep_data: bool,
    bundle: Option<PathBuf>,
) -> Vec<String> {
    if !footprint::is_brigadier_identifier(identifier) {
        return vec![format!("{identifier} is not a Brigadier app")];
    }
    let paths = match AppPaths::resolve(data_dir.to_owned()) {
        Ok(paths) => paths,
        Err(err) => return vec![format!("{}: {err}", data_dir.display())],
    };
    let deadline = Instant::now() + QUIT_WAIT;
    while pids.iter().any(|pid| alive(*pid)) {
        if Instant::now() > deadline {
            return vec![
                "Brigadier was still running an hour later, so nothing more was removed.".into(),
            ];
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // Held while removing, so no daemon starts on this data directory meanwhile.
    let lock = match InstanceLock::try_acquire(&paths.lock_path) {
        Ok(Some(lock)) => lock,
        Ok(None) => {
            return vec![format!(
                "Brigadier was started again on {}, so nothing more was removed.",
                data_dir.display()
            )];
        }
        Err(err) => {
            return vec![format!(
                "Couldn't make sure no Brigadier runs on {} ({err}), so nothing more was removed.",
                data_dir.display()
            )];
        }
    };
    let mut failures = Vec::new();
    let remove = |target: &Target, failures: &mut Vec<String>| {
        let outcome = removal::bind(&target.root, &target.path).and_then(|bound: Bound| {
            if target.trash {
                removal::trash(&bound, &paths.instance)
            } else {
                removal::delete(&bound)
            }
        });
        if let Err(err) = outcome {
            failures.push(format!("{}: {err}", target.label));
        }
    };
    for target in targets(&paths, identifier) {
        remove(&target, &mut failures);
    }
    if let Some(bundle) = bundle {
        match check_bundle(&bundle, identifier) {
            Ok(()) => remove(&bundle_target(&bundle), &mut failures),
            Err(why) => failures.push(format!("The app {} stays: {why}", bundle.display())),
        }
    }
    if !keep_data {
        match removal::check_data_dir(data_dir) {
            Ok(()) => {
                // Moving the folder keeps the lock held on Unix, so no daemon starts on it
                // meanwhile; Windows can't move a folder with a file open in it.
                #[cfg(windows)]
                drop(lock);
                remove(&data_target(data_dir), &mut failures);
            }
            Err(err) => failures.push(err.to_string()),
        }
    }
    #[cfg(not(windows))]
    drop(lock);
    failures
}

fn alive(pid: u32) -> bool {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System};
    let mut system = System::new();
    let pid = sysinfo::Pid::from_u32(pid);
    system.refresh_processes_specifics(
        ProcessesToUpdate::Some(&[pid]),
        true,
        ProcessRefreshKind::nothing(),
    );
    system.process(pid).is_some()
}

/// Writes what couldn't be removed to `~/Library/Logs/Brigadier-uninstall.log` and says so in a
/// notification (macOS); elsewhere to standard error.
fn tell_failures(identifier: &str, failures: &[String]) {
    let text = format!(
        "Uninstalling {identifier} left these in place:\n{}\n",
        failures
            .iter()
            .map(|failure| format!("- {failure}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        let log = PathBuf::from(home).join("Library/Logs/Brigadier-uninstall.log");
        let _ = std::fs::write(&log, &text);
        let message = format!("Some things could not be removed. See {}.", log.display());
        let _ = std::process::Command::new("/usr/bin/osascript")
            .args([
                "-e",
                &format!(
                    "display notification \"{}\" with title \"Brigadier uninstall\"",
                    message.replace('\\', "\\\\").replace('"', "\\\"")
                ),
            ])
            .output();
        return;
    }
    eprint!("{text}");
}
