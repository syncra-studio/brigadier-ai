//! BUILD-PLAN D2.1's preservation test, live: a session of the machine's own CLI, started
//! through Brigadier's adapter, runs a command that prints 200 KiB with a marker 50 KiB in and
//! calls an MCP tool that returns the same. Each result is kept as the core keeps it, its
//! events are recorded in a store, the CLI's session files are removed the way Brigadier
//! removes them, and the marker is still found through each recorded event's reference, in a
//! streamed export's `outputs/`.
//!
//! The CLIs must be signed in, so this runs only on request:
//! `BRIGADIER_LIVE_CLAUDE=1 cargo test -p brigadier-core --lib -- --ignored live_claude_kept`
//! `BRIGADIER_LIVE_CODEX=1 cargo test -p brigadier-core --lib -- --ignored live_codex_kept`

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use brigadier_providers::cli::CliEnv;
use brigadier_providers::model::Origin;
use brigadier_providers::{
    Access, Artifact, BoxFuture, ItemStatus, Ledger, McpServer, Provider, ProviderEvent,
    SessionSpec, ToolSet, TurnInput,
};
use brigadier_store::{Store, StoreConfig};

use super::export::{OUTPUTS_DIR, export, worker_event};
use super::kept::{keep, kept_mut};
use crate::model::{DomainEvent, TaskId, streams};

const MARKER: &str = "PLANTED-MARKER-7f3a9c";

struct Recorded(Mutex<Vec<Artifact>>);

impl Ledger for Recorded {
    fn record(&self, artifact: Artifact) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        self.0.lock().unwrap().push(artifact);
        Box::pin(async { Ok(()) })
    }
    fn holds(&self, _: &Artifact) -> bool {
        false
    }
}

/// `dir`/gen.py (200 KiB with the marker 50 KiB in) and `dir`/server.py (an MCP server whose
/// `big_dump` returns gen.py's output).
fn scripts(dir: &Path) {
    std::fs::write(
        dir.join("gen.py"),
        format!(
            r#"import sys
out, n, planted = [], 0, False
while n < 200 * 1024:
    if n >= 50 * 1024 and not planted:
        line, planted = "{MARKER}\n", True
    else:
        line = "line %06d filler text filler text filler text filler text\n" % len(out)
    out.append(line)
    n += len(line)
sys.stdout.write("".join(out))
"#
        ),
    )
    .unwrap();
    std::fs::write(
        dir.join("server.py"),
        format!(
            r#"import json, subprocess, sys
for line in sys.stdin:
    msg = json.loads(line)
    if "id" not in msg:
        continue
    method, result = msg.get("method"), {{}}
    if method == "initialize":
        result = {{"protocolVersion": msg["params"]["protocolVersion"], "capabilities": {{"tools": {{}}}}, "serverInfo": {{"name": "probe", "version": "1"}}}}
    elif method == "tools/list":
        result = {{"tools": [{{"name": "big_dump", "description": "Returns a large text dump.", "inputSchema": {{"type": "object", "properties": {{}}}}}}]}}
    elif method == "tools/call":
        text = subprocess.run([sys.executable, {gen:?}], capture_output=True, text=True).stdout
        result = {{"content": [{{"type": "text", "text": text}}]}}
    print(json.dumps({{"jsonrpc": "2.0", "id": msg["id"], "result": result}}), flush=True)
"#,
            gen = dir.join("gen.py").display().to_string()
        ),
    )
    .unwrap();
}

