//! A started session's or Chat's permission level changed from the composer: the new level
//! holds for what runs now, on both CLIs. Raised to Full access, nothing asks the user any more
//! (the thread's, the Chat's and an inheriting worker's requests); lowered to Ask, the next turn
//! asks again.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_cli::protocol::{Agent, CliRequest, Permission, StartRequest};
use brigadier_providers::model::{Access, Origin, ToolSet};
use brigadier_providers::{ApprovalDecision, Decider, ProviderKind};

use super::cli_tests::{brigadier, start};
use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::model::{
    ConversationId, ConversationKind, ModelChoice, PermissionLevel, Setup, SetupRequest,
};
use crate::work::{CardId, CardState, TaskState};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// What each request asked got, in order.
type Answers = Arc<Mutex<Vec<Option<ApprovalDecision>>>>;

/// The user picks `level` in the composer of the session or Chat `id`.
async fn set_level(flow: &Flow, id: &ConversationId, level: PermissionLevel) {
    let mut setup = flow.core.conversation(id).unwrap().setup.unwrap();
    match &mut setup {
        Setup::Session { permission, .. } | Setup::Chat { permission, .. } => *permission = level,
    }
    flow.manager.set_setup(id.clone(), setup).await.unwrap();
}

/// The one card of `id` that waits on the user.
async fn pending_card(flow: &Flow, id: &ConversationId) -> CardId {
    let board = flow
        .until_in(id, "a card waiting on the user", |board| {
            board
                .approvals
                .values()
                .any(|approval| approval.state == CardState::Pending)
        })
        .await;
    board
        .approvals
        .values()
        .find(|approval| approval.state == CardState::Pending)
        .unwrap()
        .id
        .clone()
}

fn allowed_by_policy(board: &Board, card: &CardId) -> bool {
    board.approvals[card].state
        == CardState::Allowed {
            by: Decider::Policy,
            similar: false,
        }
}

/// A Chat at `permission` on `provider`.
async fn new_chat(
    flow: &Flow,
    provider: ProviderKind,
    permission: PermissionLevel,
) -> ConversationId {
    flow.manager
        .create_conversation(
            ConversationKind::Chat,
            None,
            Some("Chat".into()),
            Some(SetupRequest::Chat {
                model: ModelChoice {
                    provider,
                    model: None,
                    effort: None,
                    fast: None,
                    account: None,
                },
                permission,
            }),
        )
        .await
        .unwrap()
        .id
}

/// The levels the Chat `chat`'s CLIs started at, in order.
fn chat_accesses(flow: &Flow, chat: &ConversationId) -> Vec<Access> {
    let work = flow.manager.session_dir(chat).work();
    flow.specs()
        .into_iter()
        .filter(|(_, spec)| spec.cwd == work)
        .map(|(_, spec)| spec.access)
        .collect()
}

