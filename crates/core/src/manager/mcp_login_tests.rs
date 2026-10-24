use super::*;

#[test]
fn the_sign_in_page_is_the_link_with_an_oauth_request() {
    // Claude Code 2.1.310 prints it as a terminal link too.
    let claude = "Starting authentication for \"oauthy\"…\r\nVisit this URL to authorize:\r\n  \
        \u{1b}]8;;http://127.0.0.1:47615/authorize?response_type=code&state=a\u{7}\
        http://127.0.0.1:47615/authorize?response_type=code&state=a\u{1b}]8;;\u{7}\r\n";
    assert_eq!(
        sign_in_link(claude).as_deref(),
        Some("http://127.0.0.1:47615/authorize?response_type=code&state=a")
    );
    let codex = "Authorize the MCP server by opening this URL in your browser:\n\
        https://auth.example.com/authorize?code_challenge=x&redirect_uri=http%3A%2F%2F127.0.0.1\n";
    assert_eq!(
        sign_in_link(codex).as_deref(),
        Some(
            "https://auth.example.com/authorize?code_challenge=x&redirect_uri=http%3A%2F%2F127.0.0.1"
        )
    );
    assert_eq!(sign_in_link("see https://docs.example.com/mcp\n"), None);
    assert_eq!(
        last_words("Callback URL: Error: No OAuth callback URL received\n\n").as_deref(),
        Some("No OAuth callback URL received")
    );
}

#[test]
fn the_status_sums_the_agents_up() {
    let agent = |state| AgentSignIn {
        agent: ProviderKind::Claude,
        server: "s".into(),
        state,
    };
    let progress = |states: Vec<AgentSignInState>, finished| McpSignInProgress {
        agents: states.into_iter().map(agent).collect(),
        finished,
    };
    use AgentSignInState as S;
    assert_eq!(
        progress(vec![S::SignedIn, S::InBrowser], false).status(),
        McpSignInStatus::Pending
    );
    assert_eq!(
        progress(vec![S::AlreadySignedIn, S::SignedIn], true).status(),
        McpSignInStatus::SignedIn
    );
    assert_eq!(
        progress(vec![S::SignedIn, S::Cancelled], true).status(),
        McpSignInStatus::Cancelled
    );
    assert_eq!(
        progress(
            vec![
                S::Failed {
                    reason: "no".into()
                },
                S::Cancelled
            ],
            true
        )
        .status(),
        McpSignInStatus::Failed {
            reason: "no".into()
        }
    );
}

/// Stand-in CLIs on a throwaway folder: each lists `srv` (Claude's as a plugin's) as needing a
/// sign-in until its login is done, and its login prints a page and waits until the "browser"
/// visits it. Claude's refuses to run without a terminal, as the real one does.
#[cfg(unix)]
mod fake {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    pub(super) struct Fake {
        pub dir: PathBuf,
        pub clis: Clis,
    }

    impl Drop for Fake {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    const CLAUDE: &str = r#"#!/bin/sh
case "$1 $2" in
"mcp list")
  if [ -f "$STATE/claude-in" ]; then s="✔ Connected"; else s="! Needs authentication"; fi
  echo "Checking MCP server health…"; echo
  echo "plugin:p:srv: http://127.0.0.1:1/mcp (HTTP) - $s"
  echo "other: /bin/cat - ✔ Connected" ;;
"mcp login")
  [ -t 0 ] || { echo "stdin is not a terminal" >&2; exit 1; }
  [ "$3" = "plugin:p:srv" ] || { echo "No MCP server named \"$3\"" >&2; exit 1; }
  echo "Visit this URL to authorize:"
  echo "  http://127.0.0.1:1/authorize?response_type=code&state=claude"
  while [ ! -f "$STATE/go-claude" ]; do sleep 0.05; done
  touch "$STATE/claude-in"; echo "Authenticated with \"$3\"." ;;
esac
"#;

    const CODEX: &str = r#"#!/bin/sh
case "$1 $2" in
"mcp list")
  if [ -f "$STATE/codex-in" ]; then s=o_auth; else s=not_logged_in; fi
  echo "[{\"name\":\"srv\",\"auth_status\":\"$s\"}]" ;;
"mcp login")
  echo "Authorize the MCP server by opening this URL in your browser:"
  echo "http://127.0.0.1:1/authorize?code_challenge=x&state=codex"
  while [ ! -f "$STATE/go-codex" ]; do sleep 0.05; done
  touch "$STATE/codex-in"; echo "Successfully logged in to MCP server '$3'." ;;
