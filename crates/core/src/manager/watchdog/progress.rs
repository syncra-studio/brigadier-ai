//! Provider evidence of work, shared by the worker and thread watchdogs.
use std::collections::HashMap;

use brigadier_providers::{ItemStatus, ProviderEvent, Role};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandKind {
    Server,
    Build,
    #[default]
    Normal,
}

/// One command policy table, applied to shell items and command tools alike. Shell wrappers
/// and compound commands are tokenized too; a foreground server wins over a build.
pub(crate) fn command_kind(command: &str) -> CommandKind {
    let mut result = CommandKind::Normal;
    for part in command.split([';', '|', '&', '\n']) {
        let words: Vec<_> = part
            .split_whitespace()
            .map(|w| w.trim_matches(['\"', '\'', '(', ')']))
            .filter(|w| !w.is_empty())
            .collect();
        let Some(first) = words.iter().position(|w| *w != "env" && !w.contains('=')) else {
            continue;
        };
        let mut first = first;
        loop {
            let executable = words[first].rsplit('/').next().unwrap_or(words[first]);
            let args = &words[first + 1..];
            let skip = match executable {
                "env" | "command" | "exec" | "npx" => Some(1),
                "npm" if args.first() == Some(&"exec") => Some(2),
                "sh" | "bash" | "zsh"
                    if args
                        .first()
                        .is_some_and(|arg| arg.contains('c') && arg.starts_with('-')) =>
                {
                    Some(2)
                }
                _ => None,
            };
            match skip {
                Some(skip) if first + skip < words.len() => first += skip,
                _ => break,
            }
        }
        let executable = words[first].rsplit('/').next().unwrap_or(words[first]);
        let args = &words[first + 1..];
        let has = |arg: &str| args.contains(&arg);
        let package = matches!(executable, "npm" | "pnpm" | "yarn" | "bun");
        if matches!(executable, "vite" | "serve" | "watch")
            || args.iter().any(|w| w.starts_with("--watch"))
            || (executable == "tail" && (has("-f") || has("-F") || has("--follow")))
            || (executable == "next" && args.first() == Some(&"dev"))
            || (matches!(executable, "python" | "python3")
                && args.windows(2).any(|w| w == ["-m", "http.server"]))
            || (package
                && (args.first().is_some_and(|w| matches!(*w, "dev" | "start"))
                    || args
                        .windows(2)
                        .any(|w| w == ["run", "dev"] || w == ["run", "start"])))
        {
            return CommandKind::Server;
        }
        if matches!(executable, "make" | "pytest" | "tsc")
            || (executable == "cargo"
                && args
                    .iter()
                    .any(|w| matches!(*w, "build" | "test" | "check" | "clippy")))
            || (executable == "go" && args.first().is_some_and(|w| matches!(*w, "test" | "build")))
            || (package
                && (args.first().is_some_and(|w| matches!(*w, "test" | "build"))
                    || args
                        .windows(2)
                        .any(|w| w == ["run", "test"] || w == ["run", "build"])))
        {
            result = CommandKind::Build;
        }
    }
    result
}

impl CommandKind {}

