//! Watching GitHub Actions after a push: a cheap probe that says whether a push's CI can be
//! watched from here, the watch itself through the user's `gh`, and the envelope lines that
//! report the result to the thread.
//!
//! Plumbing only: it lists runs, reads their jobs and failing logs, and reports. Deciding what
//! to do about a failure is the thread's job.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::{Error, Result};

/// How many runs the first `gh run list` asks for: `gh`'s own default.
const FIRST_LIMIT: usize = 20;
/// The most runs one listing asks for. A push with more matching runs than this is reported
/// as incomplete.
const RUN_LIST_CAP: usize = 640;
/// How long one `gh` call may take before it counts as a failed call.
const CALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Failed `gh` calls in a row (not auth, not missing) after which the watch gives up.
const MAX_FAILED_CALLS: u32 = 5;
/// The trimmed failing log handed to the thread, in bytes.
const KEY_LINES_BYTES: usize = 4096;
/// The last lines of each failing log kept in the key lines.
const TAIL_LINES: usize = 40;
/// Error-looking lines of each failing log kept in the key lines.
const ERROR_LINES: usize = 20;
/// A key line longer than this is cut.
const LINE_CHARS: usize = 240;
/// The only host this watches.
const GITHUB: &str = "github.com";
/// Run or job conclusions that count as failed.
const FAILED_CONCLUSIONS: [&str; 5] = [
    "failure",
    "cancelled",
    "timed_out",
    "startup_failure",
    "action_required",
];

/// The user's `gh`, run with exactly the user's environment.
#[derive(Debug, Clone)]
pub(crate) struct Gh {
    pub program: PathBuf,
    pub vars: Vec<(OsString, OsString)>,
}

/// What one `gh` call printed and how it exited.
#[derive(Debug, Clone)]
pub(crate) struct GhOutput {
    /// The exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl GhOutput {
    fn success(&self) -> bool {
        self.code == Some(0)
    }
}

/// Why `gh` gave no output at all.
#[derive(Debug)]
enum RunError {
    /// It could not be started: missing, or not runnable.
    Spawn(std::io::Error),
    /// It did not finish within [`CALL_TIMEOUT`].
    Timeout,
}

impl Gh {
    /// Runs `gh` with `args`. An error means `gh` could not be started (or never finished);
    /// a non-zero exit is an `Ok` with its code.
    async fn output(&self, args: &[&str]) -> std::result::Result<GhOutput, RunError> {
        let mut tries = 0;
        let child = loop {
            let mut command = tokio::process::Command::new(&self.program);
            command
                .args(args)
                .env_clear()
                .envs(self.vars.iter().map(|(key, value)| (key, value)))
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);
            match command.spawn() {
                Ok(child) => break child,
                // A script just written by another thread can still be open for writing in a
                // forked child for a moment; that clears on its own.
                Err(err) if err.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 10 => {
                    tries += 1;
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                Err(err) => return Err(RunError::Spawn(err)),
            }
        };
        let output = tokio::time::timeout(CALL_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| RunError::Timeout)?
            .map_err(RunError::Spawn)?;
        Ok(GhOutput {
            code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
}

/// What the probe found for one push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probe {
    /// GitHub Actions on github.com, and `gh` is signed in there.
    Watchable {
        /// `owner/name`, for `--repo`.
        repo: String,
        host: String,
    },
    /// There is CI, but not one this can watch.
    NotWatchable {
        /// One short plain sentence.
        why: String,
        /// The host's page for the commit, when one can be made.
        link: Option<String>,
    },
    /// The pushed tree has no CI files this knows.
    NoCi,
}

/// Other CI systems, by the file or folder that turns them on.
const OTHER_CI: [(&str, &str); 5] = [
    (".gitlab-ci.yml", "GitLab CI"),
    (".circleci", "CircleCI"),
    ("Jenkinsfile", "Jenkins"),
    ("azure-pipelines.yml", "Azure Pipelines"),
    ("bitbucket-pipelines.yml", "Bitbucket Pipelines"),
];

/// Says whether the push of `sha` to `remote_url` has CI this can watch. It reads the pushed
/// tree (not the working tree) in `repo_dir`.
pub(crate) async fn probe(repo_dir: &Path, remote_url: &str, sha: &str, gh: Option<&Gh>) -> Probe {
    let remote = Remote::parse(remote_url);
    let link = remote.as_ref().and_then(|remote| remote.commit_link(sha));
    let files = match pushed_ci_files(repo_dir, sha).await {
        Ok(files) => files,
        Err(err) => {
            return Probe::NotWatchable {
                why: format!("Couldn't read the pushed commit {}: {err}", short_sha(sha)),
                link,
            };
        }
    };
    let has_workflows = files.iter().any(|file| is_workflow(file));
    let other = OTHER_CI.iter().find(|(path, _)| {
        files
            .iter()
            .any(|file| file == path || file.starts_with(&format!("{path}/")))
    });
    let github = remote
        .as_ref()
        .filter(|remote| remote.host == GITHUB && remote.owner_parts == 1);
    if has_workflows && let Some(remote) = github {
        let Some(gh) = gh else {
            return Probe::NotWatchable {
                why: "gh isn't installed".into(),
                link,
            };
        };
        return match gh
            .output(&["auth", "status", "--hostname", &remote.host])
            .await
        {
            Ok(out) if out.success() => Probe::Watchable {
                repo: remote.repo(),
                host: remote.host.clone(),
            },
            Ok(_) => Probe::NotWatchable {
                why: format!("gh isn't signed in to {}", remote.host),
                link,
            },
            Err(RunError::Spawn(_)) => Probe::NotWatchable {
                why: "gh isn't installed".into(),
                link,
            },
            Err(RunError::Timeout) => Probe::NotWatchable {
                why: "gh didn't answer".into(),
                link,
            },
        };
    }
    if let Some((_, name)) = other {
        return Probe::NotWatchable {
            why: format!("This repository's CI is {name}"),
            link,
        };
    }
    if has_workflows {
        return Probe::NotWatchable {
            why: "This repository has GitHub Actions, but the push didn't go to github.com".into(),
            link,
        };
    }
    Probe::NoCi
}

/// A file GitHub Actions runs as a workflow: directly in `.github/workflows`, `.yml` or
/// `.yaml`.
fn is_workflow(path: &str) -> bool {
    path.strip_prefix(".github/workflows/").is_some_and(|name| {
        !name.contains('/') && (name.ends_with(".yml") || name.ends_with(".yaml"))
    })
}

/// The CI-related paths in the tree of `sha`.
async fn pushed_ci_files(repo_dir: &Path, sha: &str) -> Result<Vec<String>> {
    let mut args = vec![
        "ls-tree",
        "-r",
        "-z",
        "--name-only",
        sha,
        "--",
        ".github/workflows",
    ];
    args.extend(OTHER_CI.iter().map(|(path, _)| *path));
    let output = tokio::process::Command::new("git")
        .args(&args)
        .current_dir(repo_dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|err| Error::Invalid(format!("could not run git: {err}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(Error::Invalid(stderr.trim().to_string()));
    }
    Ok(String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .collect())
}

/// A remote URL taken apart.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Remote {
    /// Lower-cased, without a user or port.
    host: String,
    /// Everything before the name: `owner`, or `group/subgroup` on GitLab.
    owner: String,
    /// How many segments `owner` has.
    owner_parts: usize,
    name: String,
}

impl Remote {
    /// Reads `git@host:o/n.git`, `ssh://git@host[:port]/o/n`, `https://[user@]host/o/n(.git)`
    /// and the like.
    fn parse(url: &str) -> Option<Self> {
        let url = url.trim();
        let (authority, path) = if let Some((_, rest)) = url.split_once("://") {
            rest.split_once('/')?
        } else {
            // scp-like: [user@]host:path
            let (authority, path) = url.split_once(':')?;
            if authority.contains('/') {
                return None;
            }
            (authority, path)
        };
        let host = authority.rsplit('@').next()?;
        let host = host.split(':').next()?.to_ascii_lowercase();
        if host.is_empty() {
            return None;
        }
        let path = path.trim_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let (name, owner) = segments.split_last()?;
        if owner.is_empty() {
            return None;
        }
        Some(Self {
            host,
            owner: owner.join("/"),
            owner_parts: owner.len(),
            name: (*name).to_string(),
        })
    }

    fn repo(&self) -> String {
        format!("{}/{}", self.owner, self.name)
    }

    /// The host's page for the commit, for the hosts whose page shape is known.
    fn commit_link(&self, sha: &str) -> Option<String> {
        let (host, repo) = (&self.host, self.repo());
        if host == GITHUB && self.owner_parts == 1 {
            Some(format!("https://github.com/{repo}/commit/{sha}"))
        } else if host == "gitlab.com" || host.starts_with("gitlab.") {
            Some(format!("https://{host}/{repo}/-/commit/{sha}"))
        } else if host == "bitbucket.org" && self.owner_parts == 1 {
            Some(format!("https://bitbucket.org/{repo}/commits/{sha}"))
        } else {
            None
        }
    }
}

/// How often the watch polls and how long it waits.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Timing {
    /// Between polls.
    pub poll: Duration,
    /// How long to wait for the first run before reporting no runs.
    pub first_run: Duration,
    /// How long after the last new run appeared, with every run done, the watch ends.
    pub settle: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(15),
            first_run: Duration::from_secs(180),
            settle: Duration::from_secs(60),
        }
    }
}

