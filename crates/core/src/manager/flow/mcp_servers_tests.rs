//! The MCP servers each agent's CLI tells of are on its conversation's view while that CLI
//! runs: recorded whole, changed one at a time, gone once it ends, and never what an earlier
//! daemon process heard.

use std::sync::Arc;

use brigadier_providers::{McpServerState, McpStatus, ProviderEvent, ProviderKind};

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::{AgentMcpServers, DomainEvent};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn server(name: &str, status: McpStatus, tools: &[&str]) -> McpServerState {
    McpServerState {
        name: name.into(),
        status,
        tools: tools.iter().map(|tool| (*tool).to_owned()).collect(),
        transport: None,
    }
}

async fn shown(flow: &Flow) -> Vec<AgentMcpServers> {
    flow.core
        .conversation_view(flow.conversation.clone(), 10)
        .await
        .unwrap()
        .mcp_servers
}

#[tokio::test]
async fn the_lead_s_mcp_servers_show_while_its_cli_runs() {
    let flow = Flow::start(
        "mcp-servers",
        Options::default(),
        script(|turn| async move {
            if turn.input.contains("Crash.") {
                return Reply::crashed(9);
            }
            turn.emit(ProviderEvent::McpServers {
                servers: vec![
                    server("github", McpStatus::Connected, &["list_issues"]),
                    server("linear", McpStatus::Connecting, &[]),
                ],
                at_start: true,
            })
            .await;
            turn.emit(ProviderEvent::McpServerChanged {
                server: server("linear", McpStatus::NeedsAuth, &[]),
            })
            .await;
            // A change that names no tools keeps the ones known.
            turn.emit(ProviderEvent::McpServerChanged {
                server: server("github", McpStatus::Failed, &[]),
            })
            .await;
            Reply::text("Hi.")
        }),
    )
    .await;
    // What an earlier daemon process heard is not shown.
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![DomainEvent::McpServersUpdated {
                conversation_id: flow.conversation.clone(),
                task_id: Some("gone".into()),
                provider: ProviderKind::Codex,
                servers: vec![server("old", McpStatus::Connected, &[])],
                at_start: false,
                boot: "an-earlier-boot".into(),
            }],
        )
        .await
        .unwrap();

    flow.say("Hello.").await;
    flow.idle().await;
    assert_eq!(
        shown(&flow).await,
        [AgentMcpServers {
            task_id: None,
            provider: ProviderKind::Claude,
            servers: vec![
                server("github", McpStatus::Failed, &["list_issues"]),
                server("linear", McpStatus::NeedsAuth, &[]),
            ],
            at_start: true,
        }]
    );
    // Recorded on the conversation's stream (once per change), never in the CLI's log.
    let recorded = flow
        .events()
        .await
        .into_iter()
        .filter(|event| matches!(event, DomainEvent::McpServersUpdated { boot, .. } if boot == crate::boot_id()))
        .count();
    assert_eq!(recorded, 3);

    flow.say("Crash.").await;
    super::eventually_async("the crashed CLI's servers to go", || async {
        shown(&flow).await.is_empty()
    })
    .await;
    flow.stop().await;
}

/// S16c: Brigadier gives no session an MCP server of its own: a thread starts with nothing in
/// its MCP config from Brigadier, and with the user's own setup (where their servers are), and
/// it reaches Brigadier through `brigadier`.
#[tokio::test]
async fn a_session_starts_with_no_brigadier_mcp_server_and_the_users_setup() {
    let flow = Flow::start(
        "mcp-servers-none",
        Options::default(),
        script(|_| async move { Reply::text("Hi.") }),
    )
    .await;
    flow.say("Hello.").await;
    flow.settled().await;
    let specs = flow.specs();
    assert!(!specs.is_empty());
    for (_, spec) in &specs {
        assert!(spec.mcp_servers.is_empty(), "{:?}", spec.mcp_servers);
    }
    let (_, thread) = flow.thread_specs().into_iter().next().expect("the thread");
    assert!(thread.tools.user_setup(), "the user's own servers load");
    assert!(
        thread
            .env
            .iter()
            .any(|(key, _)| key == brigadier_cli::protocol::GRANT_ENV),
        "it calls Brigadier through `brigadier`"
    );
    flow.stop().await;
}