/// Runs the preservation test against `provider` on `model`; returns what the export copied.
async fn preserved(provider: Arc<dyn Provider>, model: &str, effort: Option<&str>) {
    let root = std::env::temp_dir().join(format!("brigadier-kept-live-{}", uuid::Uuid::new_v4()));
    let cwd = root.join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let ok = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&cwd)
        .status()
        .unwrap();
    assert!(ok.success());
    scripts(&root);
    let store = Store::open(StoreConfig {
        db_path: root.join("events.sqlite"),
        blobs_dir: root.join("blobs"),
        readers: 2,
    })
    .unwrap();
    let spec = SessionSpec {
        cwd: cwd.clone(),
        model: Some(model.into()),
        effort: effort.map(str::to_owned),
        fast: false,
        origin: Origin::New,
        ephemeral: false,
        access: Access::Full,
        append_system_prompt: None,
        mcp_servers: vec![McpServer {
            name: "probe".into(),
            command: PathBuf::from("python3"),
            args: vec![root.join("server.py").display().to_string()],
            env: Vec::new(),
            tool_timeout_secs: Some(120),
            trusted: true,
            always_load: true,
            prompt_tools: Vec::new(),
        }],
        tools: ToolSet::Lean,
        add_dirs: Vec::new(),
        skill_plugins: Vec::new(),
        env: Vec::new(),
        unset_env: Vec::new(),
        low_priority: false,
        record_to: None,
        redactor: None,
        owned_cwd: true,
        sweep_dir: None,
        auto_compact: true,
        allowed_models: None,
        auto_review: false,
        omit_ai_coauthors: false,
        output_hook: None,
        deny_tools: None,
        protection: Default::default(),
    };
    let ledger = Arc::new(Recorded(Mutex::default()));
    let started = provider.start(spec, ledger.clone()).await.expect("started");
    let mut events = started.events;
    started
        .session
        .send(TurnInput::text(format!(
            "This is a test of how tool output is stored. Make exactly these two tool calls, one \
             after the other: 1) run this shell command once: python3 {}  2) call the big_dump \
             tool of the probe MCP server once. Don't read or search their output afterwards. \
             Then reply with only the word DONE.",
            root.join("gen.py").display()
        )))
        .await
        .unwrap();
    let task = TaskId("live".into());
    let stream = streams::task(&task);
    let mut results = 0;
    let mut saved: Vec<String> = Vec::new();
    let turn = async {
        while let Some(mut event) = events.recv().await {
            if let Some(kept) = kept_mut(&mut event) {
                saved.extend(kept.file.as_ref().map(|file| file.path.clone()));
                keep(kept, |bytes| {
                    let blobs = store.blobs().clone();
                    async move { Ok(blobs.put(bytes).await?.to_string()) }
                })
                .await;
            }
            if matches!(
                &event,
                ProviderEvent::Command {
                    status: ItemStatus::Completed | ItemStatus::Failed,
                    ..
                } | ProviderEvent::ToolCall {
                    status: ItemStatus::Completed | ItemStatus::Failed,
                    ..
                }
            ) {
                results += 1;
            }
            let done = matches!(event, ProviderEvent::TurnCompleted { .. });
            let event = DomainEvent::WorkerEvent {
                task_id: task.clone(),
                event,
            };
            let new = brigadier_store::NewEvent::new(
                stream.clone(),
                event.kind(),
                crate::now_ms(),
                &event,
            )
            .unwrap();
            store.append(vec![new]).await.unwrap();
            if done {
                break;
            }
        }
    };
    tokio::time::timeout(std::time::Duration::from_secs(600), turn)
        .await
        .expect("the turn ended");
    assert!(results >= 2, "both calls ran");
    started.session.close().await;
    // The CLI's own session files go, as Brigadier's cleanup removes them.
    let made = ledger.0.lock().unwrap().clone();
    assert!(!made.is_empty(), "the session recorded what it made");
    provider.remove(made).await.unwrap();
    for path in &saved {
        assert!(!Path::new(path).exists(), "{path} is removed with its session");
    }

    let out = root.join("handoff");
    std::fs::create_dir_all(&out).unwrap();
    let file = tokio::fs::File::create(out.join("transcript.md"))
        .await
        .unwrap();
    let blobs = store.blobs().clone();
    let exported = export(
        |after| store.read_stream_since(stream.clone(), after, 2_000),
        |hash: String| {
            let blobs = blobs.clone();
            async move { Ok(blobs.file_of(&hash.parse()?)) }
        },
        &HashMap::new(),
        worker_event,
        "The worker",
        tokio::io::BufWriter::new(file),
        &out.join(OUTPUTS_DIR),
    )
    .await;
    let transcript = std::fs::read_to_string(out.join("transcript.md")).unwrap();
    assert!(exported.complete(), "{:?}\n{transcript}", exported.gaps);
    let mut found = 0;
    for entry in std::fs::read_dir(out.join(OUTPUTS_DIR)).unwrap() {
        let path = entry.unwrap().path();
        let text = std::fs::read_to_string(&path).unwrap();
        eprintln!(
            "{}: {} bytes, marker {}",
            path.display(),
            text.len(),
            text.contains(MARKER)
        );
        found += usize::from(text.contains(MARKER));
    }
    assert!(found >= 2, "the marker of both results: {transcript}");
    eprintln!("kept outputs found after the CLI's files were removed: {found}");
    store.shutdown().await.unwrap();
    std::fs::remove_dir_all(&root).unwrap();
}

fn platform(data: &Path) -> Arc<dyn brigadier_sandbox::Platform> {
    std::fs::create_dir_all(data).unwrap();
    brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(data.to_path_buf()),
    })
    .unwrap()
}

#[tokio::test]
#[ignore = "runs the real Claude CLI"]
async fn live_claude_kept_outputs_outlive_the_cli_session() {
    if std::env::var_os("BRIGADIER_LIVE_CLAUDE").is_none() {
        return;
    }
    let data = std::env::temp_dir().join(format!("brigadier-kept-data-{}", uuid::Uuid::new_v4()));
    let platform = platform(&data);
    let env = Arc::new(CliEnv::capture(&platform));
    let claude = Arc::new(brigadier_providers::claude::Claude::new(platform, env));
    preserved(claude, "haiku", None).await;
    let _ = std::fs::remove_dir_all(&data);
}

#[tokio::test]
#[ignore = "runs the real Codex CLI"]
async fn live_codex_kept_outputs_outlive_the_cli_session() {
    if std::env::var_os("BRIGADIER_LIVE_CODEX").is_none() {
        return;
    }
    let data = std::env::temp_dir().join(format!("brigadier-kept-data-{}", uuid::Uuid::new_v4()));
    let platform = platform(&data);
    let env = Arc::new(CliEnv::capture(&platform));
    let codex = Arc::new(brigadier_providers::codex::Codex::new(platform, env));
    preserved(codex, "gpt-6.1-sol", Some("low")).await;
    let _ = std::fs::remove_dir_all(&data);
}