esac
"#;

    pub(super) fn fake(name: &str) -> Fake {
        let dir = std::env::temp_dir().join(format!(
            "brigadier-mcp-login-{name}-{}",
            uuid::Uuid::new_v4().simple()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        for (file, text) in [("claude", CLAUDE), ("codex", CODEX)] {
            let path = dir.join(file);
            std::fs::write(&path, text).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let bin = dir.clone();
        Fake {
            clis: Clis {
                env: vec![
                    ("PATH".into(), "/bin:/usr/bin".into()),
                    ("STATE".into(), dir.display().to_string()),
                ],
                binary: Arc::new(move |agent| {
                    Some(bin.join(match agent {
                        Agent::Claude => "claude",
                        Agent::Codex => "codex",
                    }))
                }),
            },
            dir,
        }
    }

    pub(super) fn wait(
        logins: &McpLogins,
        id: &str,
        done: impl Fn(&McpSignInProgress) -> bool,
    ) -> McpSignInProgress {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            let progress = logins.progress(id).unwrap();
            if done(&progress) {
                return progress;
            }
            assert!(Instant::now() < deadline, "still {progress:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

#[cfg(unix)]
#[test]
fn one_sign_in_runs_each_agents_own_login_in_turn() {
    let fake = fake::fake("turn");
    let logins = Arc::new(McpLogins::default());
    let opened = Arc::new(Mutex::new(Vec::<String>::new()));
    let (seen, state) = (opened.clone(), fake.dir.clone());
    // The "browser": approves each page as it opens.
    let open: Open = Arc::new(move |link: &str| {
        seen.lock().unwrap().push(link.to_owned());
        let agent = link.rsplit("state=").next().unwrap();
        std::fs::write(state.join(format!("go-{agent}")), "").unwrap();
    });
    let target = Target {
        server: "srv".into(),
        plugin: Some("p".into()),
        project: None,
    };
    let id = logins.start(fake.clis.clone(), target.clone(), open.clone());
    let progress = fake::wait(&logins, &id, |p| p.finished);
    assert_eq!(progress.status(), McpSignInStatus::SignedIn, "{progress:?}");
    assert_eq!(
        progress
            .agents
            .iter()
            .map(|a| (a.agent, a.server.as_str(), a.state.clone()))
            .collect::<Vec<_>>(),
        [
            (
                ProviderKind::Claude,
                "plugin:p:srv",
                AgentSignInState::SignedIn
            ),
            (ProviderKind::Codex, "srv", AgentSignInState::SignedIn),
        ]
    );
    // Claude first, then Codex.
    let opened = opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 2);
    assert!(opened[0].ends_with("state=claude") && opened[1].ends_with("state=codex"));
    // The lists read afterwards say neither needs it.
    for agent in [Agent::Claude, Agent::Codex] {
        let listed = logins.listed(&fake.clis, agent, None).unwrap();
        assert!(
            listed.iter().all(|entry| !entry.needs_sign_in),
            "{listed:?}"
        );
    }
    // Again: nobody needs it.
    let id = logins.start(fake.clis.clone(), target, open);
    let progress = fake::wait(&logins, &id, |p| p.finished);
    assert!(
        progress
            .agents
            .iter()
            .all(|a| a.state == AgentSignInState::AlreadySignedIn)
    );
}

#[cfg(unix)]
#[test]
fn a_cancelled_sign_in_stops_its_cli_and_skips_the_rest() {
    let fake = fake::fake("cancel");
    let logins = Arc::new(McpLogins::default());
    let open: Open = Arc::new(|_: &str| {});
    let target = Target {
        server: "srv".into(),
        plugin: Some("p".into()),
        project: None,
    };
    let id = logins.start(fake.clis.clone(), target.clone(), open.clone());
    fake::wait(&logins, &id, |p| {
        p.agents.first().map(|a| &a.state) == Some(&AgentSignInState::InBrowser)
    });
    // Asking again while it waits gives the same sign-in, its page opened again.
    assert_eq!(logins.start(fake.clis.clone(), target.clone(), open), id);
    assert_eq!(logins.pending(&target).as_deref(), Some(id.as_str()));
    logins.reopen(&id).unwrap();
    logins.cancel(&id).unwrap();
    let progress = fake::wait(&logins, &id, |p| p.finished);
    assert_eq!(progress.status(), McpSignInStatus::Cancelled);
    assert!(
        progress
            .agents
            .iter()
            .all(|a| a.state == AgentSignInState::Cancelled),
        "{progress:?}"
    );
    assert!(!fake.dir.join("claude-in").exists());
    assert_eq!(logins.pending(&target), None);
    assert!(logins.reopen(&id).is_err());
}

#[cfg(unix)]
#[test]
fn a_login_that_fails_says_why_in_the_clis_words() {
    let fake = fake::fake("fail");
    let logins = Arc::new(McpLogins::default());
    // Not a plugin's: Claude has no `srv` of its own, so only Codex lists it.
    let target = Target {
        server: "srv".into(),
        plugin: None,
        project: None,
    };
    std::fs::write(fake.dir.join("codex"), "#!/bin/sh\ncase \"$1 $2\" in\n\"mcp list\") echo '[{\"name\":\"srv\",\"auth_status\":\"not_logged_in\"}]' ;;\n*) echo 'Error: the server refused the client' >&2; exit 1 ;;\nesac\n").unwrap();
    let id = logins.start(fake.clis.clone(), target, Arc::new(|_: &str| {}));
    let progress = fake::wait(&logins, &id, |p| p.finished);
    assert_eq!(
        progress.status(),
        McpSignInStatus::Failed {
            reason: "the server refused the client".into()
        },
        "{progress:?}"
    );
    assert_eq!(progress.agents.len(), 1);
}

/// A server only a project configures: each CLI lists, signs in to and signs out of it only
/// when run in that project's folder.
#[cfg(unix)]
#[test]
fn a_project_servers_sign_in_and_out_run_in_that_project() {
    let fake = fake::fake("project");
    let project = fake.dir.join("project");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join(".here"), "").unwrap();
    // Claude needs a terminal for its login; neither knows `srv` outside the project.
    let script = |listed: &str, agent: &str| {
        format!(
            "#!/bin/sh
[ -f .here ] || {{ echo 'No MCP server named srv' >&2; exit 1; }}
             case \"$1 $2\" in
\"mcp list\") if [ -f \"$STATE/{agent}-in\" ]; then echo '{}'; else echo '{listed}'; fi ;;
             \"mcp login\") echo 'http://127.0.0.1:1/authorize?response_type=code'; touch \"$STATE/{agent}-in\" ;;
             \"mcp logout\") rm -f \"$STATE/{agent}-in\"; touch \"$STATE/{agent}-out\" ;;
esac
",
            listed.replace("Needs authentication", "Connected").replace("not_logged_in", "o_auth"),
        )
    };
    std::fs::write(
        fake.dir.join("claude"),
        script(
            "srv: http://127.0.0.1:1/mcp (HTTP) - ! Needs authentication",
            "claude",
        ),
    )
    .unwrap();
    std::fs::write(
        fake.dir.join("codex"),
        script(r#"[{"name":"srv","auth_status":"not_logged_in"}]"#, "codex"),
    )
    .unwrap();
    let logins = Arc::new(McpLogins::default());
    let target = Target {
        server: "srv".into(),
        plugin: None,
        project: Some(project.clone()),
    };
    let id = logins.start(fake.clis.clone(), target.clone(), Arc::new(|_: &str| {}));
    let progress = fake::wait(&logins, &id, |p| p.finished);
    assert_eq!(progress.status(), McpSignInStatus::SignedIn, "{progress:?}");
    assert_eq!(progress.agents.len(), 2, "{progress:?}");
    assert!(fake.dir.join("claude-in").exists() && fake.dir.join("codex-in").exists());

    logins.sign_out(&fake.clis, &target).unwrap();
    assert!(fake.dir.join("claude-out").exists() && fake.dir.join("codex-out").exists());
}

/// The real CLIs, signed in to a loopback OAuth server from one call, then used signed in.
/// Run with `--ignored`. Claude keeps its sign-in in the login Keychain under the throwaway
/// config folder's own item, which the test deletes after.
#[cfg(target_os = "macos")]
mod live {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{TcpListener, TcpStream};

    /// A loopback OAuth server and MCP endpoint: 401 with its resource metadata until a
    /// bearer token it issued comes, auto-approving `/authorize`. Logs `METHOD path auth`.
    struct OAuthServer {
        base: String,
        log: Arc<Mutex<Vec<String>>>,
    }

    const TOKEN: &str = "w05c-live-token";

    fn respond(stream: &mut TcpStream, status: &str, headers: &[(&str, String)], body: &str) {
        let mut text = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            text.push_str(&format!("{name}: {value}\r\n"));
        }
        if !body.is_empty() {
            text.push_str("Content-Type: application/json\r\n");
        }
        text.push_str("\r\n");
        text.push_str(body);
        let _ = stream.write_all(text.as_bytes());
    }

    fn query(path: &str, key: &str) -> Option<String> {
        let query = path.split_once('?')?.1;
        query.split('&').find_map(|pair| {
            let (name, value) = pair.split_once('=')?;
            (name == key).then(|| value.to_owned())
        })
    }

    fn decode(text: &str) -> String {
        let mut out = Vec::new();
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'%' && i + 2 < bytes.len() {
                out.push(u8::from_str_radix(&text[i + 1..i + 3], 16).unwrap());
                i += 3;
            } else {
                out.push(bytes[i]);
                i += 1;
            }
        }
        String::from_utf8(out).unwrap()
    }

    fn handle(mut stream: TcpStream, base: &str, log: &Mutex<Vec<String>>) {
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() {
            return;
        }
        let mut parts = line.split_whitespace();
        let (method, path) = (
            parts.next().unwrap_or("").to_owned(),
            parts.next().unwrap_or("").to_owned(),
        );
        let (mut length, mut auth) = (0usize, None);
        loop {
            let mut header = String::new();
            if reader.read_line(&mut header).is_err() || header.trim().is_empty() {
                break;
            }
            let (name, value) = header.split_once(':').unwrap_or((&header, ""));
            match name.to_ascii_lowercase().as_str() {
                "content-length" => length = value.trim().parse().unwrap_or(0),
                "authorization" => auth = Some(value.trim().to_owned()),
                _ => {}
            }
        }
        let mut body = vec![0; length];
        let _ = std::io::Read::read_exact(&mut reader, &mut body);
        let body = String::from_utf8_lossy(&body).into_owned();
        let signed = auth.as_deref() == Some(&format!("Bearer {TOKEN}"));
        let rpc: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        log.lock().unwrap().push(format!(
            "{method} {} signed={signed} {}",
            path.split('?').next().unwrap(),
            rpc.get("method").and_then(|m| m.as_str()).unwrap_or("")
        ));
        let route = path.split('?').next().unwrap_or("");
        match (method.as_str(), route) {
            ("GET", p) if p.starts_with("/.well-known/oauth-protected-resource") => respond(
                &mut stream,
                "200 OK",
                &[],
                &serde_json::json!({"resource": format!("{base}/mcp"), "authorization_servers": [base], "scopes_supported": ["probe"]}).to_string(),
            ),
            ("GET", p) if p.starts_with("/.well-known/oauth-authorization-server") => respond(
                &mut stream,
                "200 OK",
                &[],
                &serde_json::json!({
                    "issuer": base,
                    "authorization_endpoint": format!("{base}/authorize"),
                    "token_endpoint": format!("{base}/token"),
                    "registration_endpoint": format!("{base}/register"),
                    "response_types_supported": ["code"],
                    "grant_types_supported": ["authorization_code", "refresh_token"],
                    "code_challenge_methods_supported": ["S256"],
                    "token_endpoint_auth_methods_supported": ["none"],
                    "scopes_supported": ["probe"],
                })
                .to_string(),
            ),
            ("POST", "/register") => {
                let mut client: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
                client["client_id"] = "w05c-live-client".into();
                client["token_endpoint_auth_method"] = "none".into();
                respond(&mut stream, "201 Created", &[], &client.to_string());
            }
            ("GET", "/authorize") => {
                let redirect = decode(&query(&path, "redirect_uri").unwrap_or_default());
                let state = query(&path, "state").unwrap_or_default();
                respond(
                    &mut stream,
                    "302 Found",
                    &[("Location", format!("{redirect}?code=live-code&state={state}"))],
                    "",
                );
            }
            ("POST", "/token") => respond(
                &mut stream,
                "200 OK",
                &[],
                &serde_json::json!({"access_token": TOKEN, "token_type": "Bearer", "expires_in": 3600, "refresh_token": "w05c-refresh", "scope": "probe"}).to_string(),
            ),
            ("POST", "/mcp") if !signed => respond(
                &mut stream,
                "401 Unauthorized",
                &[(
                    "WWW-Authenticate",
                    format!("Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource/mcp\""),
                )],
                "{}",
            ),
            ("POST", "/mcp") => {
                let Some(id) = rpc.get("id").cloned() else {
                    return respond(&mut stream, "202 Accepted", &[], "");
                };
                let result = match rpc["method"].as_str() {
                    Some("initialize") => serde_json::json!({
                        "protocolVersion": rpc["params"]["protocolVersion"],
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "w05c-live", "version": "0.1.0"}
                    }),
                    Some("tools/list") => serde_json::json!({"tools": [{
                        "name": "signed_probe", "description": "signed in",
                        "inputSchema": {"type": "object", "properties": {}}
                    }]}),
                    _ => serde_json::json!({}),
                };
                respond(
                    &mut stream,
                    "200 OK",
                    &[],
                    &serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string(),
                );
            }
            ("GET", "/mcp") => respond(&mut stream, "405 Method Not Allowed", &[], ""),
            _ => respond(&mut stream, "404 Not Found", &[], "{}"),
        }
    }

    impl OAuthServer {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
            let log = Arc::new(Mutex::new(Vec::new()));
            let (url, seen) = (base.clone(), log.clone());
            std::thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    let (url, seen) = (url.clone(), seen.clone());
                    std::thread::spawn(move || handle(stream, &url, &seen));
                }
            });
            Self { base, log }
        }

        fn signed(&self, method: &str) -> usize {
            self.log
                .lock()
                .unwrap()
                .iter()
                .filter(|line| line.contains("signed=true") && line.ends_with(method))
                .count()
        }
    }

    fn run(program: &str, args: &[&str], env: &[(String, String)], cwd: &Path) -> String {
        let output = std::process::Command::new(program)
            .args(args)
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd)
            .output()
            .unwrap();
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        println!("$ {program} {}\n{text}", args.join(" "));
        assert!(output.status.success(), "{program} {args:?}");
        text
    }

    /// Codex's own thread start, then its MCP inventory (which lists each server's tools).
    fn codex_tools(env: &[(String, String)], cwd: &Path) -> serde_json::Value {
        use std::process::Stdio;
        let mut child = std::process::Command::new("codex")
            .arg("app-server")
            .env_clear()
            .envs(env.iter().map(|(k, v)| (k, v)))
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut call = |id: u64, method: &str, params: serde_json::Value| {
            let request =
                serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
            writeln!(stdin, "{request}").unwrap();
            loop {
                let line = lines.next().unwrap().unwrap();
                let message: serde_json::Value = serde_json::from_str(&line).unwrap();
                if message["id"] == id {
                    return message["result"].clone();
                }
            }
        };
        call(
            1,
            "initialize",
            serde_json::json!({"clientInfo": {"name": "w05c", "version": "0.1.0"}, "capabilities": {"experimentalApi": true}}),
        );
        let thread = call(
            2,
            "thread/start",
            serde_json::json!({"cwd": cwd, "ephemeral": true, "approvalPolicy": "never", "sandbox": "read-only"}),
        );
        let status = call(
            3,
            "mcpServerStatus/list",
            serde_json::json!({"threadId": thread["thread"]["id"]}),
        );
        let _ = child.kill();
        let _ = child.wait();
        status
    }

    #[test]
    #[ignore = "runs the real claude and codex against a loopback OAuth server"]
    fn live_one_click_signs_both_clis_in_and_their_tools_list_signed() {
        let server = OAuthServer::start();
        let root = PathBuf::from(format!(
            "/tmp/20261024-1114-plugins-redesign-w05-w05c-signin-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let (claude_dir, codex_home, project) = (
            root.join("claude"),
            root.join("codex"),
            root.join("project"),
        );
        for dir in [&claude_dir, &codex_home, &project] {
            std::fs::create_dir_all(dir).unwrap();
        }
        // Codex keeps its sign-ins in a file in its home, not the Keychain. Its server is
        // written here: `codex mcp add --url` starts a sign-in in the real browser at once.
        std::fs::write(
            codex_home.join("config.toml"),
            format!(
                "mcp_oauth_credentials_store = \"file\"\n\n[mcp_servers.oauthy]\nurl = \"{}/mcp\"\n",
                server.base
            ),
        )
        .unwrap();
        // Claude keeps its own in the login Keychain, which it finds through the real HOME.
        let mut env: Vec<(String, String)> = ["PATH", "HOME", "USER", "LOGNAME", "TMPDIR"]
            .iter()
            .filter_map(|name| Some(((*name).to_owned(), std::env::var(name).ok()?)))
            .collect();
        env.extend([
            (
                "CLAUDE_CONFIG_DIR".to_owned(),
                claude_dir.display().to_string(),
            ),
            ("CODEX_HOME".to_owned(), codex_home.display().to_string()),
            ("TERM".to_owned(), "dumb".to_owned()),
            ("NO_COLOR".to_owned(), "1".to_owned()),
            ("DISABLE_AUTOUPDATER".to_owned(), "1".to_owned()),
        ]);
        // Claude's Keychain item for the throwaway folder, and the folder, however it ends.
        struct Cleanup(PathBuf, String);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let deleted = std::process::Command::new("/usr/bin/security")
                    .args(["delete-generic-password", "-s", &self.1])
                    .output()
                    .map(|out| out.status.success());
                println!("deleted Keychain item {}: {deleted:?}", self.1);
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let cli_env = brigadier_providers::cli::CliEnv::from_vars(
            env.iter().map(|(k, v)| (k.into(), v.into())),
        );
        let _cleanup = Cleanup(
            root.clone(),
            brigadier_providers::login_mark::claude_keychain_service(&cli_env, &claude_dir)
                .unwrap(),
        );
        let url = format!("{}/mcp", server.base);
        run(
            "claude",
            &[
                "mcp",
                "add",
                "--transport",
                "http",
                "oauthy",
                &url,
                "--scope",
                "user",
            ],
            &env,
            &project,
        );

        let logins = Arc::new(McpLogins::default());
        let which = |name: &str| {
            let path = std::env::var("PATH").unwrap();
            std::env::split_paths(&path)
                .map(|dir| dir.join(name))
                .find(|p| p.is_file())
        };
        let (claude, codex) = (which("claude"), which("codex"));
        let clis = Clis {
            env: env.clone(),
            binary: Arc::new(move |agent| match agent {
                Agent::Claude => claude.clone(),
                Agent::Codex => codex.clone(),
            }),
        };
        // The "browser": follows the page to the CLI's own loopback callback.
        let open: Open = Arc::new(|link: &str| {
            println!("opening {link}");
            let link = link.to_owned();
            std::thread::spawn(move || {
                let status = std::process::Command::new("/usr/bin/curl")
                    .args(["-s", "-L", "-o", "/dev/null", &link])
                    .status();
                println!("browser: {status:?}");
            });
        });
        let target = Target {
            server: "oauthy".into(),
            plugin: None,
            project: Some(project.clone()),
        };
        let id = logins.start(clis.clone(), target, open);
        let deadline = Instant::now() + Duration::from_secs(120);
        let progress = loop {
            let progress = logins.progress(&id).unwrap();
            if progress.finished || Instant::now() > deadline {
                break progress;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        println!("progress: {progress:#?}");
        assert_eq!(progress.status(), McpSignInStatus::SignedIn);
        assert_eq!(progress.agents.len(), 2);
        assert!(
            progress
                .agents
                .iter()
                .all(|a| a.state == AgentSignInState::SignedIn)
        );

        // Signed in, each CLI's own way of using it lists the server's tools.
        let before = server.signed("tools/list");
        let listed = run("claude", &["mcp", "list"], &env, &project);
        assert!(
            listed.contains("oauthy:") && listed.contains("Connected"),
            "{listed}"
        );
        let status = codex_tools(&env, &project);
        println!("codex mcpServerStatus/list: {status}");
        let oauthy = status["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|server| server["name"] == "oauthy")
            .unwrap();
        assert_eq!(oauthy["authStatus"], "oAuth");
        assert!(oauthy["tools"].get("signed_probe").is_some(), "{oauthy}");
        assert!(
            server.signed("tools/list") >= before + 2,
            "{:#?}",
            server.log.lock().unwrap()
        );
        for agent in [Agent::Claude, Agent::Codex] {
            let listed = logins.listed(&clis, agent, Some(&project)).unwrap();
            assert!(
                listed.iter().all(|entry| !entry.needs_sign_in),
                "{listed:?}"
            );
        }
        println!("server log:\n{}", server.log.lock().unwrap().join("\n"));
    }
}
