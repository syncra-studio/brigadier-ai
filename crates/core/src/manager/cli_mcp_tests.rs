//! `brigadier mcp`'s client on its own: a stdio server, a streamable-HTTP one and a gateway
//! server, each kept across calls.

use super::*;
use axum::{
    Router,
    body::Body,
    extract::State,
    http::{Request, Response},
    routing::any,
};

/// A stdio MCP server: two pages of tools; `remember` and `recall` share the process's state;
/// `pid` names it; `fail` answers an error result, `boom` a protocol error; `picture` an image;
/// `ask` asks Brigadier a question of its own first; `spawn` starts a process of its own. It logs a line on stdout as it starts.
pub(in crate::manager) const FAKE_SERVER: &str = r#"
import json, os, subprocess, sys
kept = None
pending = None
TOOLS = [
    {"name": "remember", "description": "Keeps a value.", "inputSchema": {"type": "object", "properties": {"value": {"type": "string"}}}},
    {"name": "recall", "inputSchema": {"type": "object"}, "outputSchema": {"type": "object"}},
    {"name": "pid", "inputSchema": {"type": "object"}},
    {"name": "fail", "inputSchema": {"type": "object"}},
    {"name": "boom", "inputSchema": {"type": "object"}},
    {"name": "picture", "inputSchema": {"type": "object"}},
    {"name": "ask", "inputSchema": {"type": "object"}},
]
def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()
def text(i, words, error=False):
    send({"jsonrpc": "2.0", "id": i, "result": {"content": [{"type": "text", "text": words}], "isError": error}})
print("starting up", flush=True)
for line in sys.stdin:
    m = json.loads(line)
    if "id" not in m:
        continue
    i, method, p = m["id"], m.get("method"), m.get("params") or {}
    if method is None:
        if i == "s1" and pending is not None:
            text(pending, json.dumps(m.get("error")))
            pending = None
    elif method == "initialize":
        send({"jsonrpc": "2.0", "id": i, "result": {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}}, "serverInfo": {"name": "fake", "version": "1"}}})
    elif method == "tools/list":
        two = p.get("cursor") == "two"
        result = {"tools": TOOLS[3:] if two else TOOLS[:3]}
        if not two:
            result["nextCursor"] = "two"
        send({"jsonrpc": "2.0", "method": "notifications/message", "params": {"level": "info", "data": "listing"}})
        send({"jsonrpc": "2.0", "id": i, "result": result})
    elif method == "tools/call":
        name, args = p["name"], p.get("arguments") or {}
        if name == "remember":
            kept = args["value"]
            text(i, "kept")
        elif name == "recall":
            send({"jsonrpc": "2.0", "id": i, "result": {"content": [{"type": "text", "text": str(kept)}], "structuredContent": {"value": kept}}})
        elif name == "pid":
            text(i, str(os.getpid()))
        elif name == "spawn":
            text(i, str(subprocess.Popen(["sleep", "300"]).pid))
        elif name == "fail":
            text(i, "it broke", True)
        elif name == "boom":
            send({"jsonrpc": "2.0", "id": i, "error": {"code": -32000, "message": "kaboom", "data": {"why": "test"}}})
        elif name == "picture":
            send({"jsonrpc": "2.0", "id": i, "result": {"content": [{"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"}]}})
        elif name == "ask":
            pending = i
            send({"jsonrpc": "2.0", "id": "s1", "method": "roots/list"})
    else:
        send({"jsonrpc": "2.0", "id": i, "error": {"code": -32601, "message": "no " + str(method)}})
"#;

fn stdio(script: &Path) -> McpServer {
    McpServer {
        gateway: None,
        name: "fake".into(),
        transport: McpTransport::Stdio {
            command: "python3".into(),
            args: vec![script.display().to_string()],
            cwd: None,
        },
        env: Vec::new(),
        tool_timeout_secs: None,
        trusted: false,
        always_load: false,
    }
}

fn platform() -> Arc<dyn brigadier_sandbox::Platform> {
    brigadier_sandbox::native(brigadier_sandbox::PlatformOptions { data_dir: None }).unwrap()
}

