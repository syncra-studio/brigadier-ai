//! Connect: one step that gets an agent working, from the first-run setup or Providers. When
//! its binary is missing, the vendor's own native installer runs; then, unless it is signed in
//! already, the vendor's own sign-in, which opens its page in the browser; the agent is asked
//! whether it is signed in the moment its login is stored (where it keeps it changes: a
//! [`LoginMark`]), each time the sign-in prints a line or ends, and every few seconds besides.
//! Each agent's progress is published as `agentConnectChanged`; failures are one plain sentence
//! (what the installer printed goes to the log only). Connect again after a failure retries;
//! Cancel stops it. An extra account's sign-in, run in a terminal, is followed the same way
//! ([`follow_account_sign_in`]).
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
//!   terminal. Claude Code's also prints a page that shows a code and reads that code pasted on
//!   stdin: for when the browser can't hand the sign-in back (a firewall, a browser that blocks
//!   localhost), the page is offered and a pasted code passed on.
//!   Whatever opens the browser hands it its own environment, home included (`open` and
//!   LaunchServices alike). So the sign-in gets a `BROWSER` that only passes the link back,
//!   and Brigadier opens it with the account's own home. Claude Code uses it; Codex ignores
//!   it on macOS and opens the page itself (codex-cli 0.163.0, 2026-10-20: `codex login` has
//!   no option to skip the browser and keep the localhost callback; `--device-auth` drops the
//!   callback). That is harmless in the app, but a test run with a scratch `HOME` signs Codex
//!   in only with the browser already running (docs/DEVELOPMENT.md, Providers).
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

use brigadier_core::accounts::AccountRef;
use brigadier_core::runtime::Runtime;
use brigadier_core::{
    AgentConnect, ConnectProgress, ConnectStep, ConnectView, DomainEvent, now_ms, streams,
};
use brigadier_providers::cli::CliEnv;
use brigadier_providers::login_mark::LoginMark;
use brigadier_providers::process::{self, CliProcess};
use brigadier_providers::{ProviderKind, ProviderStatus};
use brigadier_sandbox::{Platform, SpawnSpec};
use brigadier_store::{NewEvent, Retention};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::server::Daemon;

/// An installer that runs longer than this is stopped (a download on a slow line takes a few
/// minutes; one that hangs never ends).
const INSTALL_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// The vendor's sign-in page left open this long is given up on.
const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(15 * 60);
/// How long a connected agent's row waits for its overview before it goes anyway.
const CHECK_TIMEOUT: Duration = Duration::from_secs(60);
/// How often the agent is asked whether it is signed in yet, besides each time its sign-in
/// prints a line or ends.
const POLL_EVERY: Duration = Duration::from_secs(3);
/// How often where the agent keeps its login is looked at while its sign-in runs: a read of a
/// file's or a Keychain item's dates, nothing spawned.
const MARK_EVERY: Duration = Duration::from_millis(100);
/// How often the agent is asked instead when where it keeps its login can't be watched.
const UNWATCHED_POLL: Duration = Duration::from_millis(500);
/// A sign-in that ended well is asked this many times, this far apart, before it counts as not
/// signed in: its login may land a moment after.
const SETTLE_TRIES: u32 = 4;
const SETTLE_EVERY: Duration = Duration::from_millis(250);
/// Once signed in, how long its sign-in may take to end by itself before it is stopped.
pub(crate) const LINGER: Duration = Duration::from_secs(20);
/// After asking a process to end, how long before it is killed.
const STOP_GRACE: Duration = Duration::from_secs(2);
/// The installer's last lines kept for the log.
const TAIL_LINES: usize = 30;
/// How often the sign-in is checked for the link it wants opened.
#[cfg(any(unix, windows))]
const LINK_EVERY: Duration = Duration::from_millis(20);

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
    /// While it signs in: the page that shows a code to paste, once the sign-in offered one.
    code_page: Option<String>,
    /// Takes pasted codes to the sign-in, while it runs.
    codes: Option<mpsc::UnboundedSender<String>>,
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
                code_page: None,
                codes: None,
            },
        );
        Some((attempt, cancel))
    }

    /// Where the running attempt's pasted codes arrive. None once it is not the running one.
    fn listen(
        &self,
        provider: ProviderKind,
        attempt: u64,
    ) -> Option<mpsc::UnboundedReceiver<String>> {
        let mut state = self.state();
        let entry = state
            .get_mut(&provider)
            .filter(|entry| entry.attempt == attempt && entry.cancel.is_some())?;
        let (sender, pasted) = mpsc::unbounded_channel();
        entry.codes = Some(sender);
        Some(pasted)
    }

    /// The running attempt's sign-in offers `page` for a code; false once it is not the
    /// running one.
    fn offer_code_page(&self, provider: ProviderKind, attempt: u64, page: String) -> bool {
        let mut state = self.state();
        match state.get_mut(&provider) {
            Some(entry)
                if entry.attempt == attempt
                    && entry.cancel.is_some()
                    && entry.progress == ConnectProgress::SigningIn =>
            {
                entry.code_page = Some(page);
                true
            }
            _ => false,
        }
    }

    /// Passes `code` to the sign-in running for `provider`; false when none offered a page.
    fn send_code(&self, provider: ProviderKind, code: String) -> bool {
        let state = self.state();
        state
            .get(&provider)
            .filter(|entry| entry.code_page.is_some())
            .and_then(|entry| entry.codes.as_ref())
            .is_some_and(|codes| codes.send(code).is_ok())
    }

    /// The attempt's next step; false (nothing to publish) once it is not the running one.
    fn report(&self, provider: ProviderKind, attempt: u64, progress: ConnectProgress) -> bool {
        let mut state = self.state();
        match state.get_mut(&provider) {
            Some(entry) if entry.attempt == attempt && entry.cancel.is_some() => {
                if progress != ConnectProgress::SigningIn {
                    entry.code_page = None;
                }
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
                        code_page: None,
                        codes: None,
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
                    code_page: entry.code_page.clone(),
                })
            })
            .collect(),
    }
}

