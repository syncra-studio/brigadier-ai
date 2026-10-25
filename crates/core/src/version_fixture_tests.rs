//! D9(c): real versioned recordings pass through the adapters and the production S9/S10
//! view builder. UPDATE_FIXTURES=1 deliberately replaces the reviewed presentation snapshots.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use brigadier_providers::cli::CliEnv;
use brigadier_providers::record::{Direction, Recording};
use brigadier_providers::{Provider, TurnStatus, claude, codex};
use brigadier_sandbox::{PlatformOptions, native_for_identifier};
use serde_json::{Value, json};

use super::*;

fn snapshot(path: &Path, value: &Value) {
    let text = serde_json::to_string_pretty(value).unwrap() + "\n";
    if std::env::var("UPDATE_FIXTURES").as_deref() == Ok("1") {
        std::fs::write(path, &text).unwrap();
    }
    assert!(
        std::fs::read_to_string(path).unwrap() == text,
        "{}: replay changed; inspect before UPDATE_FIXTURES=1",
        path.display()
    );
}

fn unique(entries: &[FileEntry], label: &str) -> HashSet<String> {
    let ids: Vec<_> = entries
        .iter()
        .filter_map(|entry| session_file::item_id(&entry.event).map(str::to_owned))
        .collect();
    let distinct: HashSet<_> = ids.iter().cloned().collect();
    assert_eq!(ids.len(), distinct.len(), "{label}: duplicate CLI items");
    distinct
}

fn shown(event: &ProviderEvent) -> bool {
    matches!(
        event,
        ProviderEvent::Message { .. }
            | ProviderEvent::MessageDelta { .. }
            | ProviderEvent::Reasoning { .. }
            | ProviderEvent::ReasoningDelta { .. }
            | ProviderEvent::ToolCall { .. }
            | ProviderEvent::Command { .. }
            | ProviderEvent::CommandOutputDelta { .. }
            | ProviderEvent::FileChanges { .. }
            | ProviderEvent::Image { .. }
            | ProviderEvent::Error { .. }
            | ProviderEvent::CompactionEnded { .. }
            | ProviderEvent::TaskNotice { .. }
    )
}

fn replay(provider: &dyn Provider, path: &Path) -> (Recording, Vec<(i64, ProviderEvent)>) {
    let recording = Recording::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(recording.header.provider, provider.kind());
    let mut parser = provider.replayer();
    let mut events: Vec<_> = recording
        .lines
        .iter()
        .flat_map(|row| {
            let at = recording.header.recorded_at_ms + row.t as i64;
            parser
                .feed(row.dir, &row.line)
                .into_iter()
                .map(move |event| (at, event))
        })
        .collect();
    // Task notices omit start times; the live parser supplies wall time. The recording's
    // offset supplies that clock in a replay, independent of today's test run.
    let mut task_starts = HashMap::new();
    for (at, event) in &mut events {
        if let ProviderEvent::BackgroundTask { task } = event {
            task.started_at_ms = *task_starts.entry(task.id.clone()).or_insert(*at);
            if task.ended_at_ms.is_some() {
                task.ended_at_ms = Some(*at);
            }
        }
    }
    assert!(!events.is_empty(), "{} parsed no events", path.display());
    (recording, events)
}

fn check_scenario(name: &str, events: &[(i64, ProviderEvent)]) {
    let any = |matches: fn(&ProviderEvent) -> bool| events.iter().any(|(_, event)| matches(event));
    let present = match name {
        "plain" => any(
            |event| matches!(event, ProviderEvent::Message { role: Role::Assistant, text, .. } if !text.is_empty()),
        ),
        "tools" => any(|event| {
            matches!(
                event,
                ProviderEvent::ToolCall { .. } | ProviderEvent::Command { .. }
            )
        }),
        "background" => any(|event| {
            matches!(
                event,
                ProviderEvent::BackgroundTask { .. } | ProviderEvent::BackgroundJobs { .. }
            )
        }),
        "interrupt" => any(|event| {
            matches!(
                event,
                ProviderEvent::TurnCompleted {
                    status: TurnStatus::Interrupted,
                    ..
                }
            )
        }),
        "compaction" => {
            any(|event| matches!(event, ProviderEvent::CompactionEnded { error: None, .. }))
        }
        "resume" => {
            any(|event| matches!(event, ProviderEvent::SessionStarted { .. }))
                && any(|event| {
                    matches!(
                        event,
                        ProviderEvent::TurnCompleted {
                            status: TurnStatus::Completed,
                            ..
                        }
                    )
                })
        }
        "plan" => any(|event| matches!(event, ProviderEvent::PlanProposed { .. })),
        "question" => any(|event| matches!(event, ProviderEvent::QuestionsAsked { .. })),
        _ => panic!("unrecognized scenario {name}"),
    };
    assert!(
        present,
        "{name} recording does not exercise its advertised behavior"
    );
}