/// A Chat started at Accept edits and raised to Full access, on both CLIs: the request its
/// CLI waits on goes, and what it asks after does too, with no card; its next turn starts at
/// Full access. Lowered to Ask, its next turn starts at Ask and asks the user again.
#[tokio::test]
async fn a_chat_raised_to_full_asks_nothing_and_lowered_to_ask_asks_again() {
    for provider in [ProviderKind::Claude, ProviderKind::Codex] {
        let answers: Answers = Arc::default();
        let log = answers.clone();
        let flow = Flow::start(
            "chat-level-switch",
            Options::default(),
            script(move |turn| {
                let log = log.clone();
                async move {
                    if turn.chat().is_none() {
                        return Reply::text("ok");
                    }
                    if turn.input.contains("Fetch both.") || turn.input.contains("Fetch again.") {
                        let first = turn.ask_approval("curl https://example.com", "curl").await;
                        log.lock().unwrap().push(first);
                    }
                    if turn.input.contains("Fetch both.") {
                        let second = turn.ask_approval("curl https://example.org", "curl").await;
                        log.lock().unwrap().push(second);
                    }
                    Reply::text("Done.")
                }
            }),
        )
        .await;
        let chat = new_chat(&flow, provider, PermissionLevel::AcceptEdits).await;
        flow.say_to(&chat, "Fetch both.").await;
        let card = pending_card(&flow, &chat).await;
        set_level(&flow, &chat, PermissionLevel::Full).await;
        let board = flow.settled_in(&chat).await;
        assert_eq!(
            *answers.lock().unwrap(),
            vec![Some(ApprovalDecision::Allow), Some(ApprovalDecision::Allow)],
            "{provider:?}"
        );
        assert!(allowed_by_policy(&board, &card), "{provider:?}: {board:#?}");
        assert_eq!(
            board.approvals.len(),
            1,
            "{provider:?}: the second asked no card"
        );

        flow.say_to(&chat, "Thanks.").await;
        flow.until_in(&chat, "the second turn", |board| {
            board.requests.len() == 2
                && board
                    .requests
                    .values()
                    .all(|request| request.state == crate::work::RequestState::Done)
        })
        .await;
        assert_eq!(
            chat_accesses(&flow, &chat).last(),
            Some(&Access::Full),
            "{provider:?}"
        );

        set_level(&flow, &chat, PermissionLevel::Ask).await;
        flow.say_to(&chat, "Fetch again.").await;
        let again = pending_card(&flow, &chat).await;
        assert_eq!(
            chat_accesses(&flow, &chat).last(),
            Some(&Access::Ask),
            "{provider:?}"
        );
        flow.manager
            .answer_card(chat.clone(), again, ApprovalDecision::Allow)
            .await
            .unwrap();
        flow.until_in(&chat, "the third turn", |board| {
            board.requests.len() == 3
                && board
                    .requests
                    .values()
                    .all(|request| request.state == crate::work::RequestState::Done)
        })
        .await;
        flow.stop().await;
    }
}

/// A session at Full access lowered to Ask, on both CLIs: its thread's next turn starts at Ask
/// and asks the user again.
#[tokio::test]
async fn a_session_lowered_from_full_to_ask_asks_again() {
    for thread in [ProviderKind::Claude, ProviderKind::Codex] {
        let flow = Flow::start(
            "session-lowered",
            Options {
                permission: PermissionLevel::Full,
                thread,
                ..Options::default()
            },
            script(move |turn| async move {
                if turn.input.contains("Fetch it.") {
                    turn.ask_approval("curl https://example.com", "curl").await;
                }
                Reply::text("Done.")
            }),
        )
        .await;
        flow.say("Hello.").await;
        flow.settled().await;
        set_level(&flow, &flow.conversation, PermissionLevel::Ask).await;
        flow.say("Fetch it.").await;
        let card = pending_card(&flow, &flow.conversation).await;
        let specs = flow.thread_specs();
        assert_eq!(specs.last().unwrap().1.access, Access::Ask, "{thread:?}");
        flow.manager
            .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
            .await
            .unwrap();
        flow.settled().await;
        flow.stop().await;
    }
}

/// A session's worker that asks before each fetch: started by the thread's "Start one." with
/// `agent` (and `permission`, when given); each answer it got goes to `answers`.
fn with_worker(agent: Agent, permission: Option<Permission>, answers: Answers) -> Script {
    script(move |turn| {
        let answers = answers.clone();
        async move {
            if turn.worker().is_some() {
                if turn.input.contains("Fetch both.") {
                    for url in ["https://example.com", "https://example.org"] {
                        let answer = turn.ask_approval(&format!("curl {url}"), "curl").await;
                        answers.lock().unwrap().push(answer);
                    }
                } else if turn.input.contains("Fetch it.") {
                    let answer = turn.ask_approval("curl https://example.com", "curl").await;
                    answers.lock().unwrap().push(answer);
                }
                return Reply::text("Done.");
            }
            if let Some(prompt) = turn.input.strip_prefix("Start one: ") {
                brigadier(
                    &turn,
                    CliRequest::Start(Box::new(StartRequest {
                        agent: Some(agent),
                        permission,
                        ..start(prompt)
                    })),
                )
                .await;
                return Reply::text("Started one.");
            }
            Reply::text("Noted.")
        }
    })
}

