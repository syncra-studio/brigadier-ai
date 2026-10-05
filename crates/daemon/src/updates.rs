//! Newer versions of Brigadier and the agent CLIs, for the rail's update button: a minute after
//! launch and then every six hours, and again after an update.
//!
//! - Brigadier: the repository's latest GitHub release against this build's version. Its
//!   Update opens the release's page.
//! - Each agent CLI: the version it reports against the newest one its own updater would
//!   install. How it was installed (its real path) picks both the source of that version and
//!   the command Update runs: the CLI's own `update` for its native installer, the package
//!   manager's for npm, pnpm, bun or Homebrew. Claude Code follows its release channel
//!   (`autoUpdatesChannel`), and offers nothing its settings forbid installing.
//!
//! A lookup that fails (offline, an error page) finds nothing to update: no error is shown.
//! Results and each update's progress are published as `updatesChanged`.
//!
//! Development builds can fake newer versions with `BRIGADIER_FAKE_UPDATES=app,claude,codex`.
//! A faked CLI's Update runs the program `BRIGADIER_FAKE_UPDATE_RUN` names (with the CLI's name
//! as its argument) instead of anything real, so a stub can stand in for success or failure.

use std::collections::HashSet;
use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use brigadier_core::{
    DomainEvent, UpdateAction, UpdateItem, UpdateProgress, UpdateTarget, UpdatesView, now_ms,
    streams,
};
use brigadier_providers::ProviderKind;
use brigadier_providers::cli::{CliEnv, parse_version};
use brigadier_providers::process;
use brigadier_store::{NewEvent, Retention};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::server::Daemon;

/// The first check after launch, then how often.
const FIRST_CHECK: Duration = Duration::from_secs(60);
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// One lookup on the network, or one `brew info`.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(5);
const BREW_TIMEOUT: Duration = Duration::from_secs(10);
/// An update itself.
const UPDATE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// A CLI's `--version` after its update.
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
/// Answers larger than this aren't a version.
const MAX_ANSWER_BYTES: u64 = 256 * 1024;

const RELEASES_URL: &str =
    "https://api.github.com/repos/syncra-studio/brigadier-ai/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/syncra-studio/brigadier-ai/releases";

/// The update view and the update running now.
#[derive(Default)]
pub struct Updates {
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    view: UpdatesView,
    running: Option<UpdateTarget>,
    /// Faked targets whose (stub) update succeeded: current from then on.
    faked_done: HashSet<UpdateTarget>,
}