/// One watched run, as the UI shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RunState {
    pub id: u64,
    pub workflow: String,
    pub status: String,
    pub conclusion: String,
    pub url: String,
    pub jobs_done: usize,
    pub jobs_total: usize,
}

/// The watch so far, after one poll.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Progress {
    pub runs: Vec<RunState>,
    /// No run has appeared yet.
    pub waiting_for_first: bool,
}

/// One failing job (or a failed run that has no failing job).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FailedJob {
    pub workflow: String,
    pub job: String,
    pub url: String,
    /// The run failed without a failing job (it never started its jobs, say).
    pub whole_run: bool,
}

/// How a watch ended.
#[derive(Debug, Clone)]
pub(crate) enum Outcome {
    /// Every run finished and none failed.
    Passed {
        runs: Vec<RunState>,
        /// Jobs across every run.
        jobs: usize,
        /// More runs matched than one listing could read.
        incomplete: bool,
    },
    /// At least one run failed.
    Failed {
        runs: Vec<RunState>,
        jobs: usize,
        failed: Vec<FailedJob>,
        /// The error-looking and last lines of the failing logs, about 4 KB.
        key_lines: String,
        /// The failing logs, untrimmed.
        full_log: String,
        incomplete: bool,
    },
    /// No run started for the push within the first-run window.
    NoRuns,
    /// `gh` went missing, was signed out, or kept failing.
    GhUnavailable { why: String },
}

/// Watches the GitHub Actions runs of the push of `sha` to `branch` in `repo` (`owner/name`)
/// until they all finish, calling `progress` after each poll.
pub(crate) async fn watch(
    gh: &Gh,
    repo: &str,
    branch: &str,
    sha: &str,
    timing: Timing,
    progress: impl FnMut(&Progress),
) -> Outcome {
    watch_capped(gh, repo, branch, sha, timing, RUN_LIST_CAP, progress).await
}

/// A run the watch follows.
#[derive(Debug, Clone)]
struct Tracked {
    workflow: String,
    status: String,
    conclusion: String,
    url: String,
    /// The listing's attempt: a re-run that ended between two polls bumps it.
    attempt: u64,
    jobs: Vec<Job>,
    /// `run view` said completed: its jobs are final.
    settled: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ListedRun {
    #[serde(rename = "databaseId")]
    id: u64,
    #[serde(rename = "workflowName", default)]
    workflow: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    url: String,
    #[serde(rename = "headSha", default)]
    head_sha: String,
    #[serde(rename = "headBranch", default)]
    head_branch: String,
    #[serde(default)]
    attempt: u64,
}

#[derive(Debug, Clone, Default, Deserialize)]
struct ViewedRun {
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    jobs: Vec<Job>,
}

#[derive(Debug, Clone, Deserialize)]
struct Job {
    #[serde(default)]
    name: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    conclusion: String,
    #[serde(default)]
    url: String,
}

/// Why one `gh` call didn't give an answer.
enum CallError {
    /// `gh` is gone or signed out: the watch ends.
    Unavailable(String),
    /// Anything else: the next poll tries again.
    Failed(String),
}

/// Runs `gh` and returns its output when it succeeded.
async fn call(gh: &Gh, args: &[&str]) -> std::result::Result<String, CallError> {
    let out = match gh.output(args).await {
        Ok(out) => out,
        Err(RunError::Timeout) => {
            return Err(CallError::Failed("gh did not finish in time".into()));
        }
        Err(RunError::Spawn(err)) => {
            return Err(CallError::Unavailable(format!("gh couldn't start: {err}")));
        }
    };
    if out.success() {
        return Ok(out.stdout);
    }
    let stderr = out.stderr.trim();
    // Exit code 4 is gh's "requires authentication"; a revoked token comes back as a 401.
    if out.code == Some(4) || stderr.contains("gh auth login") || stderr.contains("HTTP 401") {
        return Err(CallError::Unavailable(format!(
            "gh isn't signed in to {GITHUB}"
        )));
    }
    let first = stderr.lines().next().unwrap_or("").trim();
    Err(CallError::Failed(if first.is_empty() {
        format!("gh exited with {:?}", out.code)
    } else {
        first.to_string()
    }))
}

/// Lists the runs for `sha` on `branch`, raising the limit until every run fits or `cap` is
/// hit. Returns the runs bound to the push and whether the cap cut the list.
async fn list_runs(
    gh: &Gh,
    repo: &str,
    branch: &str,
    sha: &str,
    limit: &mut usize,
    cap: usize,
) -> std::result::Result<(Vec<ListedRun>, bool), CallError> {
    loop {
        let limit_arg = limit.to_string();
        let stdout = call(
            gh,
            &[
                "run",
                "list",
                "--repo",
                repo,
                "--commit",
                sha,
                "--branch",
                branch,
                "--json",
                "databaseId,workflowName,status,conclusion,url,headSha,headBranch,event,attempt",
                "--limit",
                &limit_arg,
            ],
        )
        .await?;
        let runs: Vec<ListedRun> = parse_json(&stdout)?;
        if runs.len() >= *limit {
            if *limit < cap {
                *limit = (*limit * 2).min(cap);
                continue;
            }
            return Ok((bound(runs, branch, sha), true));
        }
        return Ok((bound(runs, branch, sha), false));
    }
}

/// Only the runs of this very push: its sha on its branch.
fn bound(runs: Vec<ListedRun>, branch: &str, sha: &str) -> Vec<ListedRun> {
    runs.into_iter()
        .filter(|run| run.head_sha == sha && run.head_branch == branch)
        .collect()
}

fn parse_json<T: serde::de::DeserializeOwned + Default>(
    stdout: &str,
) -> std::result::Result<T, CallError> {
    if stdout.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(stdout)
        .map_err(|err| CallError::Failed(format!("gh printed something unexpected: {err}")))
}

/// `watch`, with the listing cap as a parameter.
async fn watch_capped(
    gh: &Gh,
    repo: &str,
    branch: &str,
    sha: &str,
    timing: Timing,
    cap: usize,
    mut progress: impl FnMut(&Progress),
) -> Outcome {
    let start = Instant::now();
    let mut tracked: BTreeMap<u64, Tracked> = BTreeMap::new();
    let mut last_new = start;
    let mut limit = FIRST_LIMIT.min(cap);
    let mut incomplete = false;
    let mut failed_calls = 0;
    loop {
        let known = tracked.len();
        let polled = poll(gh, repo, branch, sha, cap, &mut limit, &mut tracked).await;
        // A run that appeared counts even when a later call of the same poll failed.
        if tracked.len() > known {
            last_new = Instant::now();
        }
        match polled {
            Ok(capped) => {
                failed_calls = 0;
                incomplete |= capped;
            }
            Err(CallError::Unavailable(why)) => return Outcome::GhUnavailable { why },
            Err(CallError::Failed(why)) => {
                failed_calls += 1;
                if failed_calls >= MAX_FAILED_CALLS {
                    return Outcome::GhUnavailable {
                        why: format!("gh kept failing: {why}"),
                    };
                }
                // What the runs are now is unknown: only a poll that went through ends it.
                tokio::time::sleep(timing.poll).await;
                continue;
            }
        }
        progress(&Progress {
            runs: run_states(&tracked),
            waiting_for_first: tracked.is_empty(),
        });
        let now = Instant::now();
        if tracked.is_empty() {
            if now.duration_since(start) >= timing.first_run {
                return Outcome::NoRuns;
            }
        } else if tracked.values().all(|run| run.settled)
            && now.duration_since(last_new) >= timing.settle
        {
            return finish(gh, repo, &tracked, incomplete).await;
        }
        tokio::time::sleep(timing.poll).await;
    }
}

/// One poll: lists the runs, adds new ones, and reads the jobs of every run not yet final.
/// Returns whether the listing was cut.
async fn poll(
    gh: &Gh,
    repo: &str,
    branch: &str,
    sha: &str,
    cap: usize,
    limit: &mut usize,
    tracked: &mut BTreeMap<u64, Tracked>,
) -> std::result::Result<bool, CallError> {
    let (listed, capped) = list_runs(gh, repo, branch, sha, limit, cap).await?;
    for run in listed {
        let entry = tracked.entry(run.id).or_insert_with(|| Tracked {
            workflow: run.workflow.clone(),
            status: String::new(),
            conclusion: String::new(),
            url: run.url.clone(),
            attempt: run.attempt,
            jobs: Vec::new(),
            settled: false,
        });
        // A run that was re-run goes back to in progress, or ended again between two polls
        // under the same id with a new attempt or result.
        if run.status != "completed"
            || run.attempt != entry.attempt
            || (entry.settled && run.conclusion != entry.conclusion)
        {
            entry.settled = false;
        }
        entry.attempt = run.attempt;
        if !entry.settled {
            entry.status = run.status;
            entry.conclusion = run.conclusion;
        }
    }
    for (id, run) in tracked.iter_mut().filter(|(_, run)| !run.settled) {
        let id = id.to_string();
        let stdout = call(
            gh,
            &[
                "run",
                "view",
                &id,
                "--repo",
                repo,
                "--json",
                "status,conclusion,jobs",
            ],
        )
        .await?;
        let view: ViewedRun = parse_json(&stdout)?;
        run.status = view.status;
        run.conclusion = view.conclusion;
        run.jobs = view.jobs;
        run.settled = run.status == "completed";
    }
    Ok(capped)
}

fn run_states(tracked: &BTreeMap<u64, Tracked>) -> Vec<RunState> {
    tracked
        .iter()
        .map(|(id, run)| RunState {
            id: *id,
            workflow: run.workflow.clone(),
            status: run.status.clone(),
            conclusion: run.conclusion.clone(),
            url: run.url.clone(),
            jobs_done: run
                .jobs
                .iter()
                .filter(|job| job.status == "completed")
                .count(),
            jobs_total: run.jobs.len(),
        })
        .collect()
}

fn is_failed(conclusion: &str) -> bool {
    FAILED_CONCLUSIONS.contains(&conclusion)
}

/// The result once every run is done: passed, or failed with the failing jobs and their logs.
async fn finish(
    gh: &Gh,
    repo: &str,
    tracked: &BTreeMap<u64, Tracked>,
    incomplete: bool,
) -> Outcome {
    let runs = run_states(tracked);
    let jobs = runs.iter().map(|run| run.jobs_total).sum();
    let mut failed = Vec::new();
    let mut failed_runs = Vec::new();
    for (id, run) in tracked {
        let before = failed.len();
        failed.extend(
            run.jobs
                .iter()
                .filter(|job| is_failed(&job.conclusion))
                .map(|job| FailedJob {
                    workflow: run.workflow.clone(),
                    job: job.name.clone(),
                    url: if job.url.is_empty() {
                        run.url.clone()
                    } else {
                        job.url.clone()
                    },
                    whole_run: false,
                }),
        );
        let run_failed = is_failed(&run.conclusion);
        if run_failed && failed.len() == before {
            failed.push(FailedJob {
                workflow: run.workflow.clone(),
                job: format!("the run ({})", run.conclusion),
                url: run.url.clone(),
                whole_run: true,
            });
        }
        if run_failed || failed.len() > before {
            failed_runs.push((*id, run));
        }
    }
    if failed.is_empty() {
        return Outcome::Passed {
            runs,
            jobs,
            incomplete,
        };
    }
    let mut full_log = String::new();
    let mut key_lines = Vec::new();
    for (id, run) in failed_runs {
        let id_arg = id.to_string();
        let log = match call(
            gh,
            &["run", "view", &id_arg, "--repo", repo, "--log-failed"],
        )
        .await
        {
            Ok(log) => log,
            Err(CallError::Unavailable(why) | CallError::Failed(why)) => {
                format!("(gh couldn't read this run's failing log: {why})\n")
            }
        };
        let heading = format!("== {} (run {id}) ==", run.workflow);
        full_log.push_str(&heading);
        full_log.push('\n');
        full_log.push_str(&log);
        if !log.ends_with('\n') {
            full_log.push('\n');
        }
        key_lines.push(heading);
        key_lines.extend(pick_key_lines(&log));
    }
    Outcome::Failed {
        runs,
        jobs,
        failed,
        key_lines: trim_to(&key_lines, KEY_LINES_BYTES),
        full_log,
        incomplete,
    }
}

/// The error-looking lines and the last lines of one failing log, in log order, each with
/// its timestamp dropped.
fn pick_key_lines(log: &str) -> Vec<String> {
    let lines: Vec<&str> = log.lines().filter(|line| !line.trim().is_empty()).collect();
    let tail_from = lines.len().saturating_sub(TAIL_LINES);
    let mut errors = 0;
    let mut picked = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        let text = log_text(line);
        let keep = if index >= tail_from {
            true
        } else if errors < ERROR_LINES && looks_like_error(&text) {
            errors += 1;
            true
        } else {
            false
        };
        if keep {
            picked.push(cut_line(&text));
        }
    }
    picked
}

