//! The child threads a Codex thread starts (its auto-review "guardian", its subagents), read
//! from their rollouts: the parent's own totals leave their use out, so Brigadier meters it from
//! there ([`super::usage`]).

use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

use brigadier_providers::TokenUsage;
use brigadier_providers::cli::CliEnv;
use serde_json::Value;

/// `$CODEX_HOME`, by default `~/.codex`.
fn codex_home(env: &CliEnv) -> Option<PathBuf> {
    match env.var("CODEX_HOME") {
        Some(home) if !home.is_empty() => Some(PathBuf::from(home)),
        _ => Some(env.home()?.join(".codex")),
    }
}

/// Where Codex keeps its rollouts for `env`: `$CODEX_HOME/sessions`.
pub(crate) fn codex_sessions(env: &CliEnv) -> Option<PathBuf> {
    codex_home(env).map(|home| home.join("sessions"))
}

/// A thread another thread started, and what it used.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ChildThread {
    pub(crate) id: String,
    pub(crate) usage: Option<TokenUsage>,
}

/// A rollout's token count. Codex counts cached input inside `input_tokens`; Brigadier keeps
/// the two apart, as for its app-server sessions. A count it leaves out is 0 (it reports no
/// cache writes).
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
        model_calls: 0,
        cache_write_1h_tokens: 0,
        cache_write_5m_tokens: 0,
    }
}

/// The child threads of `thread`, from their rollouts: Codex keeps one per thread in
/// `<sessions>/YYYY/MM/DD/rollout-<time>-<thread>.jsonl`, its first line the `session_meta`
/// (a child's names its `parent_thread_id`) and each `token_count` line the thread's total so
/// far.
#[cfg(test)]
pub(crate) fn child_threads(sessions: &Path, thread: &str) -> Vec<ChildThread> {
    child_threads_since(sessions, thread, None)
}

/// The same, among the rollouts written since `since` (all when absent). A child starts with
/// its parent or later, so it is looked for in the parent's day folder and the ones after it: a
/// long-lived thread (a worker resumed the next day, a session's thread) starts children on
/// later days, and one auto-review child serves its parent's whole life.
pub(crate) fn child_threads_since(
    sessions: &Path,
    thread: &str,
    since: Option<std::time::SystemTime>,
) -> Vec<ChildThread> {
    let suffix = format!("-{thread}.jsonl");
    let all = rollouts(sessions);
    let Some(parent) = all
        .iter()
        .find(|path| path.to_string_lossy().ends_with(&suffix))
    else {
        return Vec::new();
    };
    let Some(first_day) = parent.parent() else {
        return Vec::new();
    };
    let mut children = Vec::new();
    for path in &all {
        if path == parent || path.parent().is_none_or(|day| day < first_day) {
            continue;
        }
        if let Some(since) = since
            && fs::metadata(path)
                .and_then(|meta| meta.modified())
                .is_ok_and(|modified| modified < since)
        {
            continue;
        }
        let Ok(file) = File::open(path) else {
            continue;
        };
        let mut lines = BufReader::new(file).lines().map_while(Result::ok);
        let Some(meta) = lines
            .next()
            .and_then(|line| serde_json::from_str::<Value>(&line).ok())
        else {
            continue;
        };
        if meta
            .pointer("/payload/parent_thread_id")
            .and_then(Value::as_str)
            != Some(thread)
        {
            continue;
        }
        let Some(id) = meta.pointer("/payload/id").and_then(Value::as_str) else {
            continue;
        };
        let usage = lines
            .filter_map(|line| serde_json::from_str::<Value>(&line).ok())
            .filter_map(|line| {
                (line.pointer("/payload/type").and_then(Value::as_str) == Some("token_count"))
                    .then(|| line.pointer("/payload/info/total_token_usage").cloned())
                    .flatten()
            })
            .last()
            .as_ref()
            .map(codex_usage);
        children.push(ChildThread {
            id: id.to_owned(),
            usage,
        });
    }
    children
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
    use std::time::Duration;

    use super::*;

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn a_childs_use_is_read_from_its_rollout() {
        let sessions = Temp(
            std::env::temp_dir().join(format!("brigadier-codex-children-{}", std::process::id())),
        );
        let day = sessions.join("2026/10/07");
        fs::create_dir_all(&day).unwrap();
        let parent = "01a116c2-7168-7023-a3d9-75ca70387e74";
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
                r#"{"type":"session_meta","payload":{"id":"01a116c2-9999","parent_thread_id":"someone-else"}}"#
                    .to_owned(),
                token_count(9, 0, 9),
            ]
            .join("\n"),
        )
        .unwrap();
        let children = child_threads(&sessions, parent);
        assert_eq!(
            children,
            [ChildThread {
                id: "01a116c2-71f5".into(),
                usage: Some(TokenUsage {
                    input_tokens: 152401 - 117504,
                    cached_input_tokens: 117504,
                    cache_write_tokens: 0,
                    output_tokens: 1023,
                    reasoning_tokens: 121,
                    cost_usd: None,
                    model_calls: 0,
                    ..TokenUsage::default()
                }),
            }]
        );
        assert_eq!(
            child_threads(Path::new("/nonexistent/sessions"), parent),
            []
        );
    }

    #[test]
    fn a_long_lived_threads_auto_review_is_found_on_a_later_day() {
        let sessions = Temp(
            std::env::temp_dir().join(format!("brigadier-codex-later-{}", std::process::id())),
        );
        let (first, next, before) = (
            sessions.join("2026/10/07"),
            sessions.join("2026/10/08"),
            sessions.join("2026/10/06"),
        );
        for day in [&first, &next, &before] {
            fs::create_dir_all(day).unwrap();
        }
        let parent = "01a11559-1d2a";
        fs::write(
            first.join(format!("rollout-2026-10-07T10-00-00-{parent}.jsonl")),
            format!(r#"{{"type":"session_meta","payload":{{"id":"{parent}"}}}}"#),
        )
        .unwrap();
        let child = |id: &str| {
            [
                format!(
                    r#"{{"type":"session_meta","payload":{{"id":"{id}","parent_thread_id":"{parent}","source":{{"subagent":{{"other":"guardian"}}}},"thread_source":"guardian_review"}}}}"#
                ),
                r#"{"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":60,"output_tokens":5}}}}"#.to_owned(),
            ]
            .join("\n")
        };
        fs::write(
            next.join("rollout-2026-10-08T09-00-00-guardian.jsonl"),
            child("guardian"),
        )
        .unwrap();
        // A folder before the parent's can't hold its child.
        fs::write(
            before.join("rollout-2026-10-06T09-00-00-older.jsonl"),
            child("older"),
        )
        .unwrap();
        let children = child_threads(&sessions, parent);
        let later = child_threads_since(
            &sessions,
            parent,
            Some(std::time::SystemTime::now() + Duration::from_secs(60)),
        );
        assert_eq!(
            children,
            [ChildThread {
                id: "guardian".into(),
                usage: Some(TokenUsage {
                    input_tokens: 40,
                    cached_input_tokens: 60,
                    output_tokens: 5,
                    ..TokenUsage::default()
                }),
            }]
        );
        // Nothing written since then.
        assert_eq!(later, []);
    }
}
