//! One-shot reviews (THREAD-PLAN.md §2 Q12): a change, or the outline of one, read once and
//! read-only by the vendor other than its author's, in a checkout of its own. Nothing waits
//! for one; the session manager hands the findings to whoever asked.
//!
//! - **Codex** runs `codex exec review --base <base>` (an outline: `codex exec` with the review
//!   asked for on stdin), read-only, at effort high. `--json` prints its events, which carry the
//!   turn's token use; `-o` keeps its final message, the review ([`run_codex`]). A range review
//!   runs in a child thread whose use `--json` reports as zero (codex-cli 0.160.1), so it is read
//!   from that thread's rollout instead ([`child_usage`]). A review of a
//!   range takes no prompt of its own: Codex's own review instructions apply.
//! - **Claude** runs through Brigadier's adapter with read-only access and only the tools that
//!   read a change; the range is named in the prompt ([`claude_prompt`]).
//!
//! Either way each finding is a line that starts with its priority (`[P1] …`), which
//! [`count_findings`] counts.

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::cli::CliEnv;
use brigadier_providers::process::{self, Options};
use brigadier_providers::{Artifact, Ledger, ProviderKind, TokenUsage};
use brigadier_sandbox::Platform;
use serde_json::Value;

/// A reviewer's reasoning effort.
pub const EFFORT: &str = "high";

/// How long a stopped review's CLI gets to exit before its process tree is killed.
const EXIT_GRACE: Duration = Duration::from_secs(3);

/// The role a Claude reviewer's session starts with.
pub const REVIEW_ROLE: &str = "You are a one-shot reviewer for Brigadier. You read one piece of another model's work, once, and change nothing: your final message is your review, and nobody can answer questions. Read precisely (line ranges, the diff and the code around it), never the whole repository.";

/// How every reviewer writes its findings, so they can be counted.
const FINDINGS_FORMAT: &str = "Write each finding as one line, `- [P1] what is wrong — path:line`, with the fix on the next line, indented. P0 must be fixed before anything ships, P1 is a bug to fix now, P2 should be fixed, P3 is minor. Don't restate the work or praise it. No findings: write exactly `No findings.`";

/// What a one-shot review reads.
#[derive(Debug, Clone, Copy)]
pub enum Subject<'a> {
    /// The commits after `base`, up to the checkout's HEAD.
    Code { base: &'a str },
    /// A lead's outline of work not started yet, against its brief; the checkout holds the
    /// code it starts from.
    Plan { brief: &'a str, outline: &'a str },
}

/// What a review came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    /// The reviewer's final message.
    pub text: String,
    /// The findings it lists ([`count_findings`]).
    pub findings: u32,
    /// What it used, when its CLI said (Codex; Claude's use is metered from its session's
    /// events instead).
    pub usage: Option<TokenUsage>,
}

/// The prompt a Claude reviewer gets.
pub fn claude_prompt(subject: Subject<'_>) -> String {
    match subject {
        Subject::Code { base } => format!(
            "Review the change in this checkout: `git diff {base}...HEAD`, made of the commits in `git log {base}..HEAD`. Read the whole diff and the code around it. Find what is wrong: bugs, a caller or contract the change breaks, missing error handling, a security problem, stray files, needless scope. Run only `git diff`, `git log` and `git show`; don't build or run tests.\n\n{FINDINGS_FORMAT}"
        ),
        Subject::Plan { .. } => plan_prompt(subject),
    }
}

/// The prompt for a review of an outline, for either vendor.
fn plan_prompt(subject: Subject<'_>) -> String {
    let (brief, outline) = match subject {
        Subject::Plan { brief, outline } => (brief, outline),
        Subject::Code { .. } => ("", ""),
    };
    format!(
        "Review this outline before any code is written. It is advisory: the orchestrator weighs your findings against the brief, and the work may already have started when they arrive.\n\nThe brief:\n{brief}\n\nThe outline:\n{outline}\n\nRead the code the outline names (precise reads, line ranges). Find what would make the work fail or miss its brief: a wrong assumption about the code, a missed caller or file, a step in the wrong order, a \"done when\" it can't check for real, needless scope. Change nothing.\n\n{FINDINGS_FORMAT}"
    )
}

