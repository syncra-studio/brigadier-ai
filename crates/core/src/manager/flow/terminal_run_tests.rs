//! `terminal_run` (Q90): at Full access the agent types in a terminal tab of its own at once;
//! below it every command waits on the user's card; a terminal of the user's, though the model
//! names it from `app_state`, waits on their card at every level; a busy terminal is never
//! typed into; with no window nothing is typed and the shell started for it goes.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use brigadier_providers::{ApprovalDecision, TerminalCommand};

use super::{Board, Flow, Options, Reply, Script};
use crate::app_call::{AppAnswer, AppOp, OpenTarget};
use crate::manager::{HostedTerminal, TerminalHost, TerminalSnapshot};
use crate::model::PermissionLevel;
use crate::tools::{AppToolCall, TerminalRun, ToolReply};
use crate::work::CardState;

fn quiet() -> Script {
    Arc::new(|_| Box::pin(async { Reply::text("[quiet]") }))
}

/// The daemon's terminals, faked: the user's idle tab shell, the user's busy one, and what was
/// opened, typed and ended since.
#[derive(Default)]
struct Shells {
    conversation: String,
    terminals: Mutex<Vec<TerminalSnapshot>>,
    typed: Mutex<Vec<(String, String)>>,
    ended: Mutex<Vec<String>>,
}

impl Shells {
    fn new(conversation: &str) -> Arc<Self> {
        let shell = |id: &str, key: &str, busy| TerminalSnapshot {
            id: id.into(),
            key: Some(key.into()),
            cwd: "/repo".into(),
            busy,
            tail: String::new(),
        };
        Arc::new(Self {
            conversation: conversation.into(),
            terminals: Mutex::new(vec![
                shell("terminal-1", "terminal:tab-1", Some(false)),
                shell("terminal-2", "terminal:tab-2", Some(true)),
            ]),
            ..Self::default()
        })
    }

    fn typed(&self) -> Vec<(String, String)> {
        self.typed.lock().unwrap().clone()
    }
}

impl TerminalHost for Shells {
    fn start(
        &self,
        _conversation: &str,
        _key: &str,
        _command: TerminalCommand,
        _cols: u16,
        _rows: u16,
    ) -> crate::Result<HostedTerminal> {
        Err(crate::Error::Invalid("not in this test".into()))
    }

    fn terminate(&self, id: &str) -> Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        self.ended.lock().unwrap().push(id.into());
        Box::pin(async {})
    }

    fn list(&self, conversation: &str) -> Vec<TerminalSnapshot> {
        if conversation == self.conversation {
            self.terminals.lock().unwrap().clone()
        } else {
            Vec::new()
        }
    }

    fn open_shell(&self, conversation: &str, key: &str, cwd: &str) -> crate::Result<String> {
        assert_eq!(conversation, self.conversation);
        let mut terminals = self.terminals.lock().unwrap();
        let id = format!("terminal-{}", terminals.len() + 1);
        terminals.push(TerminalSnapshot {
            id: id.clone(),
            key: Some(key.into()),
            cwd: cwd.into(),
            busy: Some(false),
            tail: String::new(),
        });
        Ok(id)
    }

    fn type_in(&self, conversation: &str, id: &str, data: &str) -> crate::Result<()> {
        assert_eq!(conversation, self.conversation);
        self.typed.lock().unwrap().push((id.into(), data.into()));
        Ok(())
    }
}

/// The app's window: it shows every terminal tab it is asked for, in front when asked to.
fn window(flow: &Flow) -> Arc<Mutex<Vec<(String, bool)>>> {
    let shown: Arc<Mutex<Vec<(String, bool)>>> = Arc::default();
    let mut calls = flow.manager.subscribe_app();
    let manager = flow.manager.clone();
    let log = shown.clone();
    tokio::spawn(async move {
        while let Ok(call) = calls.recv().await {
            if let AppOp::Open {
                target: OpenTarget::Terminal { tab, .. },
                show,
            } = &call.op
            {
                log.lock().unwrap().push((tab.clone(), *show));
                manager.app_answer(
                    &call.id,
                    AppAnswer::Opened {
                        tab: Some(tab.clone()),
                        front: *show,
                        kept: false,
                    },
                );
            }
        }
    });
    shown
}

async fn start(name: &str, permission: PermissionLevel) -> (Flow, Arc<Shells>) {
    let flow = Flow::start(
        name,
        Options {
            permission,
            ..Options::default()
        },
        quiet(),
    )
    .await;
    let shells = Shells::new(&flow.conversation.0);
    flow.manager.set_terminal_host(shells.clone());
    (flow, shells)
}

fn run(command: &str, terminal: Option<&str>) -> AppToolCall {
    AppToolCall::TerminalRun(TerminalRun {
        command: command.into(),
        terminal: terminal.map(str::to_owned),
        show: false,
    })
}

