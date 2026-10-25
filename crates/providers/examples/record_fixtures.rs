//! D9(c), D25, D29: tiny real runs through the production adapters, only for missing coverage.
//! Usage: tools/record-cli-fixtures.sh <claude|codex>. No CLI is used by the replay tests.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::cli::CliEnv;
use brigadier_providers::record::{Direction, FORMAT, Header, Recorder, Recording};
use brigadier_providers::*;
use brigadier_sandbox::{PlatformOptions, native_for_identifier};
use serde_json::{Value, json};

#[derive(Default)]
struct Kept(Mutex<Vec<Artifact>>);

impl Ledger for Kept {
    fn record(&self, artifact: Artifact) -> BoxFuture<'_, Result<()>> {
        self.0.lock().unwrap().push(artifact);
        Box::pin(async { Ok(()) })
    }
    fn holds(&self, _: &Artifact) -> bool {
        false
    }
}

fn spec(cwd: &Path, kind: ProviderKind, recording: PathBuf, origin: Origin) -> SessionSpec {
    SessionSpec {
        cwd: cwd.into(), model: Some(match kind { ProviderKind::Claude => "haiku", ProviderKind::Codex => "gpt-6-luna" }.into()),
        effort: Some("low".into()), fast: false, origin, ephemeral: false, access: Access::Full,
        append_system_prompt: Some("This is a tiny protocol recording. Follow only the one-line request; keep replies tiny, preserving native plan markup. Do not inspect the environment or start subagents.".into()),
        mcp_servers: vec![], tools: ToolSet::Default, add_dirs: vec![], project: None,
        skill_plugins: vec![], installed_skills: vec![], switched_off: Default::default(),
        env: vec![], unset_env: vec![], low_priority: true, record_to: Some(recording), redactor: None,
        owned_cwd: true, sweep_dir: Some(cwd.into()), keeps_detached: false, auto_compact: true,
        allowed_models: Some(AllowedModels::default()), omit_ai_coauthors: true, output_hook: None,
        stop_hook: None, naked_worker: false, native_compaction: None,
    }
}

/// Use Recorder's same scrubber for native files, then unwrap the raw lines, preserving the
/// native JSONL format rather than inventing transcripts from stream events (S9/S10).
fn export_native(
    source: &Path,
    dest: &Path,
    kind: ProviderKind,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let intermediate = dest.with_extension("scrubbing");
    let recorder = Recorder::create(
        &intermediate,
        &Header {
            fixture: FORMAT,
            provider: kind,
            cli_version: None,
            recorded_at_ms: 0,
            title: "Native transcript".into(),
        },
        None,
    )?;
    for line in std::fs::read_to_string(source)?.lines() {
        recorder.record(Direction::Out, line);
    }
    recorder.finish().map_err(|_| "native scrubber failed")?;
    let scrubbed = Recording::parse(&std::fs::read_to_string(&intermediate)?)?;
    let text: String = scrubbed
        .lines
        .into_iter()
        .map(|row| format!("{}\n", row.line))
        .collect();
    std::fs::write(dest, text)?;
    std::fs::remove_file(intermediate)?;
    Ok(())
}

async fn receive(started: &mut Started) -> Result<ProviderEvent> {
    tokio::time::timeout(Duration::from_secs(90), started.events.recv())
        .await
        .map_err(|_| Error::Timeout("fixture event"))?
        .ok_or(Error::Closed)
}

fn observe(event: &ProviderEvent, native: &mut Option<PathBuf>, usage: &mut Vec<Value>) {
    match event {
        ProviderEvent::SessionStarted {
            file: Some(file), ..
        } => *native = Some(file.into()),
        ProviderEvent::Usage { total, .. } => {
            eprintln!("usage {}", serde_json::to_string(total).unwrap());
            usage.push(serde_json::to_value(total).unwrap());
        }
        ProviderEvent::Error { error } => eprintln!("CLI error: {}", error.message),
        _ => {}
    }
}