/// `codex exec`'s arguments for a review of `subject`, its final message written to `output`.
pub fn codex_args(subject: Subject<'_>, model: Option<&str>, output: &Path) -> Vec<String> {
    let mut args: Vec<String> = vec!["exec".into()];
    if let Subject::Code { .. } = subject {
        args.push("review".into());
    }
    args.extend([
        "-c".into(),
        format!("model_reasoning_effort=\"{EFFORT}\""),
        "-c".into(),
        "sandbox_mode=\"read-only\"".into(),
        "--json".into(),
        "-o".into(),
        output.display().to_string(),
    ]);
    if let Some(model) = model {
        args.extend(["-m".into(), model.to_owned()]);
    }
    match subject {
        // `exec review` takes no prompt together with `--base`.
        Subject::Code { base } => args.extend(["--base".into(), base.to_owned()]),
        // The prompt comes on stdin.
        Subject::Plan { .. } => args.push("-".into()),
    }
    args
}

/// How many findings a review lists: its lines that start (after a list marker) with a
/// priority, `[P0]` to `[P3]`.
pub fn count_findings(text: &str) -> u32 {
    let count = text
        .lines()
        .filter(|line| {
            let line = line.trim_start();
            let line = line
                .strip_prefix("- ")
                .or_else(|| line.strip_prefix("* "))
                .or_else(|| line.strip_prefix("• "))
                .unwrap_or_else(|| {
                    // "1. [P1] …"
                    let digits = line.trim_start_matches(|c: char| c.is_ascii_digit());
                    if digits.len() < line.len() {
                        digits.strip_prefix(". ").unwrap_or(line)
                    } else {
                        line
                    }
                })
                .trim_start()
                .trim_start_matches("**");
            let mut chars = line.chars();
            chars.next() == Some('[')
                && chars.next() == Some('P')
                && chars.next().is_some_and(|c| c.is_ascii_digit())
                && chars.next() == Some(']')
        })
        .count();
    u32::try_from(count).unwrap_or(u32::MAX)
}

/// What `codex exec --json` printed, as far as a review needs it.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct CodexEvents {
    /// Its thread, from `thread.started`.
    pub thread_id: Option<String>,
    /// The thread's token use at its last completed turn.
    pub usage: Option<TokenUsage>,
    /// The last message the agent wrote.
    pub last_message: Option<String>,
    /// Why its turn failed, if it did.
    pub error: Option<String>,
}

impl CodexEvents {
    /// Takes in one line of its output; a line that isn't an event is skipped.
    pub fn read(&mut self, line: &str) {
        let Ok(event) = serde_json::from_str::<Value>(line) else {
            return;
        };
        match event.get("type").and_then(Value::as_str) {
            Some("thread.started") => {
                self.thread_id = event
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
            }
            // Its usage is the thread's total so far: the last one counts.
            Some("turn.completed") => {
                if let Some(usage) = event.get("usage") {
                    self.usage = Some(codex_usage(usage));
                }
            }
            Some("item.completed") => {
                let item = event.get("item");
                if item
                    .and_then(|item| item.get("type"))
                    .and_then(Value::as_str)
                    == Some("agent_message")
                    && let Some(text) = item
                        .and_then(|item| item.get("text"))
                        .and_then(Value::as_str)
                {
                    self.last_message = Some(text.to_owned());
                }
            }
            Some("turn.failed") => {
                self.error = event
                    .get("error")
                    .and_then(|error| error.get("message"))
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .or_else(|| Some("its turn failed".into()));
            }
            Some("error") => {
                if let Some(message) = event.get("message").and_then(Value::as_str) {
                    self.error = Some(message.to_owned());
                }
            }
            _ => {}
        }
    }
}

/// A `turn.completed` event's usage. Codex counts cached input inside `input_tokens`; Brigadier
/// keeps the two apart, as for its app-server sessions. A count it leaves out is 0 (it reports
/// no cache writes).
fn codex_usage(usage: &Value) -> TokenUsage {
    let count = |key: &str| usage.get(key).and_then(Value::as_i64).unwrap_or(0);
    let cached = count("cached_input_tokens");
    TokenUsage {
        input_tokens: (count("input_tokens") - cached).max(0),
        cached_input_tokens: cached,
        cache_write_tokens: count("cache_write_input_tokens"),
        output_tokens: count("output_tokens"),
        reasoning_tokens: count("reasoning_output_tokens"),
        cost_usd: None,
    }
}