/// A `--log-failed` line is `job<TAB>step<TAB>timestamp text`; this keeps `job: text`.
fn log_text(line: &str) -> String {
    let mut parts = line.splitn(3, '\t');
    let (Some(job), Some(_step), Some(rest)) = (parts.next(), parts.next(), parts.next()) else {
        return line.trim_end().to_string();
    };
    let text = match rest.split_once(' ') {
        Some((stamp, text))
            if stamp.ends_with('Z') && stamp.starts_with(|c: char| c.is_ascii_digit()) =>
        {
            text
        }
        _ => rest,
    };
    format!("{job}: {}", text.trim_end())
}

fn looks_like_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    [
        "error",
        "failed",
        "failure",
        "panicked",
        "fatal",
        "assert",
        "exception",
    ]
    .iter()
    .any(|word| lower.contains(word))
        || text.contains("FAIL")
}

fn cut_line(text: &str) -> String {
    if text.chars().count() <= LINE_CHARS {
        return text.to_string();
    }
    let cut: String = text.chars().take(LINE_CHARS).collect();
    format!("{cut}…")
}

/// Joins `lines` and keeps at most `max` bytes, dropping whole lines from the front.
fn trim_to(lines: &[String], max: usize) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut size = 0;
    // Room for the "…" line that marks the cut.
    let max = max.saturating_sub("…\n".len());
    for line in lines.iter().rev() {
        if size + line.len() + 1 > max {
            kept.push("…");
            break;
        }
        size += line.len() + 1;
        kept.push(line);
    }
    kept.reverse();
    kept.join("\n")
}

/// The first 8 characters of a sha.
pub(crate) fn short_sha(sha: &str) -> &str {
    sha.get(..8).unwrap_or(sha)
}

/// A push that put on the remote's `branch` a commit the local `branch` doesn't have (a fix
/// pushed with `HEAD:main` from the session branch): the two have parted, and the thread is told
/// to land it here too. Empty when the local branch has it, or there is no local branch.
async fn parted_note(repo: &Path, branch: &str, sha: &str) -> String {
    let local = format!("refs/heads/{branch}");
    let has = |args: Vec<&str>| {
        let mut command = tokio::process::Command::new("git");
        command.arg("-C").arg(repo).args(args).stdin(Stdio::null());
        async move { command.output().await.is_ok_and(|out| out.status.success()) }
    };
    if branch.is_empty()
        || !has(vec!["rev-parse", "--verify", "--quiet", &local]).await
        || has(vec!["merge-base", "--is-ancestor", sha, &local]).await
    {
        return String::new();
    }
    format!(
        "\nThe local `{branch}` doesn't have {}: this push skipped the merge here. Land it in `{branch}` through the usual merge before anything else, so the two don't part.",
        short_sha(sha)
    )
}

/// What the thread does on a red run: the fix reaches the watched branch the way its changes
/// always do, so a base branch here and on the remote never part.
pub(crate) const FIX_THE_FAILURE: &str = "\nFix the failure (a worker, or a tiny fix of your own) and land it the way the watched branch takes changes: a session branch directly; a base branch like `main` only through the usual merge into it here, then push that branch, never a commit the local branch doesn't have. Then watch again: the user's yes to watching covers the push and the watch, and nothing else; the merge follows its usual rules.";

/// How the thread says a CI result that isn't a failure to fix: the request's answer when it
/// has none yet, else one update line under it (`prompts::LATE_EVENT`), never a new ending.
pub(crate) const AFTER_THE_ANSWER: &str = "If you already answered this request, write one `Update:` line with the result, never a new ending; otherwise tell the user the result in one line.";

/// The line that tells the thread how a watch ended.
pub(crate) fn outcome_envelope(sha: &str, outcome: &Outcome) -> String {
    let short = short_sha(sha);
    let incomplete_note = |incomplete: bool| {
        if incomplete {
            format!(
                "\nMore than {RUN_LIST_CAP} runs matched this push; only the first {RUN_LIST_CAP} were checked."
            )
        } else {
            String::new()
        }
    };
    match outcome {
        Outcome::Passed {
            jobs, incomplete, ..
        } => format!(
            "[ci {short} · passed · {jobs} {}]{}",
            plural(*jobs, "job", "jobs"),
            incomplete_note(*incomplete)
        ),
        Outcome::Failed {
            jobs,
            failed,
            key_lines,
            incomplete,
            ..
        } => {
            let failing_jobs = failed.iter().filter(|job| !job.whole_run).count();
            let head = if failing_jobs > 0 {
                format!("{failing_jobs} of {jobs} jobs failed")
            } else {
                let runs = failed.len();
                format!("{runs} {} failed", plural(runs, "run", "runs"))
            };
            let mut text = format!("[ci {short} · failed · {head}]");
            for job in failed {
                text.push_str(&format!("\n{} › {} ({})", job.workflow, job.job, job.url));
            }
            text.push_str(&incomplete_note(*incomplete));
            if !key_lines.is_empty() {
                text.push_str("\n\n");
                text.push_str(key_lines);
            }
            text
        }
        Outcome::NoRuns => format!(
            "[ci {short} · no runs] No GitHub Actions run started for this push (its workflows may not run on this branch)."
        ),
        Outcome::GhUnavailable { why } => cant_watch_envelope(sha, why),
    }
}

/// The line that says this push's CI can't be watched, and why.
pub(crate) fn cant_watch_envelope(sha: &str, why: &str) -> String {
    format!("[ci {} · can't watch] {why}", short_sha(sha))
}

fn plural<'a>(n: usize, one: &'a str, many: &'a str) -> &'a str {
    if n == 1 { one } else { many }
}