impl Updates {
    pub fn view(&self) -> UpdatesView {
        self.state().view.clone()
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Checks for newer versions on schedule, until the daemon stops.
pub async fn keep_current(daemon: Arc<Daemon>, stop: CancellationToken) -> anyhow::Result<()> {
    let mut wait = FIRST_CHECK;
    loop {
        tokio::select! {
            () = stop.cancelled() => return Ok(()),
            () = tokio::time::sleep(wait) => {}
        }
        wait = CHECK_EVERY;
        check(&daemon).await;
    }
}

/// Looks for newer versions now and publishes what it finds.
pub async fn check(daemon: &Daemon) {
    let faked = faked_targets();
    let done = daemon.updates.state().faked_done.clone();
    let env = daemon.runtime.cli_env().clone();
    let mut found = Vec::new();

    if faked.contains(&UpdateTarget::App) {
        if !done.contains(&UpdateTarget::App) {
            found.push(fake_item(
                UpdateTarget::App,
                env!("CARGO_PKG_VERSION"),
                None,
            ));
        }
    } else if let Some(item) = blocking(app_update).await {
        found.push(item);
    }

    for kind in ProviderKind::ALL {
        let target = target_of(kind);
        let Some(status) = daemon.runtime.overview(kind).and_then(|o| o.status) else {
            continue;
        };
        let (Some(path), Some(version)) = (status.path, status.version) else {
            continue;
        };
        if faked.contains(&target) {
            if !done.contains(&target) {
                found.push(fake_item(target, &version, fake_runner()));
            }
            continue;
        }
        let platform = daemon.runtime.platform().clone();
        let env = env.clone();
        let path = PathBuf::from(path);
        if let Some(item) = cli_update(&platform, &env, kind, &path, &version).await {
            found.push(item);
        }
    }

    let view = {
        let mut state = daemon.updates.state();
        // An update running keeps its row as it is.
        if let Some(running) = state.running
            && let Some(row) = state.view.items.iter().find(|i| i.target == running)
        {
            let row = row.clone();
            found.retain(|item| item.target != running);
            found.push(row);
        }
        found.sort_by_key(|item| item.target as u8);
        state.view = UpdatesView {
            items: found,
            checked_at_ms: Some(now_ms()),
        };
        state.view.clone()
    };
    publish(daemon, view).await;
}

/// Starts updating `target` in the background. Refused while another update runs, or when
/// Brigadier can't run this one.
pub fn start(daemon: &Arc<Daemon>, target: UpdateTarget) -> Result<(), String> {
    let (item, view) = {
        let mut state = daemon.updates.state();
        if state.running.is_some() {
            return Err("another update is running; try again when it finishes".into());
        }
        let Some(item) = state.view.items.iter_mut().find(|i| i.target == target) else {
            return Err("there's no update for this".into());
        };
        if !matches!(item.action, UpdateAction::Run { .. }) {
            return Err("Brigadier can't run this update".into());
        }
        item.progress = UpdateProgress::Updating;
        let item = item.clone();
        state.running = Some(target);
        (item, state.view.clone())
    };
    let daemon = daemon.clone();
    tokio::spawn(async move {
        publish(&daemon, view).await;
        let (current, progress) = run_update(&daemon, &item).await;
        let view = {
            let mut state = daemon.updates.state();
            state.running = None;
            if matches!(progress, UpdateProgress::Updated) && faked_targets().contains(&target) {
                state.faked_done.insert(target);
            }
            if let Some(row) = state.view.items.iter_mut().find(|i| i.target == target) {
                row.current = current;
                row.progress = progress;
            }
            state.view.clone()
        };
        if let Some(kind) = provider_of(target) {
            daemon.runtime.refresh_providers(Some(kind));
        }
        publish(&daemon, view).await;
    });
    Ok(())
}

/// Runs the update and reads the version after: the version then, and how it went.
async fn run_update(daemon: &Daemon, item: &UpdateItem) -> (String, UpdateProgress) {
    let failed = |error: String| (item.current.clone(), UpdateProgress::Failed { error });
    let Some(kind) = provider_of(item.target) else {
        return failed("Brigadier can't run this update".into());
    };
    let faked = faked_targets().contains(&item.target);
    let env = daemon.runtime.cli_env().clone();
    let (program, args) = if faked {
        match fake_runner() {
            Some(stub) => (stub, vec![OsString::from(kind.binary())]),
            None => return failed("set BRIGADIER_FAKE_UPDATE_RUN to a program to fake it".into()),
        }
    } else {
        let Some(path) = daemon
            .runtime
            .overview(kind)
            .and_then(|o| o.status)
            .and_then(|s| s.path)
        else {
            return failed(format!("{} isn't installed", kind.label()));
        };
        match update_command(&env, kind, Path::new(&path)) {
            Some(command) => command,
            None => return failed("Brigadier can't run this update".into()),
        }
    };
    let mut spec = env.spec(&program);
    spec.args = args;
    spec.cwd = env.home();
    tracing::info!(target = ?item.target, program = %program.display(), "updating");
    let platform = daemon.runtime.platform().clone();
    match process::run(&platform, &spec, UPDATE_TIMEOUT).await {
        Ok(output) if output.code == Some(0) => {}
        Ok(output) => {
            let error = last_lines(&output.stderr)
                .or_else(|| last_lines(&output.stdout))
                .unwrap_or_else(|| match output.code {
                    Some(code) => format!("the update stopped with code {code}"),
                    None => "the update was stopped".into(),
                });
            tracing::warn!(target = ?item.target, %error, "update failed");
            return failed(error);
        }
        Err(err) => {
            tracing::warn!(target = ?item.target, error = %err, "update failed");
            return failed(format!("the update couldn't finish: {err}"));
        }
    }
    if faked {
        return (item.latest.clone(), UpdateProgress::Updated);
    }
    // The update said it worked: the CLI must say so too.
    let Some(path) = daemon
        .runtime
        .overview(kind)
        .and_then(|o| o.status)
        .and_then(|s| s.path)
    else {
        return failed(format!("{} can't be found after the update", kind.label()));
    };
    let mut spec = env.spec(Path::new(&path));
    spec.args = vec!["--version".into()];
    let now = process::run(&platform, &spec, VERSION_TIMEOUT)
        .await
        .ok()
        .and_then(|output| parse_version(&output.stdout));
    match now {
        Some(now) if !is_newer(&item.latest, &now) => (now, UpdateProgress::Updated),
        Some(now) => (
            now.clone(),
            UpdateProgress::Failed {
                error: format!(
                    "the update finished, but {} still reports {now}",
                    kind.label()
                ),
            },
        ),
        None => failed(format!(
            "the update finished, but {} didn't report its version",
            kind.label()
        )),
    }
}

async fn publish(daemon: &Daemon, updates: UpdatesView) {
    let event = DomainEvent::UpdatesChanged { updates };
    let result = async {
        let new = NewEvent::new(streams::UPDATES, event.kind(), now_ms(), &event)?;
        daemon
            .store
            .append_with(vec![new], Some(Retention { keep_last: 1 }))
            .await?;
        Ok::<_, brigadier_store::Error>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(%error, "could not publish updates.changed");
    }
}

// ----- Brigadier -------------------------------------------------------------------------

/// The latest release, when it's newer than this build.
fn app_update() -> Option<UpdateItem> {
    let release = get_json(RELEASES_URL)?;
    let tag = release.get("tag_name")?.as_str()?;
    let latest = tag.trim_start_matches('v');
    let current = env!("CARGO_PKG_VERSION");
    if !is_newer(latest, current) {
        return None;
    }
    let url = release
        .get("html_url")
        .and_then(Value::as_str)
        .filter(|url| url.starts_with("https://github.com/"))
        .unwrap_or(RELEASES_PAGE);
    Some(UpdateItem {
        target: UpdateTarget::App,
        current: current.into(),
        latest: latest.into(),
        action: UpdateAction::Download { url: url.into() },
        progress: UpdateProgress::Available,
    })
}

// ----- agent CLIs ------------------------------------------------------------------------

/// How a CLI was installed, from where its program really is.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Install {
    /// The CLI's own installer: its `update` knows where it lives.
    Native,
    /// npm's global folder; `prefix` is the folder npm installs into.
    Npm {
        prefix: PathBuf,
    },
    Pnpm,
    Bun,
    Homebrew {
        cask: bool,
        name: String,
    },
    /// A version manager (Volta, mise, asdf) or an unknown package folder.
    Other,
}

fn install_of(real: &Path, package: &str) -> Install {
    let path = real.to_string_lossy().replace('\\', "/");
    for (marker, cask) in [("/Caskroom/", true), ("/Cellar/", false)] {
        if let Some((_, rest)) = path.split_once(marker)
            && let Some(name) = rest.split('/').next().filter(|name| !name.is_empty())
        {
            return Install::Homebrew {
                cask,
                name: name.into(),
            };
        }
    }
    if ["/.volta/", "/mise/", "/.asdf/"]
        .iter()
        .any(|marker| path.contains(marker))
    {
        return Install::Other;
    }
    if path.contains("/.bun/") {
        return Install::Bun;
    }
    if path.contains("/node_modules/") && (path.contains("/pnpm/") || path.contains("/.pnpm/")) {
        return Install::Pnpm;
    }
    if let Some((before, _)) = path.split_once(&format!("/node_modules/{package}/")) {
        // `<prefix>/lib/node_modules` on macOS and Linux, `<prefix>/node_modules` on Windows.
        let prefix = before.strip_suffix("/lib").unwrap_or(before);
        return Install::Npm {
            prefix: PathBuf::from(prefix),
        };
    }
    // npm's Windows shim sits beside the package folder, not in it.
    if let Some(dir) = real.parent()
        && dir.join("node_modules").join(package).is_dir()
    {
        return Install::Npm {
            prefix: dir.to_owned(),
        };
    }
    if path.contains("/node_modules/") {
        return Install::Other;
    }
    Install::Native
}

fn package_of(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Claude => "@anthropic-ai/claude-code",
        ProviderKind::Codex => "@openai/codex",
    }
}

