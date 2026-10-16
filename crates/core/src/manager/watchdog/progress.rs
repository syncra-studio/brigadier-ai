//! Provider evidence of work, shared by the worker and thread watchdogs.
use std::collections::HashMap;

use brigadier_providers::{ItemStatus, ProviderEvent, Role};

use super::Timing;

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

impl CommandKind {
    pub(crate) fn deadline(self, timing: &Timing) -> i64 {
        match self {
            Self::Server => timing.server,
            Self::Build => timing.build,
            Self::Normal => timing.command,
        }
    }
}

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
        | ProviderEvent::ContextSize { .. }
        | ProviderEvent::RateLimits { .. }
        | ProviderEvent::ApprovalRequested { .. }
        | ProviderEvent::ApprovalResolved { .. }
        | ProviderEvent::Error { .. }
        | ProviderEvent::Notice { .. }
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
            } => Some((item_id, *status, command_kind(command), false)),
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
                ))
            }
            _ => None,
        };
        if let Some((id, status, kind, wait)) = running {
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

    pub(crate) fn waiting(&self) -> bool {
        !self.waits.is_empty()
    }

    pub(crate) fn command_deadline(&self, timing: &Timing) -> Option<i64> {
        self.commands
            .values()
            .map(|(kind, at)| at.saturating_add(kind.deadline(timing)))
            .min()
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
            (json!({"type":"backgroundJobs","running":1}), false),
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
    fn watchdog_build_output_extends_its_deadline_but_chatter_does_not() {
        let timing = Timing::scaled(180);
        let mut evidence = Progress::default();
        assert!(evidence.observe(&command("cargo build"), 0));
        assert_eq!(evidence.command_deadline(&timing), Some(30 * 60_000));
        assert!(!evidence.observe(
            &ProviderEvent::Progress {
                item_id: "c".into()
            },
            29 * 60_000
        ));
        let output = ProviderEvent::CommandOutputDelta {
            item_id: "c".into(),
            text: "compiled crate one".into(),
        };
        assert!(evidence.observe(&output, 29 * 60_000));
        assert_eq!(evidence.command_deadline(&timing), Some(59 * 60_000));
        assert!(!evidence.observe(&output, 58 * 60_000));
        assert_eq!(evidence.command_deadline(&timing), Some(59 * 60_000));
        let text = ProviderEvent::MessageDelta {
            item_id: "m".into(),
            text: "still working".into(),
            phase: None,
        };
        assert!(evidence.observe(&text, 58 * 60_000));
        assert_eq!(evidence.command_deadline(&timing), Some(59 * 60_000));
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
    #[test]
    fn watchdog_foreground_server_output_does_not_extend_its_deadline() {
        let timing = Timing::scaled(180);
        let mut evidence = Progress::default();
        evidence.observe(&command("pnpm dev"), 0);
        for minute in 1..=10 {
            evidence.observe(
                &ProviderEvent::CommandOutputDelta {
                    item_id: "c".into(),
                    text: format!("request {minute}"),
                },
                minute * 60_000,
            );
            assert_eq!(evidence.command_deadline(&timing), Some(2 * 60_000));
        }
    }
}