/// The thread's tools (SESSION-FIXES-PLAN.md Q1): `ci_status` after its push, and `watch_ci`
/// once the user said yes. The watch runs on its own; its result comes back as a message.
impl super::SessionManager {
    /// The user's `gh`, from their login PATH, with their environment.
    fn gh(&self) -> Option<Gh> {
        let env = self.runtime.cli_env();
        Some(Gh {
            program: env.which("gh")?,
            vars: env.vars(),
        })
    }

    /// The session's repository, the URL of the remote pushed to, and the full sha pushed.
    async fn ci_push(
        &self,
        id: &crate::model::ConversationId,
        push: &crate::tools::CiPush,
    ) -> Result<(PathBuf, String, String)> {
        let Some(crate::model::Setup::Session { repo, .. }) = self.core.conversation(id)?.setup
        else {
            return Err(Error::Invalid(
                "CI is watched for a session's pushes".into(),
            ));
        };
        let repo = PathBuf::from(repo);
        let git = |args: Vec<String>| {
            let repo = repo.clone();
            async move {
                let out = tokio::process::Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(&args)
                    .stdin(Stdio::null())
                    .output()
                    .await
                    .map_err(|err| Error::Invalid(format!("git didn't run: {err}")))?;
                if out.status.success() {
                    Ok(String::from_utf8_lossy(&out.stdout).trim().to_owned())
                } else {
                    Err(Error::Invalid(format!(
                        "`git {}`: {}",
                        args.join(" "),
                        String::from_utf8_lossy(&out.stderr).trim()
                    )))
                }
            }
        };
        let remote = push.remote.trim();
        let url = if remote.contains("://") || remote.contains('@') {
            remote.to_owned()
        } else {
            // Where `git push` sends it: a pushurl or pushInsteadOf may differ from the fetch URL.
            git(vec![
                "remote".into(),
                "get-url".into(),
                "--push".into(),
                remote.into(),
            ])
            .await?
        };
        let sha = git(vec![
            "rev-parse".into(),
            "--verify".into(),
            format!("{}^{{commit}}", push.sha.trim()),
        ])
        .await?;
        Ok((repo, url, sha))
    }

    /// `ci_status`: whether the push's CI can be watched from here, and what to tell the user.
    pub(crate) async fn ci_status(
        &self,
        id: &crate::model::ConversationId,
        push: crate::tools::CiPush,
    ) -> Result<String> {
        let (repo, url, sha) = self.ci_push(id, &push).await?;
        let short = short_sha(&sha);
        let parted = parted_note(&repo, push.branch.trim(), &sha).await;
        let status = match probe(&repo, &url, &sha, self.gh().as_ref()).await {
            Probe::Watchable { repo, .. } => format!(
                "watchable: {repo} runs GitHub Actions on {short}. Ask the user once with ask_user, \"Watch CI for {short}?\" with Watch and No. On Watch, call watch_ci with the same remote, branch and sha; on No, say nothing more about it."
            ),
            Probe::NotWatchable { why, link } => format!(
                "not watchable: {why} Tell the user in one line that you can't watch this repository's CI from here{}, and don't offer to.",
                link.map(|link| format!(", with its link {link}"))
                    .unwrap_or_default()
            ),
            Probe::NoCi => {
                "no CI: the pushed commit has no CI files. Say nothing about CI.".to_owned()
            }
        };
        Ok(status + &parted)
    }

    /// `watch_ci`: starts watching the push's runs, shown live as a `CiWatch` row, and returns
    /// at once; the result comes back as a `[ci …]` message.
    pub(crate) async fn watch_ci(
        &self,
        id: &crate::model::ConversationId,
        push: crate::tools::CiPush,
    ) -> Result<String> {
        let (dir, url, sha) = self.ci_push(id, &push).await?;
        let gh = self.gh();
        let (Probe::Watchable { repo, host }, Some(gh)) =
            (probe(&dir, &url, &sha, gh.as_ref()).await, gh)
        else {
            return Err(Error::Invalid(
                "This push's CI can't be watched from here: call ci_status, and tell the user what it says.".into(),
            ));
        };
        let conv = self.conv(id)?;
        let request = self.request_for(id, None).await;
        let branch = push.branch.trim().to_owned();
        let short = short_sha(&sha).to_owned();
        // The user's yes to watching it: carried on from the watch whose fix this is, or the
        // card they answered since they last wrote.
        let board = self.core.board(id).await?;
        let spoke = self.user_wrote_at(id, &board).await?;
        let consent = watch_consent(&board, &repo, &branch, spoke);
        self.ci_row(
            id,
            request.clone(),
            &repo,
            &branch,
            &sha,
            &Progress {
                runs: Vec::new(),
                waiting_for_first: true,
            },
            None,
            consent.as_ref(),
        )
        .await;
        let this = self.arc();
        let id = id.clone();
        let timing = Timing::default();
        let reply = format!(
            "Watching CI for {short} on `{branch}`: the user sees the runs live, and the result comes as a [ci {short} …] message. Reply {} meanwhile.{}",
            super::prompts::QUIET,
            parted_note(&dir, &branch, &sha).await
        );
        let watching = tokio::spawn(async move {
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Progress>();
            let rows = async {
                let mut last: Option<String> = None;
                while let Some(progress) = rx.recv().await {
                    let now = serde_json::to_string(&progress).unwrap_or_default();
                    if last.as_deref() == Some(now.as_str()) {
                        continue;
                    }
                    last = Some(now);
                    this.ci_row(
                        &id,
                        request.clone(),
                        &repo,
                        &branch,
                        &sha,
                        &progress,
                        None,
                        consent.as_ref(),
                    )
                    .await;
                }
            };
            // `HOST/OWNER/REPO`, so a GH_HOST set to another host doesn't send the run
            // commands where the probe didn't look (gh 2.101.0 `--repo [HOST/]OWNER/REPO`).
            let target = format!("{host}/{repo}");
            let outcome = async {
                watch(&gh, &target, &branch, &sha, timing, move |progress| {
                    let _ = tx.send(progress.clone());
                })
                .await
            };
            let ((), outcome) = tokio::join!(rows, outcome);
            let (runs, result) = match &outcome {
                Outcome::Passed { runs, .. } => (runs.clone(), "passed"),
                Outcome::Failed { runs, .. } => (runs.clone(), "failed"),
                Outcome::NoRuns => (Vec::new(), "no runs"),
                Outcome::GhUnavailable { .. } => (Vec::new(), "stopped"),
            };
            this.ci_row(
                &id,
                request.clone(),
                &repo,
                &branch,
                &sha,
                &Progress {
                    runs,
                    waiting_for_first: false,
                },
                Some(result),
                consent.as_ref(),
            )
            .await;
            let mut text = outcome_envelope(&sha, &outcome);
            match &outcome {
                Outcome::Failed { full_log, .. } => {
                    let path = this
                        .owned_dir("orch", &id.0)
                        .join(format!("ci-{short}.log"));
                    if std::fs::create_dir_all(path.parent().unwrap_or(Path::new("/")))
                        .and_then(|()| std::fs::write(&path, full_log))
                        .is_ok()
                    {
                        text.push_str(&format!("\nThe whole failing log: {}", path.display()));
                    }
                    text.push_str(FIX_THE_FAILURE);
                }
                _ => text.push_str(&format!("\n{AFTER_THE_ANSWER}")),
            }
            this.deliver_for(
                &id,
                super::conversation::Envelope {
                    kind: crate::work::InjectionKind::Report,
                    label: format!("ci {short}"),
                    task_id: None,
                    text,
                },
                request,
            )
            .await;
        });
        conv.ci_watches
            .lock()
            .unwrap()
            .push(watching.abort_handle());
        Ok(reply)
    }

    /// Ends the conversation's CI watches when it closes, after a restart cut them off, or
    /// when the user presses Stop: their tasks stop, and a row still checking reads "stopped",
    /// so no watch is left that never ends, and none wakes the thread later. The requests they
    /// kept working settle.
    pub(crate) async fn stop_ci_watches(&self, id: &crate::model::ConversationId) {
        if let Ok(conv) = self.conv(id) {
            for watch in conv.ci_watches.lock().unwrap().drain(..) {
                watch.abort();
            }
        }
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let open: Vec<crate::work::OrchestratorStepKind> = board
            .orchestrator_steps
            .iter()
            .filter(|step| {
                matches!(
                    step.kind,
                    crate::work::OrchestratorStepKind::CiWatch { result: None, .. }
                )
            })
            .map(|step| step.kind.clone())
            .collect();
        let ended = !open.is_empty();
        for mut kind in open {
            if let crate::work::OrchestratorStepKind::CiWatch {
                waiting, result, ..
            } = &mut kind
            {
                *waiting = false;
                *result = Some("stopped".into());
            }
            self.orchestrator_step(id, kind).await;
        }
        if ended {
            self.settle_requests(id).await;
        }
    }