/// Claude's native file omits turn ends. Reconstruct the minimal turns.jsonl facts from
/// each real exchange in this native session, including its separately recorded resume.
/// Older manual captures did not request user echoes: their stdin text identifies the same
/// native user item. Never use tool results as a user message.
fn turn_records(
    provider: &dyn Provider,
    folder: &Path,
    paths: &[Value],
    entries: &[FileEntry],
) -> Vec<TurnRecord> {
    let mut records = Vec::new();
    for path in paths {
        let recording = Recording::parse(
            &std::fs::read_to_string(folder.join(path.as_str().unwrap())).unwrap(),
        )
        .unwrap();
        let mut parser = provider.replayer();
        let mut user = None;
        for row in recording.lines {
            if row.dir == Direction::In {
                let raw: Value = serde_json::from_str(&row.line).unwrap();
                if raw["type"] == "user" {
                    let content = &raw["message"]["content"];
                    let text = content.as_str().map(str::to_owned).unwrap_or_else(|| {
                        content
                            .as_array()
                            .unwrap()
                            .iter()
                            .filter_map(|part| part["text"].as_str())
                            .collect::<Vec<_>>()
                            .join("\n")
                    });
                    user = entries.iter().find_map(|entry| match &entry.event {
                        ProviderEvent::Message {
                            item_id,
                            role: Role::User,
                            text: held,
                        } if held == &text => Some(item_id.clone()),
                        _ => None,
                    });
                }
            }
            for event in parser.feed(row.dir, &row.line) {
                if let ProviderEvent::Message {
                    item_id,
                    role: Role::User,
                    ..
                } = &event
                {
                    user = Some(item_id.clone());
                }
                if let ProviderEvent::TurnCompleted { status, .. } = event {
                    records.push(TurnRecord {
                        user_message: user.clone(),
                        end: Some(match status {
                            TurnStatus::Completed => TurnEnd::Done,
                            TurnStatus::Interrupted => TurnEnd::Stopped,
                            TurnStatus::Failed => TurnEnd::Failed,
                        }),
                        error: None,
                        ended_at_ms: Some(recording.header.recorded_at_ms + row.t as i64),
                        requests: vec![],
                        limit: None,
                    });
                }
            }
        }
    }
    records
}