/// One Codex review, run to its end by [`run_codex`].
pub struct CodexRun<'a> {
    pub platform: Arc<dyn Platform>,
    pub env: &'a CliEnv,
    /// The checkout it reviews: its working folder, and the folder whatever it leaves running
    /// is ended in.
    pub cwd: &'a Path,
    /// Where its final message is written (outside the checkout).
    pub output: &'a Path,
    pub model: Option<&'a str>,
    pub subject: Subject<'a>,
    /// Records its process for the crash sweep.
    pub ledger: Arc<dyn Ledger>,
    /// Its time box.
    pub time: Duration,
}

/// Runs `codex exec` for one review and returns it, or why there is none.
pub async fn run_codex(run: CodexRun<'_>) -> Result<Review, String> {
    let binary = run
        .env
        .resolve(ProviderKind::Codex)
        .ok_or_else(|| "Codex isn't installed".to_owned())?;
    let mut spec = run.env.spec(&binary);
    spec.args = codex_args(run.subject, run.model, run.output)
        .into_iter()
        .map(Into::into)
        .collect();
    spec.cwd = Some(run.cwd.to_owned());
    spec.low_priority = true;
    let spawned = process::spawn(
        run.platform,
        &spec,
        Options {
            ledger: Some(run.ledger.clone()),
            owned_dir: Some(run.cwd.to_owned()),
            ..Options::default()
        },
    )
    .map_err(|err| format!("Codex didn't start: {err}"))?;
    let process = spawned.process;
    let mut stdout = spawned.stdout;
    let recorded = run
        .ledger
        .record(Artifact::Process {
            pid: process.pid(),
            started_at_ms: process.started_at_ms(),
        })
        .await;
    if let Err(err) = recorded {
        tracing::warn!(pid = process.pid(), error = %err, "could not record a review's process");
    }
    if let Subject::Plan { .. } = run.subject
        && let Err(err) = process.write_line(&plan_prompt(run.subject)).await
    {
        process.shutdown(EXIT_GRACE).await;
        return Err(format!("Codex didn't take the review: {err}"));
    }
    process.close_stdin().await;
    let mut events = CodexEvents::default();
    let ran = tokio::time::timeout(run.time, async {
        while let Some(line) = stdout.recv().await {
            events.read(&line);
        }
        process.exited().await
    })
    .await;
    let Ok(exit) = ran else {
        process.shutdown(EXIT_GRACE).await;
        return Err(format!(
            "it ran out of its {}-minute time box",
            run.time.as_secs() / 60
        ));
    };
    let written = tokio::fs::read_to_string(run.output)
        .await
        .unwrap_or_default();
    let text = Some(written.trim().to_owned())
        .filter(|text| !text.is_empty())
        .or_else(|| {
            events
                .last_message
                .as_deref()
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        });
    let Some(text) = text else {
        let why = events
            .error
            .or_else(|| process.stderr_tail())
            .unwrap_or_else(|| match exit.code {
                Some(code) => format!("Codex exited with code {code}"),
                None => "Codex ended without a review".to_owned(),
            });
        return Err(why);
    };
    let usage = match (events.usage, events.thread_id) {
        (Some(usage), _) if usage != TokenUsage::default() => Some(usage),
        (reported, Some(thread)) => {
            let sessions = codex_home(run.env).map(|home| home.join("sessions"));
            tokio::task::spawn_blocking(move || child_usage(&sessions?, &thread))
                .await
                .ok()
                .flatten()
                .or(reported)
        }
        (reported, None) => reported,
    };
    Ok(Review {
        findings: count_findings(&text),
        text,
        usage,
    })
}

/// `$CODEX_HOME`, by default `~/.codex`.
fn codex_home(env: &CliEnv) -> Option<PathBuf> {
    match env.var("CODEX_HOME") {
        Some(home) if !home.is_empty() => Some(PathBuf::from(home)),
        _ => Some(env.home()?.join(".codex")),
    }
}