    /// Files or updates the push's `CiWatch` row.
    #[allow(clippy::too_many_arguments)]
    async fn ci_row(
        &self,
        id: &crate::model::ConversationId,
        request: Option<String>,
        repo: &str,
        branch: &str,
        sha: &str,
        progress: &Progress,
        result: Option<&str>,
        consent: Option<&crate::work::CiConsent>,
    ) {
        let runs = progress
            .runs
            .iter()
            .map(|run| crate::work::CiRun {
                workflow: run.workflow.clone(),
                status: run.status.clone(),
                conclusion: run.conclusion.clone(),
                url: run.url.clone(),
                jobs_done: u32::try_from(run.jobs_done).unwrap_or(u32::MAX),
                jobs_total: u32::try_from(run.jobs_total).unwrap_or(u32::MAX),
            })
            .collect();
        self.orchestrator_step_in(
            id,
            request,
            crate::work::OrchestratorStepKind::CiWatch {
                repo: repo.to_owned(),
                branch: branch.to_owned(),
                sha: sha.to_owned(),
                waiting: progress.waiting_for_first,
                runs,
                result: result.map(str::to_owned),
                consent: consent.cloned(),
            },
        )
        .await;
    }
}

/// How a watch ends the user's yes to it: what passed, saw no runs or stopped needs no fix.
const LOOP_ENDS: [&str; 3] = ["passed", "no runs", "stopped"];

/// A watched push's row, as the board keeps it: one per repository, branch and sha.
struct WatchRow<'a> {
    repo: &'a str,
    branch: &'a str,
    result: Option<&'a str>,
    consent: Option<&'a crate::work::CiConsent>,
}

fn watch_rows(board: &crate::board::Board) -> Vec<WatchRow<'_>> {
    board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            crate::work::OrchestratorStepKind::CiWatch {
                repo,
                branch,
                result,
                consent,
                ..
            } => Some(WatchRow {
                repo,
                branch,
                result: result.as_deref(),
                consent: consent.as_ref(),
            }),
            _ => None,
        })
        .collect()
}

/// The user's yeses to watching CI that still cover a CI fix's push, each with the
/// repository and branch it covers: the newest per repository and branch, unless a watch under
/// it passed, saw no runs or stopped, or the user wrote since (`spoke`: when they last did).
/// A failed watch keeps it: the fix is pushed and watched again under the same yes.
pub(crate) fn live_ci_consents(
    board: &crate::board::Board,
    spoke: Option<i64>,
) -> Vec<(String, String, crate::work::CiConsent)> {
    let rows = watch_rows(board);
    let mut seen: Vec<(&str, &str)> = Vec::new();
    let mut live = Vec::new();
    for row in rows.iter().rev() {
        let Some(consent) = row.consent else {
            continue;
        };
        if seen.contains(&(row.repo, row.branch)) {
            continue;
        }
        seen.push((row.repo, row.branch));
        let ended = rows.iter().any(|other| {
            other.repo == row.repo
                && other.branch == row.branch
                && other.consent.is_some_and(|c| c.card == consent.card)
                && other
                    .result
                    .is_some_and(|result| LOOP_ENDS.contains(&result))
        });
        let wrote_since = spoke.is_some_and(|at| at > consent.answered_at_ms);
        if !ended && !wrote_since {
            live.push((row.repo.to_owned(), row.branch.to_owned(), consent.clone()));
        }
    }
    live
}

/// The yes a new watch of `repo` and `branch` holds: the one still live for them (a fix's
/// push watched again), else the newest card the user answered since they last wrote that no
/// watch holds yet. Which card the thread asked on, and whether its answer is a yes, is the
/// escalation reviewer's to read; Brigadier only binds the user's own answer to the watch.
pub(crate) fn watch_consent(
    board: &crate::board::Board,
    repo: &str,
    branch: &str,
    spoke: Option<i64>,
) -> Option<crate::work::CiConsent> {
    if let Some((_, _, consent)) = live_ci_consents(board, spoke)
        .into_iter()
        .find(|(r, b, _)| r == repo && b == branch)
    {
        return Some(consent);
    }
    let held = watch_rows(board);
    board
        .questions
        .values()
        .filter(|question| matches!(question.kind, crate::work::QuestionKind::Orchestrator))
        .filter(|question| {
            !held
                .iter()
                .any(|row| row.consent.is_some_and(|c| c.card == question.id))
        })
        .filter_map(|question| {
            let answer = question.answer.as_ref()?;
            let at = question.answered_at_ms?;
            spoke
                .is_none_or(|spoke| at > spoke)
                .then(|| crate::work::CiConsent {
                    card: question.id.clone(),
                    question: question.text.clone(),
                    answer: answer.clone(),
                    answered_at_ms: at,
                })
        })
        .max_by_key(|consent| consent.answered_at_ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The board of a session whose thread asked to watch CI on a card: `answered` when the
    /// user answered it, and the watches since.
    fn ci_board(
        answered: Option<i64>,
        watches: &[(&str, Option<&str>, bool)],
    ) -> crate::board::Board {
        use crate::work::{OrchestratorStep, OrchestratorStepKind, Question, QuestionKind};
        let mut board = crate::board::Board::default();
        let card = crate::model::CardId("card-1".into());
        board.questions.insert(
            card.clone(),
            Question {
                id: card,
                conversation_id: crate::model::ConversationId("c".into()),
                task_id: None,
                request_id: None,
                position: 1,
                kind: QuestionKind::Orchestrator,
                text: "Watch CI for 97c298d5?".into(),
                options: vec!["Watch".into(), "No".into()],
                recommended: Some(0),
                items: Vec::new(),
                answer: answered.map(|_| "Watch".into()),
                answers: Vec::new(),
                created_at_ms: 5,
                answered_at_ms: answered,
                words_id: None,
                answered_by: None,
            },
        );
        for (seq, (sha, result, consented)) in watches.iter().enumerate() {
            let consent = consented
                .then(|| watch_consent(&board, "o/n", "fix", Some(1)))
                .flatten();
            board.apply(
                &crate::model::DomainEvent::OrchestratorStepped {
                    step: OrchestratorStep {
                        request_id: None,
                        kind: OrchestratorStepKind::CiWatch {
                            repo: "o/n".into(),
                            branch: "fix".into(),
                            sha: (*sha).into(),
                            waiting: false,
                            runs: Vec::new(),
                            result: result.map(str::to_owned),
                            consent,
                        },
                        at_ms: 20,
                        position: 0,
                    },
                },
                i64::try_from(seq).unwrap() + 2,
            );
        }
        board
    }

    fn live(board: &crate::board::Board, spoke: Option<i64>) -> Vec<(String, String, String)> {
        live_ci_consents(board, spoke)
            .into_iter()
            .map(|(repo, branch, consent)| (repo, branch, consent.card.0))
            .collect()
    }

    #[test]
    fn a_watch_holds_the_card_the_user_answered_since_they_last_wrote() {
        // Answered at 10, after the user's last message at 1.
        let board = ci_board(Some(10), &[]);
        let consent = watch_consent(&board, "o/n", "fix", Some(1)).unwrap();
        assert_eq!(consent.card.0, "card-1");
        assert_eq!(consent.question, "Watch CI for 97c298d5?");
        assert_eq!(consent.answer, "Watch");
        assert_eq!(consent.answered_at_ms, 10);
        // Never answered, or answered before the user's last message: no consent.
        assert!(watch_consent(&ci_board(None, &[]), "o/n", "fix", Some(1)).is_none());
        assert!(watch_consent(&board, "o/n", "fix", Some(11)).is_none());
    }

    #[test]
    fn a_watch_the_user_never_said_yes_to_covers_no_push() {
        let board = ci_board(None, &[("aaaa", None, true)]);
        assert!(live(&board, Some(1)).is_empty());
    }

    #[test]
    fn a_red_watch_keeps_the_yes_for_the_fix_and_its_new_watch() {
        // Red: the fix's push is covered while nothing else happened.
        let red = ci_board(Some(10), &[("aaaa", Some("failed"), true)]);
        assert_eq!(
            live(&red, Some(1)),
            [("o/n".into(), "fix".into(), "card-1".into())]
        );
        // The fix's watch carries the same yes, though no card was answered for it.
        let board = ci_board(
            Some(10),
            &[("aaaa", Some("failed"), true), ("bbbb", None, true)],
        );
        assert_eq!(live(&board, Some(1)).len(), 1);
        // Another repository's or branch's watch gets nothing from it.
        assert!(watch_consent(&board, "o/other", "fix", Some(1)).is_none());
        assert!(watch_consent(&board, "o/n", "main", Some(1)).is_none());
    }

    #[test]
    fn a_watch_that_passed_saw_no_runs_or_stopped_ends_the_yes() {
        for end in ["passed", "no runs", "stopped"] {
            let board = ci_board(
                Some(10),
                &[("aaaa", Some("failed"), true), ("bbbb", Some(end), true)],
            );
            assert!(live(&board, Some(1)).is_empty(), "{end}");
            // A card used up isn't taken again by the next watch.
            assert!(
                watch_consent(&board, "o/n", "fix", Some(1)).is_none(),
                "{end}"
            );
        }
    }

    #[test]
    fn the_users_next_message_ends_the_yes() {
        let board = ci_board(Some(10), &[("aaaa", Some("failed"), true)]);
        assert_eq!(live(&board, Some(9)).len(), 1);
        assert!(live(&board, Some(12)).is_empty());
    }

    const SHA: &str = "97c298d5aaaabbbbccccddddeeeeffff00001111";

    #[test]
    fn remotes_parse() {
        for url in [
            "git@github.com:o/n.git",
            "ssh://git@github.com/o/n",
            "ssh://git@github.com:22/o/n.git",
            "https://github.com/o/n.git",
            "https://github.com/o/n",
            "https://user@GitHub.com/o/n/",
        ] {
            let remote = Remote::parse(url).unwrap_or_else(|| panic!("{url}"));
            assert_eq!(remote.host, "github.com", "{url}");
            assert_eq!(remote.repo(), "o/n", "{url}");
            assert_eq!(remote.owner_parts, 1, "{url}");
        }
        let gitlab = Remote::parse("git@gitlab.com:group/sub/n.git").unwrap();
        assert_eq!(
            gitlab.commit_link("abc").as_deref(),
            Some("https://gitlab.com/group/sub/n/-/commit/abc")
        );
        let bitbucket = Remote::parse("https://bitbucket.org/o/n.git").unwrap();
        assert_eq!(
            bitbucket.commit_link("abc").as_deref(),
            Some("https://bitbucket.org/o/n/commits/abc")
        );
        assert_eq!(
            Remote::parse("git@example.com:o/n")
                .unwrap()
                .commit_link("abc"),
            None
        );
        assert_eq!(Remote::parse("/some/local/path"), None);
        assert_eq!(Remote::parse("https://github.com/only"), None);
    }

    #[test]
    fn workflow_files() {
        assert!(is_workflow(".github/workflows/ci.yml"));
        assert!(is_workflow(".github/workflows/ci.yaml"));
        assert!(!is_workflow(".github/workflows/sub/ci.yml"));
        assert!(!is_workflow(".github/workflows/README.md"));
    }

    #[test]
    fn envelopes() {
        let passed = Outcome::Passed {
            runs: Vec::new(),
            jobs: 5,
            incomplete: false,
        };
        assert_eq!(
            outcome_envelope(SHA, &passed),
            "[ci 97c298d5 · passed · 5 jobs]"
        );
        let failed = Outcome::Failed {
            runs: Vec::new(),
            jobs: 5,
            failed: vec![
                FailedJob {
                    workflow: "CI".into(),
                    job: "test".into(),
                    url: "https://x/1".into(),
                    whole_run: false,
                },
                FailedJob {
                    workflow: "CI".into(),
                    job: "lint".into(),
                    url: "https://x/2".into(),
                    whole_run: false,
                },
            ],
            key_lines: "test: error: boom".into(),
            full_log: String::new(),
            incomplete: false,
        };
        assert_eq!(
            outcome_envelope(SHA, &failed),
            "[ci 97c298d5 · failed · 2 of 5 jobs failed]\nCI › test (https://x/1)\nCI › lint (https://x/2)\n\ntest: error: boom"
        );
        assert_eq!(
            outcome_envelope(SHA, &Outcome::NoRuns),
            "[ci 97c298d5 · no runs] No GitHub Actions run started for this push (its workflows may not run on this branch)."
        );
        assert_eq!(
            cant_watch_envelope(SHA, "gh isn't signed in to github.com"),
            "[ci 97c298d5 · can't watch] gh isn't signed in to github.com"
        );
    }

    #[test]
    fn key_lines_keep_errors_and_the_tail() {
        let mut log = String::new();
        log.push_str("build\tRun tests\t2026-10-10T22:47:23.1234567Z error[E0425]: not found\n");
        for i in 0..100 {
            log.push_str(&format!(
                "build\tRun tests\t2026-10-10T22:47:23Z line {i}\n"
            ));
        }
        let picked = pick_key_lines(&log);
        assert_eq!(picked[0], "build: error[E0425]: not found");
        assert_eq!(picked.len(), 1 + TAIL_LINES);
        assert_eq!(picked.last().unwrap(), "build: line 99");
        let long: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();
        let trimmed = trim_to(&long, KEY_LINES_BYTES);
        assert!(trimmed.len() <= KEY_LINES_BYTES);
        assert!(trimmed.starts_with('…') && trimmed.ends_with("line 999"));
    }

    /// A fake `gh`: a shell script that answers each call from files in its folder.
    ///
    /// A call's key is `auth`, `list-<limit>`, `view-<id>` or `log-<id>`. Its n-th call answers
    /// with `<key>.<n>`, or the highest-numbered `<key>.<m>` below n, or `<key>`; a sibling
    /// `.exit` file holds the exit code and `.err` its stderr. A `<file>.rmself` marker makes
    /// the script delete itself after answering. Every call's arguments go to `calls`.
    #[cfg(unix)]
    mod fake {
        use std::os::unix::fs::PermissionsExt;
        use std::path::{Path, PathBuf};

        use super::super::Gh;

        const SCRIPT: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$*" >> "$D/calls"
case "$1 $2" in
  "auth status") key=auth ;;
  "run list")
    limit=""; prev=""
    for a in "$@"; do
      if [ "$prev" = "--limit" ]; then limit="$a"; fi
      prev="$a"
    done
    key="list-$limit" ;;
  "run view")
    key="view-$3"
    for a in "$@"; do
      if [ "$a" = "--log-failed" ]; then key="log-$3"; fi
    done ;;
  *) echo "unknown call: $*" >&2; exit 1 ;;
