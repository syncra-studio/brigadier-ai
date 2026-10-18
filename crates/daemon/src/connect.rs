//! Connect: one step that gets an agent working, from the first-run setup or Providers. When
//! its binary is missing, the vendor's own native installer runs; then, unless it is signed in
//! already, the vendor's own sign-in, which opens its page in the browser and is checked every
//! few seconds until the agent says it is signed in. Each agent's progress is published as
//! `agentConnectChanged`; failures are one plain sentence (what the installer printed goes to
//! the log only). Connect again after a failure retries; Cancel stops it.
//!
//! Both run as plain child processes, with stdin closed for the installer and nothing typed
//! into either. That is their unattended contract, checked on 2026-10-18 against Claude Code's
//! `install.sh` (2.1.302) and Codex's (0.163.0), with no terminal, stdin closed and a scratch
//! home:
//! - Claude Code's installer asks nothing; its `claude install` prints plain text when piped.
//!   It leaves the shell profiles alone, so `~/.local/bin` may not be on the login PATH
//!   ([`CliEnv::installer_folder`] finds it there).
//! - Codex's asks "Start Codex now?" (and whether to remove a conflicting npm or Homebrew
//!   install) on `/dev/tty`, and in a terminal waits for the answer forever.
//!   `CODEX_NON_INTERACTIVE=1` skips both questions (the answer is no), as having no
//!   terminal does; it is set either way.
//! - Each sign-in (`claude auth login`, `codex login`) opens the browser and waits, with no
//!   terminal; Claude Code's also offers to read a pasted code, which nobody types here.
//!   Whatever opens the browser hands it its own environment, home included (`open` and
//!   LaunchServices alike). So the sign-in gets a `BROWSER` that only passes the link back,
//!   and Brigadier opens it with the account's own home. Claude Code uses it; Codex ignores
//!   it on macOS and opens the page itself.
//! - `curl … | sh` runs with `pipefail`, so a download that fails fails the install instead of
//!   piping an empty script into a shell that succeeds. Either installer offline exits non-zero
//!   with curl's error on stderr (7: can't connect).

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use brigadier_core::{
    AgentConnect, ConnectProgress, ConnectStep, ConnectView, DomainEvent, now_ms, streams,
};
use brigadier_providers::cli::CliEnv;
use brigadier_providers::process::{self, CliProcess};
use brigadier_providers::{ProviderKind, ProviderStatus};
use brigadier_sandbox::{Platform, SpawnSpec};
use brigadier_store::{NewEvent, Retention};
use tokio_util::sync::CancellationToken;

use crate::server::Daemon;

/// An installer that runs longer than this is stopped (a download on a slow line takes a few
/// minutes; one that hangs never ends).
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The vendor's sign-in page left open this long is given up on.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long a connected agent's row waits for its overview before it goes anyway.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the agent is asked whether it is signed in yet.
const POLL_EVERY: Duration = Duration::from_secs(3);
/// After asking a process to end, how long before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// The installer's last lines kept for the log.
const TAIL_LINES: usize = 30;
/// How often the sign-in is checked for the link it wants opened.
#[cfg(unix)]
const LINK_EVERY: Duration = Duration::from_millis(200);

/// Every agent's Connect: how it goes, and how to stop the one running.
#[derive(Default)]
pub struct Connects {
    state: Mutex<HashMap<ProviderKind, Entry>>,
    /// Numbers each attempt, so one cancelled and still ending can't touch the retry after it.
    attempts: AtomicU64,
    /// Held while a view is read and published, so the last one published is the newest.
    publishing: tokio::sync::Mutex<()>,
}

struct Entry {
    /// Which attempt this is.
    attempt: u64,
    progress: ConnectProgress,
    /// Present while it runs.
    cancel: Option<CancellationToken>,
}