/// What the child threads of `thread` used, from their rollouts: Codex keeps one per thread in
/// `<sessions>/YYYY/MM/DD/rollout-<time>-<thread>.jsonl`, its first line the `session_meta`
/// (a child's names its `parent_thread_id`) and each `token_count` line the thread's total so
/// far. A child starts with its parent, so it is looked for in the parent's folder.
pub fn child_usage(sessions: &Path, thread: &str) -> Option<TokenUsage> {
    let suffix = format!("-{thread}.jsonl");
    let parent = rollouts(sessions)
        .into_iter()
        .find(|path| path.to_string_lossy().ends_with(&suffix))?;
    let mut total: Option<TokenUsage> = None;
    for path in fs::read_dir(parent.parent()?).ok()?.flatten() {
        let path = path.path();
        if path == parent || !is_rollout(&path) {
            continue;
        }
        let Ok(file) = File::open(&path) else {
            continue;
        };
        let mut lines = BufReader::new(file).lines().map_while(Result::ok);
        let is_child = lines
            .next()
            .and_then(|line| serde_json::from_str::<Value>(&line).ok())
            .is_some_and(|meta| {
                meta.pointer("/payload/parent_thread_id")
                    .and_then(Value::as_str)
                    == Some(thread)
            });
        if !is_child {
            continue;
        }
        let last = lines
            .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
            .filter_map(|line| {
                (line.pointer("/payload/type").and_then(Value::as_str) == Some("token_count"))
                    .then(|| line.pointer("/payload/info/total_token_usage").cloned())
                    .flatten()
            })
            .last();
        if let Some(usage) = last.as_ref().map(codex_usage) {
            let sum = total.get_or_insert_with(TokenUsage::default);
            sum.input_tokens += usage.input_tokens;
            sum.cached_input_tokens += usage.cached_input_tokens;
            sum.cache_write_tokens += usage.cache_write_tokens;
            sum.output_tokens += usage.output_tokens;
            sum.reasoning_tokens += usage.reasoning_tokens;
        }
    }
    total
}

/// Every rollout under `sessions` (its year, month and day folders).
fn rollouts(sessions: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![sessions.to_owned()];
    for _ in 0..3 {
        dirs = dirs
            .iter()
            .filter_map(|dir| fs::read_dir(dir).ok())
            .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect();
    }
    dirs.iter()
        .filter_map(|dir| fs::read_dir(dir).ok())
        .flat_map(|entries| entries.flatten().map(|entry| entry.path()))
        .filter(|path| is_rollout(path))
        .collect()
}

