//! S28/S35: inspect what the real adapters send to stand-in CLIs, without live accounts.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use crate::cli::CliEnv;
use crate::*;

struct Fake(PathBuf);

impl Fake {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "20261022-0805-take-over-5-sessions-w189-config-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(root.join("chat")).unwrap();
        std::fs::create_dir_all(root.join("record")).unwrap();
        std::fs::write(
            root.join("catalog.json"),
            json!({"models": [
                {"slug": "probe-model", "max_context_window": 200000}
            ]})
            .to_string(),
        )
        .unwrap();
        let binary = root.join("fake-cli");
        std::fs::write(&binary, r#"#!/usr/bin/env python3
import json, os, sys
if '--version' in sys.argv:
    print('2.1.309' if os.environ['PROVIDER'] == 'claude' else 'codex-cli 0.163.0')
    sys.exit(0)
def capture(value):
    with open(os.environ['CAPTURE'], 'a') as f:
        f.write(json.dumps(value) + '\n')
capture({'argv': sys.argv[1:]})
for line in sys.stdin:
    m = json.loads(line)
    capture(m)
    if m.get('type') == 'control_request':
        result = {'rawMaxTokens': 200000} if m['request']['subtype'] == 'get_context_usage' else {}
        print(json.dumps({'type': 'control_response', 'response': {
            'subtype': 'success', 'request_id': m['request_id'], 'response': result}}), flush=True)
    elif 'id' in m and 'method' in m:
        method, p = m['method'], m.get('params', {})
        result = {}
        if method == 'config/read':
            result = {'config': {'model': 'probe-model', 'model_catalog_json': os.environ['CATALOG'], 'mcp_servers': {}}, 'layers': []}
        elif method == 'model/list' or method == 'mcpServerStatus/list':
            result = {'data': [], 'nextCursor': None}
        elif method == 'thread/start':
            thread = {'id': 'probe-thread', 'sessionId': 'probe-thread', 'cliVersion': '0.163.0',
                'createdAt': 0, 'updatedAt': 0, 'cwd': p['cwd'], 'ephemeral': False,
                'modelProvider': 'test', 'preview': '', 'projectId': None,
                'source': 'appServer', 'status': {'type': 'idle'}, 'turns': []}
            result = {'thread': thread, 'model': 'probe-model', 'modelProvider': 'test',
                'approvalPolicy': p['approvalPolicy'], 'approvalsReviewer': 'user',
                'cwd': p['cwd'], 'sandbox': {'type': 'dangerFullAccess'}}
        elif method == 'turn/start':
            result = {'turn': {'id': 'probe-turn', 'items': [], 'status': 'inProgress'}}
        print(json.dumps({'jsonrpc': '2.0', 'id': m['id'], 'result': result}), flush=True)
"#).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(binary, std::fs::Permissions::from_mode(0o755)).unwrap();
        Self(root)
    }

