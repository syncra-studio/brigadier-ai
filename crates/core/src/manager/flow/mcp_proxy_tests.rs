//! `brigadier mcp` (D5) as a session's thread and a worker call it: a worker started with no
//! `--mcp` grant still lists, reads and calls the session's server; each caller keeps its own
//! connection, so its state carries from one call (and turn) to the next and never crosses to
//! another caller's; a worker's connection closes once the worker has ended.

use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{CliRequest, Tool};
use serde_json::{Value, json};

use super::cli_tests::{brigadier, tool};
use super::context_tests::write;
use super::{Flow, Options, Reply, Script, eventually_async};
use crate::manager::cli_mcp::tests::FAKE_SERVER;

fn mcp(action: &str, args: Value) -> CliRequest {
    tool(Tool::Mcp, action, args)
}

fn call(name: &str, arguments: Value) -> CliRequest {
    mcp(
        "call",
        json!({"server": "fake", "tool": name, "arguments": arguments}),
    )
}

fn text(output: &Value) -> String {
    assert!(output.get("error").is_none(), "{output:#}");
    output["text"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn an_ungranted_worker_calls_the_sessions_server_and_keeps_its_own_state() {
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let script: Script = {
        let seen = seen.clone();
        Arc::new(move |turn| {
            let seen = seen.clone();
            Box::pin(async move {
                let keep = |name: &str, value: Value| {
                    seen.lock().unwrap().push((name.to_owned(), value));
                };
                if turn.worker().is_some() {
                    keep(
                        "worker list",
                        brigadier(&turn, mcp("list", json!({}))).await,
                    );
                    keep(
                        "worker tools",
                        brigadier(&turn, mcp("tools", json!({"server": "fake"}))).await,
                    );
                    keep(
                        "worker remember",
                        brigadier(&turn, call("remember", json!({"value": "worker"}))).await,
                    );
                    keep(
                        "worker recall",
                        brigadier(&turn, call("recall", json!({}))).await,
                    );
                    keep("worker pid", brigadier(&turn, call("pid", json!({}))).await);
                    keep(
                        "worker fail",
                        brigadier(&turn, call("fail", json!({}))).await,
                    );
                    keep(
                        "worker missing",
                        brigadier(&turn, mcp("tools", json!({"server": "not-in-the-setup"}))).await,
                    );
                    return Reply::text("Called.");
                }
                if turn.input.contains("again") {
                    keep(
                        "thread recall",
                        brigadier(&turn, call("recall", json!({}))).await,
                    );
                    return Reply::text("Recalled.");
                }
                keep(
                    "thread list",
                    brigadier(&turn, mcp("list", json!({}))).await,
                );
                keep(
                    "thread remember",
                    brigadier(&turn, call("remember", json!({"value": "thread"}))).await,
                );
                let task = turn.start_worker("Call", "Call the fake server.").await;
                keep("task", json!(task.0));
                Reply::text("Started.")
            })
        })
    };
    let flow = Flow::start("mcp-proxy", Options::default(), script).await;
    let server = flow.scratch().join("fake.py");
    std::fs::write(&server, FAKE_SERVER).unwrap();
    write(&flow.repo.join(".git/info/exclude"), ".mcp.json\n");
    write(
        &flow.repo.join(".mcp.json"),
        &json!({"mcpServers": {"fake": {"command": "python3", "args": [server]}}}).to_string(),
    );
    flow.manager.registry().refresh();
    flow.say("Use the fake server.").await;
    flow.settled().await;
    let got = |name: &str| {
        seen.lock()
            .unwrap()
            .iter()
            .find(|(seen, _)| seen == name)
            .map(|(_, value)| value.clone())
    };
    flow.until("the worker called", |_| got("worker missing").is_some())
        .await;
    flow.say("again").await;
    flow.settled().await;

    assert!(text(&got("thread list").unwrap()).contains("- fake (stdio), loaded at your launch"));
    let list = text(&got("worker list").unwrap());
    assert!(
        list.contains("- fake (stdio)\n"),
        "a worker's launch loads no server: {list}"
    );
    let tools = text(&got("worker tools").unwrap());
    assert!(
        tools.contains("- remember: Keeps a value.") && tools.contains("- ask"),
        "{tools}"
    );
    assert_eq!(text(&got("worker remember").unwrap()), "kept\n");
    // Each caller's own connection: the worker reads its own value, the thread its own, a turn
    // later.
    assert!(text(&got("worker recall").unwrap()).starts_with("worker\n"));
    assert!(text(&got("thread recall").unwrap()).starts_with("thread\n"));
    let failed = got("worker fail").unwrap();
    assert!(
        failed["error"]["message"]
            .as_str()
            .unwrap()
            .contains("it broke"),
        "{failed:#}"
    );
    assert!(
        got("worker missing").unwrap()["error"]["message"]
            .as_str()
            .unwrap()
            .contains("`brigadier mcp list`")
    );

    // The worker's connection goes once it has ended; the thread's stays.
    let pid = text(&got("worker pid").unwrap()).trim().to_owned();
    let task = crate::work::TaskId(got("task").unwrap().as_str().unwrap().to_owned());
    flow.manager.stop_task(task).await.unwrap();
    eventually_async("the worker's connection closed", || async {
        flow.manager.sweep_mcp().await == 1
    })
    .await;
    let alive = std::process::Command::new("kill")
        .args(["-0", &pid])
        .status()
        .is_ok_and(|status| status.success());
    assert!(!alive, "the worker's server outlived it");
    flow.stop().await;
}