/// Whether `pid` is still there within a few seconds (an ended process may wait to be reaped).
async fn outlives(pid: &str) -> bool {
    for _ in 0..50 {
        if !alive(pid) {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    true
}

fn here() -> Vec<(String, String)> {
    std::env::vars().collect()
}

async fn call(conn: &mut Conn, tool: &str, arguments: Value) -> Result<Value, Failure> {
    conn.request(
        "tools/call",
        json!({"name": tool, "arguments": arguments}),
        CALL,
    )
    .await
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(unix)]
#[tokio::test]
async fn a_stdio_server_pages_its_tools_keeps_its_state_and_says_its_errors() {
    let dir = super::super::flow::Scratch::new("mcp-stdio");
    let script = dir.join("fake.py");
    std::fs::write(&script, FAKE_SERVER).unwrap();
    let mut conn = Conn::open(stdio(&script), platform(), &dir, &here(), None)
        .await
        .unwrap_or_else(|err| panic!("{err}"));
    assert_eq!(conn.version.as_deref(), Some("2025-06-18"));

    let Ok(tools) = list_tools(&mut conn, "fake").await else {
        panic!("tools/list failed")
    };
    for tool in [
        "remember", "recall", "pid", "fail", "boom", "picture", "ask",
    ] {
        assert!(tools.contains(&format!("- {tool}")), "{tools}");
    }
    assert!(
        tools.contains("Keeps a value.") && tools.contains("output:"),
        "{tools}"
    );

    // One process across calls: what one call keeps, the next reads.
    assert!(
        call(&mut conn, "remember", json!({"value": "v1"}))
            .await
            .is_ok()
    );
    let Ok(recalled) = call(&mut conn, "recall", json!({})).await else {
        panic!("recall failed")
    };
    let (text, _) = result_text(&recalled);
    assert!(
        text.starts_with("v1\n") && text.contains("structuredContent:"),
        "{text}"
    );
    assert!(text.contains("\"value\": \"v1\""), "{text}");
    let pid = |result: Result<Value, Failure>| match result {
        Ok(result) => result["content"][0]["text"].as_str().unwrap().to_owned(),
        Err(failure) => panic!("{}", failure.message()),
    };
    let first = pid(call(&mut conn, "pid", json!({})).await);
    assert_eq!(first, pid(call(&mut conn, "pid", json!({})).await));

    let Ok(failed) = call(&mut conn, "fail", json!({})).await else {
        panic!("fail failed")
    };
    assert_eq!(failed["isError"], true);
    assert_eq!(result_text(&failed).0, "it broke\n");
    match call(&mut conn, "boom", json!({})).await {
        Err(Failure::Rpc(message)) => {
            assert!(
                message.contains("-32000") && message.contains("kaboom"),
                "{message}"
            );
            assert!(message.contains("\"why\":\"test\""), "{message}");
        }
        _ => panic!("boom answered no protocol error"),
    }
    let Ok(picture) = call(&mut conn, "picture", json!({})).await else {
        panic!("picture failed")
    };
    let (text, images) = result_text(&picture);
    assert_eq!(text, "[image image/png]\n");
    assert_eq!(images, ["data:image/png;base64,iVBORw0KGgo="]);
    // A question of the server's own is turned down, and its call still answers.
    let Ok(asked) = call(&mut conn, "ask", json!({})).await else {
        panic!("ask failed")
    };
    assert!(result_text(&asked).0.contains("-32601"), "{asked}");
    // Still the same process after every kind of answer.
    assert_eq!(first, pid(call(&mut conn, "pid", json!({})).await));

    let started = pid(call(&mut conn, "spawn", json!({})).await);
    assert!(alive(&first) && alive(&started));
    conn.close().await;
    assert!(
        !outlives(&first).await,
        "the server outlived its connection"
    );
    assert!(
        !outlives(&started).await,
        "what the server started outlived it"
    );

    // A connection dropped mid-use (its caller hung up) ends its server's tree too.
    let mut conn = Conn::open(stdio(&script), platform(), &dir, &here(), None)
        .await
        .unwrap_or_else(|err| panic!("{err}"));
    let started = pid(call(&mut conn, "spawn", json!({})).await);
    drop(conn);
    assert!(
        !outlives(&started).await,
        "a dropped connection left its server's process"
    );
}

#[derive(Default)]
struct Http {
    kept: Mutex<Option<String>>,
    seen: Mutex<Vec<(String, Option<String>)>>,
    ended_on: Mutex<Option<String>>,
}

/// A streamable-HTTP server that names a session at `initialize`, wants it and the protocol
/// version on every later request, and answers each over SSE after a notification.
async fn http_server(state: State<Arc<Http>>, request: Request<Body>) -> Response<Body> {
    let method = request.method().to_string();
    let session = request
        .headers()
        .get("mcp-session-id")
        .map(|value| value.to_str().unwrap().to_owned());
    let version = request.headers().get("mcp-protocol-version").cloned();
    let body = axum::body::to_bytes(request.into_body(), 1 << 20)
        .await
        .unwrap();
    state
        .seen
        .lock()
        .unwrap()
        .push((method.clone(), session.clone()));
    if method == "DELETE" {
        *state.ended_on.lock().unwrap() = version.map(|v| v.to_str().unwrap().to_owned());
        return Response::builder().status(204).body(Body::empty()).unwrap();
    }
    let message: Value = serde_json::from_slice(&body).unwrap();
    let Some(id) = message.get("id").cloned() else {
        return Response::builder().status(202).body(Body::empty()).unwrap();
    };
    if message["method"] == "initialize" {
        let answer = json!({"jsonrpc": "2.0", "id": id, "result": {"protocolVersion": "2025-11-25", "capabilities": {"tools": {}}}});
        return Response::builder()
            .header("mcp-session-id", "s-1")
            .header("content-type", "application/json")
            .body(Body::from(answer.to_string()))
            .unwrap();
    }
    if session.as_deref() != Some("s-1") || version.is_none() {
        return Response::builder().status(400).body(Body::empty()).unwrap();
    }
    let arguments = &message["params"]["arguments"];
    let result = match message["params"]["name"].as_str() {
        Some("remember") => {
            *state.kept.lock().unwrap() = arguments["value"].as_str().map(str::to_owned);
            json!({"content": [{"type": "text", "text": "kept"}]})
        }
        _ => {
            json!({"content": [{"type": "text", "text": state.kept.lock().unwrap().clone().unwrap_or_default()}]})
        }
    };
    let answer = json!({"jsonrpc": "2.0", "id": id, "result": result});
    let stream = format!(
        "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\"}}\n\n\
         event: message\ndata: {answer}\n\n"
    );
    Response::builder()
        .header("content-type", "text/event-stream")
        .body(Body::from(stream))
        .unwrap()
}

#[tokio::test]
async fn an_http_server_keeps_its_session_across_calls_and_is_ended_on_close() {
    let state = Arc::new(Http::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/mcp", any(http_server))
        .with_state(state.clone());
    let served = tokio::spawn(async move { axum::serve(listener, router).await });
    let server = McpServer {
        gateway: None,
        name: "web".into(),
        transport: McpTransport::Http { url },
        env: vec![("x-team".into(), "core".into())],
        tool_timeout_secs: None,
        trusted: false,
        always_load: false,
    };
    let mut conn = Conn::open(server, platform(), Path::new("/"), &[], None)
        .await
        .unwrap_or_else(|err| panic!("{err}"));
    assert!(
        call(&mut conn, "remember", json!({"value": "h1"}))
            .await
            .is_ok()
    );
    let Ok(recalled) = call(&mut conn, "recall", json!({})).await else {
        panic!("recall failed")
    };
    assert_eq!(result_text(&recalled).0, "h1\n");
    conn.close().await;
    let seen = state.seen.lock().unwrap().clone();
    assert_eq!(seen.first(), Some(&("POST".to_owned(), None)));
    assert!(
        seen[1..]
            .iter()
            .all(|(_, session)| session.as_deref() == Some("s-1")),
        "{seen:?}"
    );
    assert_eq!(
        seen.last().map(|(method, _)| method.as_str()),
        Some("DELETE")
    );
    assert_eq!(
        state.ended_on.lock().unwrap().as_deref(),
        Some("2025-11-25")
    );
    served.abort();
}

#[tokio::test]
async fn a_gateway_server_is_reached_through_its_own_lease_released_on_close() {
    use super::super::mcp_gateway::{
        McpGatewayService, SECRET_HEADER, store::MemoryStore, tests::Fixture,
    };
    let fixture = Fixture::new().await;
    let gateway = McpGatewayService::with_store(Arc::new(MemoryStore::default()), None).unwrap();
    fixture.signed_in(&gateway).await;
    let server = McpServer {
        gateway: Some(brigadier_providers::McpGateway {
            resource: fixture.resource(),
            row_id: "row".into(),
            url: String::new(),
            secret_header: (SECRET_HEADER.into(), String::new()),
        }),
        name: "remote".into(),
        transport: McpTransport::Http {
            url: fixture.resource(),
        },
        env: Vec::new(),
        tool_timeout_secs: None,
        trusted: false,
        always_load: false,
    };
    let leased = lease(&gateway, &server).expect("a gateway server is leased");
    let (url, secret) = match (&leased.1.transport, leased.1.env.first()) {
        (McpTransport::Http { url }, Some(secret)) => (url.clone(), secret.clone()),
        other => panic!("{other:?}"),
    };
    assert!(!secret.1.is_empty());
    let mut conn = Conn::open(server, platform(), Path::new("/"), &[], Some(leased))
        .await
        .unwrap_or_else(|err| panic!("{err}"));
    let Ok(tools) = list_tools(&mut conn, "remote").await else {
        panic!("tools/list failed")
    };
    assert!(tools.contains("- fixture-tool"), "{tools}");
    let post = || {
        reqwest::Client::new()
            .post(&url)
            .header(&secret.0, &secret.1)
            .header("accept", "application/json, text/event-stream")
            .json(&json!({"jsonrpc": "2.0", "id": 9, "method": "tools/list"}))
            .send()
    };
    assert!(post().await.unwrap().status().is_success());
    conn.close().await;
    assert_eq!(
        post().await.unwrap().status(),
        401,
        "the lease outlived its connection"
    );
}
