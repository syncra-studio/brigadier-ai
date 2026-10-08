//! Several accounts of a CLI: a chat moves to another account of the same provider when its
//! account hits a limit, falls back to the other provider only when every account is used up
//! (or switching is off), and switches account in one step, resuming its session.

use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;

use super::{Flow, Options, Reply, Script, Turn};
use crate::manager::prompts::CONTINUE_ON_ACCOUNT;
use crate::model::{
    ConversationId, ConversationKind, DomainEvent, ModelChoice, Setup, SetupRequest,
};
use crate::work::RequestState;

/// One turn as a scripted CLI saw it.
#[derive(Debug, Clone)]
struct Seen {
    provider: ProviderKind,
    account: Option<String>,
    native_id: String,
    input: String,
}

type Log = Arc<Mutex<Vec<Seen>>>;

/// A script that logs each orchestrator or chat turn and answers with `answer`.
fn logging<F>(log: &Log, answer: F) -> Script
where
    F: Fn(&Seen) -> Reply + Send + Sync + 'static,
{
    let (log, answer) = (log.clone(), Arc::new(answer));
    Arc::new(move |turn: Turn| {
        let (log, answer) = (log.clone(), answer.clone());
        Box::pin(async move {
            if turn.is_review() {
                return Reply::text("No findings.");
            }
            let seen = Seen {
                provider: turn.provider,
                account: turn.account.clone(),
                native_id: turn.native_id.clone(),
                input: turn.input.clone(),
            };
            log.lock().unwrap().push(seen.clone());
            answer(&seen)
        })
    })
}

fn seen(log: &Log) -> Vec<Seen> {
    log.lock().unwrap().clone()
}

async fn notices(flow: &Flow) -> Vec<String> {
    flow.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::ConversationNotice { notice, .. } => Some(notice.text),
            _ => None,
        })
        .collect()
}

fn orchestrator(flow: &Flow) -> ModelChoice {
    match flow.core.conversation(&flow.conversation).unwrap().setup {
        Some(Setup::Session { orchestrator, .. }) => orchestrator,
        other => panic!("a session's setup, not {other:?}"),
    }
}