/// Pure classification against the preceding evidence for the same item and event kind.
/// Transport liveness, notices, quota/status snapshots and echoed user input prove no work.
/// Repeated snapshots/deltas do not buy more time; usage must grow, rather than merely change.
pub(crate) fn progress(event: &ProviderEvent, previous: Option<&ProviderEvent>) -> bool {
    if previous == Some(event) {
        return false;
    }
    match event {
        ProviderEvent::Message {
            role: Role::Assistant,
            text,
            ..
        }
        | ProviderEvent::MessageDelta { text, .. }
        | ProviderEvent::Reasoning { text, .. }
        | ProviderEvent::ReasoningDelta { text, .. }
        | ProviderEvent::CommandOutputDelta { text, .. } => !text.is_empty(),
        ProviderEvent::ToolCall { .. }
        | ProviderEvent::Command { .. }
        | ProviderEvent::FileChanges { .. }
        | ProviderEvent::Image { .. }
        | ProviderEvent::TurnStarted { .. }
        | ProviderEvent::CompactionStarted { .. }
        | ProviderEvent::CompactionRefused
        | ProviderEvent::CompactionEnded { .. }
        | ProviderEvent::TurnCompleted { .. } => true,
        ProviderEvent::Usage { total, .. } => {
            let count = |u: &brigadier_providers::TokenUsage| {
                u.input_tokens + u.output_tokens + u.reasoning_tokens + u.model_calls
            };
            let before = match previous {
                Some(ProviderEvent::Usage { total, .. }) => count(total),
                _ => 0,
            };
            count(total) > before
        }
        ProviderEvent::SessionStarted { .. }
        | ProviderEvent::Message { .. }
        | ProviderEvent::Looked { .. }
        | ProviderEvent::Progress { .. }
        | ProviderEvent::BackgroundJobs { .. }
        | ProviderEvent::BackgroundTask { .. }
        | ProviderEvent::McpServers { .. }
        | ProviderEvent::McpServerChanged { .. }
        | ProviderEvent::ContextSize { .. }
        | ProviderEvent::RateLimits { .. }
        | ProviderEvent::ApprovalRequested { .. }
        | ProviderEvent::ApprovalResolved { .. }
        | ProviderEvent::Error { .. }
        | ProviderEvent::Notice { .. }
        | ProviderEvent::TaskNotice { .. }
        | ProviderEvent::Exited { .. } => false,
    }
}

/// `event` without the whole output it may carry on its way to the store (up to 64 MiB, see
/// `KeptOutput`): what is compared is the event as shown, and the history keeps no copy of it.
fn light(event: &ProviderEvent) -> ProviderEvent {
    let mut event = event.clone();
    if let ProviderEvent::Command { kept, .. } | ProviderEvent::ToolCall { kept, .. } = &mut event {
        *kept = None;
    }
    event
}

#[derive(Debug, Default)]
pub(crate) struct Progress {
    previous: HashMap<String, ProviderEvent>,
    commands: HashMap<String, (CommandKind, i64)>,
    waits: std::collections::HashSet<String>,
    last_call: Option<LastCall>,
}

#[derive(Debug)]
struct LastCall {
    id: String,
    label: String,
    started: Option<i64>,
    ended: Option<i64>,
}