/// The sessions the workers' CLIs were started with, in order.
fn worker_specs(flow: &Flow) -> Vec<brigadier_providers::model::SessionSpec> {
    flow.specs()
        .into_iter()
        .filter(|(_, spec)| spec.tools == ToolSet::Lean)
        .map(|(_, spec)| spec)
        .collect()
}

/// A session at Accept edits raised to Full access while its worker (on either CLI, at the
/// session's level) waits on a card: that request goes, and what the worker asks after does
/// too, with no card. The user asked for Full access; a worker running at the level it
/// inherited follows it.
#[tokio::test]
async fn a_session_raised_to_full_its_inheriting_workers_ask_nothing() {
    for agent in [Agent::Claude, Agent::Codex] {
        let answers: Answers = Arc::default();
        let flow = Flow::start(
            "worker-raised",
            Options {
                permission: PermissionLevel::AcceptEdits,
                ..Options::default()
            },
            with_worker(agent, None, answers.clone()),
        )
        .await;
        flow.say("Start one: Fetch both.").await;
        let card = pending_card(&flow, &flow.conversation).await;
        set_level(&flow, &flow.conversation, PermissionLevel::Full).await;
        flow.until("the worker's two answers", |_| {
            answers.lock().unwrap().len() == 2
        })
        .await;
        assert_eq!(
            *answers.lock().unwrap(),
            vec![Some(ApprovalDecision::Allow), Some(ApprovalDecision::Allow)],
            "{agent:?}"
        );
        let board = flow.board_of(&flow.conversation).await;
        assert!(allowed_by_policy(&board, &card), "{agent:?}: {board:#?}");
        assert_eq!(
            board.approvals.len(),
            1,
            "{agent:?}: the second asked no card"
        );
        flow.stop().await;
    }
}

/// A session at Full access lowered to Ask while its worker (on either CLI, at the session's
/// level) rests between turns: its next turn resumes the same CLI session at Ask, and what it
/// asks is the user's card again.
#[tokio::test]
async fn a_session_lowered_to_ask_its_inheriting_workers_ask_again() {
    for agent in [Agent::Claude, Agent::Codex] {
        let answers: Answers = Arc::default();
        let flow = Flow::start(
            "worker-lowered",
            Options {
                permission: PermissionLevel::Full,
                ..Options::default()
            },
            with_worker(agent, None, answers.clone()),
        )
        .await;
        flow.say("Start one: Say hi.").await;
        let board = flow
            .until("the worker's first turn", |board| {
                board
                    .tasks
                    .values()
                    .any(|task| task.state == TaskState::Reported)
            })
            .await;
        let task = board.tasks.values().next().unwrap().id.clone();
        assert_eq!(worker_specs(&flow).len(), 1, "{agent:?}");
        assert_eq!(worker_specs(&flow)[0].access, Access::Full, "{agent:?}");
        set_level(&flow, &flow.conversation, PermissionLevel::Ask).await;
        flow.manager
            .send_to_worker(
                flow.conversation.clone(),
                task,
                "Fetch it.".into(),
                Vec::new(),
                Vec::new(),
            )
            .await
            .unwrap();
        let card = pending_card(&flow, &flow.conversation).await;
        let specs = worker_specs(&flow);
        assert_eq!(specs.len(), 2, "{agent:?}: it started again");
        assert_eq!(specs[1].access, Access::Ask, "{agent:?}");
        assert!(
            matches!(specs[1].origin, Origin::Resume { .. }),
            "{agent:?}: {:?}",
            specs[1].origin
        );
        flow.manager
            .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
            .await
            .unwrap();
        flow.until("the worker's answer", |_| {
            answers.lock().unwrap().len() == 1
        })
        .await;
        assert_eq!(
            *answers.lock().unwrap(),
            vec![Some(ApprovalDecision::Allow)]
        );
        flow.stop().await;
    }
}

