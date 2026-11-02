//! D167, live: the must-follow rules (the standing instructions, D58) are in force after a
//! compaction and in a reborn session, through each CLI's real adapter. The session names the
//! command its rules say to run after `/compact`, and a fresh worker session (a rebirth starts
//! one the same way) names it on its first turn. Its CLI sessions are removed after.
//!
//! The CLIs must be signed in, so this runs only on request:
//! `BRIGADIER_LIVE_STANDING=1 cargo test -p brigadier-core --lib -- --ignored live_standing --nocapture`

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::cli::CliEnv;
use brigadier_providers::model::Origin;
use brigadier_providers::{
    Access, Artifact, BoxFuture, Ledger, Provider, ProviderEvent, ProviderKind, ProviderSession,
    Role, SessionSpec, Started, ToolSet, TurnInput,
};
use tokio::sync::mpsc::Receiver;

use super::prompts::{session_instructions, standing_text, worker_instructions};
use crate::model::Settings;

/// A fact only the rules hold: the command to run at the start and after any compaction.
const ASK: &str = "Without running any tool or command: which `brigadier` command do your \
                   Brigadier instructions say to run when you start and after any compaction? \
                   The command only.";

fn knows_the_rules(said: &str) -> bool {
    said.contains("brigadier context") || said.contains("brigadier state")
}

struct Made(Mutex<Vec<Artifact>>);

impl Ledger for Made {
    fn record(&self, artifact: Artifact) -> BoxFuture<'_, brigadier_providers::Result<()>> {
        self.0.lock().unwrap().push(artifact);
        Box::pin(async { Ok(()) })
    }
    fn holds(&self, _: &Artifact) -> bool {
        false
    }
}

fn spec(cwd: &Path, model: Option<&str>, instructions: String) -> SessionSpec {
    SessionSpec {
        cwd: cwd.to_path_buf(),
        model: model.map(Into::into),
        effort: None,
        fast: false,
        origin: Origin::New,
        ephemeral: false,
        access: Access::ReadOnly,
        append_system_prompt: Some(standing_text(&instructions)),
        mcp_servers: Vec::new(),
        tools: ToolSet::Thread,
        skill_plugins: Vec::new(),
        switched_off: Default::default(),
        add_dirs: Vec::new(),
        project: None,
        env: Vec::new(),
        unset_env: Vec::new(),
        low_priority: false,
        record_to: None,
        redactor: None,
        owned_cwd: false,
        sweep_dir: None,
        keeps_detached: false,
        auto_compact: true,
        allowed_models: None,
        omit_ai_coauthors: false,
        output_hook: None,
        stop_hook: None,
        naked_worker: false,
        native_compaction: None,
        temp_tag: None,
    }
}

/// What the turn running now said once it ends, and whether it compacted.
async fn turn_end(events: &mut Receiver<ProviderEvent>) -> (String, bool) {
    let (mut said, mut compacted) = (String::new(), false);
    let end = async {
        while let Some(event) = events.recv().await {
            match event {
                ProviderEvent::Message {
                    role: Role::Assistant,
                    text,
                    ..
                } => said.push_str(&text),
                ProviderEvent::CompactionEnded { .. } => compacted = true,
                ProviderEvent::TurnCompleted { .. } => return,
                ProviderEvent::Exited { .. } => panic!("the CLI exited"),
                _ => {}
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(300), end)
        .await
        .expect("the turn ended");
    (said, compacted)
}

async fn ask(session: &Arc<dyn ProviderSession>, events: &mut Receiver<ProviderEvent>) -> String {
    session.send(TurnInput::text(ASK)).await.unwrap();
    turn_end(events).await.0
}

async fn standing(provider: Arc<dyn Provider>, kind: ProviderKind, model: Option<&str>) {
    let cwd = std::env::temp_dir().join(format!("core-goals-w128-standing-{kind:?}"));
    std::fs::create_dir_all(&cwd).unwrap();
    let made = Arc::new(Made(Mutex::default()));
    let settings = Settings::default();

    // The orchestrator: a turn, a compaction, then the rule.
    let Started {
        session,
        mut events,
    } = provider
        .start(
            spec(&cwd, model, session_instructions(&settings, kind)),
            made.clone(),
        )
        .await
        .unwrap();
    session.send(TurnInput::text("Say OK.")).await.unwrap();
    turn_end(&mut events).await;
    session.compact(None).await.unwrap();
    assert!(turn_end(&mut events).await.1, "{kind:?} compacted");
    let compacted = ask(&session, &mut events).await;
    session.close().await;

    // A reborn worker: a fresh session started as every worker's is.
    let Started {
        session,
        mut events,
    } = provider
        .start(
            spec(&cwd, model, worker_instructions(&settings, kind)),
            made.clone(),
        )
        .await
        .unwrap();
    let reborn = ask(&session, &mut events).await;
    session.close().await;

    let artifacts = made.0.lock().unwrap().clone();
    let _ = provider.remove(artifacts).await;
    let _ = std::fs::remove_dir_all(&cwd);
    eprintln!("{kind:?} after compaction: {compacted}\n{kind:?} reborn worker: {reborn}");
    assert!(knows_the_rules(&compacted), "{kind:?}: {compacted}");
    assert!(knows_the_rules(&reborn), "{kind:?}: {reborn}");
}

fn env_of(
    kind: &str,
) -> (
    Arc<dyn brigadier_sandbox::Platform>,
    Arc<CliEnv>,
    std::path::PathBuf,
) {
    let data = std::env::temp_dir().join(format!("core-goals-w128-standing-data-{kind}"));
    std::fs::create_dir_all(&data).unwrap();
    let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(data.clone()),
    })
    .unwrap();
    let env = Arc::new(CliEnv::capture(&platform));
    (platform, env, data)
}

#[tokio::test]
#[ignore = "runs the real CLIs"]
async fn live_standing_rules_survive_compaction_and_rebirth() {
    if std::env::var_os("BRIGADIER_LIVE_STANDING").is_none() {
        return;
    }
    let only = std::env::var("BRIGADIER_LIVE_STANDING").unwrap_or_default();
    let wanted = |kind: &str| only == "1" || only == kind;
    if wanted("claude") {
        let (platform, env, data) = env_of("claude");
        let claude = Arc::new(brigadier_providers::claude::Claude::new(platform, env));
        standing(claude, ProviderKind::Claude, Some("haiku")).await;
        let _ = std::fs::remove_dir_all(data);
    }
    if wanted("codex") {
        let (platform, env, data) = env_of("codex");
        let codex = Arc::new(brigadier_providers::codex::Codex::new(platform, env));
        standing(codex, ProviderKind::Codex, Some("gpt-6.1-sol")).await;
        let _ = std::fs::remove_dir_all(data);
    }
    if wanted("opencode") {
        let (platform, env, data) = env_of("opencode");
        let opencode = Arc::new(brigadier_providers::opencode::OpenCode::new(platform, env));
        // A free model: the check spends none of the user's OpenCode Go plan.
        standing(
            opencode,
            ProviderKind::OpenCode,
            Some("opencode/glm-5.3-flash-free"),
        )
        .await;
        let _ = std::fs::remove_dir_all(data);
    }
}