impl Connects {
    fn state(&self) -> MutexGuard<'_, HashMap<ProviderKind, Entry>> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn view(&self) -> ConnectView {
        view_of(&self.state())
    }

    /// A new attempt for `provider`, beginning at `progress`. None while one already runs.
    fn begin(
        &self,
        provider: ProviderKind,
        progress: ConnectProgress,
    ) -> Option<(u64, CancellationToken)> {
        let mut state = self.state();
        if state
            .get(&provider)
            .is_some_and(|entry| entry.cancel.is_some())
        {
            return None;
        }
        let attempt = self.attempts.fetch_add(1, Ordering::Relaxed);
        let cancel = CancellationToken::new();
        state.insert(
            provider,
            Entry {
                attempt,
                progress,
                cancel: Some(cancel.clone()),
            },
        );
        Some((attempt, cancel))
    }

    /// The attempt's next step; false (nothing to publish) once it is not the running one.
    fn report(&self, provider: ProviderKind, attempt: u64, progress: ConnectProgress) -> bool {
        let mut state = self.state();
        match state.get_mut(&provider) {
            Some(entry) if entry.attempt == attempt && entry.cancel.is_some() => {
                entry.progress = progress;
                true
            }
            _ => false,
        }
    }

    /// How the attempt ended: kept to show, or its row gone. False once it is not the current one.
    fn finish(
        &self,
        provider: ProviderKind,
        attempt: u64,
        outcome: Option<ConnectProgress>,
    ) -> bool {
        let mut state = self.state();
        if state
            .get(&provider)
            .is_none_or(|entry| entry.attempt != attempt)
        {
            return false;
        }
        match outcome {
            Some(progress) => {
                state.insert(
                    provider,
                    Entry {
                        attempt,
                        progress,
                        cancel: None,
                    },
                );
            }
            None => {
                state.remove(&provider);
            }
        }
        true
    }

    /// Stops the agent's attempt, if it runs; its row leaves the view either way. False when
    /// there was no row.
    fn stop(&self, provider: ProviderKind) -> bool {
        let mut state = self.state();
        let Some(entry) = state.remove(&provider) else {
            return false;
        };
        if let Some(cancel) = entry.cancel {
            cancel.cancel();
        }
        true
    }
}

fn view_of(state: &HashMap<ProviderKind, Entry>) -> ConnectView {
    ConnectView {
        agents: ProviderKind::ALL
            .into_iter()
            .filter_map(|provider| {
                state.get(&provider).map(|entry| AgentConnect {
                    provider,
                    progress: entry.progress.clone(),
                })
            })
            .collect(),
    }
}

/// Starts connecting `provider` in the background. Refused while it already runs.
pub fn start(daemon: &Arc<Daemon>, provider: ProviderKind) -> Result<(), String> {
    // What the last check found says where it begins; the flow checks again first.
    let missing = daemon
        .runtime
        .overview(provider)
        .and_then(|overview| overview.status)
        .is_some_and(|status| status.path.is_none());
    let progress = if missing {
        ConnectProgress::Installing
    } else {
        ConnectProgress::SigningIn
    };
    let Some((attempt, cancel)) = daemon.connects.begin(provider, progress) else {
        return Err(format!("{} is already connecting", provider.label()));
    };
    let daemon = daemon.clone();
    tokio::spawn(async move {
        publish(&daemon).await;
        let ops = Ops::of(&daemon);
        let reporter = daemon.clone();
        let report = move |progress: ConnectProgress| {
            if reporter.connects.report(provider, attempt, progress) {
                let reporter = reporter.clone();
                tokio::spawn(async move { publish(&reporter).await });
            }
        };
        let outcome = connect(&ops, provider, &cancel, &report).await;
        let outcome = if outcome == Some(ConnectProgress::Connected) {
            // Its row says Connected until its overview (signed in, its models) is in, then
            // goes: the overview says Connected from then on, and a later sign-out shows as one.
            if daemon
                .connects
                .report(provider, attempt, ConnectProgress::Connected)
            {
                publish(&daemon).await;
            }
            daemon.runtime.check_provider(provider, CHECK_TIMEOUT).await;
            None
        } else {
            // Its overview (found or not, signed in or not) is published by the check.
            daemon.runtime.refresh_providers(Some(provider));
            outcome
        };
        if daemon.connects.finish(provider, attempt, outcome) {
            publish(&daemon).await;
        }
    });
    Ok(())
}

/// Stops the agent's Connect, if it runs; its row leaves the view either way.
pub async fn cancel(daemon: &Arc<Daemon>, provider: ProviderKind) {
    if daemon.connects.stop(provider) {
        publish(daemon).await;
    }
}

/// Publishes the view as it is now. Each change publishes after it, one at a time, so the
/// last view published is never older than the last change.
async fn publish(daemon: &Daemon) {
    let _publishing = daemon.connects.publishing.lock().await;
    let connect = daemon.connects.view();
    let event = DomainEvent::AgentConnectChanged { connect };
    let result = async {
        let new = NewEvent::new(streams::CONNECT, event.kind(), now_ms(), &event)?;
        daemon
            .store
            .append_with(vec![new], Some(Retention { keep_last: 1 }))
            .await?;
        Ok::<_, brigadier_store::Error>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(%error, "could not publish agent.connectChanged");
    }
}

type Status =
    Arc<dyn Fn(ProviderKind) -> Pin<Box<dyn Future<Output = ProviderStatus> + Send>> + Send + Sync>;

