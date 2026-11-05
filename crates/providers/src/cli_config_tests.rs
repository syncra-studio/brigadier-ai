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
            "20261022-0805-take-over-5-sessions-w181-config-{}",
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
capture({'argv': sys.argv[1:], 'env': {k: v for k, v in os.environ.items() if k.startswith('BRIGADIER_MCP_') or k in ('ANTHROPIC_API_KEY', 'OPENAI_API_KEY')}})
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
            other => unreachable!("{other} isn't driven here"),
        }
    }

    /// Claude on an extra account, started from an environment with a made-up login variable.
    fn account(&self) -> Box<dyn Provider> {
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
            ("PROVIDER".into(), "claude".into()),
            (
                "CAPTURE".into(),
                self.0.join("capture.jsonl").into_os_string(),
            ),
            ("ANTHROPIC_API_KEY".into(), MADE_UP_LOGIN.into()),
        ]));
        env.choose(ProviderKind::Claude, Some(self.0.join("fake-cli")));
        std::fs::create_dir_all(self.0.join("extra")).unwrap();
        Box::new(claude::Claude::for_account(
            platform,
            &env,
            &self.0.join("extra"),
        ))
    }

    fn captured_start(&self) -> Value {
        let capture = std::fs::read_to_string(self.0.join("capture.jsonl")).unwrap();
        serde_json::from_str(capture.lines().next().unwrap()).unwrap()
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
                    // B: launch fills gateway metadata for resolved remote servers.
                    gateway: None,
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
                    sources: Default::default(),
                }]
            } else {
                vec![]
            },
            tools: ToolSet::Thread,
            skill_plugins: vec![],
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
            naked_worker: false,
            native_compaction: None,
            temp_tag: None,
        }
    }
}

impl Drop for Fake {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

const MADE_UP_LOGIN: &str = "made-up-login-value";

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
            "untrusted",
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
                    // The daemon's socket and binary stay off its command line (#811).
                    let settings = flag("--settings").unwrap().as_str().unwrap();
                    assert!(settings.ends_with(".json"), "{settings}");
                    assert!(
                        !args
                            .iter()
                            .any(|arg| arg.as_str().unwrap().contains("autoMemoryEnabled")),
                        "{args:?}"
                    );
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
                other => unreachable!("{other} isn't driven here"),
            }
        }
    }
}

fn header_server() -> McpServer {
    let var = |name: &str| McpValuePart::Var(name.into());
    McpServer {
        gateway: None,
        name: "web".into(),
        transport: McpTransport::Http {
            url: "https://mcp.example.invalid/mcp".into(),
        },
        env: vec![
            ("Authorization".into(), format!("Bearer {MADE_UP_LOGIN}")),
            ("X-Key".into(), "made-up-key-value".into()),
        ],
        tool_timeout_secs: None,
        trusted: false,
        always_load: false,
        sources: McpSources {
            references: vec![
                (
                    "Authorization".into(),
                    vec![
                        McpValuePart::Text("Bearer ".into()),
                        var("ANTHROPIC_API_KEY"),
                    ],
                ),
                ("X-Key".into(), vec![var("OTHER_VAR")]),
            ],
            headers_helper: None,
        },
    }
}

fn headers_in(args: &[Value]) -> Value {
    let at = args.iter().position(|arg| arg == "--mcp-config").unwrap();
    let config: Value = serde_json::from_str(args[at + 1].as_str().unwrap()).unwrap();
    config["mcpServers"]["web"]["headers"].clone()
}

/// An extra account's CLI runs without its login variables, so a header that reads one by name
/// would reach the server without it: its value goes by alias, in that CLI's environment only,
/// and any other variable stays a reference. The user's own account keeps the reference.
#[tokio::test]
async fn an_extra_accounts_header_on_a_login_variable_goes_by_alias_never_on_its_command_line() {
    let fake = Fake::new();
    let provider = fake.account();
    let mut spec = fake.chat(Access::Full, false);
    spec.mcp_servers = vec![header_server()];
    let started = provider.start(spec, Arc::new(Kept)).await.unwrap();
    let row = fake.captured_start();
    let args = row["argv"].as_array().unwrap();
    assert_eq!(
        headers_in(args),
        json!({"Authorization": "${BRIGADIER_MCP_0_0}", "X-Key": "${OTHER_VAR}"})
    );
    assert_eq!(
        row["env"],
        json!({"BRIGADIER_MCP_0_0": format!("Bearer {MADE_UP_LOGIN}")})
    );
    assert!(!row["argv"].to_string().contains(MADE_UP_LOGIN));
    let at = args.iter().position(|arg| arg == "--settings").unwrap();
    let layer = std::fs::read_to_string(args[at + 1].as_str().unwrap()).unwrap();
    assert!(!layer.contains(MADE_UP_LOGIN));
    started.session.close().await;

    let mut resume = fake.chat(Access::Full, false);
    resume.mcp_servers = vec![header_server()];
    resume.origin = Origin::Resume {
        native_id: "00000000-0000-4000-8000-000000000000".into(),
    };
    let terminal = provider
        .terminal(resume, fake.0.join("chat"))
        .await
        .unwrap();
    let args: Vec<Value> = terminal.args.iter().map(|arg| json!(arg)).collect();
    assert_eq!(headers_in(&args)["Authorization"], "${BRIGADIER_MCP_0_0}");
    assert!(!format!("{args:?}").contains(MADE_UP_LOGIN));

    std::fs::write(fake.0.join("capture.jsonl"), "").unwrap();
    let own = fake.provider(ProviderKind::Claude);
    let mut spec = fake.chat(Access::Full, false);
    spec.mcp_servers = vec![header_server()];
    let started = own.start(spec, Arc::new(Kept)).await.unwrap();
    let row = fake.captured_start();
    assert_eq!(
        headers_in(row["argv"].as_array().unwrap())["Authorization"],
        "${BRIGADIER_MCP_0_0_0}${ANTHROPIC_API_KEY}"
    );
    started.session.close().await;
}

