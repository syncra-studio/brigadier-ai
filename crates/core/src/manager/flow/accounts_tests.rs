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
            == "Claude Code hit its usage limit on this computer's login; continuing on Account acct-b."),
        "{notices:#?}"
    );
    // Only that note: the CLI's own limit message is not shown once the chat went on.
    assert!(
        !notices
            .iter()
            .any(|text| text.contains("hit your usage limit")),
        "{notices:#?}"
    );
    // The note to carry on is the CLI's input only, never a message of the chat's.
    let messages = flow.core.all_messages(&flow.conversation).await.unwrap();
    assert!(
        messages
            .iter()
            .all(|message| !message.text.contains(CONTINUE_ON_ACCOUNT)),
        "{messages:#?}"
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
    // The last account's limit is shown as before, since no account took the chat over.
    let notices = notices(&flow).await;
    assert_eq!(
        notices
            .iter()
            .filter(|text| *text == "You've hit your usage limit.")
            .count(),
        1,
        "{notices:#?}"
    );
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
    let deadline = tokio::time::Instant::now() + super::PATIENCE;
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
        notices.iter().any(|text| text
            .contains("hit its usage limit on this computer's login; task 1 continues on")),
        "{notices:#?}"
    );
    flow.stop().await;
}

#[tokio::test]
async fn an_account_in_use_is_not_removed_and_a_chat_pinned_to_a_removed_one_runs_on() {
    let log = Log::default();
    let hold = Arc::new(tokio::sync::Notify::new());
    let release = hold.clone();
    let script: Script = {
        let log = log.clone();
        Arc::new(move |turn: Turn| {
            let (log, release) = (log.clone(), release.clone());
            Box::pin(async move {
                log.lock().unwrap().push(Seen {
                    provider: turn.provider,
                    account: turn.account.clone(),
                    native_id: turn.native_id.clone(),
                    input: turn.input.clone(),
                });
                if turn.input.contains("Hold.") {
                    release.notified().await;
                }
                Reply::text("Done.")
            })
        })
    };
    let flow = Flow::start("accounts-remove", Options::default(), script).await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
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
    flow.say("Hold.").await;
    flow.until("the turn runs", |_| !seen(&log).is_empty())
        .await;
    let refused = flow.manager.remove_account("acct-b").await;
    assert!(refused.is_err(), "{refused:?}");
    assert_eq!(flow.core.settings().accounts.len(), 1);
    hold.notify_waiters();
    flow.settled().await;
    // Once the turn is over, the account can go (the chat's idle CLI on it is closed).
    let settings = flow.manager.remove_account("acct-b").await.unwrap();
    assert!(settings.accounts.is_empty());
    assert!(
        flow.manager
            .runtime
            .accounts_view()
            .accounts
            .iter()
            .all(|view| view.account.account.is_none())
    );
    // The chat still names the removed account, and runs on the user's own login.
    flow.say("After.").await;
    flow.until("the next turn", |_| seen(&log).len() >= 2).await;
    flow.settled().await;
    assert_eq!(seen(&log).last().unwrap().account, None);
    flow.stop().await;
}