async fn turn(
    started: &mut Started,
    prompt: &str,
    interrupt: bool,
    native: &mut Option<PathBuf>,
    usage: &mut Vec<Value>,
) -> Result<()> {
    started.session.send(TurnInput::text(prompt)).await?;
    let mut interrupted = false;
    loop {
        let event = receive(started).await?;
        observe(&event, native, usage);
        match event {
            ProviderEvent::QuestionsAsked { id, questions } => {
                let answers = questions.iter().map(|_| vec!["Yes".into()]).collect();
                started.session.answer_questions(id, Some(answers)).await?;
            }
            ProviderEvent::ApprovalRequested { request } => {
                started
                    .session
                    .answer(request.id, ApprovalDecision::Allow)
                    .await?;
            }
            ProviderEvent::Command {
                status: ItemStatus::InProgress,
                ..
            }
            | ProviderEvent::ToolCall {
                status: ItemStatus::InProgress,
                ..
            } if interrupt && !interrupted => {
                // Interrupt once the CLI starts its command, with no wait for its sleep.
                started.session.interrupt().await?;
                interrupted = true;
            }
            ProviderEvent::Error { ref error }
                if matches!(error.kind, ErrorKind::UsageLimit | ErrorKind::RateLimit) =>
            {
                return Err(Error::Rejected(error.message.clone()));
            }
            ProviderEvent::TurnCompleted { .. } => return Ok(()),
            ProviderEvent::Exited { .. } => return Err(Error::Closed),
            _ => {}
        }
    }
}

