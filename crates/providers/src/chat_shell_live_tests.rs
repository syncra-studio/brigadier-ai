//! Live (STRIP-DOWN-PLAN S16c): a Chat's shell runs `brigadier` and nothing else, started with
//! the arguments and config each adapter builds for a Chat. A stand-in `brigadier`, first on
//! `PATH` as the real one is, talks to a Unix socket the test listens on, as the real one talks
//! to the daemon's; the test answers Codex's approval requests as Brigadier answers a Chat's
//! ([`crate::policy::runs_only_brigadier`]):
//! `BRIGADIER_LIVE_CHAT_SHELL=1 cargo test -p brigadier-providers --lib -- --ignored live_chat_shell --nocapture`

use std::io::{Read, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::cli::CliEnv;
use crate::model::*;
use crate::{Artifact, BoxFuture, Ledger, Provider, Result, Started};

/// What the stand-in prints for `brigadier app state`, from the socket.
const STATE: &str = "Brigadier: one window, on the Chats tab (over the socket)";

/// The prompt: two `brigadier` commands (one with a here-document), then four that aren't: two
/// reads in the Chat's own folder, one outside it, and the network.
const PROMPT: &str = "This is a test of your shell. Make exactly these six shell calls, one \
    call each, in order, even if one is refused, then quote what each returned: \
    1) `brigadier app state` \
    2) `brigadier report save --title Probe` with its text in a here-document: the line \
    `<<'EOF'` after the command, then the line `hello from the probe`, then the line `EOF` \
    3) `ls` \
    4) `cat notes.txt` \
    5) `ls /` \
    6) `curl -sI https://example.com`";

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

/// A scratch folder with the stand-in `brigadier` and the socket it reaches; removed on drop.
struct Fake {
    root: PathBuf,
    /// What reached the socket, one entry per call.
    calls: Arc<Mutex<Vec<String>>>,
}

impl Fake {
    fn new(name: &str) -> Self {
        // A short path: a Unix socket's is at most 104 bytes on macOS.
        let root = PathBuf::from("/tmp").join(format!(
            "brigadier-chat-shell-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("bin")).unwrap();
        std::fs::create_dir_all(root.join("chat")).unwrap();
        std::fs::write(root.join("chat/notes.txt"), "PELICAN-42\n").unwrap();
        let script = root.join("bin/brigadier");
        std::fs::write(
            &script,
            "#!/usr/bin/python3\nimport os, socket, sys\nwords = ' '.join(sys.argv[1:])\n\
             text = sys.stdin.read() if 'save' in words else ''\n\
             s = socket.socket(socket.AF_UNIX)\ns.connect(os.environ['BRIGADIER_FAKE_SOCKET'])\n\
             s.sendall((words + '\\n' + text).encode())\ns.shutdown(socket.SHUT_WR)\n\
             print(s.recv(4096).decode())\n",
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
        let listener = UnixListener::bind(root.join("sock")).unwrap();
        let calls: Arc<Mutex<Vec<String>>> = Arc::default();
        let kept = calls.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut call = String::new();
                let _ = stream.read_to_string(&mut call);
                let reply = if call.starts_with("app state") {
                    STATE.to_owned()
                } else {
                    format!("Saved: [Probe](brigadier-page:p1) ({} bytes)", call.len())
                };
                let _ = stream.write_all(reply.as_bytes());
                kept.lock().unwrap().push(call);
            }
        });
        Self { root, calls }
    }

    /// What the session's environment gets: `brigadier`'s folder first on `PATH` (as
    /// `SessionManager::cli_env` puts it), and the socket.
    fn env(&self) -> Vec<(String, String)> {
        let path = std::env::var("PATH").unwrap_or_default();
        vec![
            (
                "PATH".into(),
                format!("{}:{path}", self.root.join("bin").display()),
            ),
            (
                "BRIGADIER_FAKE_SOCKET".into(),
                self.root.join("sock").display().to_string(),
            ),
        ]
    }

    fn spec(&self, model: &str, effort: &str) -> SessionSpec {
        SessionSpec {
            cwd: self.root.join("chat"),
            model: Some(model.into()),
            effort: Some(effort.into()),
            fast: false,
            origin: Origin::New,
            ephemeral: true,
            access: Access::ReadOnly,
            append_system_prompt: None,
            mcp_servers: Vec::new(),
            tools: ToolSet::Chat,
            skill_plugins: Vec::new(),
            installed_skills: Vec::new(),
            switched_off: Default::default(),
            add_dirs: Vec::new(),
            project: None,
            env: self.env(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: false,
            sweep_dir: None,
            auto_compact: true,
            allowed_models: None,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            deny_tools: None,
            lean_worker: false,
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// What a turn did: its commands (command, exit code, output) and its approvals (command,
/// allowed), with a log of every event but the streamed deltas.
#[derive(Default)]
struct Seen {
    commands: Vec<(String, Option<i32>, String)>,
    approvals: Vec<(String, bool)>,
    log: Vec<String>,
}

/// Runs the prompt in `provider`'s Chat and answers its approvals as Brigadier does.
async fn run(provider: &dyn Provider, spec: SessionSpec) -> Seen {
    let kept = Arc::new(Kept(Mutex::default()));
    let Started {
        session,
        mut events,
    } = provider.start(spec, kept.clone()).await.expect("started");
    session.send(TurnInput::text(PROMPT)).await.unwrap();
    let mut seen = Seen::default();
    let turn = async {
        while let Some(event) = events.recv().await {
            match &event {
                ProviderEvent::MessageDelta { .. }
                | ProviderEvent::ReasoningDelta { .. }
                | ProviderEvent::CommandOutputDelta { .. } => continue,
                ProviderEvent::ApprovalRequested { request } => {
                    let command = request.command.clone().unwrap_or_default();
                    let allowed = request.kind == ApprovalKind::Command
                        && crate::policy::runs_only_brigadier(&command);
                    let decision = if allowed {
                        ApprovalDecision::Allow
                    } else {
                        ApprovalDecision::Deny {
                            message: "Declined by Brigadier: a Chat only searches the web and \
                                      runs `brigadier` commands, one at a time."
                                .into(),
                        }
                    };
                    session.answer(request.id.clone(), decision).await.unwrap();
                    seen.approvals.push((command, allowed));
                }
                ProviderEvent::Command {
                    command,
                    exit_code,
                    output: Some(output),
                    ..
                } => seen
                    .commands
                    .push((command.clone(), *exit_code, output.clone())),
                _ => {}
            }
            seen.log.push(format!("{event:?}"));
            if matches!(
                event,
                ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. }
            ) {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(300), turn)
        .await
        .expect("the turn ended");
    session.close().await;
    let artifacts = kept.0.lock().unwrap().clone();
    let _ = provider.remove(artifacts).await;
    seen
}

fn report(name: &str, fake: &Fake, seen: &Seen) {
    eprintln!("== {name}: what reached the socket");
    for call in fake.calls.lock().unwrap().iter() {
        eprintln!("{call:?}");
    }
    eprintln!("== {name}: approvals asked (command, allowed)");
    for approval in &seen.approvals {
        eprintln!("{approval:?}");
    }
    eprintln!("== {name}: commands (command, exit code, output)");
    for command in &seen.commands {
        eprintln!("{command:?}");
    }
    eprintln!("== {name}: events");
    for line in &seen.log {
        eprintln!("{line}");
    }
}

fn platform(fake: &Fake) -> Arc<dyn brigadier_sandbox::Platform> {
    std::fs::create_dir_all(fake.root.join("data")).unwrap();
    brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(fake.root.join("data")),
    })
    .unwrap()
}

/// Claude: `brigadier` runs, the here-document too; `ls /` and `curl` are refused unasked. Claude
/// Code's own read-only commands run in the Chat's folder even so (`ls`, `cat notes.txt`, checked
/// on 2.1.309): reported, not asserted.
#[tokio::test]
#[ignore = "runs the real Claude CLI"]
async fn live_chat_shell_claude_runs_only_brigadier() {
    if std::env::var_os("BRIGADIER_LIVE_CHAT_SHELL").is_none() {
        return;
    }
    let fake = Fake::new("claude");
    let platform = platform(&fake);
    let env = Arc::new(CliEnv::capture(&platform));
    let claude = crate::claude::Claude::new(platform, env);
    let seen = run(&claude, fake.spec("sonnet", "low")).await;
    report("claude", &fake, &seen);
    let calls = fake.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|call| call.starts_with("app state")));
    assert!(
        calls
            .iter()
            .any(|call| call.contains("hello from the probe")),
        "the here-document's text reached the socket: {calls:?}"
    );
    assert!(seen.approvals.is_empty(), "a Chat never asks");
    for (command, _, output) in &seen.commands {
        for ran in ["Applications", "HTTP/"] {
            assert!(!output.contains(ran), "{command} ran: {output}");
        }
    }
}

/// Codex: `brigadier` runs once Brigadier allows it and reaches the socket from the read-only
/// sandbox; Codex asks about the others too, and Brigadier declines them.
#[tokio::test]
#[ignore = "runs the real Codex CLI"]
async fn live_chat_shell_codex_runs_only_brigadier() {
    if std::env::var_os("BRIGADIER_LIVE_CHAT_SHELL").is_none() {
        return;
    }
    let fake = Fake::new("codex");
    let platform = platform(&fake);
    let env = Arc::new(CliEnv::capture(&platform));
    let codex = crate::codex::Codex::new(platform, env);
    let seen = run(&codex, fake.spec("gpt-6-luna", "low")).await;
    report("codex", &fake, &seen);
    let calls = fake.calls.lock().unwrap().clone();
    assert!(calls.iter().any(|call| call.starts_with("app state")));
    assert!(
        seen.approvals
            .iter()
            .any(|(command, allowed)| command.contains("curl") && !allowed),
        "curl was asked about and declined"
    );
    for (command, _, output) in &seen.commands {
        for ran in ["notes.txt", "PELICAN-42", "Applications", "HTTP/"] {
            assert!(!output.contains(ran), "{command} ran: {output}");
        }
    }
}