#[tokio::test]
async fn a_limit_moves_the_chat_to_another_account_and_its_work_is_done_once() {
    let log = Log::default();
    // The work (a side effect) is done whenever a turn is given the user's message; the
    // user's own login is at its limit right after.
    let done = Arc::new(Mutex::new(0));
    let did = done.clone();
    let flow = Flow::start(
        "accounts-switch",
        Options::default(),
        logging(&log, move |turn| {
            if turn.input.contains("Do the thing.") && !turn.input.contains(CONTINUE_ON_ACCOUNT) {
                *did.lock().unwrap() += 1;
            }
            match &turn.account {
                None => Reply::limited(),
                Some(_) => Reply::text("Done."),
            }
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    flow.say("Do the thing.").await;
    let board = flow.settled().await;
    assert!(
        board
            .requests
            .values()
            .all(|request| request.state == RequestState::Done),
        "{:?}",
        board.requests
    );
    let turns = seen(&log);
    assert_eq!(turns.len(), 2, "{turns:#?}");
    assert_eq!(turns[0].account, None);
    assert_eq!(turns[1].account.as_deref(), Some("acct-b"));
    assert_eq!(turns[1].provider, ProviderKind::Claude);
    // The same CLI session, resumed on the other account, told to carry on.
    assert_eq!(turns[1].native_id, turns[0].native_id);
    assert!(turns[1].input.contains(CONTINUE_ON_ACCOUNT));
    assert_eq!(*done.lock().unwrap(), 1, "the work is done once");
    // The chat now runs on that account, and says so.
    assert_eq!(orchestrator(&flow).account.as_deref(), Some("acct-b"));
    let notices = notices(&flow).await;
    assert!(
        notices.iter().any(|text| text
            == "Claude Code hit its usage limit on your own login; continuing on Account acct-b."),
        "{notices:#?}"
    );
    flow.stop().await;
}

#[tokio::test]
async fn with_every_account_at_its_limit_the_chat_falls_back_to_the_other_provider() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-all-limited",
        Options::default(),
        logging(&log, |turn| match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Done on Codex."),
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    flow.say("Do the thing.").await;
    flow.until("a Codex turn", |_| {
        seen(&log)
            .iter()
            .any(|turn| turn.provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let turns = seen(&log);
    let path: Vec<_> = turns
        .iter()
        .map(|turn| (turn.provider, turn.account.as_deref()))
        .collect();
    assert_eq!(
        path,
        [
            (ProviderKind::Claude, None),
            (ProviderKind::Claude, Some("acct-b")),
            (ProviderKind::Codex, None),
        ]
    );
    let fallback = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .fallback
        .expect("Codex stands in");
    assert_eq!(fallback.choice.provider, ProviderKind::Codex);
    flow.stop().await;
}

#[tokio::test]
async fn with_switching_off_a_limit_falls_back_to_the_other_provider_at_once() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-switch-off",
        Options::default(),
        logging(&log, |turn| match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Done on Codex."),
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], false)
        .await;
    flow.say("Do the thing.").await;
    flow.until("a Codex turn", |_| {
        seen(&log)
            .iter()
            .any(|turn| turn.provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let path: Vec<_> = seen(&log)
        .iter()
        .map(|turn| (turn.provider, turn.account.clone()))
        .collect();
    assert_eq!(
        path,
        [(ProviderKind::Claude, None), (ProviderKind::Codex, None)]
    );
    flow.stop().await;
}

#[tokio::test]
async fn switching_a_chats_account_resumes_its_session_on_the_other_account() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-one-click",
        Options::default(),
        logging(&log, |_| Reply::text("Noted.")),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    flow.say("First.").await;
    flow.settled().await;
    let Some(Setup::Session {
        repo,
        environment,
        permission,
        orchestrator,
        workers_see_uncommitted,
        plan_mode,
    }) = flow.core.conversation(&flow.conversation).unwrap().setup
    else {
        panic!("a session");
    };
    flow.manager
        .set_setup(
            flow.conversation.clone(),
            Setup::Session {
                repo,
                environment,
                permission,
                orchestrator: ModelChoice {
                    account: Some("acct-b".into()),
                    ..orchestrator
                },
                workers_see_uncommitted,
                plan_mode,
            },
        )
        .await
        .unwrap();
    flow.say("Second.").await;
    flow.until("the second turn", |_| seen(&log).len() >= 2)
        .await;
    flow.settled().await;
    let turns = seen(&log);
    assert_eq!(turns[0].account, None);
    assert_eq!(turns[1].account.as_deref(), Some("acct-b"));
    assert_eq!(turns[1].native_id, turns[0].native_id, "the same session");
    assert!(turns[1].input.contains("Second."));
    // Back to the user's own login the same way.
    let specs = flow.thread_specs();
    assert!(matches!(
        specs.last().map(|(_, spec)| &spec.origin),
        Some(brigadier_providers::model::Origin::Resume { .. })
    ));
    flow.stop().await;
}

#[tokio::test]
async fn two_chats_on_two_accounts_run_at_the_same_time() {
    let log = Log::default();
    // Each chat's turn waits until both are running.
    let both = Arc::new(tokio::sync::Barrier::new(2));
    let gate = both.clone();
    let script: Script = {
        let log = log.clone();
        Arc::new(move |turn: Turn| {
            let (log, gate) = (log.clone(), gate.clone());
            Box::pin(async move {
                log.lock().unwrap().push(Seen {
                    provider: turn.provider,
                    account: turn.account.clone(),
                    native_id: turn.native_id.clone(),
                    input: turn.input.clone(),
                });
                if turn.input.contains("Chat on") {
                    gate.wait().await;
                }
                Reply::text("Hello.")
            })
        })
    };
    let flow = Flow::start("accounts-two-chats", Options::default(), script).await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let mut chats: Vec<ConversationId> = Vec::new();
    for account in [crate::accounts::OWN, "acct-b"] {
        let chat = flow
            .manager
            .create_conversation(
                ConversationKind::Chat,
                None,
                Some(account.into()),
                Some(SetupRequest::Chat {
                    model: ModelChoice {
                        provider: ProviderKind::Claude,
                        model: Some("claude-opus-5-5".into()),
                        effort: None,
                        fast: None,
                        account: Some(account.into()),
                    },
                }),
            )
            .await
            .unwrap();
        chats.push(chat.id);
    }
    for (chat, account) in chats.iter().zip([crate::accounts::OWN, "acct-b"]) {
        flow.manager
            .send_message(
                chat.clone(),
                format!("Chat on {account}."),
                Vec::new(),
                Vec::new(),
                false,
                None,
            )
            .await
            .unwrap();
    }
    // Both turns ran at once (each waited for the other), each on its own account.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(60);
    loop {
        let mut done = true;
        for chat in &chats {
            let board = flow.core.board(chat).await.unwrap();
            done &= !board.requests.is_empty()
                && board
                    .requests
                    .values()
                    .all(|request| request.state == RequestState::Done);
        }
        if done {
            break;
        }
        assert!(tokio::time::Instant::now() < deadline, "both chats answer");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    let mut accounts: Vec<_> = seen(&log)
        .iter()
        .filter(|turn| turn.input.contains("Chat on"))
        .map(|turn| turn.account.clone())
        .collect();
    accounts.sort();
    assert_eq!(accounts, [None, Some("acct-b".to_owned())]);
    flow.stop().await;
}

#[tokio::test]
async fn a_workers_task_carries_on_with_another_account_of_its_provider() {
    let log = Log::default();
    let workers = log.clone();
    let flow = Flow::start(
        "accounts-worker",
        Options::default(),
        Arc::new(move |turn: Turn| {
            let workers = workers.clone();
            Box::pin(async move {
                if turn.is_review() {
                    return Reply::text("No findings.");
                }
                if turn.is_orchestrator() {
                    if turn.input.contains("[report task-1") {
                        return Reply::text("Looked around.");
                    }
                    if turn.input.contains("Look around.") {
                        let reply = turn
                            .call(
                                "delegate_task",
                                serde_json::json!({"effort": "high", "title": "Look around", "kind": "scout", "spec": "List the files."}),
                            )
                            .await;
                        assert!(!reply.is_error, "{}", reply.text);
                    }
                    return Reply::text("[quiet]");
                }
                workers.lock().unwrap().push(Seen {
                    provider: turn.provider,
                    account: turn.account.clone(),
                    native_id: turn.native_id.clone(),
                    input: String::new(),
                });
                if turn.account.is_none() {
                    return Reply::limited();
                }
                let reply = turn
                    .call("submit_report", serde_json::json!({"summary": "A README."}))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                Reply::text("Reported.")
            })
        }),
    )
    .await;
    flow.add_accounts(
        &[
            (ProviderKind::Claude, "claude-b"),
            (ProviderKind::Codex, "codex-b"),
        ],
        true,
    )
    .await;
    flow.say("Look around.").await;
    let board = flow
        .until("the task is done", |board| {
            board
                .tasks
                .values()
                .any(|task| task.number == 1 && task.state == crate::work::TaskState::Done)
        })
        .await;
    assert_eq!(Flow::task(&board, 1).state, crate::work::TaskState::Done);
    let turns = seen(&log);
    assert!(turns.len() >= 2, "{turns:#?}");
    // Its own provider, on the other account.
    assert_eq!(turns[0].account, None);
    assert_eq!(turns[1].provider, turns[0].provider);
    assert!(turns[1].account.is_some(), "{turns:#?}");
    let notices = notices(&flow).await;
    assert!(
        notices
            .iter()
            .any(|text| text.contains("hit its usage limit on your own login; task 1 continues on")),
        "{notices:#?}"
    );
    flow.stop().await;
}