#[test]
fn every_cli_version_replays_and_reloads_without_duplicate_items() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../providers/fixtures/versions");
    // No shell lookup, installed CLI, account, network or model request in this test.
    let platform = native_for_identifier(
        "ai.brigadier.version-replay-tests",
        PlatformOptions {
            data_dir: Some(root.join("unused-data")),
        },
    )
    .unwrap();
    let env = Arc::new(CliEnv::from_vars([]));
    let catalogue: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("index.json")).unwrap()).unwrap();
    for historical in catalogue["historicalRecordings"].as_array().unwrap() {
        let kind: ProviderKind = serde_json::from_value(historical["provider"].clone()).unwrap();
        let provider: Box<dyn Provider> = match kind {
            ProviderKind::Claude => Box::new(claude::Claude::new(platform.clone(), env.clone())),
            ProviderKind::Codex => Box::new(codex::Codex::new(platform.clone(), env.clone())),
        };
        let (recording, _) = replay(
            provider.as_ref(),
            &root.join(historical["recording"].as_str().unwrap()),
        );
        assert_eq!(
            recording.header.cli_version.as_deref(),
            historical["cliVersion"].as_str()
        );
    }
    // These derived envelopes never carried a version. Keep testing them without silently
    // relabelling them as a real recording of today's CLI (D25/D29).
    for historical in catalogue["historicalLimits"].as_array().unwrap() {
        assert!(historical["cliVersion"].is_null());
        let provider = claude::Claude::new(platform.clone(), env.clone());
        let mut parser = provider.replayer();
        let raw =
            std::fs::read_to_string(root.join(historical["recording"].as_str().unwrap())).unwrap();
        assert!(raw.lines().flat_map(|line| parser.feed(Direction::Out, line)).any(|event| matches!(event, ProviderEvent::Error { error } if error.kind == ErrorKind::UsageLimit)));
    }
    let mut folders: Vec<PathBuf> = std::fs::read_dir(&root)
        .unwrap()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    folders.sort();
    assert!(!folders.is_empty());
    for folder in folders {
        let index: Value =
            serde_json::from_str(&std::fs::read_to_string(folder.join("index.json")).unwrap())
                .unwrap();
        let kind: ProviderKind = serde_json::from_value(index["provider"].clone()).unwrap();
        let provider: Box<dyn Provider> = match kind {
            ProviderKind::Claude => Box::new(claude::Claude::new(platform.clone(), env.clone())),
            ProviderKind::Codex => Box::new(codex::Codex::new(platform.clone(), env.clone())),
        };
        match index["limit"]["status"].as_str() {
            Some("missing") => assert!(index["limit"]["reason"].is_string()),
            Some("recorded") => {
                let (_, events) = replay(
                    provider.as_ref(),
                    &folder.join(index["limit"]["recording"].as_str().unwrap()),
                );
                assert!(events.iter().any(|(_, event)| matches!(event, ProviderEvent::Error { error } if error.limit.is_some())), "limit coverage needs a real recorded rejection (D25/D29)");
            }
            other => panic!("limit coverage must be explicit: {other:?}"),
        }
        for path in index["reusedRecordings"].as_array().unwrap() {
            let (recording, _) = replay(provider.as_ref(), &folder.join(path.as_str().unwrap()));
            assert_eq!(
                recording.header.cli_version.as_deref(),
                index["cliVersion"].as_str()
            );
        }
        let scenarios = index["scenarios"].as_object().unwrap();
        assert_eq!(scenarios.len(), 8);
        for (name, case) in scenarios {
            let (recording, events) = replay(
                provider.as_ref(),
                &folder.join(case["recording"].as_str().unwrap()),
            );
            assert_eq!(
                recording.header.cli_version.as_deref(),
                index["cliVersion"].as_str()
            );
            check_scenario(name, &events);
            if name == "resume" {
                assert!(
                    recording
                        .lines
                        .iter()
                        .any(|row| row.line.contains("thread/resume")
                            || row.line.contains("isReplay")),
                    "resume has no native session continuation evidence"
                );
            }
            // An older recording may be ephemeral (no rollout). Reuse that protocol capture
            // and independently replay its tiny persisted counterpart, rather than pretending
            // files from different real sessions carry the same item ids (D29, S9/S10).
            let native_events = if let Some(path) = case["nativeRecording"].as_str() {
                let (native_recording, events) = replay(provider.as_ref(), &folder.join(path));
                assert_eq!(
                    native_recording.header.cli_version.as_deref(),
                    index["cliVersion"].as_str()
                );
                events
            } else {
                events.clone()
            };
            let native = folder.join(case["native"].as_str().unwrap());
            let versions: HashSet<String> = std::fs::read_to_string(&native)
                .unwrap()
                .lines()
                .filter_map(|line| {
                    let row: Value = serde_json::from_str(line).unwrap();
                    row.get("version")
                        .or_else(|| row.pointer("/payload/cli_version"))
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                })
                .collect();
            assert_eq!(
                versions,
                HashSet::from([index["cliVersion"].as_str().unwrap().to_owned()])
            );
            let entries = session_file::read(kind, &native).unwrap();
            assert!(
                !entries.is_empty(),
                "{} read no native items",
                native.display()
            );
            // A fresh reader models a daemon reload; a kept reader must agree on two reads.
            let mut reading = session_file::Reading::new(kind, &native);
            assert_eq!(reading.read().unwrap(), entries);
            assert_eq!(reading.read().unwrap(), entries);
            let records = turn_records(
                provider.as_ref(),
                &folder,
                case["historyRecordings"].as_array().unwrap(),
                &entries,
            );
            let mut stream = Vec::new();
            let mut phases = HashMap::new();
            for (at, event) in &native_events {
                if let ProviderEvent::MessageDelta {
                    item_id,
                    phase: Some(phase),
                    ..
                } = event
                {
                    phases.insert(item_id.clone(), *phase);
                }
                if shown(event) {
                    let phase = session_file::item_id(event)
                        .and_then(|id| phases.get(id))
                        .copied();
                    let entry = FileEntry {
                        event: event.clone(),
                        at_ms: Some(*at),
                        updated_at_ms: None,
                        parent: None,
                        steer: false,
                        images: vec![],
                        phase,
                    };
                    stream = session_file::merge(stream, vec![entry]);
                }
            }
            let native_ids = unique(&entries, name);
            let stream_ids = unique(&stream, name);
            assert!(
                !native_ids.is_disjoint(&stream_ids),
                "{name}: live and native files share no CLI ids"
            );
            let files = vec![Arc::new(entries)];
            let live = LiveTurn {
                entries: stream,
                ..Default::default()
            };
            let before_reload = view(files.clone(), Some(&live), &records);
            let reloaded = view(files, None, &records);
            unique(&before_reload.entries, name);
            unique(&reloaded.entries, name);
            // Replaying completed live entries already in the file cannot duplicate or replace
            // the file's authoritative items, even when timing and output representations differ.
            for entry in &reloaded.entries {
                if let Some(id) = session_file::item_id(&entry.event) {
                    let held = before_reload
                        .entries
                        .iter()
                        .find(|held| session_file::item_id(&held.event) == Some(id))
                        .unwrap();
                    assert_eq!(
                        held, entry,
                        "{name}: file item changed when live entries joined"
                    );
                }
            }
            let events: Vec<_> = events
                .iter()
                .filter(|(_, event)| {
                    shown(event)
                        || matches!(
                            event,
                            ProviderEvent::QuestionsAsked { .. }
                                | ProviderEvent::QuestionsWithdrawn { .. }
                                | ProviderEvent::PlanProposed { .. }
                                | ProviderEvent::TurnCompleted { .. }
                                | ProviderEvent::BackgroundTask { .. }
                                | ProviderEvent::CompactionStarted { .. }
                        )
                })
                .map(|(_, event)| event)
                .collect();
            snapshot(
                &folder.join(case["view"].as_str().unwrap()),
                &json!({"events":events,"live":before_reload,"persisted":reloaded}),
            );
        }
    }
}