    fn provider(&self, kind: ProviderKind) -> Box<dyn Provider> {
        let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
            data_dir: Some(self.0.join("data")),
        })
        .unwrap();
        platform
            .private_fs()
            .create_private_dir(&platform.paths().data_dir)
            .unwrap();
        let env = Arc::new(CliEnv::from_vars([
            ("HOME".into(), self.0.as_os_str().to_owned()),
            ("PATH".into(), std::env::var_os("PATH").unwrap()),
            (
                "PROVIDER".into(),
                if kind == ProviderKind::Claude {
                    "claude"
                } else {
                    "codex"
                }
                .into(),
            ),
            (
                "CAPTURE".into(),
                self.0.join("capture.jsonl").into_os_string(),
            ),
            (
                "CATALOG".into(),
                self.0.join("catalog.json").into_os_string(),
            ),
        ]));
        env.choose(kind, Some(self.0.join("fake-cli")));
        match kind {
            ProviderKind::Claude => Box::new(claude::Claude::new(platform, env)),
            ProviderKind::Codex => Box::new(codex::Codex::new(platform, env)),
        }
    }

    fn chat(&self, access: Access, computer: bool) -> SessionSpec {
        SessionSpec {
            cwd: self.0.join("chat"),
            model: Some("probe-model".into()),
            effort: None,
            fast: false,
            origin: Origin::New,
            ephemeral: false,
            access,
            append_system_prompt: None,
            mcp_servers: if computer {
                vec![McpServer {
                    name: "computer".into(),
                    transport: McpTransport::Stdio {
                        command: self.0.join("computer"),
                        args: vec!["mcp".into()],
                        cwd: None,
                    },
                    env: vec![("BRIGADIER_CU_SOCKET_AUTH_TOKEN".into(), "test-token".into())],
                    tool_timeout_secs: Some(120),
                    trusted: true,
                    always_load: true,
                }]
            } else {
                vec![]
            },
            tools: ToolSet::Thread,
            skill_plugins: vec![],
            installed_skills: vec![],
            switched_off: Default::default(),
            add_dirs: vec![self.0.join("record")],
            project: None,
            env: vec![],
            unset_env: vec![],
            low_priority: false,
            record_to: None,
            redactor: None,
            owned_cwd: true,
            sweep_dir: None,
            keeps_detached: true,
            auto_compact: true,
            allowed_models: None,
            omit_ai_coauthors: false,
            output_hook: None,
            stop_hook: None,
            lean_worker: false,
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

struct Kept;
impl Ledger for Kept {
    fn record(&self, _: Artifact) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Ok(()) })
    }
    fn holds(&self, _: &Artifact) -> bool {
        false
    }
}

#[tokio::test]
async fn claude_chats_pass_their_levels_and_computer_config_to_the_cli() {
    chat_config(ProviderKind::Claude).await;
}

#[tokio::test]
async fn codex_chats_pass_their_levels_and_computer_config_to_the_cli() {
    chat_config(ProviderKind::Codex).await;
}