/// The newer version of a CLI its install can take, and how to take it.
async fn cli_update(
    platform: &Arc<dyn brigadier_sandbox::Platform>,
    env: &Arc<CliEnv>,
    kind: ProviderKind,
    path: &Path,
    current: &str,
) -> Option<UpdateItem> {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let package = package_of(kind);
    let install = install_of(&real, package);
    let limits = match kind {
        ProviderKind::Claude => claude_limits(env),
        ProviderKind::Codex => Limits::default(),
    };
    if limits.updates_off {
        return None;
    }
    let latest = match &install {
        Install::Homebrew { cask, name } => brew_version(platform, env, *cask, name).await?,
        _ => {
            let url = format!("https://registry.npmjs.org/-/package/{package}/dist-tags");
            let tags = blocking(move || get_json(&url)).await?;
            tags.get(limits.channel)?.as_str()?.to_owned()
        }
    };
    if !is_newer(&latest, current)
        || limits
            .maximum
            .as_deref()
            .is_some_and(|max| is_newer(&latest, max))
    {
        return None;
    }
    let display = command_line(kind, path, &install, limits.channel);
    let action = if update_command(env, kind, path).is_some() {
        UpdateAction::Run { command: display }
    } else {
        UpdateAction::Manual { command: display }
    };
    Some(UpdateItem {
        target: target_of(kind),
        current: current.into(),
        latest,
        action,
        progress: UpdateProgress::Available,
    })
}