async fn record(
    provider: &dyn Provider,
    folder: &Path,
    group: &str,
    cwd: &Path,
    ledger: Arc<Kept>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let kind = provider.kind();
    let path = folder.join(format!("{group}.jsonl"));
    // A partially captured lifecycle already paid for its background turn and compaction.
    // Resume that CLI session to fill only its remaining interrupt/resume coverage (D29).
    let continuing = group == "lifecycle" && path.exists();
    let origin = if continuing {
        let recording = Recording::parse(&std::fs::read_to_string(&path)?)?;
        let id = recording.lines.iter().find_map(|row| {
            let line: Value = serde_json::from_str(&row.line).ok()?;
            line.pointer("/result/thread/id").and_then(Value::as_str).map(str::to_owned)
        }).ok_or("partial lifecycle has no resumable thread; preserve it and inspect before spending again")?;
        Origin::Resume { native_id: id }
    } else {
        Origin::New
    };
    let path = if continuing {
        folder.join("interrupt.jsonl")
    } else {
        path
    };
    let mut started = provider
        .start(spec(cwd, kind, path.clone(), origin), ledger.clone())
        .await?;
    let mut native = None;
    let mut usage = vec![];
    let result: std::result::Result<(), Box<dyn std::error::Error>> = async {
        if group == "plan-question" {
            started.session.set_plan_mode(true).await?;
            let prompt = if kind == ProviderKind::Claude {
                "Use AskUserQuestion to ask Ready? (Yes/No), then ExitPlanMode with a one-line plan to create ok.txt. Do not implement it."
            } else {
                "Use request_user_input to ask Ready? (Yes/No), then return <proposed_plan>Create ok.txt.</proposed_plan>. Do not implement it."
            };
            turn(&mut started, prompt, false, &mut native, &mut usage).await?;
        } else {
            if !continuing {
            let prompt = if kind == ProviderKind::Claude {
                "Run sleep 2 with Bash run_in_background=true, reply OK."
            } else {
                "Run sleep 2 with exec_command yield_time_ms=1, reply OK without waiting."
            };
            turn(&mut started, prompt, false, &mut native, &mut usage).await?;
            // Native compaction on a tiny history, never context filling (D29).
            started.session.compact(None).await?;
            loop {
                let event = receive(&mut started).await?;
                observe(&event, &mut native, &mut usage);
                if let ProviderEvent::CompactionEnded { error, .. } = event {
                    if let Some(error) = error { return Err(error.into()); }
                    break;
                }
            }
            // Both CLIs report the compaction item before its turn ends. Drain the end so
            // the next input cannot be steered into an unsteerable compact turn.
            loop { if matches!(receive(&mut started).await?, ProviderEvent::TurnCompleted { .. }) { break; } }
            }
            turn(&mut started, "Run sleep 30 using your shell tool.", true, &mut native, &mut usage).await?;
        }
        Ok(())
    }.await;
    let id = started.session.native_id();
    started.session.close().await;
    while let Ok(Some(event)) =
        tokio::time::timeout(Duration::from_secs(2), started.events.recv()).await
    {
        observe(&event, &mut native, &mut usage);
        if matches!(event, ProviderEvent::Exited { .. }) {
            break;
        }
    }
    drop(started);
    result?;
    if group == "lifecycle" {
        let resume_path = folder.join("resume.jsonl");
        let mut resumed = provider
            .start(
                spec(cwd, kind, resume_path, Origin::Resume { native_id: id }),
                ledger.clone(),
            )
            .await?;
        let result = turn(&mut resumed, "Reply OK.", false, &mut native, &mut usage).await;
        resumed.session.close().await;
        while let Ok(Some(event)) =
            tokio::time::timeout(Duration::from_secs(2), resumed.events.recv()).await
        {
            observe(&event, &mut native, &mut usage);
            if matches!(event, ProviderEvent::Exited { .. }) {
                break;
            }
        }
        drop(resumed);
        result?;
    }
    let source = session_file::locate(
        kind,
        &native.ok_or("CLI did not report its native transcript")?,
    );
    export_native(&source, &folder.join(format!("{group}.native.jsonl")), kind)?;
    let mut native_totals = Value::Null;
    for line in std::fs::read_to_string(&source)?.lines() {
        let row: Value = serde_json::from_str(line)?;
        if row["type"] == "cost-state" {
            native_totals = json!({"modelUsage":row["modelUsage"],"costUsd":row["totalCostUSD"]});
        }
        if row["type"] == "event_msg" && row["payload"]["type"] == "token_count" {
            native_totals =
                json!({"tokens":row.pointer("/payload/info/total_token_usage"),"costUsd":null});
        }
    }
    std::fs::write(
        folder.join(format!("{group}.usage.json")),
        serde_json::to_string_pretty(&json!({
            "reports": usage, "nativeTotals": native_totals, "note": "CLI-reported cumulative totals, not additive. Codex supplies no dollar cost. For total spend across resume, use the native Claude cost-state or Codex thread/tokenUsage totals; do not add cumulative reports."
        }))? + "\n",
    )?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() == Some("--scrub-native") {
        let args: Vec<String> = std::env::args().collect();
        let kind = if args.get(2).map(String::as_str) == Some("claude") {
            ProviderKind::Claude
        } else {
            ProviderKind::Codex
        };
        export_native(
            Path::new(args.get(3).ok_or("no source")?),
            Path::new(args.get(4).ok_or("no destination")?),
            kind,
        )?;
        return Ok(());
    }
    if std::env::args().nth(1).as_deref() == Some("--cleanup") {
        let args: Vec<String> = std::env::args().collect();
        let kind = match args.get(2).map(String::as_str) {
            Some("claude") => ProviderKind::Claude,
            Some("codex") => ProviderKind::Codex,
            _ => return Err("cleanup needs claude|codex".into()),
        };
        let scratch = PathBuf::from(args.get(3).ok_or("cleanup needs its scratch directory")?);
        std::fs::create_dir_all(scratch.join("data"))?;
        let platform = native_for_identifier(
            "ai.brigadier.cli-fixtures",
            PlatformOptions {
                data_dir: Some(scratch.join("data")),
            },
        )?;
        let env = Arc::new(CliEnv::capture(&platform));
        let provider: Box<dyn Provider> = match kind {
            ProviderKind::Claude => Box::new(claude::Claude::new(platform, env)),
            ProviderKind::Codex => Box::new(codex::Codex::new(platform, env)),
        };
        let artifacts =
            serde_json::from_str(&std::fs::read_to_string(scratch.join("cleanup.json"))?)?;
        provider.remove(artifacts).await?;
        std::fs::remove_dir_all(scratch)?;
        return Ok(());
    }
    let kind = match std::env::args().nth(1).as_deref() {
        Some("claude") => ProviderKind::Claude,
        Some("codex") => ProviderKind::Codex,
        _ => return Err("usage: record_fixtures <claude|codex>".into()),
    };
    let version_output = std::process::Command::new(kind.binary())
        .arg("--version")
        .output()?;
    let version_text = String::from_utf8(version_output.stdout)?;
    let version = version_text
        .split_whitespace()
        .find(|part| part.as_bytes().first().is_some_and(u8::is_ascii_digit))
        .ok_or("no CLI version")?;
    let folder = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/versions")
        .join(format!("{kind}-{version}"));
    std::fs::create_dir_all(&folder)?;
    let index_path = folder.join("index.json");
    if !index_path.exists() {
        let scenarios: serde_json::Map<String, Value> = ["plain", "tools", "background", "interrupt", "compaction", "resume", "plan", "question"].into_iter().map(|name| {
            let group = if matches!(name, "plan" | "question") { "plan-question" } else { "lifecycle" };
            let recording = if name == "resume" { "resume" } else { group };
            (name.to_owned(), json!({"recording":format!("{recording}.jsonl"),"native":format!("{group}.native.jsonl"),"view":format!("{name}.view.json"),"historyRecordings":if group == "lifecycle" { vec!["lifecycle.jsonl", "resume.jsonl"] } else { vec!["plan-question.jsonl"] }}))
        }).collect();
        std::fs::write(
            &index_path,
            serde_json::to_string_pretty(
                &json!({"provider":kind,"cliVersion":version,"limit":{"status":"missing","reason":"D25/D29: capture only an opportunistic real limit"},"reusedRecordings":[],"scenarios":scenarios}),
            )? + "\n",
        )?;
    }
    let mut index: Value = serde_json::from_str(&std::fs::read_to_string(&index_path)?)?;
    let mut groups = BTreeSet::new();
    for scenario in index["scenarios"]
        .as_object()
        .ok_or("no scenarios")?
        .values()
    {
        let path = folder.join(
            scenario
                .get("nativeRecording")
                .unwrap_or(&scenario["recording"])
                .as_str()
                .ok_or("no recording")?,
        );
        let native = folder.join(scenario["native"].as_str().ok_or("no native transcript")?);
        if !path.exists() || !native.exists() {
            let name = scenario["native"]
                .as_str()
                .unwrap()
                .trim_end_matches(".native.jsonl");
            groups.insert(name.to_owned());
        }
    }
    if groups.is_empty() {
        println!("{kind}-{version}: nothing missing; no recording or model call");
        return Ok(());
    }
    let scratch = std::env::var_os("RECORD_FIXTURE_SCRATCH")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::temp_dir().join(format!("brigadier-cli-fixtures-{}", std::process::id()))
        });
    std::fs::create_dir(&scratch)?;
    std::fs::create_dir(scratch.join("data"))?;
    let platform = native_for_identifier(
        "ai.brigadier.cli-fixtures",
        PlatformOptions {
            data_dir: Some(scratch.join("data")),
        },
    )?;
    let env = Arc::new(CliEnv::capture(&platform));
    // Pin this version's real executable: a concurrent CLI updater may replace its symlink.
    if let Some(binary) = env.resolve(kind) {
        env.choose(kind, Some(binary.canonicalize()?));
    }
    let provider: Box<dyn Provider> = match kind {
        ProviderKind::Claude => Box::new(claude::Claude::new(platform, env)),
        ProviderKind::Codex => Box::new(codex::Codex::new(platform, env)),
    };
    let ledger = Arc::new(Kept::default());
    let mut result = Ok(());
    for group in groups {
        eprintln!("Recording missing {kind}-{version}/{group} (D29)");
        let cwd = scratch.join(&group);
        std::fs::create_dir(&cwd)?;
        if let Err(error) = record(provider.as_ref(), &folder, &group, &cwd, ledger.clone()).await {
            result = Err(error);
            break;
        }
    }
    for entry in std::fs::read_dir(&folder)?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "jsonl")
            || path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".native.jsonl"))
        {
            continue;
        }
        let Ok(recording) = Recording::parse(&std::fs::read_to_string(&path)?) else {
            continue;
        };
        let mut replay = provider.replayer();
        if recording.lines.iter().any(|row| replay.feed(row.dir, &row.line).iter().any(|event| matches!(event, ProviderEvent::Error { error } if matches!(error.kind, ErrorKind::UsageLimit | ErrorKind::RateLimit)))) {
            index["limit"] = json!({"status":"recorded","recording":path.file_name().unwrap().to_string_lossy(),"reason":"Opportunistic real CLI limit; never provoked (D25/D29)."});
            std::fs::write(&index_path, serde_json::to_string_pretty(&index)? + "\n")?;
        }
    }
    let artifacts = ledger.0.lock().unwrap().clone();
    std::fs::write(
        scratch.join("cleanup.json"),
        serde_json::to_string(&artifacts)?,
    )?;
    provider.remove(artifacts).await?;
    std::fs::remove_dir_all(&scratch)?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_export_drains_the_scrubber_before_unwrapping_the_lines() {
        let root = std::env::temp_dir().join(format!(
            "brigadier-native-export-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&root).unwrap();
        let source = root.join("source.jsonl");
        let dest = root.join("native.jsonl");
        let text = format!("user@example.test {}", "x".repeat(64_000));
        std::fs::write(&source, format!("{}\n", json!({"type":"user","uuid":"u1","message":{"role":"user","content":text},"accountId":"private-account"}))).unwrap();
        export_native(&source, &dest, ProviderKind::Claude).unwrap();
        let raw = std::fs::read_to_string(&dest).unwrap();
        assert!(!raw.contains("user@example.test"));
        assert!(!raw.contains("private-account"));
        let entries = session_file::read(ProviderKind::Claude, &dest).unwrap();
        assert_eq!(entries.len(), 1);
        assert!(
            matches!(&entries[0].event, ProviderEvent::Message { text, .. } if text.ends_with(&"x".repeat(64_000)))
        );
        assert!(!dest.with_extension("scrubbing").exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}