/// What Connect runs with: the daemon's in use, a test's in the tests.
struct Ops {
    platform: Arc<dyn Platform>,
    env: Arc<CliEnv>,
    /// Whether the agent is found and signed in, as its CLI says now.
    status: Status,
    /// The installer, as one shell line (PowerShell on Windows).
    installer: Arc<dyn Fn(ProviderKind) -> String + Send + Sync>,
    install_timeout: Duration,
    sign_in_timeout: Duration,
    poll: Duration,
    /// Opens the vendor's sign-in page in the browser.
    #[cfg(unix)]
    open_link: Arc<dyn Fn(&str) + Send + Sync>,
}

impl Ops {
    fn of(daemon: &Daemon) -> Self {
        let runtime = daemon.runtime.clone();
        Self {
            platform: runtime.platform().clone(),
            env: runtime.cli_env().clone(),
            status: Arc::new(move |provider| {
                let runtime = runtime.clone();
                Box::pin(async move { runtime.provider_status(provider).await })
            }),
            installer: Arc::new(|provider| provider.install_command(cfg!(windows)).to_owned()),
            install_timeout: INSTALL_TIMEOUT,
            sign_in_timeout: SIGN_IN_TIMEOUT,
            poll: POLL_EVERY,
            #[cfg(unix)]
            open_link: Arc::new(open_in_browser),
        }
    }
}

/// Opens `link` in the user's browser with their account's home, whatever `HOME` Brigadier
/// itself runs with: a browser started with another home can't find its keychain.
#[cfg(unix)]
fn open_in_browser(link: &str) {
    let opener = if cfg!(target_os = "macos") {
        "/usr/bin/open"
    } else {
        "xdg-open"
    };
    let mut command = std::process::Command::new(opener);
    command
        .arg(link)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    if let Ok(Some(user)) = nix::unistd::User::from_uid(nix::unistd::getuid()) {
        command.env("HOME", user.dir);
    }
    match command.spawn() {
        // Reaped off the runtime; it ends as soon as the browser has the link.
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(error) => tracing::warn!(%error, "the sign-in page didn't open"),
    }
}

/// A `BROWSER` for the sign-in that only passes the link back: the script, and the file the
/// link lands in. Removed when dropped.
#[cfg(unix)]
struct LinkCatcher {
    dir: PathBuf,
}

#[cfg(unix)]
impl LinkCatcher {
    fn new() -> Option<Self> {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("brigadier-sign-in-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).ok()?;
        let catcher = Self { dir };
        let script = catcher.script();
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n",
                catcher.dir.join("link").display()
            ),
        )
        .ok()?;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).ok()?;
        Some(catcher)
    }

    fn script(&self) -> PathBuf {
        self.dir.join("browser")
    }

    /// The first link the sign-in asked to open, once it has.
    fn link(&self) -> Option<String> {
        let text = std::fs::read_to_string(self.dir.join("link")).ok()?;
        text.lines()
            .map(str::trim)
            .find(|line| line.starts_with("https://") || line.starts_with("http://"))
            .map(str::to_owned)
    }
}