/// The program and arguments Brigadier runs to update the CLI at `path`, or `None` when the
/// user has to.
fn update_command(
    env: &CliEnv,
    kind: ProviderKind,
    path: &Path,
) -> Option<(PathBuf, Vec<OsString>)> {
    let real = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let package = package_of(kind);
    let channel = match kind {
        ProviderKind::Claude => claude_limits(env).channel,
        ProviderKind::Codex => "latest",
    };
    let spec = format!("{package}@{channel}");
    let args = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
    match install_of(&real, package) {
        Install::Native => Some((path.to_owned(), args(&["update"]))),
        Install::Npm { prefix } => {
            let mut words = args(&["install", "-g", "--prefix"]);
            words.push(prefix.into_os_string());
            words.push(spec.into());
            Some((env.which("npm")?, words))
        }
        Install::Pnpm => Some((env.which("pnpm")?, args(&["add", "-g", &spec]))),
        Install::Bun => Some((env.which("bun")?, args(&["add", "-g", &spec]))),
        Install::Homebrew { cask, name } => {
            let mut words = args(&["upgrade"]);
            if cask {
                words.push("--cask".into());
            }
            words.push(name.into());
            Some((env.which("brew")?, words))
        }
        Install::Other => None,
    }
}

/// The update command as the user would type it.
fn command_line(kind: ProviderKind, path: &Path, install: &Install, channel: &str) -> String {
    let spec = format!("{}@{channel}", package_of(kind));
    match install {
        Install::Native => {
            let program = path.file_stem().map_or_else(
                || kind.binary().into(),
                |s| s.to_string_lossy().into_owned(),
            );
            format!("{program} update")
        }
        Install::Npm { .. } | Install::Other => format!("npm install -g {spec}"),
        Install::Pnpm => format!("pnpm add -g {spec}"),
        Install::Bun => format!("bun add -g {spec}"),
        Install::Homebrew { cask: true, name } => format!("brew upgrade --cask {name}"),
        Install::Homebrew { cask: false, name } => format!("brew upgrade {name}"),
    }
}

/// What a CLI's settings allow its updater to install.
struct Limits {
    /// The npm dist-tag its updater follows.
    channel: &'static str,
    /// The newest version it may run.
    maximum: Option<String>,
    /// Updates are turned off altogether.
    updates_off: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            channel: "latest",
            maximum: None,
            updates_off: false,
        }
    }
}