struct Refused;
impl Ledger for Refused {
    fn record(&self, _: Artifact) -> BoxFuture<'_, Result<()>> {
        Box::pin(async { Err(Error::Invalid("scratch ledger refusal".into())) })
    }
    fn holds(&self, _: &Artifact) -> bool {
        false
    }
}

/// The settings file can hold a naked worker's user-settings env: a start that fails takes it
/// with it, and one that started keeps it until its CLI has exited.
#[tokio::test]
async fn a_claude_settings_file_goes_with_a_failed_start_and_with_its_cli() {
    let fake = Fake::new();
    let provider = fake.account();
    std::fs::write(
        fake.0.join("extra/settings.json"),
        r#"{"env":{"MADE_UP_SETTING":"made-up-setting-value"}}"#,
    )
    .unwrap();
    let dir = fake.0.join("data/run/claude-settings");
    let files = || -> Vec<_> {
        std::fs::read_dir(&dir)
            .map(|files| files.map(|file| file.unwrap().path()).collect())
            .unwrap_or_default()
    };
    let naked = || SessionSpec {
        naked_worker: true,
        ..fake.chat(Access::Full, false)
    };

    let started = provider.start(naked(), Arc::new(Kept)).await.unwrap();
    assert_eq!(files().len(), 1);
    started.session.close().await;
    for _ in 0..100 {
        if files().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert!(files().is_empty(), "its CLI has exited: {:?}", files());

    let failed = provider.start(naked(), Arc::new(Refused)).await;
    assert!(failed.is_err());
    assert!(files().is_empty(), "the start failed: {:?}", files());
}

/// Codex's extra account runs without its login variables too. A header it would read by name
/// from one goes by alias instead, and never as a literal header in its config.
#[tokio::test]
async fn an_extra_codex_accounts_header_on_a_login_variable_goes_by_alias() {
    let fake = Fake::new();
    let platform = brigadier_sandbox::native(brigadier_sandbox::PlatformOptions {
        data_dir: Some(fake.0.join("data")),
    })
    .unwrap();
    platform
        .private_fs()
        .create_private_dir(&platform.paths().data_dir)
        .unwrap();
    let env = Arc::new(CliEnv::from_vars([
        ("HOME".into(), fake.0.as_os_str().to_owned()),
        ("PATH".into(), std::env::var_os("PATH").unwrap()),
        ("PROVIDER".into(), "codex".into()),
        (
            "CAPTURE".into(),
            fake.0.join("capture.jsonl").into_os_string(),
        ),
        (
            "CATALOG".into(),
            fake.0.join("catalog.json").into_os_string(),
        ),
        ("OPENAI_API_KEY".into(), MADE_UP_LOGIN.into()),
    ]));
    env.choose(ProviderKind::Codex, Some(fake.0.join("fake-cli")));
    std::fs::create_dir_all(fake.0.join("extra")).unwrap();
    let provider = codex::Codex::for_account(platform, &env, &fake.0.join("extra"));
    let mut server = header_server();
    server.env.truncate(1);
    server.sources.references = vec![(
        "Authorization".into(),
        vec![
            McpValuePart::Text("Bearer ".into()),
            McpValuePart::Var("OPENAI_API_KEY".into()),
        ],
    )];
    let mut spec = fake.chat(Access::Full, false);
    spec.mcp_servers = vec![server];
    let started = provider.start(spec, Arc::new(Kept)).await.unwrap();
    let capture = std::fs::read_to_string(fake.0.join("capture.jsonl")).unwrap();
    let rows: Vec<Value> = capture
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let thread = rows
        .iter()
        .find(|row| row["method"] == "thread/start")
        .unwrap();
    let web = &thread["params"]["config"]["mcp_servers"]["web"];
    assert!(web.get("bearer_token_env_var").is_none(), "{web}");
    assert_eq!(web["http_headers"], json!({}));
    let alias = web["env_http_headers"]["Authorization"].as_str().unwrap();
    assert_eq!(
        rows[0]["env"][alias],
        json!(format!("Bearer {MADE_UP_LOGIN}"))
    );
    assert!(rows[0]["env"].get("OPENAI_API_KEY").is_none());
    assert!(!rows[0]["argv"].to_string().contains(MADE_UP_LOGIN));
    assert!(!thread.to_string().contains(MADE_UP_LOGIN));
    started.session.close().await;
}