esac
n=$(cat "$D/$key.count" 2>/dev/null || echo 0)
n=$((n + 1))
echo "$n" > "$D/$key.count"
f="$D/$key"
i=$n
while [ "$i" -gt 0 ]; do
  if [ -f "$D/$key.$i" ]; then f="$D/$key.$i"; break; fi
  i=$((i - 1))
done
[ -f "$f" ] && cat "$f"
[ -f "$f.err" ] && cat "$f.err" >&2
[ -f "$f.rmself" ] && rm -f "$0"
code=0
[ -f "$f.exit" ] && code=$(cat "$f.exit")
exit "$code"
"#;

        /// A folder with the fake `gh` and its answers; removed on drop.
        pub(super) struct FakeGh {
            pub dir: PathBuf,
        }

        impl FakeGh {
            pub fn new(name: &str) -> Self {
                let dir = std::env::temp_dir().join(format!(
                    "brigadier-ci-{name}-{}",
                    uuid::Uuid::new_v4().simple()
                ));
                std::fs::create_dir_all(&dir).unwrap();
                let script = dir.join("gh");
                std::fs::write(&script, SCRIPT).unwrap();
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
                Self { dir }
            }

            pub fn gh(&self) -> Gh {
                Gh {
                    program: self.dir.join("gh"),
                    vars: vec![("PATH".into(), "/usr/bin:/bin".into())],
                }
            }

            /// Writes the answer file `name` (e.g. `list-20.3`).
            pub fn answer(&self, name: &str, body: &str) -> &Self {
                std::fs::write(self.dir.join(name), body).unwrap();
                self
            }

            pub fn path(&self) -> &Path {
                &self.dir
            }

            /// Every call so far, one per line.
            pub fn calls(&self) -> Vec<String> {
                std::fs::read_to_string(self.dir.join("calls"))
                    .unwrap_or_default()
                    .lines()
                    .map(str::to_string)
                    .collect()
            }
        }

        impl Drop for FakeGh {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }
    }

    #[cfg(unix)]
    mod with_fake_gh {
        use std::path::Path;
        use std::time::Duration;

        use super::super::*;
        use super::SHA;
        use super::fake::FakeGh;

        const BRANCH: &str = "main";
        const REPO: &str = "o/n";

        /// Fast polls. The first-run window is generous: a script just written can take a
        /// while to start the first time, and only the no-runs test waits it out.
        fn timing() -> Timing {
            Timing {
                poll: Duration::from_millis(10),
                first_run: Duration::from_secs(5),
                settle: Duration::from_millis(60),
            }
        }

        fn listed(id: u64, workflow: &str, status: &str, conclusion: &str) -> String {
            listed_at(id, workflow, status, conclusion, SHA, BRANCH)
        }

        fn listed_at(
            id: u64,
            workflow: &str,
            status: &str,
            conclusion: &str,
            sha: &str,
            branch: &str,
        ) -> String {
            format!(
                r#"{{"databaseId":{id},"workflowName":"{workflow}","status":"{status}","conclusion":"{conclusion}","url":"https://github.com/o/n/actions/runs/{id}","headSha":"{sha}","headBranch":"{branch}","event":"push"}}"#
            )
        }

        fn list(runs: &[String]) -> String {
            format!("[{}]", runs.join(","))
        }

        /// A `run view --json status,conclusion,jobs` answer; jobs are `(name, status,
        /// conclusion)`.
        fn view(status: &str, conclusion: &str, jobs: &[(&str, &str, &str)]) -> String {
            let jobs: Vec<String> = jobs
                .iter()
                .map(|(name, status, conclusion)| {
                    format!(
                        r#"{{"name":"{name}","status":"{status}","conclusion":"{conclusion}","url":"https://github.com/o/n/actions/runs/1/job/{name}","databaseId":1}}"#
                    )
                })
                .collect();
            format!(
                r#"{{"status":"{status}","conclusion":"{conclusion}","jobs":[{}]}}"#,
                jobs.join(",")
            )
        }

        async fn run_watch(fake: &FakeGh, timing: Timing) -> (Outcome, Vec<Progress>) {
            let mut seen = Vec::new();
            let outcome = watch(&fake.gh(), REPO, BRANCH, SHA, timing, |p| {
                seen.push(p.clone())
            })
            .await;
            (outcome, seen)
        }

        #[tokio::test]
        async fn delayed_discovery_waits_then_follows_the_run() {
            let fake = FakeGh::new("delayed");
            fake.answer("list-20.1", "[]")
                .answer("list-20.4", &list(&[listed(1, "CI", "in_progress", "")]))
                .answer(
                    "list-20.6",
                    &list(&[listed(1, "CI", "completed", "success")]),
                )
                .answer(
                    "view-1.1",
                    &view(
                        "in_progress",
                        "",
                        &[("a", "completed", "success"), ("b", "in_progress", "")],
                    ),
                )
                .answer(
                    "view-1.3",
                    &view(
                        "completed",
                        "success",
                        &[("a", "completed", "success"), ("b", "completed", "success")],
                    ),
                );
            let (outcome, seen) = run_watch(&fake, timing()).await;
            let Outcome::Passed {
                jobs,
                runs,
                incomplete,
            } = &outcome
            else {
                panic!("{outcome:?}");
            };
            assert_eq!((*jobs, runs.len(), *incomplete), (2, 1, false));
            assert!(
                seen[..3]
                    .iter()
                    .all(|p| p.waiting_for_first && p.runs.is_empty())
            );
            assert!(!seen[3].waiting_for_first);
            assert_eq!(
                (seen[3].runs[0].jobs_done, seen[3].runs[0].jobs_total),
                (1, 2)
            );
            assert_eq!(
                outcome_envelope(SHA, &outcome),
                "[ci 97c298d5 · passed · 2 jobs]"
            );
            // Every call names the repository.
            assert!(fake.calls().iter().all(|call| call.contains("--repo o/n")));
        }

        #[tokio::test]
        async fn no_run_in_the_first_window_is_no_runs() {
            let fake = FakeGh::new("noruns");
            fake.answer("list-20", "[]");
            let timing = Timing {
                first_run: Duration::from_millis(300),
                ..timing()
            };
            let (outcome, seen) = run_watch(&fake, timing).await;
            assert!(matches!(outcome, Outcome::NoRuns), "{outcome:?}");
            // How many polls fit in the window depends on how fast gh starts: each said it
            // waited.
            assert!(!seen.is_empty());
            assert!(seen.iter().all(|p| p.waiting_for_first));
        }

        #[tokio::test]
        async fn a_later_workflow_joins_the_watch() {
            let fake = FakeGh::new("staggered");
            let done = view(
                "completed",
                "success",
                &[("a", "completed", "success"), ("b", "completed", "success")],
            );
            fake.answer(
                "list-20.1",
                &list(&[listed(1, "CI", "completed", "success")]),
            )
            // The second workflow shows up after the first completed, inside settle.
            .answer(
                "list-20.3",
                &list(&[
                    listed(1, "CI", "completed", "success"),
                    listed(2, "Deploy", "in_progress", ""),
                ]),
            )
            .answer("view-1", &done)
            .answer(
                "view-2.1",
                &view("in_progress", "", &[("c", "in_progress", "")]),
            )
            .answer(
                "view-2.2",
                &view(
                    "completed",
                    "success",
                    &[
                        ("c", "completed", "success"),
                        ("d", "completed", "skipped"),
                        ("e", "completed", "success"),
                    ],
                ),
            );
            // A settle long enough that the second workflow surely lands inside it.
            let timing = Timing {
                settle: Duration::from_millis(500),
                ..timing()
            };
            let (outcome, _) = run_watch(&fake, timing).await;
            let Outcome::Passed { jobs, runs, .. } = &outcome else {
                panic!("{outcome:?}");
            };
            assert_eq!(*jobs, 5);
            assert_eq!(
                runs.iter().map(|r| r.workflow.as_str()).collect::<Vec<_>>(),
                ["CI", "Deploy"]
            );
            // A completed run's jobs are read once.
            let views_of_1 = fake
                .calls()
                .iter()
                .filter(|c| c.starts_with("run view 1 "))
                .count();
            assert_eq!(views_of_1, 1);
        }

        #[tokio::test]
        async fn a_rerun_that_ends_between_two_polls_replaces_the_first_result() {
            let fake = FakeGh::new("rerun");
            let attempt = |n: u32, conclusion: &str| {
                listed(1, "CI", "completed", conclusion).replace(
                    r#""event":"push""#,
                    &format!(r#""event":"push","attempt":{n}"#),
                )
            };
            fake.answer("list-20.1", &list(&[attempt(1, "failure")]))
                // Re-run and ended again before the next poll: still completed, same id.
                .answer("list-20.2", &list(&[attempt(2, "success")]))
                .answer(
                    "view-1.1",
                    &view("completed", "failure", &[("test", "completed", "failure")]),
                )
                .answer(
                    "view-1.2",
                    &view("completed", "success", &[("test", "completed", "success")]),
                );
            let timing = Timing {
                settle: Duration::from_millis(500),
                ..timing()
            };
            let (outcome, _) = run_watch(&fake, timing).await;
            assert!(matches!(outcome, Outcome::Passed { .. }), "{outcome:?}");
        }

        #[tokio::test]
        async fn a_run_in_progress_past_settle_keeps_the_watch_open() {
            let fake = FakeGh::new("long");
            fake.answer("list-20", &list(&[listed(1, "CI", "in_progress", "")]))
                .answer(
                    "view-1",
                    &view("in_progress", "", &[("a", "in_progress", "")]),
                )
                .answer(
                    "view-1.20",
                    &view("completed", "success", &[("a", "completed", "success")]),
                );
            let started = std::time::Instant::now();
            let (outcome, seen) = run_watch(&fake, timing()).await;
            assert!(
                matches!(outcome, Outcome::Passed { jobs: 1, .. }),
                "{outcome:?}"
            );
            assert!(seen.len() >= 20);
            // Twenty 10 ms polls: well past the 60 ms settle.
            assert!(started.elapsed() >= Duration::from_millis(190));
        }

        #[tokio::test]
        async fn runs_of_another_sha_or_branch_are_ignored() {
            let fake = FakeGh::new("binding");
            fake.answer(
                "list-20",
                &list(&[
                    listed_at(7, "CI", "completed", "failure", "0000000000", BRANCH),
                    listed_at(8, "CI", "completed", "failure", SHA, "other"),
                    listed(1, "CI", "completed", "success"),
                ]),
            )
            .answer(
                "view-1",
                &view("completed", "success", &[("a", "completed", "success")]),
            );
            let (outcome, _) = run_watch(&fake, timing()).await;
            let Outcome::Passed { runs, jobs, .. } = &outcome else {
                panic!("{outcome:?}");
            };
            assert_eq!((runs.len(), runs[0].id, *jobs), (1, 1, 1));
            assert!(
                !fake
                    .calls()
                    .iter()
                    .any(|c| c.starts_with("run view 7") || c.starts_with("run view 8"))
            );
        }

        #[tokio::test]
        async fn failing_jobs_come_from_run_view_with_their_log() {
            let fake = FakeGh::new("failed");
            // The run list's conclusion is not what counts: the jobs are.
            fake.answer("list-20", &list(&[listed(1, "CI", "completed", "failure")]))
                .answer(
                    "view-1",
                    &view(
                        "completed",
                        "failure",
                        &[
                            ("fmt", "completed", "success"),
                            ("test", "completed", "failure"),
                            ("lint", "completed", "cancelled"),
                            ("build", "completed", "success"),
                            ("docs", "completed", "skipped"),
                        ],
                    ),
                )
                .answer(
                    "log-1",
                    "test\tRun tests\t2026-10-10T22:47:23Z running 3 tests\ntest\tRun tests\t2026-10-10T22:47:24Z FAIL ThreadScrollRender.test.ts:60\n",
                );
            let (outcome, _) = run_watch(&fake, timing()).await;
            let Outcome::Failed {
                jobs,
                failed,
                key_lines,
                full_log,
                ..
            } = &outcome
            else {
                panic!("{outcome:?}");
            };
            assert_eq!(*jobs, 5);
            assert_eq!(
                failed.iter().map(|j| j.job.as_str()).collect::<Vec<_>>(),
                ["test", "lint"]
            );
            assert!(key_lines.contains("test: FAIL ThreadScrollRender.test.ts:60"));
            assert!(full_log.contains("2026-10-10T22:47:24Z FAIL ThreadScrollRender"));
            let text = outcome_envelope(SHA, &outcome);
            assert!(
                text.starts_with(
                    "[ci 97c298d5 · failed · 2 of 5 jobs failed]\nCI › test (https://github.com/o/n/actions/runs/1/job/test)\nCI › lint ("
                ),
                "{text}"
            );
            assert!(text.contains("FAIL ThreadScrollRender.test.ts:60"));
        }

        #[tokio::test]
        async fn a_failed_run_without_jobs_still_fails() {
            let fake = FakeGh::new("startup");
            fake.answer(
                "list-20",
                &list(&[listed(1, "CI", "completed", "startup_failure")]),
            )
            .answer("view-1", &view("completed", "startup_failure", &[]))
            .answer("log-1", "");
            let (outcome, _) = run_watch(&fake, timing()).await;
            assert!(matches!(&outcome, Outcome::Failed { failed, .. } if failed.len() == 1));
            assert!(outcome_envelope(SHA, &outcome).starts_with(
                "[ci 97c298d5 · failed · 1 run failed]\nCI › the run (startup_failure)"
            ));
        }

        fn many(count: u64) -> String {
            list(
                &(1..=count)
                    .map(|id| listed(id, &format!("W{id}"), "completed", "success"))
                    .collect::<Vec<_>>(),
            )
        }

        fn one_job_views(fake: &FakeGh, count: u64) {
            for id in 1..=count {
                fake.answer(
                    &format!("view-{id}"),
                    &view("completed", "success", &[("j", "completed", "success")]),
                );
            }
        }

        #[tokio::test]
        async fn more_than_twenty_runs_are_all_seen() {
            let fake = FakeGh::new("paging");
            fake.answer("list-20", &many(20))
                .answer("list-40", &many(25));
            one_job_views(&fake, 25);
            let (outcome, _) = run_watch(&fake, timing()).await;
            let Outcome::Passed {
                runs,
                jobs,
                incomplete,
            } = &outcome
            else {
                panic!("{outcome:?}");
            };
            assert_eq!((runs.len(), *jobs, *incomplete), (25, 25, false));
            // After the first poll, polls start at the raised limit.
            let calls = fake.calls();
            let lists_at_20 = calls.iter().filter(|c| c.ends_with("--limit 20")).count();
            assert_eq!(lists_at_20, 1);
        }

        #[tokio::test]
        async fn hitting_the_cap_reports_incomplete() {
            let fake = FakeGh::new("cap");
            fake.answer("list-20", &many(20))
                .answer("list-40", &many(40));
            one_job_views(&fake, 40);
            let outcome = watch_capped(&fake.gh(), REPO, BRANCH, SHA, timing(), 40, |_| {}).await;
            assert!(
                matches!(
                    outcome,
                    Outcome::Passed {
                        incomplete: true,
                        jobs: 40,
                        ..
                    }
                ),
                "{outcome:?}"
            );
            assert!(outcome_envelope(SHA, &outcome).contains("only the first"));
        }

        /// A poll that fails says nothing of the runs: with every listing failing, the
        /// first-run window passing doesn't make it "no runs"; gh failing does end it.
        #[tokio::test]
        async fn failed_polls_end_nothing_until_gh_kept_failing() {
            let fake = FakeGh::new("failing");
            fake.answer("list-20", "")
                .answer("list-20.exit", "1")
                .answer("list-20.err", "HTTP 500: Server Error\n");
            let timing = Timing {
                first_run: Duration::ZERO,
                ..timing()
            };
            let (outcome, _) = run_watch(&fake, timing).await;
            assert!(
                matches!(&outcome, Outcome::GhUnavailable { why } if why.starts_with("gh kept failing")),
                "{outcome:?}"
            );
        }

        #[tokio::test]
        async fn gh_going_missing_mid_watch_ends_it() {
            let fake = FakeGh::new("missing");
            fake.answer("list-20", &list(&[listed(1, "CI", "in_progress", "")]))
                .answer(
                    "view-1",
                    &view("in_progress", "", &[("a", "in_progress", "")]),
                )
                .answer(
                    "view-1.2",
                    &view("in_progress", "", &[("a", "in_progress", "")]),
                )
                .answer("view-1.2.rmself", "");
            let (outcome, seen) = run_watch(&fake, timing()).await;
            assert!(
                matches!(&outcome, Outcome::GhUnavailable { why } if why.starts_with("gh couldn't start")),
                "{outcome:?}"
            );
            assert_eq!(seen.len(), 2);
        }

        #[tokio::test]
        async fn gh_signed_out_mid_watch_ends_it() {
            let fake = FakeGh::new("signedout");
            fake.answer("list-20", &list(&[listed(1, "CI", "in_progress", "")]))
                .answer("list-20.2", "")
                .answer("list-20.2.exit", "4")
                .answer(
                    "list-20.2.err",
                    "To get started with GitHub CLI, please run:  gh auth login\n",
                )
                .answer(
                    "view-1",
                    &view("in_progress", "", &[("a", "in_progress", "")]),
                );
            let (outcome, _) = run_watch(&fake, timing()).await;
            assert!(
                matches!(&outcome, Outcome::GhUnavailable { why } if why == "gh isn't signed in to github.com"),
                "{outcome:?}"
            );
            assert_eq!(
                outcome_envelope(SHA, &outcome),
                "[ci 97c298d5 · can't watch] gh isn't signed in to github.com"
            );
        }

        fn git(dir: &Path, args: &[&str]) -> String {
            let out = std::process::Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@t",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(dir)
                .output()
                .unwrap();
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        }

        /// A repository with `files` committed; returns the commit's sha.
        fn repo_with(dir: &Path, files: &[&str]) -> String {
            std::fs::create_dir_all(dir).unwrap();
            git(dir, &["init", "-q", "-b", "main"]);
            for file in files {
                let path = dir.join(file);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(&path, "x\n").unwrap();
            }
            std::fs::write(dir.join("README"), "x\n").unwrap();
            git(dir, &["add", "-A"]);
            git(dir, &["commit", "-q", "-m", "x"]);
            git(dir, &["rev-parse", "HEAD"])
        }

        /// A fix pushed with `HEAD:main` from a session branch leaves the local `main` behind:
        /// the thread hears it. A push the local branch has, or of a branch with no local one,
        /// says nothing.
        #[tokio::test]
        async fn a_push_the_local_branch_lacks_is_named() {
            let fake = FakeGh::new("parted");
            let repo = fake.path().join("repo");
            let base = repo_with(&repo, &[]);
            assert_eq!(parted_note(&repo, "main", &base).await, "");
            git(&repo, &["checkout", "-q", "-b", "brigadier/x/session"]);
            std::fs::write(repo.join("fix"), "x\n").unwrap();
            git(&repo, &["add", "-A"]);
            git(&repo, &["commit", "-q", "-m", "fix"]);
            let fix = git(&repo, &["rev-parse", "HEAD"]);
            let note = parted_note(&repo, "main", &fix).await;
            assert!(note.contains("The local `main` doesn't have"), "{note}");
            assert!(note.contains(short_sha(&fix)), "{note}");
            assert_eq!(parted_note(&repo, "brigadier/x/session", &fix).await, "");
            assert_eq!(parted_note(&repo, "release", &fix).await, "");
        }

        #[tokio::test]
        async fn probe_watchable() {
            let fake = FakeGh::new("probe-ok");
            fake.answer("auth", "");
            let repo = fake.path().join("repo");
            let sha = repo_with(&repo, &[".github/workflows/ci.yml"]);
            // The working tree doesn't count: only the pushed commit.
            std::fs::remove_dir_all(repo.join(".github")).unwrap();
            let probe = probe(&repo, "git@github.com:o/n.git", &sha, Some(&fake.gh())).await;
            assert_eq!(
                probe,
                Probe::Watchable {
                    repo: "o/n".into(),
                    host: "github.com".into()
                }
            );
            assert_eq!(fake.calls(), ["auth status --hostname github.com"]);
        }

        #[tokio::test]
        async fn probe_gitlab_is_not_watchable_with_its_link() {
            let fake = FakeGh::new("probe-gitlab");
            let repo = fake.path().join("repo");
            let sha = repo_with(&repo, &[".gitlab-ci.yml"]);
            let probe = probe(&repo, "git@gitlab.com:o/n.git", &sha, Some(&fake.gh())).await;
            assert_eq!(
                probe,
                Probe::NotWatchable {
                    why: "This repository's CI is GitLab CI".into(),
                    link: Some(format!("https://gitlab.com/o/n/-/commit/{sha}")),
                }
            );
        }

        #[tokio::test]
        async fn probe_without_gh_is_not_watchable() {
            let fake = FakeGh::new("probe-nogh");
            let repo = fake.path().join("repo");
            let sha = repo_with(&repo, &[".github/workflows/ci.yaml"]);
            let link = Some(format!("https://github.com/o/n/commit/{sha}"));
            let missing = Gh {
                program: fake.path().join("no-such-gh"),
                vars: Vec::new(),
            };
            for gh in [None, Some(&missing)] {
                let probe = probe(&repo, "https://github.com/o/n.git", &sha, gh).await;
                assert_eq!(
                    probe,
                    Probe::NotWatchable {
                        why: "gh isn't installed".into(),
                        link: link.clone(),
                    }
                );
            }
        }

        #[tokio::test]
        async fn probe_signed_out_is_not_watchable() {
            let fake = FakeGh::new("probe-signedout");
            fake.answer("auth", "").answer("auth.exit", "1").answer(
                "auth.err",
                "You are not logged into any accounts on github.com\n",
            );
            let repo = fake.path().join("repo");
            let sha = repo_with(&repo, &[".github/workflows/ci.yml"]);
            let probe = probe(&repo, "ssh://git@github.com/o/n", &sha, Some(&fake.gh())).await;
            assert_eq!(
                probe,
                Probe::NotWatchable {
                    why: "gh isn't signed in to github.com".into(),
                    link: Some(format!("https://github.com/o/n/commit/{sha}")),
                }
            );
        }

        #[tokio::test]
        async fn probe_no_ci() {
            let fake = FakeGh::new("probe-noci");
            let repo = fake.path().join("repo");
            let sha = repo_with(&repo, &["src/main.rs", ".github/dependabot.yml"]);
            let probe = probe(&repo, "git@github.com:o/n.git", &sha, Some(&fake.gh())).await;
            assert_eq!(probe, Probe::NoCi);
            assert!(fake.calls().is_empty());
        }
    }
}