/// Claude Code's release channel and limits, as `claude update` reads them: managed settings
/// first, then the user's (Brigadier runs the update from the home folder, outside any
/// project's settings).
fn claude_limits(env: &CliEnv) -> Limits {
    let read = |path: PathBuf| -> Value {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    };
    let managed = managed_claude_settings().map(read).unwrap_or_default();
    let user = match env.var("CLAUDE_CONFIG_DIR").filter(|dir| !dir.is_empty()) {
        Some(dir) => read(PathBuf::from(dir).join("settings.json")),
        None => env
            .home()
            .map(|home| read(home.join(".claude").join("settings.json")))
            .unwrap_or_default(),
    };
    limits_from(
        &managed,
        &user,
        env.var("DISABLE_UPDATES").and_then(|v| v.to_str()),
    )
}

/// [`claude_limits`] from the managed and user settings and the `DISABLE_UPDATES` variable.
fn limits_from(managed: &Value, user: &Value, disable_updates: Option<&str>) -> Limits {
    let channel = [managed, user]
        .iter()
        .find_map(|settings| settings.get("autoUpdatesChannel").and_then(Value::as_str))
        .map_or("latest", |channel| match channel {
            "stable" => "stable",
            _ => "latest",
        });
    let truthy = |value: Option<&str>| {
        value.is_some_and(|value| !value.is_empty() && value != "0" && value != "false")
    };
    let updates_off = truthy(disable_updates)
        || [managed, user].iter().any(|settings| {
            truthy(
                settings
                    .get("env")
                    .and_then(|env| env.get("DISABLE_UPDATES"))
                    .and_then(Value::as_str),
            )
        });
    Limits {
        channel,
        maximum: managed
            .get("requiredMaximumVersion")
            .and_then(Value::as_str)
            .map(str::to_owned),
        updates_off,
    }
}

fn managed_claude_settings() -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some("/Library/Application Support/ClaudeCode/managed-settings.json".into())
    } else if cfg!(windows) {
        Some(r"C:\Program Files\ClaudeCode\managed-settings.json".into())
    } else if cfg!(unix) {
        Some("/etc/claude-code/managed-settings.json".into())
    } else {
        None
    }
}

/// The version `brew upgrade` would install.
async fn brew_version(
    platform: &Arc<dyn brigadier_sandbox::Platform>,
    env: &CliEnv,
    cask: bool,
    name: &str,
) -> Option<String> {
    let mut spec = env.spec(&env.which("brew")?);
    spec.args = [
        "info",
        "--json=v2",
        if cask { "--cask" } else { "--formula" },
        name,
    ]
    .iter()
    .map(OsString::from)
    .collect();
    let output = process::run(platform, &spec, BREW_TIMEOUT).await.ok()?;
    if output.code != Some(0) {
        return None;
    }
    let info: Value = serde_json::from_str(&output.stdout).ok()?;
    let version = if cask {
        info.get("casks")?.get(0)?.get("version")?.as_str()?
    } else {
        info.get("formulae")?
            .get(0)?
            .get("versions")?
            .get("stable")?
            .as_str()?
    };
    // A cask's version can carry a build after a comma.
    Some(version.split(',').next()?.to_owned())
}

// ----- helpers ---------------------------------------------------------------------------

fn target_of(kind: ProviderKind) -> UpdateTarget {
    match kind {
        ProviderKind::Claude => UpdateTarget::Claude,
        ProviderKind::Codex => UpdateTarget::Codex,
    }
}

fn provider_of(target: UpdateTarget) -> Option<ProviderKind> {
    match target {
        UpdateTarget::App => None,
        UpdateTarget::Claude => Some(ProviderKind::Claude),
        UpdateTarget::Codex => Some(ProviderKind::Codex),
    }
}

/// Whether `latest` is a newer version than `current`. Versions are dotted numbers (a leading
/// `v` and build metadata after `+` are ignored; missing parts count as 0), with an optional
/// prerelease after `-` that comes before its release. One that doesn't read is never newer.
fn is_newer(latest: &str, current: &str) -> bool {
    match (Version::parse(latest), Version::parse(current)) {
        (Some(latest), Some(current)) => latest > current,
        _ => false,
    }
}