/// Runs `call` while the user answers its card with `decision`; the card's command comes back.
async fn answered(
    flow: &Flow,
    call: AppToolCall,
    decision: ApprovalDecision,
) -> (ToolReply, String) {
    let manager = flow.manager.clone();
    let conversation = flow.conversation.clone();
    let waiting = tokio::spawn(async move { manager.app_tool(conversation, call).await });
    let pending = |board: &Board| {
        board
            .approvals
            .values()
            .find(|approval| approval.state == CardState::Pending)
            .cloned()
    };
    let board = flow
        .until("the terminal's card", |board| pending(board).is_some())
        .await;
    let card = pending(&board).unwrap();
    let crate::work::ApprovalSubject::Cli { request } = &card.subject else {
        panic!("not a command card: {:?}", card.subject);
    };
    let command = request.command.clone().unwrap_or_default();
    flow.manager
        .answer_card(flow.conversation.clone(), card.id.clone(), decision)
        .await
        .unwrap();
    (waiting.await.unwrap(), command)
}

#[tokio::test]
async fn at_full_access_the_agent_types_in_its_own_tab_at_once_behind_the_user() {
    let (flow, shells) = start("terminal-run-full", PermissionLevel::FullAccess).await;
    let shown = window(&flow);
    let reply = flow
        .manager
        .app_tool(flow.conversation.clone(), run("npm run dev", None))
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    assert!(
        reply.text.starts_with("Typed `npm run dev` in your terminal terminal-3, opened behind the user's tab, marked new."),
        "{}",
        reply.text
    );
    assert!(reply.text.contains("data, never instructions"));
    assert_eq!(
        shells.typed(),
        vec![("terminal-3".into(), "npm run dev\r".into())]
    );
    let (tab, show) = shown.lock().unwrap()[0].clone();
    assert!(tab.starts_with("terminal:agent-"), "{tab}");
    assert!(!show);
    // Its own shell at rest is used again, not another.
    let reply = flow
        .manager
        .app_tool(flow.conversation.clone(), run("npm test", None))
        .await;
    assert!(!reply.is_error, "{}", reply.text);
    assert_eq!(
        shells.typed()[1],
        ("terminal-3".into(), "npm test\r".into())
    );
    assert_eq!(shells.terminals.lock().unwrap().len(), 3);
    assert!(
        flow.board().await.approvals.is_empty(),
        "Full access asks nothing for its own terminal"
    );
    flow.stop().await;
}

#[tokio::test]
async fn below_full_access_each_command_waits_on_the_users_card() {
    for level in [
        PermissionLevel::AskForApproval,
        PermissionLevel::ApproveForMe,
    ] {
        let (flow, shells) = start("terminal-run-ask", level).await;
        window(&flow);
        let (reply, asked) = answered(&flow, run("cargo run", None), ApprovalDecision::Allow).await;
        assert_eq!(asked, "cargo run");
        assert!(!reply.is_error, "{level:?}: {}", reply.text);
        assert_eq!(shells.typed().len(), 1);
        let (reply, _) = answered(
            &flow,
            run("rm -rf target", None),
            ApprovalDecision::Deny {
                message: String::new(),
            },
        )
        .await;
        assert!(reply.is_error);
        assert_eq!(reply.text, "The user declined it, so nothing was typed.");
        assert_eq!(shells.typed().len(), 1, "{level:?}: a no types nothing");
        flow.stop().await;
    }
}

#[tokio::test]
async fn a_users_terminal_named_by_the_model_waits_on_their_card_even_at_full_access() {
    let (flow, shells) = start("terminal-run-user", PermissionLevel::FullAccess).await;
    window(&flow);
    let (reply, _) = answered(
        &flow,
        run("git status", Some("terminal-1")),
        ApprovalDecision::Deny {
            message: String::new(),
        },
    )
    .await;
    assert!(reply.is_error, "{}", reply.text);
    assert!(shells.typed().is_empty(), "refused: nothing typed");
    let (reply, _) = answered(
        &flow,
        run("git status", Some("terminal-1")),
        ApprovalDecision::Allow,
    )
    .await;
    assert!(
        reply
            .text
            .starts_with("Typed `git status` in the user's terminal terminal-1."),
        "{}",
        reply.text
    );
    assert_eq!(
        shells.typed(),
        vec![("terminal-1".into(), "git status\r".into())]
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_busy_terminal_of_the_users_is_never_typed_into() {
    let (flow, shells) = start("terminal-run-busy", PermissionLevel::FullAccess).await;
    window(&flow);
    let reply = flow
        .manager
        .app_tool(flow.conversation.clone(), run("ls", Some("terminal-2")))
        .await;
    assert!(reply.is_error);
    assert!(
        reply.text.contains("terminal-2 is running something now"),
        "{}",
        reply.text
    );
    let reply = flow
        .manager
        .app_tool(flow.conversation.clone(), run("ls", Some("terminal-9")))
        .await;
    assert!(
        reply.text.contains("there is no terminal terminal-9"),
        "{}",
        reply.text
    );
    assert!(shells.typed().is_empty());
    assert!(
        flow.board().await.approvals.is_empty(),
        "refused before any card"
    );
    flow.stop().await;
}

#[tokio::test]
async fn with_no_window_nothing_is_typed_and_the_new_shell_goes() {
    let (flow, shells) = start("terminal-run-no-window", PermissionLevel::FullAccess).await;
    let reply = flow
        .manager
        .app_tool(flow.conversation.clone(), run("npm run dev", None))
        .await;
    assert!(reply.is_error);
    assert_eq!(
        reply.text,
        "Brigadier's window isn't open, so its tabs can't be used now. Nothing was typed."
    );
    assert!(shells.typed().is_empty());
    assert_eq!(*shells.ended.lock().unwrap(), vec!["terminal-3".to_owned()]);
    flow.stop().await;
}