/// A worker started at a level of its own (`--permission`, Q8) keeps it when its session is
/// raised to Full access: the card it waits on stays the user's.
#[tokio::test]
async fn a_worker_at_a_level_of_its_own_keeps_it() {
    let answers: Answers = Arc::default();
    let flow = Flow::start(
        "worker-own-level",
        Options {
            permission: PermissionLevel::AcceptEdits,
            ..Options::default()
        },
        with_worker(
            Agent::Claude,
            Some(Permission::AcceptEdits),
            answers.clone(),
        ),
    )
    .await;
    flow.say("Start one: Fetch it.").await;
    let card = pending_card(&flow, &flow.conversation).await;
    set_level(&flow, &flow.conversation, PermissionLevel::Full).await;
    let board = flow.board_of(&flow.conversation).await;
    assert_eq!(board.approvals[&card].state, CardState::Pending);
    assert!(answers.lock().unwrap().is_empty());
    flow.manager
        .answer_card(flow.conversation.clone(), card, ApprovalDecision::Allow)
        .await
        .unwrap();
    flow.until("the worker's answer", |_| {
        answers.lock().unwrap().len() == 1
    })
    .await;
    flow.stop().await;
}

/// A Chat's worker (a Fusion consult is one: started with no `--permission`) follows the
/// Chat's level as a session's does, on both CLIs. Raised to Full access while the worker's card
/// waits, that request goes; started at Accept edits moments before the switch, and asking only
/// after it, its CLI still runs in its Accept edits mode, yet nothing it asks is a card.
#[tokio::test]
async fn a_chat_raised_to_full_its_inheriting_workers_ask_nothing() {
    for agent in [Agent::Claude, Agent::Codex] {
        for asks_after_the_switch in [false, true] {
            let answers: Answers = Arc::default();
            let switched = Arc::new(AtomicBool::new(false));
            let (log, gate) = (answers.clone(), switched.clone());
            let flow = Flow::start(
                "chat-worker-raised",
                Options::default(),
                script(move |turn| {
                    let (log, gate) = (log.clone(), gate.clone());
                    async move {
                        if turn.worker().is_some() {
                            while asks_after_the_switch && !gate.load(Ordering::SeqCst) {
                                tokio::time::sleep(Duration::from_millis(20)).await;
                            }
                            for url in ["https://example.com", "https://example.org"] {
                                let answer =
                                    turn.ask_approval(&format!("curl {url}"), "curl").await;
                                log.lock().unwrap().push(answer);
                            }
                            return Reply::text("Fetched both.");
                        }
                        if turn.chat().is_some() && turn.input.contains("Consult.") {
                            brigadier(
                                &turn,
                                CliRequest::Start(Box::new(StartRequest {
                                    agent: Some(agent),
                                    ..start("Fetch both.")
                                })),
                            )
                            .await;
                            return Reply::text("Started a consult.");
                        }
                        Reply::text("ok")
                    }
                }),
            )
            .await;
            let chat = new_chat(&flow, ProviderKind::Claude, PermissionLevel::AcceptEdits).await;
            flow.say_to(&chat, "Consult.").await;
            let card = if asks_after_the_switch {
                flow.until_in(&chat, "the consult's CLI", |board| {
                    board
                        .tasks
                        .values()
                        .any(|task| task.state == TaskState::Running)
                })
                .await;
                None
            } else {
                Some(pending_card(&flow, &chat).await)
            };
            set_level(&flow, &chat, PermissionLevel::Full).await;
            switched.store(true, Ordering::SeqCst);
            flow.until_in(&chat, "the consult's two answers", |_| {
                answers.lock().unwrap().len() == 2
            })
            .await;
            let case = format!("{agent:?}, asks after the switch: {asks_after_the_switch}");
            assert_eq!(
                *answers.lock().unwrap(),
                vec![Some(ApprovalDecision::Allow), Some(ApprovalDecision::Allow)],
                "{case}"
            );
            let board = flow.board_of(&chat).await;
            match &card {
                Some(card) => {
                    assert!(allowed_by_policy(&board, card), "{case}: {board:#?}");
                    assert_eq!(board.approvals.len(), 1, "{case}: the second asked no card");
                }
                None => assert!(board.approvals.is_empty(), "{case}: {board:#?}"),
            }
            assert_eq!(
                worker_specs(&flow)[0].access,
                Access::AcceptEdits,
                "{case}: it started at the Chat's level then"
            );
            flow.stop().await;
        }
    }
}