#[derive(Debug, PartialEq, Eq)]
struct Version {
    numbers: Vec<u64>,
    /// Empty for a release.
    pre: Vec<String>,
}

impl Version {
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split('+').next()?;
        let (numbers, pre) = match text.split_once('-') {
            Some((numbers, pre)) => (numbers, pre.split('.').map(str::to_owned).collect()),
            None => (text, Vec::new()),
        };
        let mut numbers: Vec<u64> = numbers
            .split('.')
            .map(|part| part.parse().ok())
            .collect::<Option<_>>()?;
        while numbers.len() < 3 {
            numbers.push(0);
        }
        Some(Self { numbers, pre })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use std::cmp::Ordering;
        let width = self.numbers.len().max(other.numbers.len());
        let number = |numbers: &[u64], i: usize| numbers.get(i).copied().unwrap_or(0);
        for i in 0..width {
            match number(&self.numbers, i).cmp(&number(&other.numbers, i)) {
                Ordering::Equal => {}
                order => return order,
            }
        }
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        for (a, b) in self.pre.iter().zip(&other.pre) {
            let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                (Ok(a), Ok(b)) => a.cmp(&b),
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => a.cmp(b),
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        self.pre.len().cmp(&other.pre.len())
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// One HTTPS GET of a JSON document; `None` on any failure.
fn get_json(url: &str) -> Option<Value> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(LOOKUP_TIMEOUT))
        .https_only(true)
        .build()
        .into();
    let response = agent
        .get(url)
        .header(
            "User-Agent",
            concat!("Brigadier/", env!("CARGO_PKG_VERSION")),
        )
        .header("Accept", "application/json")
        .call()
        .map_err(|err| tracing::debug!(url, error = %err, "update lookup failed"))
        .ok()?;
    let mut text = String::new();
    response
        .into_body()
        .into_reader()
        .take(MAX_ANSWER_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    serde_json::from_str(&text).ok()
}

async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Option<T> + Send + 'static,
) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok().flatten()
}

/// The last few lines of a command's output, for a failure.
fn last_lines(output: &str) -> Option<String> {
    let lines: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let tail = lines[lines.len().saturating_sub(3)..].join("\n");
    let tail: String = tail.chars().take(400).collect();
    (!tail.is_empty()).then_some(tail)
}

/// Development builds: the targets `BRIGADIER_FAKE_UPDATES` names.
fn faked_targets() -> HashSet<UpdateTarget> {
    #[cfg(debug_assertions)]
    if let Ok(list) = std::env::var("BRIGADIER_FAKE_UPDATES") {
        return list
            .split(',')
            .filter_map(|name| match name.trim() {
                "app" => Some(UpdateTarget::App),
                "claude" => Some(UpdateTarget::Claude),
                "codex" => Some(UpdateTarget::Codex),
                _ => None,
            })
            .collect();
    }
    HashSet::new()
}

/// Development builds: the program a faked CLI's Update runs.
fn fake_runner() -> Option<PathBuf> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("BRIGADIER_FAKE_UPDATE_RUN") {
        return Some(PathBuf::from(path));
    }
    None
}

/// A faked newer version: the next patch release.
fn fake_item(target: UpdateTarget, current: &str, runner: Option<PathBuf>) -> UpdateItem {
    let latest = match Version::parse(current) {
        Some(version) => {
            let mut numbers = version.numbers;
            numbers[2] += 1;
            numbers
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(".")
        }
        None => "99.0.0".into(),
    };
    let action = match target {
        UpdateTarget::App => UpdateAction::Download {
            url: RELEASES_PAGE.into(),
        },
        _ => match runner {
            Some(stub) => UpdateAction::Run {
                command: format!(
                    "{} {}",
                    stub.display(),
                    provider_of(target).map_or("", |k| k.binary())
                ),
            },
            None => UpdateAction::Manual {
                command: "set BRIGADIER_FAKE_UPDATE_RUN to fake an update".into(),
            },
        },
    };
    UpdateItem {
        target,
        current: current.into(),
        latest,
        action,
        progress: UpdateProgress::Available,
    }
}