#[cfg(unix)]
impl Drop for LinkCatcher {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Connects `provider`: installs it when it is missing, then signs it in unless it is. Each
/// step is reported as it begins. Answers how it ended, or `None` once cancelled.
async fn connect(
    ops: &Ops,
    provider: ProviderKind,
    cancel: &CancellationToken,
    report: &(dyn Fn(ConnectProgress) + Send + Sync),
) -> Option<ConnectProgress> {
    let failed = |step, error: String| Some(ConnectProgress::Failed { step, error });
    let label = provider.label();
    let mut status = (ops.status)(provider).await;
    if status.path.is_none() {
        report(ConnectProgress::Installing);
        match install(ops, provider, cancel).await {
            Installed::Done => {}
            Installed::Cancelled => return None,
            Installed::Failed(error) => return failed(ConnectStep::Install, error),
        }
        status = (ops.status)(provider).await;
        if status.path.is_none() {
            tracing::warn!(%provider, "installed, but the agent isn't found");
            return failed(
                ConnectStep::Install,
                format!("{label} was installed, but Brigadier can't find it. Try again."),
            );
        }
    }
    if status.logged_in {
        return Some(ConnectProgress::Connected);
    }
    if cancel.is_cancelled() {
        return None;
    }
    report(ConnectProgress::SigningIn);
    let binary = PathBuf::from(status.path.unwrap_or_default());
    sign_in(ops, provider, binary, cancel).await
}

enum Installed {
    Done,
    Cancelled,
    Failed(String),
}

/// The spawn spec of the vendor's installer: its line in a shell that fails with the
/// download, with no questions asked, in the home folder.
fn installer_spec(ops: &Ops, provider: ProviderKind) -> SpawnSpec {
    let line = (ops.installer)(provider);
    let spec = if cfg!(windows) {
        ops.env
            .spec(&PathBuf::from("powershell.exe"))
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg(line)
    } else {
        ops.env
            .spec(&PathBuf::from("/bin/bash"))
            .arg("-c")
            .arg(format!("set -o pipefail; {line}"))
    };
    let mut spec = spec.env("CODEX_NON_INTERACTIVE", "1").env("NO_COLOR", "1");
    spec.cwd = ops.env.home();
    spec
}

async fn install(ops: &Ops, provider: ProviderKind, cancel: &CancellationToken) -> Installed {
    let label = provider.label();
    let spec = installer_spec(ops, provider);
    tracing::info!(%provider, "installing the agent with its own installer");
    let spawned = match process::spawn(ops.platform.clone(), &spec, process::Options::default()) {
        Ok(spawned) => spawned,
        Err(error) => {
            tracing::warn!(%provider, %error, "the installer didn't start");
            return Installed::Failed(format!("{label} couldn't be installed. Try again."));
        }
    };
    let child = spawned.process;
    // Nothing is ever typed into it: a question reads the end of input, and the answer is no.
    child.close_stdin().await;
    let tail = Arc::new(Mutex::new(VecDeque::new()));
    let reader = tokio::spawn({
        let tail = tail.clone();
        let mut lines = spawned.stdout;
        async move {
            while let Some(line) = lines.recv().await {
                let mut tail = tail.lock().unwrap_or_else(|p| p.into_inner());
                if tail.len() == TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
        }
    });
    let exit = tokio::select! {
        exit = child.exited() => Some(exit),
        () = cancel.cancelled() => {
            child.shutdown(STOP_GRACE).await;
            reader.abort();
            return Installed::Cancelled;
        }
        () = tokio::time::sleep(ops.install_timeout) => None,
    };
    let Some(exit) = exit else {
        child.shutdown(STOP_GRACE).await;
        reader.abort();
        tracing::warn!(%provider, "the installer took too long; stopped");
        return Installed::Failed(format!(
            "Installing {label} took too long. Check your internet connection, then try again."
        ));
    };
    let _ = tokio::time::timeout(Duration::from_secs(1), reader).await;
    if exit.code == Some(0) {
        return Installed::Done;
    }
    let output = {
        let tail = tail.lock().unwrap_or_else(|p| p.into_inner());
        let mut lines: Vec<String> = tail.iter().cloned().collect();
        lines.extend(child.stderr_tail());
        lines.join("\n")
    };
    tracing::warn!(%provider, code = ?exit.code, signal = ?exit.signal, %output, "the installer failed");
    Installed::Failed(install_error(provider, &output))
}

/// The installer's failure in one plain sentence, from what it printed.
fn install_error(provider: ProviderKind, output: &str) -> String {
    let label = provider.label();
    let lower = output.to_lowercase();
    let offline = [
        "curl: (6)",
        "curl: (7)",
        "curl: (28)",
        "curl: (35)",
        "could not resolve host",
        "failed to connect",
        "could not fetch",
        "network is unreachable",
        "unable to connect to the remote server",
        "the remote name could not be resolved",
    ];
    if offline.iter().any(|needle| lower.contains(needle)) {
        return format!(
            "Brigadier couldn't download {label}. Check your internet connection, then try again."
        );
    }
    if lower.contains("no space left") {
        return format!(
            "There isn't enough free disk space to install {label}. Free some up, then try again."
        );
    }
    format!("{label} couldn't be installed. Try again.")
}

/// Runs the vendor's sign-in (it opens the browser) and asks the agent every `poll` whether it
/// is signed in, until it is, the sign-in ends without it, or it is given up on.
async fn sign_in(
    ops: &Ops,
    provider: ProviderKind,
    binary: PathBuf,
    cancel: &CancellationToken,
) -> Option<ConnectProgress> {
    let label = provider.label();
    let failed = |error: String| {
        Some(ConnectProgress::Failed {
            step: ConnectStep::SignIn,
            error,
        })
    };
    let mut spec = ops.env.spec(&binary);
    spec.args = provider
        .login_command()
        .split(' ')
        .skip(1)
        .map(Into::into)
        .collect();
    spec.cwd = ops.env.home();
    #[cfg(unix)]
    let catcher = LinkCatcher::new();
    #[cfg(unix)]
    if let Some(catcher) = &catcher {
        spec = spec.env("BROWSER", catcher.script());
    }
    tracing::info!(%provider, "opening the agent's own sign-in");
    let spawned = match process::spawn(ops.platform.clone(), &spec, process::Options::default()) {
        Ok(spawned) => spawned,
        Err(error) => {
            tracing::warn!(%provider, %error, "the sign-in didn't start");
            return failed(format!("{label}'s sign-in didn't open. Try again."));
        }
    };
    let child: Arc<CliProcess> = spawned.process;
    // Its lines (the link it opened) aren't shown; read so it never blocks on them.
    let mut lines = spawned.stdout;
    let drain = tokio::spawn(async move { while lines.recv().await.is_some() {} });
    // The link it wants opened, opened by Brigadier the moment it arrives.
    #[cfg(unix)]
    let opener = tokio::spawn({
        let open_link = ops.open_link.clone();
        async move {
            let Some(catcher) = catcher else { return };
            loop {
                if let Some(link) = catcher.link() {
                    tracing::info!(%provider, "opening the sign-in page");
                    open_link(&link);
                    return;
                }
                tokio::time::sleep(LINK_EVERY).await;
            }
        }
    });
    let given_up = tokio::time::sleep(ops.sign_in_timeout);
    tokio::pin!(given_up);
    let outcome = loop {
        tokio::select! {
            exit = child.exited() => {
                // It ends by itself once signed in; one that ends any other way didn't sign in.
                if (ops.status)(provider).await.logged_in {
                    break Some(ConnectProgress::Connected);
                }
                tracing::info!(%provider, code = ?exit.code, stderr = ?child.stderr_tail(), "the sign-in ended without signing in");
                break failed(format!("Signing in to {label} didn't finish. Try again."));
            }
            () = cancel.cancelled() => break None,
            () = &mut given_up => {
                break failed(format!("Signing in to {label} took too long. Try again."));
            }
            () = tokio::time::sleep(ops.poll) => {
                if (ops.status)(provider).await.logged_in {
                    break Some(ConnectProgress::Connected);
                }
            }
        }
    };
    if child.is_running() {
        child.shutdown(STOP_GRACE).await;
    }
    drain.abort();
    #[cfg(unix)]
    opener.abort();
    outcome
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::path::Path;

    use brigadier_providers::Provider;
    use brigadier_providers::claude::Claude;

    use super::*;

    /// A scratch home with no agent in it, removed when dropped however the test ends.
    struct Home(PathBuf);

    impl Home {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "brigadier-connect-{name}-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            Self(dir)
        }

        fn write(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        fn has(&self, name: &str) -> bool {
            self.0.join(name).exists()
        }
    }

    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A stand-in for Claude Code's binary as its installer leaves it in `~/.local/bin`: it
    /// reports its version, says it is signed in once `signed-in` exists, and its sign-in does
    /// what `login-mode` says: `finish` (signs in, then ends), `stay` (signs in and keeps
    /// running, as the real one can while the page is open), `quit` (ends without signing in),
    /// `wait` (never signs in, never ends).
    const FAKE_CLAUDE: &str = r#"#!/bin/sh
home="$HOME"
case "$1 $2" in
  "--version "*) echo "2.1.302 (Claude Code)" ;;
  "auth status")
    if [ -f "$home/signed-in" ]; then
      echo '{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"max","email":"me@example.com"}'
    else
      echo '{"loggedIn":false}'
    fi ;;
  "auth login")
    echo "Opening browser to sign in…"
    touch "$home/login-started"
    if [ -n "$BROWSER" ]; then
      "$BROWSER" "https://claude.com/cai/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A5512%2Fcallback"
    else
      touch "$home/opened-itself"
    fi
    case "$(cat "$home/login-mode" 2>/dev/null)" in
      finish) sleep 1; touch "$home/signed-in" ;;
      stay) touch "$home/signed-in"; sleep 60 ;;
      quit) exit 1 ;;
      *) sleep 60 ;;
    esac ;;