/// Follows extra account `id`'s sign-in, run in its terminal, until it ends: the account is
/// checked again the moment where its CLI keeps its login changes (`before`: where it stood
/// before the sign-in started), and when it ends. One whose login can't be watched is checked
/// every [`UNWATCHED_POLL`] instead until it reads as signed in (for up to the sign-in timeout).
pub async fn follow_account_sign_in(
    runtime: Arc<Runtime>,
    provider: ProviderKind,
    id: String,
    mark: Option<LoginMark>,
    before: Option<String>,
    exited: impl Future<Output = ()>,
) {
    let account = AccountRef::new(provider, Some(id.clone()));
    let checker = runtime.clone();
    follow_sign_in(
        move || checker.refresh_account(id.clone()),
        move || runtime.signed_in(&account),
        mark,
        before,
        exited,
        UNWATCHED_POLL,
    )
    .await;
}

/// [`follow_account_sign_in`] with how the account is checked (`check`, which checks it again
/// after one already running) and whether it last read as signed in.
async fn follow_sign_in(
    check: impl Fn(),
    signed_in: impl Fn() -> bool,
    mark: Option<LoginMark>,
    before: Option<String>,
    exited: impl Future<Output = ()>,
    unwatched_poll: Duration,
) {
    let watched = before.is_some();
    let mut stamp = before;
    let every = if watched { MARK_EVERY } else { unwatched_poll };
    let started = tokio::time::Instant::now();
    let mut tick = tokio::time::interval_at(started + every, every);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // After a store, until when it is checked again while it doesn't read as signed in yet: its
    // status may follow the store by a moment.
    let mut settling = None;
    tokio::pin!(exited);
    loop {
        tokio::select! {
            biased;
            () = &mut exited => break,
            _ = tick.tick() => {}
        }
        match &mark {
            Some(mark) if watched => {
                let now = mark.read().await;
                if now.is_some() && now != stamp {
                    tracing::info!("an account's login was just stored; checking it");
                    stamp = now;
                    settling = Some(tokio::time::Instant::now() + SETTLE_EVERY * SETTLE_TRIES);
                    check();
                } else if settling.is_some_and(|until| tokio::time::Instant::now() < until)
                    && !signed_in()
                {
                    check();
                }
            }
            _ => {
                if started.elapsed() < SIGN_IN_TIMEOUT && !signed_in() {
                    check();
                }
            }
        }
    }
    check();
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
    // Already connecting: that one goes on, and a second sign-in never starts.
    let Some((attempt, cancel)) = daemon.connects.begin(provider, progress) else {
        return Ok(());
    };
    let offerer = daemon.clone();
    let mut codes = Codes {
        offer: Box::new(move |page| {
            if offerer.connects.offer_code_page(provider, attempt, page) {
                let offerer = offerer.clone();
                tokio::spawn(async move { publish(&offerer).await });
            }
        }),
        pasted: daemon
            .connects
            .listen(provider, attempt)
            .unwrap_or_else(|| mpsc::unbounded_channel().1),
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
        let ended = connect(&ops, provider, &cancel, &report, &mut codes).await;
        let outcome = match ended {
            Some(Ended {
                progress: ConnectProgress::Connected,
                status,
            }) => {
                // Its row says Connected until its overview (signed in, its models) is in, then
                // goes: the overview says Connected from then on, and a later sign-out shows as
                // one. The status that said so is the overview's at once; the check adds the rest.
                if daemon
                    .connects
                    .report(provider, attempt, ConnectProgress::Connected)
                {
                    publish(&daemon).await;
                }
                if let Some(status) = status {
                    daemon.runtime.note_status(status).await;
                }
                daemon.runtime.check_provider(provider, CHECK_TIMEOUT).await;
                None
            }
            ended => {
                // Its overview (found or not, signed in or not) is published by the check.
                daemon.runtime.refresh_providers(Some(provider));
                ended.map(|ended| ended.progress)
            }
        };
        if daemon.connects.finish(provider, attempt, outcome) {
            publish(&daemon).await;
        }
    });
    Ok(())
}