impl Progress {
    /// State and time are supplied by the caller; no clock or provider access here.
    pub(crate) fn observe(&mut self, event: &ProviderEvent, now: i64) -> bool {
        let item = match event {
            ProviderEvent::MessageDelta { item_id, .. }
            | ProviderEvent::Message { item_id, .. }
            | ProviderEvent::ReasoningDelta { item_id, .. }
            | ProviderEvent::Reasoning { item_id, .. }
            | ProviderEvent::ToolCall { item_id, .. }
            | ProviderEvent::Command { item_id, .. }
            | ProviderEvent::CommandOutputDelta { item_id, .. }
            | ProviderEvent::FileChanges { item_id, .. }
            | ProviderEvent::Image { item_id, .. } => item_id.as_str(),
            _ => "",
        };
        let key = format!("{:?}:{item}", std::mem::discriminant(event));
        let light = light(event);
        let moved = progress(&light, self.previous.get(&key));
        if moved {
            self.previous.insert(key, light);
        }
        let running = match event {
            ProviderEvent::Command {
                item_id,
                command,
                status,
                ..
            } => Some((
                item_id,
                *status,
                command_kind(command),
                false,
                command.clone(),
            )),
            ProviderEvent::ToolCall {
                item_id,
                name,
                input,
                status,
                ..
            } => {
                let command = input
                    .as_ref()
                    .and_then(|text| serde_json::from_str::<serde_json::Value>(text).ok())
                    .and_then(|v| {
                        v.get("command")
                            .or_else(|| v.get("cmd"))
                            .and_then(|v| v.as_str())
                            .map(str::to_owned)
                    });
                // Streamed tool arguments can arrive after the start event. Enrich the
                // label without restarting the call's elapsed-time clock.
                if let Some(command) = &command
                    && let Some(call) = &mut self.last_call
                    && call.id == *item_id
                {
                    call.label = command.clone();
                }
                let leaf = name.rsplit(['.', ':']).next().unwrap_or(name);
                let leaf = leaf
                    .rsplit("__")
                    .next()
                    .unwrap_or(leaf)
                    .to_ascii_lowercase();
                Some((
                    item_id,
                    *status,
                    command.as_deref().map(command_kind).unwrap_or_default(),
                    leaf == "await"
                        || leaf == "wait"
                        || leaf.starts_with("wait_")
                        || leaf.ends_with("_wait")
                        || leaf.ends_with("_await"),
                    command.unwrap_or_else(|| name.clone()),
                ))
            }
            _ => None,
        };
        if let Some((id, status, kind, wait, label)) = running {
            if status == ItemStatus::InProgress {
                if !self.commands.contains_key(id) {
                    self.last_call = Some(LastCall {
                        id: id.clone(),
                        label,
                        started: Some(now),
                        ended: None,
                    });
                }
            } else if let Some(call) = &mut self.last_call
                && call.id == *id
                && call.ended.is_none()
            {
                call.ended = Some(now);
            } else if !self.commands.contains_key(id)
                && self.last_call.as_ref().is_none_or(|call| call.id != *id)
            {
                // Some providers emit only a completed item, with its own duration.
                let duration = match event {
                    ProviderEvent::Command { duration_ms, .. } => *duration_ms,
                    _ => None,
                };
                self.last_call = Some(LastCall {
                    id: id.clone(),
                    label,
                    started: duration.map(|ms| now.saturating_sub(ms)),
                    ended: Some(now),
                });
            }
            if status == ItemStatus::InProgress {
                self.commands
                    .entry(id.clone())
                    .and_modify(|entry| {
                        entry.0 = kind;
                        if moved && kind == CommandKind::Build {
                            entry.1 = now;
                        }
                    })
                    .or_insert((kind, now));
                if wait {
                    self.waits.insert(id.clone());
                }
            } else {
                self.commands.remove(id);
                self.waits.remove(id);
            }
        }
        if moved
            && let ProviderEvent::CommandOutputDelta { item_id, .. } = event
            && let Some(entry) = self.commands.get_mut(item_id)
            && entry.0 == CommandKind::Build
        {
            entry.1 = now;
        }
        if matches!(
            event,
            ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
        ) {
            self.commands.clear();
            self.waits.clear();
            self.previous.clear();
        }
        moved
    }

    /// The latest call and its own elapsed time, kept after it finishes.
    pub(crate) fn last_command(&self, now: i64) -> Option<String> {
        self.last_call.as_ref().map(|call| {
            let Some(started) = call.started else {
                return call.label.clone();
            };
            format!(
                "{} ({} {} min)",
                call.label,
                if call.ended.is_some() {
                    "ran"
                } else {
                    "running"
                },
                (call.ended.unwrap_or(now) - started).max(0) / 60_000
            )
        })
    }

    pub(crate) fn waiting(&self) -> bool {
        !self.waits.is_empty()
    }