#[tokio::test]
async fn a_model_window_used_up_on_one_account_sends_that_models_work_to_another() {
    use crate::accounts::AccountRef;
    use brigadier_providers::{QuotaSnapshot, QuotaSource, QuotaWindow};

    let flow = Flow::start(
        "accounts-model-window",
        Options::default(),
        logging(&Log::default(), |_| Reply::text("Noted.")),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let runtime = &flow.manager.runtime;
    let note = |account: Option<&str>, opus_used: f64| {
        let now = crate::now_ms();
        let window = |id: &str, used: f64, model: Option<&str>| QuotaWindow {
            id: id.into(),
            label: id.into(),
            used_percent: used,
            resets_at_ms: Some(now + 60 * 60 * 1000),
            window_minutes: Some(7 * 24 * 60),
            bucket: None,
            model: model.map(Into::into),
        };
        runtime.monitor().note(
            &AccountRef::new(ProviderKind::Claude, account.map(Into::into)),
            &QuotaSnapshot {
                provider: ProviderKind::Claude,
                windows: vec![
                    window("seven_day", 30.0, None),
                    window("seven_day_opus", opus_used, Some("opus")),
                ],
                limit: None,
                observed_at_ms: now,
                source: QuotaSource::Read,
            },
            now,
        );
    };
    let own = AccountRef::own(ProviderKind::Claude);
    let opus_left = |runtime: &crate::runtime::Runtime| {
        runtime
            .provider_usage(ProviderKind::Claude, crate::now_ms())
            .unwrap()
            .windows
            .iter()
            .find(|state| state.window.id == "seven_day_opus")
            .unwrap()
            .window
            .used_percent
    };

    // Opus used up on the computer's login, with room on the other account: routing sees
    // that room, Opus work starts there under any of the model's names, and a chat on the
    // CLI's default model may move there.
    note(None, 100.0);
    note(Some("acct-b"), 20.0);
    assert_eq!(opus_left(runtime), 20.0);
    for name in ["opus", "claude-opus-5-5", "opus[1m]"] {
        assert_eq!(
            runtime
                .launch_account(ProviderKind::Claude, Some(name))
                .account
                .as_deref(),
            Some("acct-b"),
            "{name}"
        );
    }
    assert_eq!(
        runtime.launch_account(ProviderKind::Claude, Some("sonnet")),
        own
    );
    assert_eq!(
        runtime
            .switch_target(&own, None)
            .and_then(|next| next.account),
        Some("acct-b".into())
    );

    // Used up on both: no account takes it, so neither hands it to the other.
    note(Some("acct-b"), 100.0);
    assert_eq!(opus_left(runtime), 100.0);
    assert_eq!(runtime.switch_target(&own, None), None);
    assert_eq!(runtime.switch_target(&own, Some("opus")), None);
    flow.stop().await;
}

async fn chat_on(flow: &Flow, provider: ProviderKind, account: Option<&str>) -> ConversationId {
    flow.manager
        .create_conversation(
            ConversationKind::Chat,
            None,
            Some("Account cleanup".into()),
            Some(SetupRequest::Chat {
                model: ModelChoice {
                    provider,
                    model: None,
                    effort: None,
                    fast: None,
                    account: account.map(Into::into),
                },
            }),
        )
        .await
        .unwrap()
        .id
}

async fn say_to(flow: &Flow, chat: &ConversationId, text: &str) {
    flow.manager
        .send_message(chat.clone(), text.into(), vec![], vec![], false, None)
        .await
        .unwrap();
    tokio::time::timeout(super::PATIENCE, async {
        loop {
            let board = flow.core.board(chat).await.unwrap();
            if !board.requests.is_empty()
                && board
                    .requests
                    .values()
                    .all(|request| request.state == RequestState::Done)
            {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the chat answered");
}

/// Both native artifact kinds must retain the home of each run, even after the setup points
/// at the new account. Unrelated accounts may hold files with the same native id.
#[tokio::test]
async fn archiving_or_deleting_a_chat_cleans_each_accounts_own_artifacts() {
    for provider in ProviderKind::ALL {
        for switched in [false, true] {
            for delete in [false, true] {
                let log = Log::default();
                let flow = Flow::start(
                    "accounts-cleanup",
                    Options {
                        behavior: Arc::new(super::FakeBehavior {
                            cleanup: true,
                            ..Default::default()
                        }),
                        ..Options::default()
                    },
                    logging(&log, move |turn| {
                        if switched && turn.account.is_none() {
                            Reply::limited()
                        } else {
                            Reply::text("Done.")
                        }
                    }),
                )
                .await;
                flow.add_accounts(&[(provider, "acct-b"), (provider, "unrelated")], true)
                    .await;
                let chat = chat_on(
                    &flow,
                    provider,
                    if switched { None } else { Some("acct-b") },
                )
                .await;
                say_to(&flow, &chat, "Clean this chat.").await;
                let owner = format!("chat:{chat}");
                let ledger = flow.manager.runtime.ledger();
                let artifacts = ledger.artifacts(&owner);
                let native: Vec<_> = artifacts
                    .iter()
                    .filter(|artifact| {
                        matches!(
                            artifact,
                            brigadier_providers::Artifact::ClaudeSession { .. }
                                | brigadier_providers::Artifact::CodexThread { .. }
                        )
                    })
                    .cloned()
                    .collect();
                assert_eq!(native.len(), if switched { 2 } else { 1 }, "{native:?}");
                let id = seen(&log)[0].native_id.clone();
                let own = flow.dir.join("data/own").join(provider.to_string());
                let extra = flow.manager.runtime.account_home("acct-b");
                let unrelated = flow.manager.runtime.account_home("unrelated");
                // Same-id files in a home the chat never ran on must stay untouched.
                let mut kept = Vec::new();
                for home in [unrelated.as_path()]
                    .into_iter()
                    .chain((!switched).then_some(own.as_path()))
                {
                    for path in super::fake_files(home, &id) {
                        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                        std::fs::write(&path, "unrelated").unwrap();
                        kept.push(path);
                    }
                }
                for artifact in &native {
                    let home = match artifact {
                        brigadier_providers::Artifact::ClaudeSession { home, .. }
                        | brigadier_providers::Artifact::CodexThread { home, .. } => home,
                        _ => unreachable!(),
                    };
                    assert!(
                        home.as_deref()
                            .is_none_or(|home| home == extra.to_str().unwrap())
                    );
                    for path in super::fake_files(
                        home.as_ref().map(std::path::Path::new).unwrap_or(&own),
                        &id,
                    ) {
                        assert!(path.exists(), "{} was created", path.display());
                    }
                }
                if delete {
                    flow.manager.delete(chat.clone()).await.unwrap();
                } else {
                    flow.manager.archive(chat.clone()).await.unwrap();
                }
                flow.manager.cleanup_finished(&chat).await;
                assert!(ledger.artifacts(&owner).is_empty(), "cleanup acknowledged");
                let removals = flow.behavior.removals.lock().unwrap().clone();
                for artifact in &native {
                    let home = match artifact {
                        brigadier_providers::Artifact::ClaudeSession { home, .. }
                        | brigadier_providers::Artifact::CodexThread { home, .. } => home,
                        _ => unreachable!(),
                    };
                    let account = home.as_ref().map(|_| "acct-b".to_owned());
                    assert_eq!(
                        removals
                            .iter()
                            .filter(|(kind, on, batch)| *kind == provider
                                && *on == account
                                && batch.contains(artifact))
                            .count(),
                        1,
                        "{removals:?}"
                    );
                    for path in super::fake_files(
                        home.as_ref().map(std::path::Path::new).unwrap_or(&own),
                        &id,
                    ) {
                        assert!(!path.exists(), "{} was cleaned", path.display());
                    }
                }
                assert!(
                    kept.iter()
                        .all(|path| std::fs::read_to_string(path).unwrap() == "unrelated")
                );
                flow.stop().await;
            }
        }
    }
}

/// An interrupted disposal retains both homes durably. The startup sweep must resolve extra
/// adapters before attempting cleanup, rather than send the extra artifact to the own CLI.
#[tokio::test]
async fn a_restart_finishes_account_cleanup_in_the_recorded_homes() {
    for provider in ProviderKind::ALL {
        let log = Log::default();
        let mut flow = Flow::start(
            "accounts-cleanup-restart",
            Options {
                behavior: Arc::new(super::FakeBehavior {
                    cleanup: true,
                    ..Default::default()
                }),
                ..Options::default()
            },
            logging(&log, |turn| {
                if turn.account.is_none() {
                    Reply::limited()
                } else {
                    Reply::text("Done.")
                }
            }),
        )
        .await;
        flow.add_accounts(&[(provider, "acct-b")], true).await;
        let chat = chat_on(&flow, provider, None).await;
        say_to(&flow, &chat, "Before the restart.").await;
        let owner = format!("chat:{chat}");
        let native: Vec<_> = flow
            .manager
            .runtime
            .ledger()
            .artifacts(&owner)
            .into_iter()
            .filter(|artifact| {
                matches!(
                    artifact,
                    brigadier_providers::Artifact::ClaudeSession { .. }
                        | brigadier_providers::Artifact::CodexThread { .. }
                )
            })
            .collect();
        assert_eq!(native.len(), 2, "both runs recorded: {native:?}");
        flow.behavior
            .fail_cleanup
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(
            !flow
                .manager
                .runtime
                .ledger()
                .dispose(&owner)
                .await
                .is_clean()
        );
        assert!(flow.manager.runtime.ledger().disposing().contains(&owner));
        flow.behavior.removals.lock().unwrap().clear();
        flow.behavior
            .fail_cleanup
            .store(false, std::sync::atomic::Ordering::SeqCst);
        flow.restart().await;
        assert!(
            flow.manager.runtime.ledger().artifacts(&owner).is_empty(),
            "startup sweep completed the disposal"
        );
        assert!(!flow.manager.runtime.ledger().disposing().contains(&owner));
        let removals = flow.behavior.removals.lock().unwrap().clone();
        for artifact in native {
            let (id, home) = match &artifact {
                brigadier_providers::Artifact::ClaudeSession { session_id, home } => {
                    (session_id, home)
                }
                brigadier_providers::Artifact::CodexThread { thread_id, home } => (thread_id, home),
                other => panic!("unexpected artifact: {other:?}"),
            };
            let account = home.as_ref().map(|_| "acct-b".to_owned());
            let sent: Vec<_> = removals
                .iter()
                .filter(|(_, _, batch)| batch.contains(&artifact))
                .collect();
            assert_eq!(sent.len(), 1, "{removals:?}");
            assert_eq!(
                (sent[0].0, &sent[0].1),
                (provider, &account),
                "{removals:?}"
            );
            let own = flow.dir.join("data/own").join(provider.to_string());
            for path in
                super::fake_files(home.as_ref().map(std::path::Path::new).unwrap_or(&own), id)
            {
                assert!(!path.exists(), "{} survived", path.display());
            }
        }
        flow.stop().await;
    }
}

#[tokio::test]
async fn an_extra_login_makes_a_signed_out_provider_ready_and_starts_its_chat() {
    for provider in ProviderKind::ALL {
        let log = Log::default();
        let flow = Flow::start(
            "accounts-signed-out-own",
            Options {
                behavior: Arc::new(super::FakeBehavior {
                    signed_out: Mutex::default(),
                    ..Default::default()
                }),
                ..Options::default()
            },
            logging(&log, |_| Reply::text("Signed in here.")),
        )
        .await;
        flow.behavior.signed_out.lock().unwrap().push(provider);
        flow.manager.runtime.refresh_providers(Some(provider));
        flow.until("the own login is checked as signed out", |_| {
            !flow.manager.runtime.provider_signed_in(provider)
        })
        .await;
        assert!(!flow.manager.runtime.provider_signed_in(provider));
        assert!(
            !flow
                .manager
                .runtime
                .overview(provider)
                .unwrap()
                .status
                .unwrap()
                .logged_in
        );
        flow.add_accounts(&[(provider, "acct-b")], true).await;
        assert!(flow.manager.runtime.provider_signed_in(provider));
        let chat = chat_on(&flow, provider, None).await;
        say_to(&flow, &chat, "Use the signed-in account.").await;
        let turns = seen(&log);
        assert_eq!(turns.len(), 1, "{turns:?}");
        assert_eq!(turns[0].provider, provider);
        assert_eq!(turns[0].account.as_deref(), Some("acct-b"));
        assert!(flow.core.conversation(&chat).unwrap().fallback.is_none());
        flow.stop().await;
    }
}

#[tokio::test]
async fn an_unsent_steer_survives_an_account_switch_without_repeating_landed_messages() {
    for provider in ProviderKind::ALL {
        let log = Log::default();
        let release = Arc::new(tokio::sync::Notify::new());
        let script: Script = {
            let (log, release) = (log.clone(), release.clone());
            Arc::new(move |turn: Turn| {
                let (log, release) = (log.clone(), release.clone());
                Box::pin(async move {
                    log.lock().unwrap().push(Seen {
                        provider: turn.provider,
                        account: turn.account.clone(),
                        native_id: turn.native_id.clone(),
                        input: turn.input.clone(),
                    });
                    if turn.account.is_none() {
                        release.notified().await;
                        Reply::limited()
                    } else {
                        Reply::text("Both requests done.")
                    }
                })
            })
        };
        let flow = Flow::start(
            "accounts-unsent-steer",
            Options {
                thread: provider,
                behavior: Arc::new(super::FakeBehavior {
                    refuse_steers: true,
                    ..Default::default()
                }),
                ..Options::default()
            },
            script,
        )
        .await;
        flow.add_accounts(&[(provider, "acct-b")], true).await;
        flow.say("Landed first.").await;
        flow.until("the first CLI takes the message", |_| seen(&log).len() == 1)
            .await;
        flow.manager
            .send_message(
                flow.conversation.clone(),
                "Never landed steer.".into(),
                vec![],
                vec![],
                true,
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            seen(&log).len(),
            1,
            "the refused steer did not start a turn"
        );
        release.notify_one();
        flow.settled().await;
        let turns = seen(&log);
        assert_eq!(turns.len(), 2, "{turns:?}");
        assert_eq!(turns[1].account.as_deref(), Some("acct-b"));
        assert_eq!(turns[1].native_id, turns[0].native_id);
        assert!(turns[1].input.contains("Never landed steer."), "{turns:?}");
        assert!(turns[1].input.contains(CONTINUE_ON_ACCOUNT), "{turns:?}");
        assert!(!turns[1].input.contains("Landed first."), "{turns:?}");
        let injections = flow
            .core
            .list_orchestrator_log(&flow.conversation, None, 200)
            .await
            .unwrap()
            .entries
            .into_iter()
            .filter(|logged| {
                matches!(&logged.entry,
                    crate::work::OrchestratorEntry::Injection { injection }
                        if injection.kind == crate::work::InjectionKind::UserMessage
                )
            })
            .count();
        assert_eq!(injections, 2, "each delivered message logged once");
        flow.stop().await;
    }
}