fn is_rollout(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
        && path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with("rollout-"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_usage_comes_from_the_last_completed_turn_with_cached_input_apart() {
        let lines = [
            r#"{"type":"thread.started","thread_id":"01a0f87e-4f28-7432-a8d6-b6bbdb44834b"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"error","message":"a warning"}}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Looking."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":1000,"cached_input_tokens":400,"output_tokens":10,"reasoning_output_tokens":0}}"#,
            r#"{"type":"item.completed","item":{"id":"item_2","type":"agent_message","text":"- [P2] Off by one — src/a.rs:3"}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":24350,"cached_input_tokens":11008,"cache_write_input_tokens":0,"output_tokens":162,"reasoning_output_tokens":40}}"#,
            "not json",
        ];
        let mut events = CodexEvents::default();
        for line in lines {
            events.read(line);
        }
        assert_eq!(
            events.usage,
            Some(TokenUsage {
                input_tokens: 24350 - 11008,
                cached_input_tokens: 11008,
                cache_write_tokens: 0,
                output_tokens: 162,
                reasoning_tokens: 40,
                cost_usd: None,
            })
        );
        assert_eq!(
            events.last_message.as_deref(),
            Some("- [P2] Off by one — src/a.rs:3")
        );
        // A warning item is no failure.
        assert_eq!(events.error, None);
        events.read(r#"{"type":"turn.failed","error":{"message":"usage limit reached"}}"#);
        assert_eq!(events.error.as_deref(), Some("usage limit reached"));
    }

    #[test]
    fn a_range_reviews_use_is_read_from_its_child_threads_rollout() {
        let sessions =
            std::env::temp_dir().join(format!("brigadier-review-usage-{}", std::process::id()));
        let day = sessions.join("2026/10/07");
        fs::create_dir_all(&day).unwrap();
        let parent = "01a116c2-7168-7023-a3d9-75ca70387e74";
        let mut events = CodexEvents::default();
        events.read(&format!(
            r#"{{"type":"thread.started","thread_id":"{parent}"}}"#
        ));
        events.read(r#"{"type":"turn.completed","usage":{"input_tokens":0,"cached_input_tokens":0,"cache_write_input_tokens":0,"output_tokens":0,"reasoning_output_tokens":0}}"#);
        assert_eq!(events.thread_id.as_deref(), Some(parent));
        assert_eq!(events.usage, Some(TokenUsage::default()));
        fs::write(
            day.join(format!("rollout-2026-10-07T17-26-44-{parent}.jsonl")),
            r#"{"type":"session_meta","payload":{"id":"01a116c2-7168-7023-a3d9-75ca70387e74"}}"#,
        )
        .unwrap();
        let token_count = |input: i64, cached: i64, output: i64| {
            format!(
                r#"{{"type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":{input},"cached_input_tokens":{cached},"cache_write_input_tokens":0,"output_tokens":{output},"reasoning_output_tokens":121}}}}}}}}"#
            )
        };
        let child = [
            format!(
                r#"{{"type":"session_meta","payload":{{"id":"01a116c2-71f5","parent_thread_id":"{parent}","source":{{"subagent":"review"}}}}}}"#
            ),
            token_count(1000, 500, 10),
            r#"{"type":"event_msg","payload":{"type":"token_count","info":null}}"#.to_owned(),
            token_count(152401, 117504, 1023),
        ];
        fs::write(
            day.join("rollout-2026-10-07T17-26-44-01a116c2-71f5.jsonl"),
            child.join("\n"),
        )
        .unwrap();
        // Another thread's child doesn't count.
        fs::write(
            day.join("rollout-2026-10-07T17-26-44-01a116c2-9999.jsonl"),
            [
                r#"{"type":"session_meta","payload":{"parent_thread_id":"someone-else"}}"#
                    .to_owned(),
                token_count(9, 0, 9),
            ]
            .join("\n"),
        )
        .unwrap();
        let usage = child_usage(&sessions, parent);
        fs::remove_dir_all(&sessions).unwrap();
        assert_eq!(
            usage,
            Some(TokenUsage {
                input_tokens: 152401 - 117504,
                cached_input_tokens: 117504,
                cache_write_tokens: 0,
                output_tokens: 1023,
                reasoning_tokens: 121,
                cost_usd: None,
            })
        );
        assert_eq!(
            child_usage(Path::new("/nonexistent/sessions"), parent),
            None
        );
    }

    #[test]
    fn findings_are_the_lines_that_start_with_a_priority() {
        let codex = "The change mostly works.\n\nFull review comments:\n\n- [P1] Guard the empty list — src/a.rs:10-12\n  `first()` panics on an empty list.\n- [P3] Typo — README.md:4\n  \"recieve\".";
        assert_eq!(count_findings(codex), 2);
        let claude = "1. [P2] Missing await — app/x.ts:7\n   Await it.\n**[P0]** leaks the token — y.rs:1\nSee [P1] above.";
        assert_eq!(count_findings(claude), 2);
        assert_eq!(count_findings("No findings."), 0);
        assert_eq!(count_findings("- [PX] not a priority"), 0);
    }

    #[test]
    fn a_range_review_takes_no_prompt_and_a_plan_review_reads_it_from_stdin() {
        let out = Path::new("/data/scratch/review-1/review.md");
        let code = codex_args(Subject::Code { base: "abc123" }, Some("gpt-6.1-sol"), out);
        assert_eq!(
            code,
            [
                "exec",
                "review",
                "-c",
                "model_reasoning_effort=\"high\"",
                "-c",
                "sandbox_mode=\"read-only\"",
                "--json",
                "-o",
                "/data/scratch/review-1/review.md",
                "-m",
                "gpt-6.1-sol",
                "--base",
                "abc123",
            ]
        );
        let plan = codex_args(
            Subject::Plan {
                brief: "Add a flag.",
                outline: "1. Parse it.",
            },
            None,
            out,
        );
        assert_eq!(plan.first().map(String::as_str), Some("exec"));
        assert!(!plan.iter().any(|arg| arg == "review" || arg == "--base"));
        assert_eq!(plan.last().map(String::as_str), Some("-"));
    }

    #[test]
    fn a_claude_reviewer_is_told_the_range_and_the_findings_format() {
        let prompt = claude_prompt(Subject::Code { base: "abc123" });
        assert!(prompt.contains("git diff abc123...HEAD"));
        assert!(prompt.contains("git log abc123..HEAD"));
        assert!(prompt.contains("[P1]"));
        assert!(prompt.contains("No findings."));
        let plan = claude_prompt(Subject::Plan {
            brief: "Add a flag.",
            outline: "1. Parse it.",
        });
        assert!(plan.contains("Add a flag.") && plan.contains("1. Parse it."));
    }
}