    pub(crate) fn command(&self) -> Option<CommandKind> {
        if self.commands.values().any(|k| k.0 == CommandKind::Server) {
            Some(CommandKind::Server)
        } else if self.commands.values().any(|k| k.0 == CommandKind::Build) {
            Some(CommandKind::Build)
        } else if self.commands.is_empty() {
            None
        } else {
            Some(CommandKind::Normal)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn watchdog_classifies_every_provider_event_and_ignores_repeats() {
        let cases = [
            (json!({"type":"sessionStarted","nativeId":"s"}), false),
            (json!({"type":"turnStarted"}), true),
            (
                json!({"type":"messageDelta","itemId":"m","text":"new"}),
                true,
            ),
            (
                json!({"type":"message","itemId":"m","role":"assistant","text":"new"}),
                true,
            ),
            (
                json!({"type":"message","itemId":"m","role":"user","text":"echo"}),
                false,
            ),
            (
                json!({"type":"reasoningDelta","itemId":"r","text":"new"}),
                true,
            ),
            (json!({"type":"reasoning","itemId":"r","text":"new"}), true),
            (
                json!({"type":"toolCall","itemId":"t","name":"Read","status":"inProgress"}),
                true,
            ),
            (
                json!({"type":"command","itemId":"c","command":"sleep 900","status":"inProgress"}),
                true,
            ),
            (
                json!({"type":"commandOutputDelta","itemId":"c","text":"new"}),
                true,
            ),
            (
                json!({"type":"looked","itemId":"c","reads":[],"searches":[]}),
                false,
            ),
            (json!({"type":"progress","itemId":"c"}), false),
            (json!({"type":"backgroundJobs","running":["job"]}), false),
            (
                json!({"type":"fileChanges","itemId":"f","changes":[],"status":"completed"}),
                true,
            ),
            (
                json!({"type":"image","itemId":"i","status":"completed"}),
                true,
            ),
            (
                json!({"type":"usage","total":{"inputTokens":10,"cachedInputTokens":0,"cacheWriteTokens":0,"outputTokens":1,"reasoningTokens":0}}),
                true,
            ),
            (json!({"type":"contextSize","usedTokens":100}), false),
            (json!({"type":"compactionStarted","automatic":false}), true),
            (json!({"type":"compactionEnded","automatic":false}), true),
            (
                json!({"type":"rateLimits","quota":{"provider":"codex","windows":[],"observedAtMs":0}}),
                false,
            ),
            (
                json!({"type":"approvalRequested","request":{"id":"a","kind":"command","tool":"Bash","paths":[],"escalation":true}}),
                false,
            ),
            (
                json!({"type":"approvalResolved","id":"a","decision":{"type":"allow"},"decidedBy":"user"}),
                false,
            ),
            (json!({"type":"turnCompleted","status":"completed"}), true),
            (
                json!({"type":"error","error":{"kind":"process","message":"bad","willRetry":false}}),
                false,
            ),
            (
                json!({"type":"notice","level":"info","message":"heartbeat"}),
                false,
            ),
            (json!({"type":"exited"}), false),
        ];
        for (value, expected) in cases {
            let event: ProviderEvent =
                serde_json::from_value(value.clone()).unwrap_or_else(|e| panic!("{value}: {e}"));
            assert_eq!(progress(&event, None), expected, "{event:?}");
            assert!(!progress(&event, Some(&event)), "repeat {event:?}");
        }
    }

    #[test]
    fn watchdog_command_policy_covers_servers_builds_and_plain_commands() {
        for command in [
            "bash -lc 'npm run dev'",
            "npx vite",
            "npm exec vite",
            "/usr/bin/env pnpm dev",
            "npm run dev",
            "pnpm dev",
            "vite",
            "next dev",
            "tail -f app.log",
            "watch date",
            "tsc --watch",
            "python -m http.server",
            "serve .",
            "sh -c 'cd ui && pnpm dev'",
        ] {
            assert_eq!(command_kind(command), CommandKind::Server, "{command}");
        }
        for command in [
            "cargo build",
            "cargo test -p core",
            "cargo check",
            "cargo clippy",
            "pnpm build",
            "npm run test",
            "yarn test",
            "make all",
            "go test ./...",
            "pytest",
            "tsc --noEmit",
        ] {
            assert_eq!(command_kind(command), CommandKind::Build, "{command}");
        }
        for command in [
            "git status",
            "sleep 99999",
            "rg serve src",
            "echo 'cargo'",
            "curl example.test",
        ] {
            assert_eq!(command_kind(command), CommandKind::Normal, "{command}");
        }
    }

    pub(super) fn command(words: &str) -> ProviderEvent {
        ProviderEvent::Command {
            item_id: "c".into(),
            command: words.into(),
            cwd: None,
            status: ItemStatus::InProgress,
            exit_code: None,
            output: None,
            duration_ms: None,
            kept: None,
        }
    }

    #[test]
    fn watchdog_command_fact_keeps_elapsed_time_after_completion() {
        let mut evidence = Progress::default();
        let mut event = command("cargo test");
        evidence.observe(&event, 0);
        evidence.observe(&event, 5 * 60_000);
        assert_eq!(
            evidence.last_command(14 * 60_000).as_deref(),
            Some("cargo test (running 14 min)")
        );
        if let ProviderEvent::Command { status, .. } = &mut event {
            *status = ItemStatus::Completed;
        }
        evidence.observe(&event, 14 * 60_000);
        assert_eq!(
            evidence.last_command(20 * 60_000).as_deref(),
            Some("cargo test (ran 14 min)")
        );
    }

    #[test]
    fn watchdog_completed_only_command_fact_uses_the_provider_duration() {
        let mut evidence = Progress::default();
        let mut event = command("cargo test");
        if let ProviderEvent::Command {
            status,
            duration_ms,
            ..
        } = &mut event
        {
            *status = ItemStatus::Completed;
            *duration_ms = Some(14 * 60_000);
        }
        evidence.observe(&event, 20 * 60_000);
        assert_eq!(
            evidence.last_command(30 * 60_000).as_deref(),
            Some("cargo test (ran 14 min)")
        );
    }

    #[test]
    fn watchdog_tool_fact_names_the_command_and_ignores_older_call_snapshots() {
        let mut evidence = Progress::default();
        let old = command("cargo test");
        evidence.observe(&old, 0);
        let mut tool = ProviderEvent::ToolCall {
            item_id: "tool".into(),
            name: "exec_command".into(),
            input: Some(r#"{"cmd":"git status"}"#.into()),
            status: ItemStatus::InProgress,
            output: None,
            kept: None,
        };
        evidence.observe(&tool, 60_000);
        evidence.observe(&old, 120_000);
        if let ProviderEvent::ToolCall { status, .. } = &mut tool {
            *status = ItemStatus::Completed;
        }
        evidence.observe(&tool, 180_000);
        assert_eq!(
            evidence.last_command(240_000).as_deref(),
            Some("git status (ran 2 min)")
        );
    }

    #[test]
    fn watchdog_streamed_tool_arguments_enrich_the_fact_without_resetting_time() {
        let mut evidence = Progress::default();
        let mut event = ProviderEvent::ToolCall {
            item_id: "tool".into(),
            name: "mcp__brigadier__run".into(),
            input: None,
            status: ItemStatus::InProgress,
            output: None,
            kept: None,
        };
        evidence.observe(&event, 0);
        if let ProviderEvent::ToolCall { input, .. } = &mut event {
            *input = Some(r#"{"command":"cargo test"}"#.into());
        }
        evidence.observe(&event, 2 * 60_000);
        if let ProviderEvent::ToolCall { input, status, .. } = &mut event {
            *input = None;
            *status = ItemStatus::Completed;
        }
        evidence.observe(&event, 14 * 60_000);
        assert_eq!(
            evidence.last_command(20 * 60_000).as_deref(),
            Some("cargo test (ran 14 min)")
        );
    }

    #[test]
    fn watchdog_wait_tools_are_exempt_until_they_finish() {
        for name in [
            "await",
            "wait",
            "functions.wait",
            "mcp__tools__wait_for_workers",
        ] {
            let mut evidence = Progress::default();
            let event = |status| ProviderEvent::ToolCall {
                item_id: "w".into(),
                name: name.into(),
                input: None,
                status,
                output: None,
                kept: None,
            };
            evidence.observe(&event(ItemStatus::InProgress), 0);
            assert!(evidence.waiting(), "{name}");
            evidence.observe(&event(ItemStatus::Completed), 1);
            assert!(!evidence.waiting(), "{name}");
        }
    }
}