async fn chat_config(kind: ProviderKind) {
    let fake = Fake::new();
    let provider = fake.provider(kind);
    for (access, claude_mode, approval, sandbox) in [
        (Access::Ask, "default", "untrusted", "workspaceWrite"),
        (
            Access::AcceptEdits,
            "acceptEdits",
            "on-request",
            "workspaceWrite",
        ),
        (
            Access::Full,
            "bypassPermissions",
            "never",
            "dangerFullAccess",
        ),
    ] {
        for computer in [false, true] {
            std::fs::write(fake.0.join("capture.jsonl"), "").unwrap();
            let started = provider
                .start(fake.chat(access, computer), Arc::new(Kept))
                .await
                .unwrap();
            if kind == ProviderKind::Codex {
                started
                    .session
                    .send(TurnInput::text("Hello from a Chat."))
                    .await
                    .unwrap();
            }
            started.session.close().await;
            let captured: Vec<Value> = std::fs::read_to_string(fake.0.join("capture.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            match kind {
                ProviderKind::Claude => {
                    let args = captured[0]["argv"].as_array().unwrap();
                    let flag = |name: &str| {
                        args.windows(2)
                            .find(|pair| pair[0] == name)
                            .map(|pair| &pair[1])
                    };
                    assert_eq!(flag("--permission-mode"), Some(&json!(claude_mode)));
                    assert_eq!(flag("--permission-prompt-tool"), Some(&json!("stdio")));
                    assert!(
                        flag("--tools").is_none(),
                        "a Chat has the CLI's own tools: {args:?}"
                    );
                    let config: Value =
                        serde_json::from_str(flag("--mcp-config").unwrap().as_str().unwrap())
                            .unwrap();
                    assert_eq!(config["mcpServers"].get("computer").is_some(), computer);
                    if computer {
                        assert_eq!(
                            config["mcpServers"]["computer"]["command"],
                            json!(fake.0.join("computer"))
                        );
                        assert_eq!(config["mcpServers"]["computer"]["args"], json!(["mcp"]));
                    }
                }
                ProviderKind::Codex => {
                    let request = |method: &str| {
                        &captured.iter().find(|m| m["method"] == method).unwrap()["params"]
                    };
                    let start = request("thread/start");
                    assert_eq!(start["approvalPolicy"], approval);
                    let turn = request("turn/start");
                    assert_eq!(turn["approvalPolicy"], approval);
                    assert_eq!(turn["sandboxPolicy"]["type"], sandbox);
                    if access != Access::Full {
                        assert_eq!(turn["sandboxPolicy"]["networkAccess"], true);
                        assert!(
                            turn["sandboxPolicy"]["writableRoots"]
                                .as_array()
                                .unwrap()
                                .contains(&json!(fake.0.join("record").canonicalize().unwrap()))
                        );
                    }
                    let servers = &start["config"]["mcp_servers"];
                    assert_eq!(servers.get("computer").is_some(), computer);
                    if computer {
                        assert_eq!(
                            servers["computer"]["command"],
                            json!(fake.0.join("computer"))
                        );
                        assert_eq!(servers["computer"]["args"], json!(["mcp"]));
                        assert_eq!(
                            servers["computer"]["env"]["BRIGADIER_CU_SOCKET_AUTH_TOKEN"],
                            "test-token"
                        );
                    }
                }
            }
        }
    }
}

/// Optional skills use the existing per-process loading path, never the CLI's global folders.
#[tokio::test]
async fn optional_skill_roots_reach_only_the_selected_cli_session() {
    for kind in [ProviderKind::Claude, ProviderKind::Codex] {
        let fake = Fake::new();
        let provider = fake.provider(kind);
        let mut plugins = Vec::new();
        for name in ["retro", "writing-for-agents"] {
            let plugin = fake.0.join("optional-skills").join(name);
            let skill = plugin.join("skills").join(name);
            std::fs::create_dir_all(plugin.join(".claude-plugin")).unwrap();
            std::fs::create_dir_all(&skill).unwrap();
            std::fs::write(
                plugin.join(".claude-plugin/plugin.json"),
                json!({"name": name}).to_string(),
            )
            .unwrap();
            std::fs::write(
                skill.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: Test\n---\n"),
            )
            .unwrap();
            plugins.push(plugin);
        }
        for enabled in [false, true] {
            std::fs::write(fake.0.join("capture.jsonl"), "").unwrap();
            let mut spec = fake.chat(Access::AcceptEdits, false);
            if enabled {
                spec.skill_plugins = plugins.clone();
            }
            let started = provider.start(spec, Arc::new(Kept)).await.unwrap();
            if kind == ProviderKind::Codex {
                started
                    .session
                    .send(TurnInput::text("/retro"))
                    .await
                    .unwrap();
            }
            started.session.close().await;
            let captured: Vec<Value> = std::fs::read_to_string(fake.0.join("capture.jsonl"))
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str(line).unwrap())
                .collect();
            match kind {
                ProviderKind::Claude => {
                    let args = captured[0]["argv"].as_array().unwrap();
                    let loaded: Vec<_> = args
                        .windows(2)
                        .filter(|pair| pair[0] == "--plugin-dir")
                        .map(|pair| pair[1].as_str().unwrap().to_owned())
                        .collect();
                    let expected: Vec<_> = if enabled {
                        plugins.iter().map(|p| p.display().to_string()).collect()
                    } else {
                        Vec::new()
                    };
                    assert_eq!(loaded, expected);
                }
                ProviderKind::Codex => {
                    let roots = captured
                        .iter()
                        .find(|m| m["method"] == "skills/extraRoots/set");
                    if enabled {
                        let expected: Vec<_> = plugins
                            .iter()
                            .map(|p| p.join("skills").display().to_string())
                            .collect();
                        assert_eq!(roots.unwrap()["params"]["extraRoots"], json!(expected));
                    } else {
                        assert!(roots.is_none());
                    }
                }
            }
            for global in [
                ".claude/skills/retro",
                ".agents/skills/retro",
                ".codex/skills/retro",
                ".codex/skills/writing-for-agents",
            ] {
                assert!(!fake.0.join(global).exists(), "{global}");
            }
        }
    }
}