esac
"#;

    /// A stand-in for the vendor's installer: puts [`FAKE_CLAUDE`] in `~/.local/bin`, or does
    /// what `install-mode` says first: `offline` (curl can't connect), `broken` (fails with
    /// nothing plain to say), `hang` (never ends), `nothing` (succeeds, installs nothing).
    /// Each try is counted in `tries`.
    fn installer(home: &Home) -> String {
        home.write("fake-claude", FAKE_CLAUDE);
        let script = home.write(
            "install.sh",
            r#"#!/bin/sh
echo try >> "$HOME/tries"
case "$(cat "$HOME/install-mode" 2>/dev/null)" in
  offline) rm -f "$HOME/install-mode"; echo "curl: (7) Failed to connect to downloads.claude.ai port 443" >&2; exit 7 ;;
  broken) echo "Setting up Claude Code..."; echo "Installation was killed before it could finish (exit code 137)." >&2; exit 137 ;;
  hang) sleep 60 ;;
  nothing) exit 0 ;;
esac
echo "Setting up Claude Code..."
mkdir -p "$HOME/.local/bin"
cp "$HOME/fake-claude" "$HOME/.local/bin/claude"
echo "Claude Code successfully installed!"
"#,
        );
        script.display().to_string()
    }

    fn ops(home: &Home) -> Ops {
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(home.0.join("data")),
        })
        .unwrap();
        // The login PATH has neither agent: only the system's folders and an empty one.
        let env = Arc::new(CliEnv::from_vars([
            ("HOME".into(), home.0.clone().into_os_string()),
            (
                "PATH".into(),
                format!("{}:/usr/bin:/bin", home.0.join("bin").display()).into(),
            ),
        ]));
        let claude = Arc::new(Claude::new(platform.clone(), env.clone()));
        let line = installer(home);
        let opened = home.0.clone();
        Ops {
            platform,
            env,
            status: Arc::new(move |_| {
                let claude = claude.clone();
                Box::pin(async move { claude.status().await })
            }),
            installer: Arc::new(move |_| line.clone()),
            install_timeout: Duration::from_secs(20),
            sign_in_timeout: Duration::from_secs(20),
            poll: Duration::from_millis(100),
            open_link: Arc::new(move |link| {
                std::fs::write(opened.join("opened"), link).unwrap();
            }),
        }
    }

    /// Runs Connect for Claude Code, answering how it ended and every step it reported.
    async fn run(ops: &Ops) -> (Option<ConnectProgress>, Vec<ConnectProgress>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let report = {
            let seen = seen.clone();
            move |progress| seen.lock().unwrap().push(progress)
        };
        let cancel = CancellationToken::new();
        let outcome = tokio::time::timeout(
            Duration::from_secs(40),
            connect(ops, ProviderKind::Claude, &cancel, &report),
        )
        .await
        .expect("Connect ends");
        let seen = seen.lock().unwrap().clone();
        (outcome, seen)
    }

    fn failed(step: ConnectStep, error: &str) -> Option<ConnectProgress> {
        Some(ConnectProgress::Failed {
            step,
            error: error.into(),
        })
    }

    fn tries(home: &Home) -> usize {
        std::fs::read_to_string(home.0.join("tries"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_missing_agent_is_installed_then_signed_in() {
        let home = Home::new("install");
        std::fs::write(home.0.join("login-mode"), "finish").unwrap();
        let ops = ops(&home);
        // Detected as missing: not on the login PATH, nothing in the installer's folder.
        assert_eq!((ops.status)(ProviderKind::Claude).await.path, None);

        let (outcome, seen) = run(&ops).await;
        assert_eq!(outcome, Some(ConnectProgress::Connected));
        assert_eq!(
            seen,
            [ConnectProgress::Installing, ConnectProgress::SigningIn]
        );
        // Found where its installer put it, off the login PATH, and signed in.
        let status = (ops.status)(ProviderKind::Claude).await;
        assert_eq!(
            status.path.map(PathBuf::from),
            Some(home.0.join(".local/bin/claude"))
        );
        assert!(status.logged_in);
        assert_eq!(tries(&home), 1);
        // Brigadier opened the page the sign-in passed it; the sign-in opened nothing itself.
        assert_eq!(
            std::fs::read_to_string(home.0.join("opened")).unwrap(),
            "https://claude.com/cai/oauth/authorize?redirect_uri=http%3A%2F%2Flocalhost%3A5512%2Fcallback"
        );
        assert!(!home.has("opened-itself"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_install_that_fails_says_why_plainly_and_a_retry_installs() {
        let home = Home::new("offline");
        std::fs::write(home.0.join("install-mode"), "offline").unwrap();
        std::fs::write(home.0.join("login-mode"), "finish").unwrap();
        let ops = ops(&home);

        let (outcome, seen) = run(&ops).await;
        assert_eq!(
            outcome,
            failed(
                ConnectStep::Install,
                "Brigadier couldn't download Claude Code. Check your internet connection, then try again."
            )
        );
        assert_eq!(seen, [ConnectProgress::Installing]);
        // No sign-in was tried.
        assert!(!home.has("login-started"));

        // Connect again: the installer runs again, and this time it works.
        let (outcome, _) = run(&ops).await;
        assert_eq!(outcome, Some(ConnectProgress::Connected));
        assert_eq!(tries(&home), 2);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_install_failure_without_a_known_cause_and_one_that_installs_nothing() {
        let home = Home::new("broken");
        std::fs::write(home.0.join("install-mode"), "broken").unwrap();
        let ops = ops(&home);
        let (outcome, _) = run(&ops).await;
        assert_eq!(
            outcome,
            failed(
                ConnectStep::Install,
                "Claude Code couldn't be installed. Try again."
            )
        );

        // The installer says it worked, but nothing is there: that's a failure too.
        std::fs::write(home.0.join("install-mode"), "nothing").unwrap();
        let (outcome, _) = run(&ops).await;
        assert_eq!(
            outcome,
            failed(
                ConnectStep::Install,
                "Claude Code was installed, but Brigadier can't find it. Try again."
            )
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_installer_that_never_ends_is_stopped() {
        let home = Home::new("hang");
        std::fs::write(home.0.join("install-mode"), "hang").unwrap();
        let mut ops = ops(&home);
        ops.install_timeout = Duration::from_millis(500);
        let (outcome, _) = run(&ops).await;
        assert_eq!(
            outcome,
            failed(
                ConnectStep::Install,
                "Installing Claude Code took too long. Check your internet connection, then try again."
            )
        );
    }

    /// Codex's installer asks "Start Codex now? [y/N]" when it's done, reading the answer from
    /// the terminal when there is one (where nobody would ever answer), else from stdin. A
    /// stand-in doing exactly that ends at once: Connect sets `CODEX_NON_INTERACTIVE` and
    /// types nothing, so stdin is at its end.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_installer_question_is_answered_no_at_once() {
        let home = Home::new("prompt");
        let script = home.write(
            "codex-install.sh",
            r#"#!/bin/sh
echo "Codex CLI 0.163.0 installed successfully."
case "$CODEX_NON_INTERACTIVE" in
  1 | true | yes) echo skipped > "$HOME/asked" ; exit 0 ;;
esac
if ( : </dev/tty ) 2>/dev/null; then
  printf 'Start Codex now? [y/N] ' >/dev/tty
  read -r answer </dev/tty || exit 0
elif [ -t 0 ]; then
  read -r answer || exit 0
fi
echo "asked: $answer" > "$HOME/asked"
"#,
        );
        let mut ops = ops(&home);
        let line = script.display().to_string();
        ops.installer = Arc::new(move |_| line.clone());
        ops.install_timeout = Duration::from_secs(10);
        let cancel = CancellationToken::new();
        let started = std::time::Instant::now();
        assert!(matches!(
            install(&ops, ProviderKind::Codex, &cancel).await,
            Installed::Done
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            std::fs::read_to_string(home.0.join("asked")).unwrap(),
            "skipped\n"
        );

        // An installer that asks anyway, reading stdin, hears its end at once: nothing is typed
        // and stdin is closed.
        let stubborn = home.write(
            "asks-anyway.sh",
            r#"#!/bin/sh
printf 'Start Codex now? [y/N] '
read -r answer
echo "asked: $answer" > "$HOME/asked"
"#,
        );
        let line = stubborn.display().to_string();
        ops.installer = Arc::new(move |_| line.clone());
        let started = std::time::Instant::now();
        assert!(matches!(
            install(&ops, ProviderKind::Codex, &cancel).await,
            Installed::Done
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(
            std::fs::read_to_string(home.0.join("asked")).unwrap(),
            "asked: \n"
        );

        let spec = installer_spec(&ops, ProviderKind::Codex);
        assert!(
            spec.env
                .iter()
                .any(|(key, value)| key == "CODEX_NON_INTERACTIVE" && value == "1")
        );
        assert_eq!(spec.program, Path::new("/bin/bash"));
        assert!(
            spec.args
                .last()
                .is_some_and(|line| line.to_string_lossy().starts_with("set -o pipefail; "))
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_sign_in_is_noticed_while_the_page_is_still_open_and_its_process_ends() {
        let home = Home::new("stay");
        std::fs::write(home.0.join("login-mode"), "stay").unwrap();
        let ops = ops(&home);
        let started = std::time::Instant::now();
        let (outcome, seen) = run(&ops).await;
        assert_eq!(outcome, Some(ConnectProgress::Connected));
        assert_eq!(
            seen,
            [ConnectProgress::Installing, ConnectProgress::SigningIn]
        );
        // Noticed by the check, long before the sign-in would end by itself.
        assert!(started.elapsed() < Duration::from_secs(20));
        let leftover = std::process::Command::new("pgrep")
            .args([
                "-f",
                &home.0.join(".local/bin/claude").display().to_string(),
            ])
            .output()
            .unwrap();
        assert!(leftover.stdout.is_empty(), "the sign-in process still runs");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_sign_in_that_ends_without_signing_in_fails_and_a_signed_in_agent_is_connected() {
        let home = Home::new("quit");
        std::fs::write(home.0.join("login-mode"), "quit").unwrap();
        let ops = ops(&home);
        let (outcome, _) = run(&ops).await;
        assert_eq!(
            outcome,
            failed(
                ConnectStep::SignIn,
                "Signing in to Claude Code didn't finish. Try again."
            )
        );
        // Installed once: the retry goes straight to the sign-in.
        std::fs::write(home.0.join("login-mode"), "finish").unwrap();
        let (outcome, seen) = run(&ops).await;
        assert_eq!(outcome, Some(ConnectProgress::Connected));
        assert_eq!(seen, [ConnectProgress::SigningIn]);
        assert_eq!(tries(&home), 1);

        // Already signed in: nothing runs.
        std::fs::remove_file(home.0.join("login-started")).unwrap();
        let (outcome, seen) = run(&ops).await;
        assert_eq!(outcome, Some(ConnectProgress::Connected));
        assert!(seen.is_empty());
        assert!(!home.has("login-started"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_sign_in_left_open_is_given_up_on_and_cancel_stops_it() {
        let home = Home::new("wait");
        std::fs::write(home.0.join("login-mode"), "wait").unwrap();
        let mut ops = ops(&home);
        ops.sign_in_timeout = Duration::from_millis(800);
        let (outcome, _) = run(&ops).await;
        assert_eq!(
            outcome,
            failed(
                ConnectStep::SignIn,
                "Signing in to Claude Code took too long. Try again."
            )
        );

        ops.sign_in_timeout = Duration::from_secs(20);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(600)).await;
            stop.cancel();
        });
        let outcome = tokio::time::timeout(
            Duration::from_secs(10),
            connect(&ops, ProviderKind::Claude, &cancel, &|_| {}),
        )
        .await
        .expect("cancel ends it");
        assert_eq!(outcome, None);
        let leftover = std::process::Command::new("pgrep")
            .args([
                "-f",
                &home.0.join(".local/bin/claude").display().to_string(),
            ])
            .output()
            .unwrap();
        assert!(leftover.stdout.is_empty(), "the sign-in process still runs");
    }

    #[test]
    fn a_cancelled_attempt_still_ending_leaves_the_retry_after_it_alone() {
        let connects = Connects::default();
        let progress = |connects: &Connects| {
            connects
                .view()
                .agents
                .into_iter()
                .map(|agent| agent.progress)
                .collect::<Vec<_>>()
        };
        let (first, first_cancel) = connects
            .begin(ProviderKind::Claude, ConnectProgress::Installing)
            .unwrap();
        assert!(
            connects
                .begin(ProviderKind::Claude, ConnectProgress::Installing)
                .is_none()
        );
        // Cancel, then Connect again while the first is still shutting down.
        assert!(connects.stop(ProviderKind::Claude));
        assert!(first_cancel.is_cancelled());
        let (retry, retry_cancel) = connects
            .begin(ProviderKind::Claude, ConnectProgress::Installing)
            .unwrap();
        // The first's last step and its end change nothing of the retry's.
        assert!(!connects.report(ProviderKind::Claude, first, ConnectProgress::SigningIn));
        assert!(!connects.finish(ProviderKind::Claude, first, None));
        assert_eq!(progress(&connects), [ConnectProgress::Installing]);
        assert!(connects.report(ProviderKind::Claude, retry, ConnectProgress::SigningIn));
        assert_eq!(progress(&connects), [ConnectProgress::SigningIn]);
        // The retry can still be cancelled, and once ended it shows how.
        assert!(!retry_cancel.is_cancelled());
        let failed = ConnectProgress::Failed {
            step: ConnectStep::SignIn,
            error: "Signing in to Claude Code didn't finish. Try again.".into(),
        };
        assert!(connects.finish(ProviderKind::Claude, retry, Some(failed.clone())));
        assert_eq!(progress(&connects), [failed]);
        assert!(
            connects
                .begin(ProviderKind::Claude, ConnectProgress::SigningIn)
                .is_some()
        );
    }

    #[test]
    fn install_errors_are_plain() {
        let codex = |output| install_error(ProviderKind::Codex, output);
        assert_eq!(
            codex(
                "WARNING: releases.openai.com is unavailable\ncurl: (7) Failed to connect to 127.0.0.1"
            ),
            "Brigadier couldn't download Codex. Check your internet connection, then try again."
        );
        assert_eq!(
            codex("tar: write error: No space left on device"),
            "There isn't enough free disk space to install Codex. Free some up, then try again."
        );
        assert_eq!(
            codex("Installed Codex command did not report expected version"),
            "Codex couldn't be installed. Try again."
        );
    }
}