/// Passes a code pasted from the agent's code page to its running sign-in.
pub fn send_code(daemon: &Daemon, provider: ProviderKind, code: String) -> Result<(), String> {
    let code = whole_code(&code)?;
    if daemon.connects.send_code(provider, code.to_owned()) {
        Ok(())
    } else {
        Err(format!(
            "{}'s sign-in isn't waiting for a code. Sign in again.",
            provider.label()
        ))
    }
}

/// The code as the page shows it (`code#state`), trimmed. Claude Code answers a part of one
/// only on stderr and goes on waiting, so a part is refused here, where it can be said.
fn whole_code(code: &str) -> Result<&str, String> {
    let code = code.trim();
    if code.is_empty() {
        return Err("Paste the code from the page first.".into());
    }
    match code.split_once('#') {
        Some((code_part, state)) if !code_part.is_empty() && !state.is_empty() => Ok(code),
        _ => Err(
            "That isn't the whole code. Copy all of it from the page, then paste it again.".into(),
        ),
    }
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
    /// Where the agent keeps its login, watched while it signs in; `None`: it is asked every
    /// `unwatched_poll` instead.
    mark: Arc<dyn Fn(ProviderKind) -> Option<LoginMark> + Send + Sync>,
    unwatched_poll: Duration,
    /// Once signed in, how long its sign-in may take to end by itself.
    linger: Duration,
    /// Opens the vendor's sign-in page in the browser.
    #[cfg(any(unix, windows))]
    open_link: Arc<dyn Fn(&str) + Send + Sync>,
}

impl Ops {
    fn of(daemon: &Daemon) -> Self {
        let runtime = daemon.runtime.clone();
        let env = runtime.cli_env().clone();
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
            mark: Arc::new(move |provider| LoginMark::of(provider, &env)),
            unwatched_poll: UNWATCHED_POLL,
            linger: LINGER,
            #[cfg(any(unix, windows))]
            open_link: Arc::new(open_in_browser),
        }
    }
}

/// Opens `link` in the user's browser with their account's home, whatever `HOME` Brigadier
/// itself runs with: a browser started with another home can't find its keychain.
#[cfg(unix)]
pub(crate) fn open_in_browser(link: &str) {
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

/// Opens the sign-in page in the user's normal Windows browser. Passing it as an argument
/// keeps the URL out of a command shell (OAuth links contain `&` and percent escapes).
#[cfg(windows)]
pub(crate) fn open_in_browser(link: &str) {
    let mut command = std::process::Command::new("rundll32.exe");
    command
        .arg("url.dll,FileProtocolHandler")
        .arg(link)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    match command.spawn() {
        Ok(mut child) => drop(std::thread::spawn(move || child.wait())),
        Err(error) => tracing::warn!(%error, "the sign-in page didn't open"),
    }
}

/// The Windows sign-in's `BROWSER` is this binary with only a URL argument. It exits before
/// any daemon setup. An executable avoids command-file expansion of `%` in OAuth URLs.
#[cfg(windows)]
pub(crate) fn catch_browser_link() -> Option<std::process::ExitCode> {
    use std::io::Write;

    let path = std::env::var_os("BRIGADIER_SIGN_IN_LINK_FILE")?;
    let mut args = std::env::args().skip(1);
    let result = (|| -> std::io::Result<()> {
        let link = args
            .next()
            .filter(|link| {
                (link.starts_with("https://") || link.starts_with("http://"))
                    && !link.contains(['\r', '\n'])
            })
            .filter(|_| args.next().is_none())
            .ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidInput, "expected a sign-in URL")
            })?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(file, "{link}")
    })();
    Some(if result.is_ok() {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    })
}

/// A `BROWSER` for the sign-in that only passes the link back: the script, and the file the
/// link lands in. Removed when dropped.
#[cfg(any(unix, windows))]
struct LinkCatcher {
    dir: PathBuf,
    #[cfg(windows)]
    browser: PathBuf,
}

#[cfg(any(unix, windows))]
impl LinkCatcher {
    fn new() -> Option<Self> {
        #[cfg(unix)]
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("brigadier-sign-in-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).ok()?;
        #[cfg(windows)]
        let browser = std::env::current_exe().ok()?;
        // CI stages the real daemon before unit tests. A test harness cannot handle a URL
        // argument, so Windows tests use that executable beside the test's deps folder.
        #[cfg(all(windows, test))]
        let browser = browser.parent()?.parent()?.join("brigadierd.exe");
        let catcher = Self {
            dir,
            #[cfg(windows)]
            browser,
        };
        #[cfg(unix)]
        {
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
        }
        Some(catcher)
    }

    fn script(&self) -> PathBuf {
        #[cfg(unix)]
        {
            self.dir.join("browser")
        }
        #[cfg(windows)]
        {
            self.browser.clone()
        }
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

#[cfg(any(unix, windows))]
impl Drop for LinkCatcher {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// How a Connect ended: its last step, and when connected, the status that says so.
#[derive(Debug)]
struct Ended {
    progress: ConnectProgress,
    status: Option<ProviderStatus>,
}

impl Ended {
    fn connected(status: ProviderStatus) -> Self {
        Self {
            progress: ConnectProgress::Connected,
            status: Some(status),
        }
    }

    fn failed(step: ConnectStep, error: String) -> Self {
        Self {
            progress: ConnectProgress::Failed { step, error },
            status: None,
        }
    }
}

/// A sign-in's way back besides the browser: `offer` is told the page that shows a code to
/// paste, as the sign-in prints it, and codes pasted from it arrive on `pasted`.
struct Codes {
    offer: Box<dyn Fn(String) + Send + Sync>,
    pasted: mpsc::UnboundedReceiver<String>,
}

/// Connects `provider`: installs it when it is missing, then signs it in unless it is. Each
/// step is reported as it begins. Answers how it ended, or `None` once cancelled.
async fn connect(
    ops: &Ops,
    provider: ProviderKind,
    cancel: &CancellationToken,
    report: &(dyn Fn(ConnectProgress) + Send + Sync),
    codes: &mut Codes,
) -> Option<Ended> {
    let failed = |step, error: String| Some(Ended::failed(step, error));
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
        return Some(Ended::connected(status));
    }
    if cancel.is_cancelled() {
        return None;
    }
    report(ConnectProgress::SigningIn);
    let binary = PathBuf::from(status.path.unwrap_or_default());
    sign_in(ops, provider, binary, cancel, codes).await
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
            .installer_spec(&PathBuf::from("powershell.exe"))
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-Command")
            .arg(format!("$ErrorActionPreference = 'Stop'; {line}"))
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

/// Runs the vendor's sign-in (it opens the browser) and answers once the agent says it is
/// signed in, the sign-in ends without it, or it is given up on. The agent is asked the moment
/// where it keeps its login changes: Claude Code stores it seconds before it says so. Asked too
/// each time the sign-in prints a line (Claude Code's "Login successful.", Codex's
/// "Successfully logged in" on stderr; Claude Code then takes a moment more to exit), when it
/// ends, and every `poll` besides (every `unwatched_poll` when the login can't be watched).
async fn sign_in(
    ops: &Ops,
    provider: ProviderKind,
    binary: PathBuf,
    cancel: &CancellationToken,
    codes: &mut Codes,
) -> Option<Ended> {
    let label = provider.label();
    let failed = |error: String| Some(Ended::failed(ConnectStep::SignIn, error));
    let mut spec = ops.env.spec(&binary);
    spec.args = provider
        .login_command()
        .split(' ')
        .skip(1)
        .map(Into::into)
        .collect();
    spec.cwd = ops.env.home();
    // Where it keeps its login as it is before the sign-in starts, to see it stored.
    let mark = (ops.mark)(provider);
    let mut stamp = match &mark {
        Some(mark) => mark.read().await,
        None => None,
    };
    let watched = stamp.is_some();
    #[cfg(any(unix, windows))]
    let catcher = LinkCatcher::new();
    #[cfg(any(unix, windows))]
    if let Some(catcher) = &catcher {
        spec = spec.env("BROWSER", catcher.script());
        #[cfg(windows)]
        {
            spec = spec.env("BRIGADIER_SIGN_IN_LINK_FILE", catcher.dir.join("link"));
        }
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
    // Its lines aren't shown: each only says the agent is worth asking now.
    let mut lines = spawned.stdout;
    let mut stdout_open = true;
    let mut errors = child.stderr_lines();
    let mut stderr_open = true;
    let mut offered = false;
    let mut pasted_open = true;
    // A code was passed on: a sign-in that ends without signing in didn't take it.
    let mut pasted = false;
    // The link it wants opened, opened by Brigadier the moment it arrives.
    #[cfg(any(unix, windows))]
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
    let every = if watched {
        ops.poll
    } else {
        tracing::info!(%provider, "its login can't be watched; asking it more often");
        ops.poll.min(ops.unwatched_poll)
    };
    let mut poll = tokio::time::interval_at(tokio::time::Instant::now() + every, every);
    poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut look = tokio::time::interval(MARK_EVERY);
    look.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let outcome = loop {
        tokio::select! {
            biased;
            () = cancel.cancelled() => break None,
            exit = child.exited() => {
                // It ends by itself once signed in; one that ends any other way didn't sign in.
                if let Some(status) = settled(ops, provider, exit.code == Some(0)).await {
                    break Some(Ended::connected(status));
                }
                tracing::info!(%provider, code = ?exit.code, stderr = ?child.stderr_tail(), "the sign-in ended without signing in");
                if pasted {
                    break failed("That code didn't work. Try again.".into());
                }
                break failed(format!("Signing in to {label} didn't finish. Try again."));
            }
            () = &mut given_up => {
                break failed(format!("Signing in to {label} took too long. Try again."));
            }
            line = lines.recv(), if stdout_open => {
                let Some(line) = line else {
                    stdout_open = false;
                    continue;
                };
                // Lines that came together are one reason to ask.
                for line in std::iter::once(line).chain(std::iter::from_fn(|| lines.try_recv().ok())) {
                    if !offered && let Some(page) = code_page(provider, &line) {
                        tracing::info!(%provider, "the sign-in offers a code to paste");
                        offered = true;
                        (codes.offer)(page);
                    }
                }
            }
            code = codes.pasted.recv(), if pasted_open => {
                let Some(code) = code else {
                    pasted_open = false;
                    continue;
                };
                tracing::info!(%provider, "passing a pasted code to the sign-in");
                if let Err(error) = child.write_line(&code).await {
                    tracing::warn!(%provider, %error, "the sign-in didn't take the code");
                }
                pasted = true;
                continue;
            }
            changed = errors.changed(), if stderr_open => {
                if changed.is_err() {
                    stderr_open = false;
                    continue;
                }
            }
            _ = poll.tick() => {}
            _ = look.tick(), if watched => {
                let now = match &mark {
                    Some(mark) => mark.read().await,
                    None => None,
                };
                // Unchanged (or unreadable this time): nothing to ask yet.
                if now.is_none() || now == stamp {
                    continue;
                }
                tracing::info!(%provider, "its login was just stored; asking it");
                stamp = now;
                // Its status may follow the store by a moment: asked a few times.
                if let Some(status) = settled(ops, provider, true).await {
                    break Some(Ended::connected(status));
                }
                continue;
            }
        }
        let status = (ops.status)(provider).await;
        if status.logged_in {
            break Some(Ended::connected(status));
        }
    };
    #[cfg(any(unix, windows))]
    opener.abort();
    if child.is_running() {
        if outcome.as_ref().is_some_and(|ended| ended.status.is_some()) {
            // Signed in: it ends by itself once it has sent what it sends (Claude Code's take
            // seconds); stopped only if it doesn't. Read meanwhile, so it never blocks.
            let linger = ops.linger;
            tokio::spawn(async move {
                let drain = async { while lines.recv().await.is_some() {} };
                let ended = tokio::time::timeout(linger, async {
                    tokio::join!(drain, child.exited());
                });
                if ended.await.is_err() {
                    child.shutdown(STOP_GRACE).await;
                }
            });
        } else {
            child.shutdown(STOP_GRACE).await;
        }
    }
    outcome
}

/// The page Claude Code's sign-in prints for a code to paste back ("If the browser didn't open,
/// visit: …", a terminal link): its first `https` address. Codex's sign-in prints only the
/// browser's own page, which hands no code back.
fn code_page(provider: ProviderKind, line: &str) -> Option<String> {
    if provider != ProviderKind::Claude {
        return None;
    }
    let start = line.find("https://")?;
    let page: String = line[start..]
        .chars()
        .take_while(|c| !c.is_whitespace() && !c.is_control())
        .collect();
    Some(page)
}

/// The agent's status once its sign-in ended or its login was stored: it may say so a moment
/// after, so when `patient` it is asked a few times. Some when signed in.
async fn settled(ops: &Ops, provider: ProviderKind, patient: bool) -> Option<ProviderStatus> {
    let tries = if patient { SETTLE_TRIES } else { 1 };
    for attempt in 0..tries {
        if attempt > 0 {
            tokio::time::sleep(SETTLE_EVERY).await;
        }
        let status = (ops.status)(provider).await;
        if status.logged_in {
            return Some(status);
        }
    }
    None
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
    /// `wait` (never signs in, never ends), `linger` (as Claude Code's: signs in, says so, then
    /// takes seconds to end), `quick` (as Codex's: signs in, says so on stderr, ends), `stored`
    /// (as Claude Code's really does: stores the login, says so only 3 s later, then ends), `late`
    /// (ends well, signed in only by the third time it is asked after), `never` (ends well, never signs in),
    /// `paste` (as Claude Code's when the browser can't come back: prints its code page as a
    /// terminal link, then signs in once `good#state` is pasted, fails on any other code).
    /// Where it keeps its login, the file `signed-in`, is what the tests' [`LoginMark`] watches.
    const FAKE_CLAUDE: &str = r#"#!/bin/sh
home="$HOME"
now() { perl -MTime::HiRes=time -e 'printf "%.3f", time'; }
# The moment of the agent's own confirmation, whole before the login can be seen.
confirm() { now > "$home/confirmed-at.part"; mv "$home/confirmed-at.part" "$home/confirmed-at"; }
case "$1 $2" in
  "--version "*) echo "2.1.302 (Claude Code)" ;;
  "auth status")
    if [ -f "$home/late" ]; then
      echo asked >> "$home/late"
      [ "$(wc -l < "$home/late")" -ge 3 ] && rm "$home/late" && touch "$home/signed-in"
    fi
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
      linger) sleep 1; confirm; touch "$home/signed-in"; echo "Login successful."; sleep 6 ;;
      late) touch "$home/late"; exit 0 ;;
      never) exit 0 ;;
      quick) sleep 1; confirm; touch "$home/signed-in"; echo "Successfully logged in" >&2 ;;
      stored) sleep 1; now > "$home/stored-at"; touch "$home/signed-in"; sleep 3; echo "Login successful."; sleep 1 ;;
      paste)
        esc=$(printf '\033'); bel=$(printf '\007'); page="https://example.test/code?x=1"
        echo "If the browser didn't open, visit: ${esc}]8;;${page}${bel}${page}${esc}]8;;${bel}"
        read -r code
        if [ "$code" = "good#state" ]; then touch "$home/signed-in"; echo "Login successful."
        else echo "Login failed: Request failed with status code 400"; exit 1; fi ;;
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
        let stored = home.0.join("signed-in");
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
            mark: Arc::new(move |_| Some(LoginMark::file(&stored))),
            unwatched_poll: UNWATCHED_POLL,
            linger: Duration::from_millis(500),
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
            connect(ops, ProviderKind::Claude, &cancel, &report, &mut no_codes()),
        )
        .await
        .expect("Connect ends")
        .map(|ended| ended.progress);
        let seen = seen.lock().unwrap().clone();
        (outcome, seen)
    }

    /// Nothing offered, nothing pasted.
    fn no_codes() -> Codes {
        Codes {
            offer: Box::new(|_| {}),
            pasted: mpsc::unbounded_channel().1,
        }
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

    /// When the browser can't hand the sign-in back, Claude Code's code page is offered and a
    /// code pasted from it reaches the sign-in: one it takes signs in, one it doesn't says so.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_code_pasted_from_claude_codes_page_signs_in() {
        for (code, outcome) in [
            (
                "bad#state",
                failed(ConnectStep::SignIn, "That code didn't work. Try again."),
            ),
            ("good#state", Some(ConnectProgress::Connected)),
        ] {
            let home = Home::new("paste");
            std::fs::write(home.0.join("login-mode"), "paste").unwrap();
            let ops = ops(&home);
            let (sender, pasted) = mpsc::unbounded_channel();
            let offered = Arc::new(Mutex::new(None));
            let mut codes = Codes {
                // Pasted the moment the page is offered.
                offer: Box::new({
                    let offered = offered.clone();
                    move |page| {
                        *offered.lock().unwrap() = Some(page);
                        sender.send(code.to_owned()).unwrap();
                    }
                }),
                pasted,
            };
            let cancel = CancellationToken::new();
            let ended = tokio::time::timeout(
                Duration::from_secs(40),
                connect(&ops, ProviderKind::Claude, &cancel, &|_| {}, &mut codes),
            )
            .await
            .expect("Connect ends")
            .map(|ended| ended.progress);
            assert_eq!(ended, outcome, "{code}");
            assert_eq!(
                offered.lock().unwrap().as_deref(),
                Some("https://example.test/code?x=1")
            );
        }
    }

    /// Only a whole code (`code#state`, as the page shows it) is passed on; a part is refused
    /// with why, since Claude Code would only say so on stderr and go on waiting.
    #[test]
    fn only_a_whole_code_is_passed_on() {
        assert_eq!(whole_code("  abc#xyz \n"), Ok("abc#xyz"));
        for part in ["abc", "abc#", "#xyz"] {
            assert_eq!(
                whole_code(part),
                Err(
                    "That isn't the whole code. Copy all of it from the page, then paste it again."
                        .into()
                )
            );
        }
        assert_eq!(
            whole_code("  "),
            Err("Paste the code from the page first.".into())
        );
    }

    /// Codex's sign-in prints only the browser's own page: no code page is offered for it.
    #[test]
    fn only_claude_codes_sign_in_offers_a_code_page() {
        let line = "If the browser didn't open, visit: \u{1b}]8;;https://claude.com/cai/oauth/authorize?code=true\u{7}https://claude.com/x\u{1b}]8;;\u{7}";
        assert_eq!(
            code_page(ProviderKind::Claude, line).as_deref(),
            Some("https://claude.com/cai/oauth/authorize?code=true")
        );
        assert_eq!(
            code_page(ProviderKind::Claude, "Opening browser to sign in…"),
            None
        );
        assert_eq!(
            code_page(
                ProviderKind::Codex,
                "If your browser did not open, navigate to this URL to authenticate: https://auth.example.test/oauth"
            ),
            None
        );
    }

    /// A code goes to the running sign-in only once it offered its page, and the page goes
    /// from the view as the sign-in ends.
    #[test]
    fn a_code_reaches_only_a_sign_in_that_offered_its_page() {
        let connects = Connects::default();
        let (attempt, _cancel) = connects
            .begin(ProviderKind::Claude, ConnectProgress::SigningIn)
            .unwrap();
        let mut pasted = connects.listen(ProviderKind::Claude, attempt).unwrap();
        assert!(!connects.send_code(ProviderKind::Claude, "early".into()));
        assert!(!connects.offer_code_page(ProviderKind::Claude, attempt + 1, "https://old".into()));
        assert!(connects.offer_code_page(ProviderKind::Claude, attempt, "https://page".into()));
        assert_eq!(
            connects.view().agents[0].code_page.as_deref(),
            Some("https://page")
        );
        assert!(!connects.send_code(ProviderKind::Codex, "other".into()));
        assert!(connects.send_code(ProviderKind::Claude, "code#state".into()));
        assert_eq!(pasted.try_recv().ok().as_deref(), Some("code#state"));
        assert!(connects.report(ProviderKind::Claude, attempt, ConnectProgress::Connected));
        assert_eq!(connects.view().agents[0].code_page, None);
        assert!(!connects.send_code(ProviderKind::Claude, "late".into()));
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
        // Given its while to end by itself, then stopped.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while sign_in_runs(&home) {
            assert!(
                std::time::Instant::now() < deadline,
                "the sign-in process still runs"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    fn sign_in_runs(home: &Home) -> bool {
        let found = std::process::Command::new("pgrep")
            .args([
                "-f",
                &home.0.join(".local/bin/claude").display().to_string(),
            ])
            .output()
            .unwrap();
        !found.stdout.is_empty()
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
            connect(
                &ops,
                ProviderKind::Claude,
                &cancel,
                &|_| {},
                &mut no_codes(),
            ),
        )
        .await
        .expect("cancel ends it");
        assert!(outcome.is_none());
        assert!(!sign_in_runs(&home), "the sign-in process still runs");
    }

    /// The real sign-ins once the page is confirmed: Claude Code stores the login, prints
    /// "Login successful." and takes seconds more to exit (`linger`); Codex prints its line on
    /// stderr and exits (`quick`). Connected follows at once, without waiting for the exit or
    /// the next check: the slow check here never comes in time.
    #[tokio::test(flavor = "multi_thread")]
    async fn connected_follows_the_agents_own_confirmation_at_once() {
        for mode in ["linger", "quick"] {
            let home = Home::new(mode);
            std::fs::write(home.0.join("login-mode"), mode).unwrap();
            let mut ops = ops(&home);
            ops.poll = Duration::from_secs(60);
            let (outcome, _) = run(&ops).await;
            let done = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();
            assert_eq!(outcome, Some(ConnectProgress::Connected));
            let confirmed: f64 = std::fs::read_to_string(home.0.join("confirmed-at"))
                .unwrap()
                .parse()
                .unwrap();
            let lag = done - confirmed;
            eprintln!("{mode}: Connected {lag:.3}s after the agent's confirmation");
            assert!(lag < 1.0, "{mode}: Connected {lag:.3}s after");
        }
    }

    /// Claude Code stores the login seconds before it prints "Login successful." (6.8 s vs.
    /// 10.6 s in a real run). Connected follows the stored login within 0.3 s, with the slow
    /// check never coming in time; without a login to watch, the agent is asked every
    /// `unwatched_poll`. Each lag is printed, beside how it was before (no watch, a 3 s check:
    /// Connected at the success line).
    #[tokio::test(flavor = "multi_thread")]
    async fn connected_follows_the_stored_login_within_the_target() {
        let lag = |home: &Home| {
            let done = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs_f64();
            let stored: f64 = std::fs::read_to_string(home.0.join("stored-at"))
                .unwrap()
                .parse()
                .unwrap();
            done - stored
        };
        let cases: [(&str, bool, Duration, Duration, f64); 3] = [
            (
                "watched",
                true,
                Duration::from_secs(60),
                UNWATCHED_POLL,
                0.3,
            ),
            (
                "unwatched",
                false,
                Duration::from_secs(60),
                UNWATCHED_POLL,
                1.0,
            ),
            ("as before", false, POLL_EVERY, POLL_EVERY, 3.5),
        ];
        for (name, watched, poll, unwatched_poll, target) in cases {
            let home = Home::new("stored");
            std::fs::write(home.0.join("login-mode"), "stored").unwrap();
            let mut ops = ops(&home);
            // Installed first, so only the sign-in is timed.
            let (outcome, _) = run(&ops).await;
            assert_eq!(outcome, Some(ConnectProgress::Connected));
            std::fs::remove_file(home.0.join("signed-in")).unwrap();
            ops.poll = poll;
            ops.unwatched_poll = unwatched_poll;
            if !watched {
                ops.mark = Arc::new(|_| None);
            }
            let (outcome, _) = run(&ops).await;
            let lag = lag(&home);
            assert_eq!(outcome, Some(ConnectProgress::Connected));
            eprintln!("{name}: Connected {lag:.3}s after the login was stored");
            assert!(lag < target, "{name}: Connected {lag:.3}s after");
        }
    }

    /// An extra account's sign-in, run in a terminal: the account is checked the moment its
    /// login is stored, again while it doesn't read as signed in yet, and once more when the
    /// sign-in ends; one whose login can't be watched is checked every `unwatched_poll` until
    /// it reads as signed in.
    #[tokio::test(flavor = "multi_thread")]
    async fn an_accounts_sign_in_is_checked_the_moment_its_login_is_stored() {
        let home = Home::new("account");
        let stored = home.0.join("auth.json");
        let mark = LoginMark::file(&stored);
        let before = mark.read().await;
        let (ended, exited) = tokio::sync::oneshot::channel::<()>();
        let checked = Arc::new(Mutex::new(Vec::new()));
        let follow = tokio::spawn({
            let (checked, read) = (checked.clone(), checked.clone());
            follow_sign_in(
                move || checked.lock().unwrap().push(std::time::Instant::now()),
                // Its status follows the store by a moment: it reads as signed in from the
                // second check on.
                move || read.lock().unwrap().len() >= 2,
                Some(mark),
                before,
                async move {
                    let _ = exited.await;
                },
                Duration::from_secs(60),
            )
        });
        tokio::time::sleep(Duration::from_millis(400)).await;
        assert!(
            checked.lock().unwrap().is_empty(),
            "checked before anything was stored"
        );
        std::fs::write(&stored, "{}").unwrap();
        let at = std::time::Instant::now();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let first = *checked
            .lock()
            .unwrap()
            .first()
            .expect("checked once stored");
        let lag = first.duration_since(at);
        eprintln!(
            "account: checked {:.3}s after its login was stored",
            lag.as_secs_f64()
        );
        assert!(lag < Duration::from_millis(250));
        // Checked again until it reads as signed in, then left alone.
        let settled = tokio::time::Instant::now() + Duration::from_secs(5);
        while checked.lock().unwrap().len() < 2 && tokio::time::Instant::now() < settled {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert_eq!(checked.lock().unwrap().len(), 2);
        // And once more as the sign-in ends.
        ended.send(()).unwrap();
        follow.await.unwrap();
        assert_eq!(checked.lock().unwrap().len(), 3);

        // Not watched: asked every `unwatched_poll` until it reads as signed in.
        let (ended, exited) = tokio::sync::oneshot::channel::<()>();
        let asked = Arc::new(AtomicU64::new(0));
        let follow = tokio::spawn({
            let (count, read) = (asked.clone(), asked.clone());
            follow_sign_in(
                move || {
                    count.fetch_add(1, Ordering::Relaxed);
                },
                move || read.load(Ordering::Relaxed) >= 3,
                None,
                None,
                async move {
                    let _ = exited.await;
                },
                Duration::from_millis(100),
            )
        });
        tokio::time::sleep(Duration::from_millis(800)).await;
        assert_eq!(asked.load(Ordering::Relaxed), 3);
        ended.send(()).unwrap();
        follow.await.unwrap();
        assert_eq!(asked.load(Ordering::Relaxed), 4);
    }

    /// A sign-in that ends well a moment before its login is stored still connects; one that
    /// ends well without ever storing it fails, after a short wait.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_sign_in_that_ends_well_waits_a_moment_for_its_login() {
        let home = Home::new("late");
        std::fs::write(home.0.join("login-mode"), "late").unwrap();
        let mut ops = ops(&home);
        ops.poll = Duration::from_secs(60);
        let (outcome, _) = run(&ops).await;
        assert_eq!(outcome, Some(ConnectProgress::Connected));

        std::fs::remove_file(home.0.join("signed-in")).unwrap();
        std::fs::write(home.0.join("login-mode"), "never").unwrap();
        let (outcome, _) = run(&ops).await;
        assert_eq!(
            outcome,
            failed(
                ConnectStep::SignIn,
                "Signing in to Claude Code didn't finish. Try again."
            )
        );
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

#[cfg(all(test, windows))]
mod windows_tests;
