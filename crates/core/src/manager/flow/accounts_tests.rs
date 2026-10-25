//! Several accounts of a CLI: a chat moves to another account of the same provider when its
//! account hits a limit (also with switching off), falls back to the other provider only when
//! every account is used up, and switches account in one step, resuming its session.

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

/// What the thread says across its column (#506): limits and switches are no notices.
async fn moments(flow: &Flow) -> Vec<String> {
    flow.events()
        .await
        .into_iter()
        .filter_map(|event| match event {
            DomainEvent::OrchestratorStepped { step } => match step.kind {
                crate::work::OrchestratorStepKind::Moment { text } => Some(text),
                _ => None,
            },
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
    let moments = moments(&flow).await;
    assert_eq!(
        moments,
        ["Claude Code hit its usage limit on this computer's login; continuing on Account acct-b."],
    );
    // Only that divider: the CLI's own limit message is not shown, nor any notice of it.
    let notices = notices(&flow).await;
    assert!(notices.is_empty(), "{notices:#?}");
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
async fn a_switch_notice_masks_an_account_named_by_its_email() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-switch-masked",
        Options::default(),
        logging(&log, |turn| match &turn.account {
            None => Reply::limited(),
            Some(_) => Reply::text("Done."),
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let mut settings = flow.core.settings();
    settings.accounts[0].name = "work.person@corp.example.io".into();
    flow.core.update_settings(settings).await.unwrap();
    flow.manager.runtime.sync_accounts().await;
    flow.say("Do the thing.").await;
    flow.settled().await;
    let moments = moments(&flow).await;
    assert!(
        moments.iter().any(|text| text
            == "Claude Code hit its usage limit on this computer's login; continuing on wor•••••@•••••.io."),
        "{moments:#?}"
    );
    flow.stop().await;
}

/// #505: account switching is off (Use picked `acct-c`) while the session still runs on
/// `acct-b`, where it hits its limit. Another Claude account has quota, so the session goes on
/// on Claude, on the picked account, and says so; Codex doesn't stand in.
#[tokio::test]
async fn with_switching_off_a_limit_moves_the_session_to_the_picked_account() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-switch-off",
        Options::default(),
        logging(&log, |turn| {
            match (turn.provider, turn.account.as_deref()) {
                (ProviderKind::Claude, Some("acct-b")) => Reply::limited(),
                (ProviderKind::Claude, _) => Reply::text("Done."),
                (ProviderKind::Codex, _) => Reply::text("Done on Codex."),
            }
        }),
    )
    .await;
    flow.add_accounts(
        &[
            (ProviderKind::Claude, "acct-b"),
            (ProviderKind::Claude, "acct-c"),
        ],
        true,
    )
    .await;
    let before = flow.core.settings();
    let mut settings = before.clone();
    for entry in &mut settings.accounts {
        entry.default = entry.id == "acct-c";
    }
    let settings = crate::accounts::edited(&before, settings);
    assert_eq!(settings.manual_accounts, [ProviderKind::Claude]);
    flow.core.update_settings(settings).await.unwrap();
    flow.manager.runtime.sync_accounts().await;
    on_account(&flow, "acct-b").await;
    flow.say("Do the thing.").await;
    flow.until("a second turn", |_| seen(&log).len() >= 2).await;
    flow.settled().await;
    let path: Vec<_> = seen(&log)
        .iter()
        .map(|turn| (turn.provider, turn.account.clone()))
        .collect();
    assert_eq!(
        path,
        [
            (ProviderKind::Claude, Some("acct-b".into())),
            (ProviderKind::Claude, Some("acct-c".into())),
        ]
    );
    assert_eq!(orchestrator(&flow).account.as_deref(), Some("acct-c"));
    let moments = moments(&flow).await;
    assert!(
        moments.iter().any(|text| text
            == "Claude Code hit its usage limit on Account acct-b; continuing on Account acct-c."),
        "{moments:#?}"
    );
    assert!(
        !moments.iter().any(|text| text.contains("Codex")),
        "{moments:#?}"
    );
    flow.stop().await;
}

/// The session's orchestrator, named to run on `account` (as one that moved there is).
async fn on_account(flow: &Flow, account: &str) {
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
                    account: Some(account.into()),
                    ..orchestrator
                },
                workers_see_uncommitted,
                plan_mode,
            },
        )
        .await
        .unwrap();
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
                    permission: Default::default(),
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
    let deadline = tokio::time::Instant::now() + *super::PATIENCE;
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

#[tokio::test]
async fn a_running_thread_keeps_its_account_when_another_resets_sooner() {
    let log = Log::default();
    let mut flow = Flow::start(
        "accounts-keep",
        Options::default(),
        logging(&log, |_| Reply::text("Done.")),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    // New work starts on the account whose week ends first: here the second one.
    flow.weekly_resets_in(ProviderKind::Claude, None, 90);
    flow.weekly_resets_in(ProviderKind::Claude, Some("acct-b"), 20);
    flow.say("First.").await;
    flow.settled().await;
    assert_eq!(seen(&log)[0].account.as_deref(), Some("acct-b"));
    assert_eq!(orchestrator(&flow).account.as_deref(), Some("acct-b"));

    // The other account's week now ends sooner, and its CLI starts again (a restart): the
    // thread stays where its cache is, with no limit hit.
    flow.restart().await;
    flow.weekly_resets_in(ProviderKind::Claude, None, 2);
    flow.weekly_resets_in(ProviderKind::Claude, Some("acct-b"), 120);
    assert_eq!(
        flow.manager
            .runtime
            .launch_account(ProviderKind::Claude, None)
            .account,
        None
    );
    flow.say("Second.").await;
    flow.until("the next turn", |_| seen(&log).len() >= 2).await;
    flow.settled().await;
    let turns = seen(&log);
    assert_eq!(turns.len(), 2, "{turns:#?}");
    assert_eq!(turns[1].account.as_deref(), Some("acct-b"));
    assert_eq!(orchestrator(&flow).account.as_deref(), Some("acct-b"));
    flow.stop().await;
}

/// Notes `account`'s quota as a live report would: its weekly window `used` % and resetting in
/// `hours`, its 5-hour one barely used.
async fn report_weekly(flow: &Flow, account: Option<&str>, used: f64, hours: i64) {
    use brigadier_providers::{QuotaSnapshot, QuotaSource, QuotaWindow};

    let now = crate::now_ms();
    let window = |id: &str, minutes: i64, used_percent: f64, resets_at_ms: i64| QuotaWindow {
        id: id.into(),
        label: id.into(),
        used_percent,
        resets_at_ms: Some(resets_at_ms),
        window_minutes: Some(minutes),
        bucket: None,
        model: None,
    };
    let provider = ProviderKind::Claude;
    flow.manager
        .runtime
        .note_quota_snapshot(
            &crate::accounts::AccountRef::new(provider, account.map(Into::into)),
            QuotaSnapshot {
                provider,
                windows: vec![
                    window("five_hour", 300, 1.0, now + 3_600_000),
                    window("seven_day", 7 * 24 * 60, used, now + hours * 3_600_000),
                ],
                limit: None,
                observed_at_ms: now,
                source: QuotaSource::Event,
            },
        )
        .await;
}

/// The account Settings marks in use, and the weekly use the Usage menu shows for Claude.
async fn in_use(flow: &Flow) -> (Vec<Option<String>>, Option<f64>) {
    let marked = flow
        .manager
        .runtime
        .accounts_view()
        .accounts
        .into_iter()
        .filter(|view| view.account.provider == ProviderKind::Claude && view.in_use)
        .map(|view| view.account.account)
        .collect();
    let weekly = flow
        .manager
        .runtime
        .overview(ProviderKind::Claude)
        .and_then(|overview| overview.quota)
        .and_then(|quota| {
            quota
                .windows
                .into_iter()
                .find(|window| window.id == "seven_day")
        })
        .map(|window| window.used_percent);
    (marked, weekly)
}

/// The account the last published Settings → Accounts marks in use.
async fn published_in_use(flow: &Flow) -> Vec<Option<String>> {
    let page = flow
        .core
        .store()
        .read_stream(
            crate::model::streams::ACCOUNTS.into(),
            brigadier_store::StreamPage {
                limit: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    page.first()
        .map(|event| crate::sessions::decode(event).unwrap())
        .and_then(|event| match event {
            DomainEvent::AccountsChecked { accounts } => Some(accounts),
            _ => None,
        })
        .map(|view| {
            view.accounts
                .into_iter()
                .filter(|view| view.account.provider == ProviderKind::Claude && view.in_use)
                .map(|view| view.account.account)
                .collect()
        })
        .unwrap_or_default()
}

#[tokio::test]
async fn settings_and_the_usage_menu_mark_the_account_new_work_starts_on() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-in-use",
        Options::default(),
        logging(&log, |_| Reply::text("Done.")),
    )
    .await;
    flow.add_accounts(
        &[
            (ProviderKind::Claude, "acct-b"),
            (ProviderKind::Claude, "acct-c"),
        ],
        true,
    )
    .await;
    // The user's own login is the default; the second account's week ends first; the third is
    // used up.
    report_weekly(&flow, None, 7.0, 37).await;
    report_weekly(&flow, Some("acct-c"), 100.0, 34).await;
    report_weekly(&flow, Some("acct-b"), 94.0, 4).await;
    assert_eq!(
        flow.manager
            .runtime
            .launch_account(ProviderKind::Claude, None)
            .account
            .as_deref(),
        Some("acct-b")
    );
    // One account in use, the one new work starts on, and the menu shows its use.
    assert_eq!(
        in_use(&flow).await,
        (vec![Some("acct-b".into())], Some(94.0))
    );
    assert_eq!(published_in_use(&flow).await, vec![Some("acct-b".into())]);
    flow.say("First.").await;
    flow.settled().await;
    assert_eq!(seen(&log)[0].account.as_deref(), Some("acct-b"));

    // It runs out: new work starts on the next, and both places follow at once.
    report_weekly(&flow, Some("acct-b"), 100.0, 4).await;
    assert_eq!(in_use(&flow).await, (vec![None], Some(7.0)));
    assert_eq!(published_in_use(&flow).await, vec![None]);

    // Use picks an account: it turns Claude's account switching off. Used up, it can't take
    // work, so work runs on another account that can (#505).
    let before = flow.core.settings();
    let mut settings = before.clone();
    for entry in &mut settings.accounts {
        entry.default = entry.id == "acct-c";
    }
    let settings = crate::accounts::edited(&before, settings);
    assert_eq!(settings.manual_accounts, [ProviderKind::Claude]);
    flow.core.update_settings(settings).await.unwrap();
    flow.manager.runtime.sync_accounts().await;
    assert_eq!(in_use(&flow).await, (vec![None], Some(7.0)));
    // With room again, work runs there, though another account's week ends sooner.
    report_weekly(&flow, Some("acct-c"), 20.0, 60).await;
    assert_eq!(
        in_use(&flow).await,
        (vec![Some("acct-c".into())], Some(20.0))
    );
    assert_eq!(published_in_use(&flow).await, vec![Some("acct-c".into())]);

    // Its switch on again hands the choice back to the order: these resets are 23 hours
    // apart, so the account nearer its weekly limit keeps taking work.
    let mut settings = flow.core.settings();
    settings.manual_accounts.clear();
    flow.core.update_settings(settings).await.unwrap();
    flow.manager.runtime.sync_accounts().await;
    assert_eq!(
        in_use(&flow).await,
        (vec![Some("acct-c".into())], Some(20.0))
    );
    // A reset more than a day later gives way to the earlier weekly window.
    report_weekly(&flow, Some("acct-c"), 20.0, 62).await;
    assert_eq!(in_use(&flow).await, (vec![None], Some(7.0)));

    // The own login renamed is published by its name (its switch untouched); cleared, it goes
    // by its email again.
    let rename = |name: &str| {
        let before = flow.core.settings();
        let mut settings = before.clone();
        settings.own_account_names = vec![crate::model::OwnAccountName {
            provider: ProviderKind::Claude,
            name: name.into(),
        }];
        crate::accounts::edited(&before, settings)
    };
    flow.core
        .update_settings(rename(" Personal "))
        .await
        .unwrap();
    flow.manager.runtime.publish_accounts().await;
    assert_eq!(published_names(&flow).await[0], "Personal");
    assert!(flow.core.settings().manual_accounts.is_empty());
    flow.core.update_settings(rename("")).await.unwrap();
    flow.manager.runtime.publish_accounts().await;
    assert_eq!(published_names(&flow).await[0], "");
    flow.stop().await;
}

/// The names the last published Settings → Accounts gives Claude's accounts.
async fn published_names(flow: &Flow) -> Vec<String> {
    let page = flow
        .core
        .store()
        .read_stream(
            crate::model::streams::ACCOUNTS.into(),
            brigadier_store::StreamPage {
                limit: 1,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    match page
        .first()
        .map(|event| crate::sessions::decode(event).unwrap())
    {
        Some(DomainEvent::AccountsChecked { accounts }) => accounts
            .accounts
            .into_iter()
            .filter(|view| view.account.provider == ProviderKind::Claude)
            .map(|view| view.name)
            .collect(),
        _ => Vec::new(),
    }
}

#[tokio::test]
async fn a_limit_waits_on_its_own_agent() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-agents-off",
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
    // Accounts still switch (the agent switch is only about agents); with both at their
    // limit, Codex isn't asked: the message waits for Claude's reset.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    let told = loop {
        let told = moments(&flow).await;
        if told
            .iter()
            .any(|notice| notice.contains("Your message waits"))
        {
            break told;
        }
        assert!(tokio::time::Instant::now() < deadline, "no wait: {told:#?}");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    };
    assert!(
        !told.iter().any(|notice| notice.contains("then back on")),
        "{told:#?}"
    );
    let mut accounts: Vec<_> = seen(&log)
        .iter()
        .map(|turn| (turn.provider, turn.account.clone()))
        .collect();
    accounts.dedup();
    assert_eq!(
        accounts,
        [
            (ProviderKind::Claude, None),
            (ProviderKind::Claude, Some("acct-b".into()))
        ]
    );
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
                permission: Default::default(),
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
    tokio::time::timeout(*super::PATIENCE, async {
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
                // Its CLI session files are its record, held apart (S8).
                let lineage = crate::ledger::lineage_owner(&chat.0);
                let ledger = flow.manager.runtime.ledger();
                let artifacts = ledger.artifacts(&lineage);
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
                if !delete {
                    // Archive keeps the CLI's session files, in every home.
                    let held = ledger.artifacts(&lineage);
                    assert!(native.iter().all(|artifact| held.contains(artifact)));
                    assert!(
                        removals
                            .iter()
                            .all(|(_, _, batch)| native.iter().all(|a| !batch.contains(a))),
                        "{removals:?}"
                    );
                    flow.stop().await;
                    continue;
                }
                assert!(ledger.artifacts(&lineage).is_empty(), "Delete removes them");
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
        let owner = crate::ledger::lineage_owner(&chat.0);
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
                brigadier_providers::Artifact::ClaudeSession {
                    session_id, home, ..
                } => (session_id, home),
                brigadier_providers::Artifact::CodexThread {
                    thread_id, home, ..
                } => (thread_id, home),
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
        flow.stop().await;
    }
}

/// Connect's sign-in hands over the status that says it is signed in: the provider shows as
/// signed in at once, its models kept, before any check runs.
#[tokio::test]
async fn a_status_from_a_sign_in_shows_at_once() {
    let provider = ProviderKind::Claude;
    let flow = Flow::start(
        "accounts-noted-status",
        Options {
            behavior: Arc::new(super::FakeBehavior {
                signed_out: Mutex::default(),
                ..Default::default()
            }),
            ..Options::default()
        },
        logging(&Log::default(), |_| Reply::text("ok")),
    )
    .await;
    flow.behavior.signed_out.lock().unwrap().push(provider);
    flow.manager.runtime.refresh_providers(Some(provider));
    flow.until("the own login is checked as signed out", |_| {
        !flow.manager.runtime.provider_signed_in(provider)
    })
    .await;
    let before = flow.manager.runtime.overview(provider).unwrap();
    let mut status = before.status.clone().unwrap();
    status.logged_in = true;
    flow.manager.runtime.note_status(status.clone()).await;
    let after = flow.manager.runtime.overview(provider).unwrap();
    assert_eq!(after.status, Some(status));
    assert_eq!(after.models, before.models);
    assert!(flow.manager.runtime.provider_signed_in(provider));
    flow.stop().await;
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

/// Holds the end of a limited turn of `chat` before its hand-over is chosen: the first is
/// notified once a turn is held there, the second lets it go on.
fn hold_hand_over(
    flow: &Flow,
    chat: &ConversationId,
) -> (Arc<tokio::sync::Notify>, Arc<tokio::sync::Notify>) {
    let (reached, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    *flow
        .manager
        .conv(chat)
        .unwrap()
        .hand_over_pause
        .lock()
        .unwrap() = Some((reached.clone(), release.clone()));
    (reached, release)
}

async fn send(flow: &Flow, chat: &ConversationId, text: &str) {
    flow.manager
        .send_message(chat.clone(), text.into(), vec![], vec![], false, None)
        .await
        .unwrap();
}

/// Each request of `chat` by its first message's text, with its state.
async fn request_states(flow: &Flow, chat: &ConversationId) -> Vec<(String, RequestState)> {
    let board = flow.core.board(chat).await.unwrap();
    let messages = flow.core.all_messages(chat).await.unwrap();
    let mut states: Vec<_> = board
        .requests
        .values()
        .map(|request| {
            let text = messages
                .iter()
                .find(|message| message.request_id.as_deref() == Some(request.id.as_str()))
                .map(|message| message.text.clone())
                .unwrap_or_default();
            (text, request.state.clone())
        })
        .collect();
    states.sort_by(|a, b| a.0.cmp(&b.0));
    states
}

/// From the end of a turn cut short by a limit until its hand-over is chosen, its request
/// still works, whatever settles the requests meanwhile. No turn starts on the CLI at its limit
/// then, nor a compaction: a message sent meanwhile goes with the hand-over.
#[tokio::test]
async fn a_limited_turn_works_through_its_hand_over_and_nothing_starts_on_its_cli() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-hand-over",
        Options::default(),
        logging(&log, |turn| match &turn.account {
            None if turn.input.contains("Hello.") => Reply::text("Hi."),
            None => Reply::limited(),
            Some(_) => Reply::text("Done."),
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let chat = chat_on(&flow, ProviderKind::Claude, None).await;
    // Something to compact.
    say_to(&flow, &chat, "Hello.").await;
    let (reached, release) = hold_hand_over(&flow, &chat);
    send(&flow, &chat, "First.").await;
    reached.notified().await;
    // Another source settles the requests (a card answer, a worker's report).
    flow.manager.settle_requests(&chat).await;
    assert_eq!(
        request_states(&flow, &chat).await,
        vec![
            ("First.".to_owned(), RequestState::Working),
            ("Hello.".to_owned(), RequestState::Done)
        ]
    );
    let compacted = flow
        .manager
        .compact(chat.clone())
        .await
        .expect_err("no compaction during the hand-over");
    assert!(
        compacted
            .to_string()
            .contains("wait until the reply is done"),
        "{compacted}"
    );
    send(&flow, &chat, "Second.").await;
    let conv = flow.manager.conv(&chat).unwrap();
    flow.manager.next_turn_now(&conv).await;
    assert!(
        !conv.turn_running().await,
        "no turn on the CLI at its limit"
    );
    // Only this hand-over is held: a later limited turn would go on.
    *flow
        .manager
        .conv(&chat)
        .unwrap()
        .hand_over_pause
        .lock()
        .unwrap() = None;
    release.notify_one();
    super::eventually_async("both requests done", || async {
        request_states(&flow, &chat)
            .await
            .iter()
            .all(|(_, state)| *state == RequestState::Done)
    })
    .await;
    let turns = seen(&log);
    let accounts: Vec<_> = turns.iter().map(|turn| turn.account.as_deref()).collect();
    assert_eq!(accounts, [None, None, Some("acct-b")], "{turns:#?}");
    assert!(turns[2].input.contains("Second."), "{turns:#?}");
    assert_eq!(request_states(&flow, &chat).await.len(), 3);
    flow.stop().await;
}

/// The user's Stop while a limited turn is handed over ends its request Stopped, and its
/// messages don't go on their own on the other account; Resume continues it there, as after
/// any Stop.
#[tokio::test]
async fn a_stop_during_a_hand_over_ends_the_request_and_nothing_goes_on_its_own() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-hand-over-stop",
        Options::default(),
        logging(&log, |turn| match &turn.account {
            None => Reply::limited(),
            Some(_) => Reply::text("Done."),
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let chat = chat_on(&flow, ProviderKind::Claude, None).await;
    let (reached, release) = hold_hand_over(&flow, &chat);
    send(&flow, &chat, "First.").await;
    reached.notified().await;
    flow.manager.interrupt(chat.clone()).await.unwrap();
    assert_eq!(
        request_states(&flow, &chat).await,
        vec![("First.".to_owned(), RequestState::Stopped)]
    );
    release.notify_one();
    // The chat moved to the other account, and nothing waits or runs.
    let conv = flow.manager.conv(&chat).unwrap();
    super::eventually_async("the hand-over to end", || async {
        let moved = match flow.core.conversation(&chat).unwrap().setup {
            Some(Setup::Chat { model, .. }) => model.account.as_deref() == Some("acct-b"),
            _ => false,
        };
        moved && !conv.is_busy().await && idle(&flow, &chat).await
    })
    .await;
    assert_eq!(seen(&log).len(), 1, "{:#?}", seen(&log));
    assert_eq!(
        request_states(&flow, &chat).await,
        vec![("First.".to_owned(), RequestState::Stopped)]
    );
    flow.manager.resume(chat.clone()).await.unwrap();
    super::eventually_async("the resumed request done", || async {
        request_states(&flow, &chat).await == vec![("First.".to_owned(), RequestState::Done)]
    })
    .await;
    let turns = seen(&log);
    assert_eq!(turns.len(), 2, "{turns:#?}");
    assert_eq!(turns[1].account.as_deref(), Some("acct-b"));
    flow.stop().await;
}

/// Whether `chat` no longer shows working.
async fn idle(flow: &Flow, chat: &ConversationId) -> bool {
    flow.core.board(chat).await.unwrap().run == crate::work::RunState::Idle
}

/// The same Stop while a limited turn would wait for quota (no other account or model can take
/// it): nothing waits, and a Resume sent before the hand-over ended goes on its own.
#[tokio::test]
async fn a_resume_after_a_stop_during_a_quota_hand_over_goes() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-hand-over-stop-wait",
        Options::default(),
        logging(&log, |turn| {
            if turn.input.contains("asked you to resume") {
                Reply::text("Done.")
            } else {
                Reply::limited()
            }
        }),
    )
    .await;
    // Codex at its limit too: nothing can stand in.
    let hour = brigadier_providers::LimitHit {
        kind: brigadier_providers::LimitKind::UsageWindow,
        window: Some("five_hour".into()),
        resets_at_ms: Some(crate::now_ms() + 60 * 60 * 1000),
    };
    flow.manager
        .runtime
        .note_limit(&crate::accounts::AccountRef::own(ProviderKind::Codex), hour)
        .await;
    let chat = chat_on(&flow, ProviderKind::Claude, None).await;
    let (reached, release) = hold_hand_over(&flow, &chat);
    send(&flow, &chat, "First.").await;
    reached.notified().await;
    flow.manager.interrupt(chat.clone()).await.unwrap();
    flow.manager.resume(chat.clone()).await.unwrap();
    release.notify_one();
    super::eventually_async("the resumed request done", || async {
        request_states(&flow, &chat).await == vec![("First.".to_owned(), RequestState::Done)]
    })
    .await;
    let turns = seen(&log);
    assert_eq!(turns.len(), 2, "{turns:#?}");
    assert_eq!(turns[1].provider, ProviderKind::Claude, "{turns:#?}");
    flow.stop().await;
}

/// A chat whose next CLI start is held after "Again." passed its fence, with its cleanups
/// waiting for nothing: what a start that hangs past the drain wait looks like. The second
/// notify lets the start go on.
async fn held_late_start(
    name: &str,
    log: &Log,
) -> (Flow, ConversationId, Arc<tokio::sync::Notify>) {
    let flow = Flow::start(
        name,
        Options {
            behavior: Arc::new(super::FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            ..Options::default()
        },
        logging(log, |_| Reply::text("Done.")),
    )
    .await;
    let chat = chat_on(&flow, ProviderKind::Claude, None).await;
    say_to(&flow, &chat, "Hello.").await;
    // The next message starts a CLI.
    flow.manager.conv(&chat).unwrap().close_cli().await;
    let (reached, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    *flow.behavior.hold_start.lock().unwrap() = Some((reached.clone(), release.clone()));
    *flow.manager.closing.drain_wait.lock().unwrap() = Some(std::time::Duration::ZERO);
    send(&flow, &chat, "Again.").await;
    reached.notified().await;
    (flow, chat, release)
}

fn native_artifacts(flow: &Flow, chat: &ConversationId) -> Vec<brigadier_providers::Artifact> {
    flow.manager
        .runtime
        .ledger()
        .artifacts(&crate::ledger::lineage_owner(&chat.0))
        .into_iter()
        .filter(|artifact| {
            matches!(
                artifact,
                brigadier_providers::Artifact::ClaudeSession { .. }
            )
        })
        .collect()
}

/// An archive that stopped waiting for a start keeps its cleanup marked as not done, and
/// cleans again once that start has ended: nothing it recorded is left behind, and its turn
/// never runs.
#[tokio::test]
async fn an_archive_cleans_up_after_a_start_that_outlasted_it() {
    let log = Log::default();
    let (flow, chat, release) = held_late_start("late-start-archive", &log).await;
    flow.manager.archive(chat.clone()).await.unwrap();
    flow.manager.cleanup_finished(&chat).await;
    assert!(
        flow.core.conversation(&chat).unwrap().cleanup_pending,
        "the cleanup is not done while the start goes on"
    );
    release.notify_one();
    flow.manager.drained(&chat).await;
    super::eventually_async("the archive's cleanup to finish", || async {
        !flow.core.conversation(&chat).unwrap().cleanup_pending
    })
    .await;
    // The late start resumed the chat's session: its files stay with the archive (S8).
    assert_eq!(native_artifacts(&flow, &chat).len(), 1);
    assert!(
        flow.manager
            .runtime
            .ledger()
            .artifacts(&format!("chat:{chat}"))
            .is_empty()
    );
    assert!(seen(&log).iter().all(|turn| !turn.input.contains("Again.")));
    flow.stop().await;
}

/// A start admitted before an archive ends, with what it recorded, even when the chat was
/// restored before it got through: its turn never resumes, and the restored chat's own next
/// session stays.
#[tokio::test]
async fn a_start_cut_off_by_an_archive_ends_even_after_a_restore() {
    let log = Log::default();
    let (flow, chat, release) = held_late_start("late-start-restore", &log).await;
    flow.manager.archive(chat.clone()).await.unwrap();
    flow.manager.cleanup_finished(&chat).await;
    flow.manager.restore(chat.clone()).await.unwrap();
    release.notify_one();
    flow.manager.drained(&chat).await;
    assert_eq!(
        native_artifacts(&flow, &chat).len(),
        1,
        "the late start resumed the chat's session, which the archive keeps (S8)"
    );
    super::eventually_async("the archive's mark to clear", || async {
        !flow.core.conversation(&chat).unwrap().cleanup_pending
    })
    .await;
    assert!(seen(&log).iter().all(|turn| !turn.input.contains("Again.")));
    send(&flow, &chat, "After.").await;
    super::eventually_async("the restored chat to answer", || async {
        request_states(&flow, &chat)
            .await
            .contains(&("After.".to_owned(), RequestState::Done))
    })
    .await;
    // The restored chat resumes its session; "Again." never was a turn of its own.
    let turns = seen(&log);
    assert_eq!(turns.len(), 2, "{turns:#?}");
    let after = &turns[1];
    assert!(after.input.ends_with("After."), "{turns:#?}");
    assert_eq!(after.native_id, turns[0].native_id, "Restore resumes it");
    assert_eq!(
        native_artifacts(&flow, &chat),
        vec![brigadier_providers::Artifact::ClaudeSession {
            session_id: after.native_id.clone(),
            home: None,
            cwd: None,
        }]
    );
    flow.stop().await;
}

/// A restored chat that started its own CLI while a start cut off by the archive was still
/// going keeps its tools when that start ends: only what the late start made goes.
#[tokio::test]
async fn a_start_cut_off_by_an_archive_leaves_the_restored_chats_cli_alone() {
    // Each turn's input and the grant its CLI's tools run under.
    let turns: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let script: Script = {
        let turns = turns.clone();
        Arc::new(move |turn: Turn| {
            turns
                .lock()
                .unwrap()
                .push((turn.input.clone(), turn.grant.clone()));
            Box::pin(async { Reply::text("Done.") })
        })
    };
    let flow = Flow::start(
        "late-start-restored-cli",
        Options {
            behavior: Arc::new(super::FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            ..Options::default()
        },
        script,
    )
    .await;
    let chat = chat_on(&flow, ProviderKind::Claude, None).await;
    say_to(&flow, &chat, "Hello.").await;
    flow.manager.conv(&chat).unwrap().close_cli().await;
    let (reached, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    *flow.behavior.hold_start.lock().unwrap() = Some((reached.clone(), release.clone()));
    *flow.manager.closing.drain_wait.lock().unwrap() = Some(std::time::Duration::ZERO);
    send(&flow, &chat, "Again.").await;
    reached.notified().await;
    flow.manager.archive(chat.clone()).await.unwrap();
    flow.manager.cleanup_finished(&chat).await;
    flow.manager.restore(chat.clone()).await.unwrap();
    // The restored chat's own CLI, while the late start still goes.
    say_to(&flow, &chat, "After.").await;
    let grant = {
        let turns = turns.lock().unwrap();
        let (input, grant) = turns.last().unwrap();
        assert!(input.ends_with("After."), "{turns:#?}");
        grant.clone()
    };
    release.notify_one();
    flow.manager.drained(&chat).await;
    super::eventually_async("the archive's mark to clear", || async {
        !flow.core.conversation(&chat).unwrap().cleanup_pending
    })
    .await;
    assert!(
        flow.manager.grants().resolve(&grant).is_some(),
        "the restored chat's CLI keeps its tools"
    );
    let live = native_artifacts(&flow, &chat);
    assert_eq!(live.len(), 1, "the restored chat's session stays: {live:?}");
    assert!(
        turns
            .lock()
            .unwrap()
            .iter()
            .all(|(input, _)| !input.ends_with("Again.")),
        "the late start's turn never ran"
    );
    flow.stop().await;
}

/// A side chat going with its parent while a start of its own outlasts the wait is marked as
/// being deleted, so a quit before that start ends leaves the delete to the next launch; it
/// goes once the start has ended.
#[tokio::test]
async fn a_side_chat_whose_start_outlasts_its_parents_delete_stays_marked_until_it_goes() {
    let log = Log::default();
    let flow = Flow::start(
        "late-start-side-chat",
        Options {
            behavior: Arc::new(super::FakeBehavior {
                cleanup: true,
                ..Default::default()
            }),
            ..Options::default()
        },
        logging(&log, |_| Reply::text("Done.")),
    )
    .await;
    let parent = chat_on(&flow, ProviderKind::Claude, None).await;
    say_to(&flow, &parent, "Hello.").await;
    let side = flow.manager.open_side_chat(&parent, None).await.unwrap().id;
    say_to(&flow, &side, "Side hello.").await;
    flow.manager.conv(&side).unwrap().close_cli().await;
    let (reached, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    *flow.behavior.hold_start.lock().unwrap() = Some((reached.clone(), release.clone()));
    *flow.manager.closing.drain_wait.lock().unwrap() = Some(std::time::Duration::ZERO);
    send(&flow, &side, "Side again.").await;
    reached.notified().await;
    flow.manager.delete(parent.clone()).await.unwrap();
    flow.manager.cleanup_finished(&parent).await;
    assert!(flow.core.conversation(&parent).is_err(), "the parent went");
    assert!(
        flow.core.conversation(&side).is_ok_and(|now| now.deleting),
        "still there, marked, while its start goes on"
    );
    release.notify_one();
    flow.manager.drained(&side).await;
    super::eventually("the side chat to go", || {
        flow.core.conversation(&side).is_err()
    })
    .await;
    flow.manager.cleanup_finished(&side).await;
    assert!(
        flow.manager
            .runtime
            .ledger()
            .artifacts(&format!("chat:{side}"))
            .is_empty()
    );
    assert!(
        seen(&log)
            .iter()
            .all(|turn| !turn.input.contains("Side again."))
    );
    flow.stop().await;
}

/// A delete that stopped waiting for a start deletes the chat only once that start has
/// ended, so nothing it recorded outlives the chat.
#[tokio::test]
async fn a_delete_waits_for_a_start_that_outlasted_it() {
    let log = Log::default();
    let (flow, chat, release) = held_late_start("late-start-delete", &log).await;
    flow.manager.delete(chat.clone()).await.unwrap();
    flow.manager.cleanup_finished(&chat).await;
    assert!(
        flow.core.conversation(&chat).is_ok_and(|now| now.deleting),
        "still there, marked, while the start goes on"
    );
    release.notify_one();
    flow.manager.drained(&chat).await;
    super::eventually("the chat to go", || flow.core.conversation(&chat).is_err()).await;
    flow.manager.cleanup_finished(&chat).await;
    assert!(
        flow.manager
            .runtime
            .ledger()
            .artifacts(&format!("chat:{chat}"))
            .is_empty()
    );
    assert!(seen(&log).iter().all(|turn| !turn.input.contains("Again.")));
    flow.stop().await;
}

#[tokio::test]
async fn weekly_quota_about_to_be_lost_is_told_once_and_nothing_starts() {
    let log = Log::default();
    let mut flow = Flow::start(
        "accounts-lost-quota",
        Options::default(),
        logging(&log, |_| Reply::text("Noted.")),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    // 90% of acct-b's week left, 9 hours before it resets; the user's own login has days.
    flow.weekly_resets_in(ProviderKind::Claude, None, 90);
    flow.weekly_resets_in(ProviderKind::Claude, Some("acct-b"), 9);
    flow.manager.runtime.warn_of_lost_quota().await;
    flow.manager.runtime.warn_of_lost_quota().await;
    let told = |flow: &Flow| {
        let store = flow.core.store().clone();
        async move {
            let page = store
                .read_stream(
                    crate::model::streams::QUOTA_WARNINGS.into(),
                    brigadier_store::StreamPage {
                        limit: 500,
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
            page.iter()
                .filter_map(|event| match crate::sessions::decode(event).unwrap() {
                    DomainEvent::QuotaWillBeLost { warning } => Some(warning),
                    _ => None,
                })
                .collect::<Vec<_>>()
        }
    };
    let warned = told(&flow).await;
    assert_eq!(warned.len(), 1, "{warned:#?}");
    assert_eq!(warned[0].who, "Account acct-b (Claude Code)");
    assert_eq!(warned[0].left_percent, 90.0);
    // The Usage menu's line, until the reset.
    assert_eq!(flow.manager.runtime.accounts_view().warnings, warned);
    // A restart doesn't warn again of the same reset.
    flow.restart().await;
    flow.weekly_resets_in(ProviderKind::Claude, Some("acct-b"), 9);
    flow.manager.runtime.warn_of_lost_quota().await;
    assert_eq!(told(&flow).await.len(), 1);
    // Nothing started on its own.
    assert!(seen(&log).is_empty());
    flow.stop().await;
}

/// Notes `account`'s quota as a live report would: its 5-hour window `five_hour` % used and
/// resetting in `five_hour_min` minutes, its weekly one `weekly` % used and resetting in
/// `weekly_hours`.
async fn report_usage(
    flow: &Flow,
    account: Option<&str>,
    (five_hour, five_hour_min): (f64, i64),
    (weekly, weekly_hours): (f64, i64),
) {
    use brigadier_providers::{QuotaSnapshot, QuotaSource, QuotaWindow};

    let now = crate::now_ms();
    let window =
        |id: &str, label: &str, minutes: i64, used_percent: f64, resets_at_ms: i64| QuotaWindow {
            id: id.into(),
            label: label.into(),
            used_percent,
            resets_at_ms: Some(resets_at_ms),
            window_minutes: Some(minutes),
            bucket: None,
            model: None,
        };
    let provider = ProviderKind::Claude;
    flow.manager
        .runtime
        .note_quota_snapshot(
            &crate::accounts::AccountRef::new(provider, account.map(Into::into)),
            QuotaSnapshot {
                provider,
                windows: vec![
                    window(
                        "five_hour",
                        "5-hour",
                        300,
                        five_hour,
                        now + five_hour_min * 60_000,
                    ),
                    window(
                        "seven_day",
                        "Weekly",
                        7 * 24 * 60,
                        weekly,
                        now + weekly_hours * 3_600_000,
                    ),
                ],
                limit: None,
                observed_at_ms: now,
                source: QuotaSource::Event,
            },
        )
        .await;
}

/// S38 (D23): two Claude accounts whose weeks end the same day; the one nearer its limit
/// (its 5-hour window) takes the work first. Its limit moves the session to the other, the
/// same CLI session, with one "carry on" line; with both out, the message waits for the reset
/// (no provider fallback).
#[tokio::test]
async fn the_account_nearest_its_limit_works_first_and_both_out_waits() {
    let log = Log::default();
    let own_out = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let out = own_out.clone();
    let flow = Flow::start(
        "accounts-order",
        Options {
            behavior: Arc::new(super::FakeBehavior {
                session_file: true,
                ..super::FakeBehavior::default()
            }),
            ..Options::default()
        },
        logging(&log, move |turn| match turn.account.as_deref() {
            Some("acct-b") => Reply::limited(),
            _ if out.load(std::sync::atomic::Ordering::SeqCst) => Reply::limited(),
            _ => Reply::text("Done."),
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    report_usage(&flow, None, (20.0, 240), (30.0, 30)).await;
    report_usage(&flow, Some("acct-b"), (85.0, 120), (30.0, 28)).await;
    let view = flow.manager.runtime.accounts_view();
    let places: Vec<_> = view
        .accounts
        .iter()
        .filter(|view| view.account.provider == ProviderKind::Claude)
        .map(|view| (view.account.account.clone(), view.place))
        .collect();
    assert_eq!(places, [(None, Some(2)), (Some("acct-b".into()), Some(1))]);
    assert_eq!(
        view.accounts[1].reason,
        Some(crate::accounts::OrderReason::NearerLimit {
            window: "5-hour".into(),
            used_percent: 85.0
        })
    );

    flow.say("Do the thing.").await;
    flow.until("a second turn", |_| seen(&log).len() >= 2).await;
    flow.settled().await;
    let turns = seen(&log);
    assert_eq!(turns[0].account.as_deref(), Some("acct-b"));
    assert_eq!(turns[1].account, None);
    assert_eq!(turns[1].native_id, turns[0].native_id);
    assert_eq!(
        turns
            .iter()
            .filter(|turn| turn.input.contains(CONTINUE_ON_ACCOUNT))
            .count(),
        1
    );
    assert_eq!(
        moments(&flow).await,
        ["Claude Code hit its usage limit on Account acct-b; continuing on this computer's login."]
    );

    // Both out: the persisted wait and the CLI's limit supply the Resumes at card.
    assert!(!flow.core.settings().switch_providers);
    own_out.store(true, std::sync::atomic::Ordering::SeqCst);
    flow.say("Next.").await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while !moments(&flow)
        .await
        .iter()
        .any(|text| text.contains("Your message waits"))
    {
        assert!(tokio::time::Instant::now() < deadline, "no wait");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let conversation = flow.core.conversation(&flow.conversation).unwrap();
    let wait = conversation.quota_wait.expect("the Resumes at card's wait");
    assert_eq!(
        wait.limit_kind,
        Some(brigadier_providers::LimitKind::UsageWindow)
    );
    assert!(wait.resets_at_ms.is_some_and(|at| at > crate::now_ms()));
    assert_eq!(wait.model.as_ref().unwrap().provider, ProviderKind::Claude);
    assert!(wait.request.is_some());
    let thread = flow.manager.thread_view(&flow.conversation).unwrap();
    let limit = thread
        .turns
        .last()
        .and_then(|turn| turn.limit.as_ref())
        .expect("the card's limit");
    assert_eq!(limit.window.as_deref(), Some("five_hour"));
    assert_eq!(limit.kind, brigadier_providers::LimitKind::UsageWindow);
    assert!(
        flow.core
            .conversation_view(flow.conversation.clone(), 1)
            .await
            .unwrap()
            .conversation
            .quota_wait
            .is_some()
    );
    assert!(
        seen(&log)
            .iter()
            .all(|turn| turn.provider == ProviderKind::Claude),
        "{:#?}",
        seen(&log)
    );
    flow.stop().await;
}

/// S38 (D23): Use picks another account while a chat runs: its next request runs there, the
/// same CLI session resumed, and the thread says so once.
#[tokio::test]
async fn use_moves_a_running_session_to_the_picked_account_at_its_next_request() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-pick-now",
        Options::default(),
        logging(&log, |_| Reply::text("Done.")),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    report_usage(&flow, None, (50.0, 120), (30.0, 30)).await;
    report_usage(&flow, Some("acct-b"), (10.0, 120), (30.0, 30)).await;
    flow.say("First.").await;
    flow.settled().await;
    assert_eq!(seen(&log)[0].account, None);

    let before = flow.core.settings();
    let mut settings = before.clone();
    for entry in &mut settings.accounts {
        entry.default = entry.id == "acct-b";
    }
    let settings = crate::accounts::edited(&before, settings);
    assert!(!settings.switches_accounts(ProviderKind::Claude));
    flow.core.update_settings(settings).await.unwrap();
    flow.manager.runtime.sync_accounts().await;

    flow.say("Second.").await;
    flow.until("the next turn", |_| seen(&log).len() >= 2).await;
    flow.settled().await;
    let turns = seen(&log);
    assert_eq!(turns[1].account.as_deref(), Some("acct-b"));
    assert_eq!(turns[1].native_id, turns[0].native_id);
    assert_eq!(orchestrator(&flow).account.as_deref(), Some("acct-b"));
    assert_eq!(
        moments(&flow).await,
        ["Claude Code continues on Account acct-b, picked in Settings."]
    );
    // A third request stays there, with nothing more said.
    flow.say("Third.").await;
    flow.until("the third turn", |_| seen(&log).len() >= 3)
        .await;
    flow.settled().await;
    assert_eq!(seen(&log)[2].account.as_deref(), Some("acct-b"));
    assert_eq!(moments(&flow).await.len(), 1);
    flow.stop().await;
}

/// S38 (D23): every Claude account at its limit with "Switch to another agent automatically"
/// on: the session's generation ends with no note, and a successor on Codex carries on from
/// the hand-off with the message, in the same thread; the divider says why.
#[tokio::test]
async fn with_every_account_out_and_the_switch_on_the_session_seams_to_another_agent() {
    use crate::manager::seam::{Phase, read_seam};

    let log = Log::default();
    let flow = Flow::start(
        "accounts-agent-fallback",
        Options::default(),
        logging(&log, |turn| match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Done on Codex."),
        }),
    )
    .await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    flow.core.update_settings(settings).await.unwrap();

    flow.say("Do the thing.").await;
    flow.until("a turn on Codex", |_| {
        seen(&log)
            .iter()
            .any(|turn| turn.provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let turns = seen(&log);
    let path: Vec<_> = turns
        .iter()
        .map(|turn| (turn.provider, turn.account.clone()))
        .collect();
    assert_eq!(
        path,
        [
            (ProviderKind::Claude, None),
            (ProviderKind::Claude, Some("acct-b".into())),
            (ProviderKind::Codex, None),
        ]
    );
    // A fresh session: the successor starts from the hand-off, told to check git first, with
    // the message that hit the limit.
    let codex = &turns[2];
    assert_ne!(codex.native_id, turns[1].native_id);
    assert!(
        codex.input.starts_with(crate::thread_view::HANDOFF_MARK),
        "{}",
        codex.input
    );
    assert!(
        codex.input.contains("check `git status`"),
        "{}",
        codex.input
    );
    assert!(codex.input.contains("Do the thing."), "{}", codex.input);
    // The session runs on Codex now, and its seam says why.
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Codex);
    let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
    assert!(matches!(file.phase, Phase::Live), "{:?}", file.phase);
    assert_eq!(file.seams.len(), 1);
    assert_eq!(file.seams[0].provider, ProviderKind::Codex);
    assert_eq!(
        file.seams[0].end.death.as_deref(),
        Some("usage limit on Claude Code · continued on Codex")
    );
    flow.stop().await;
}

/// S38: a limit while writing the hand-off preserves the note already written, and the
/// fallback successor reads it. The preceding successful turn rules out a first-turn death.
#[tokio::test]
async fn the_agent_fallback_keeps_a_written_handoff_note() {
    use crate::manager::seam::{Phase, SeamConfig, read_seam};
    use crate::thread_view::{HANDOFF_MARK, NUDGE_MARK};
    const NOTE: &str = "Doing now: update the parser.\nNext: check the edge case N-542.";
    let log = Log::default();
    let inputs = log.clone();
    let flow = Flow::start(
        "20261022-0805-w181-fallback-note",
        Options {
            behavior: Arc::new(super::FakeBehavior {
                session_file: true,
                take_in_steers: true,
                ..super::FakeBehavior::default()
            }),
            ..Options::default()
        },
        Arc::new(move |turn| {
            let inputs = inputs.clone();
            Box::pin(async move {
                inputs.lock().unwrap().push(Seen {
                    provider: turn.provider,
                    account: turn.account.clone(),
                    native_id: turn.native_id.clone(),
                    input: turn.input.clone(),
                });
                if turn.provider == ProviderKind::Codex {
                    return Reply::text("Continued from the note.");
                }
                if turn.input == "Grow and hand off." {
                    turn.report_context(70_000).await;
                    let nudge = turn.steered().await.expect("the seam asks for the note");
                    assert!(nudge.starts_with(NUDGE_MARK), "{nudge}");
                    let path = nudge
                        .split("write the hand-off note at ")
                        .nth(1)
                        .unwrap()
                        .split(": doing now")
                        .next()
                        .unwrap();
                    std::fs::write(path, NOTE).unwrap();
                    return Reply::limited();
                }
                Reply::text("Ready.")
            })
        }),
    )
    .await;
    flow.manager.set_seam(SeamConfig {
        tokens: 60_000,
        hold: std::time::Duration::from_secs(60),
        long: None,
        tokens_set: false,
    });
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Get ready.").await;
    flow.settled().await;
    flow.say("Grow and hand off.").await;
    flow.until("the fallback reads the note", |_| {
        seen(&log)
            .iter()
            .any(|turn| turn.provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let turns = seen(&log);
    assert_eq!(turns[0].native_id, turns[1].native_id);
    let successor = turns
        .iter()
        .find(|turn| turn.provider == ProviderKind::Codex)
        .unwrap();
    assert_ne!(successor.native_id, turns[1].native_id);
    assert!(
        successor.input.starts_with(HANDOFF_MARK),
        "{}",
        successor.input
    );
    assert!(
        successor
            .input
            .contains(&format!("## Hand-off note\n{NOTE}")),
        "{}",
        successor.input
    );
    let file = read_seam(&flow.manager.session_dir(&flow.conversation).record());
    assert!(matches!(file.phase, Phase::Live));
    assert_eq!(file.seams.len(), 1);
    assert_eq!(file.seams[0].provider, ProviderKind::Codex);
    assert_eq!(
        std::fs::read_to_string(file.seams[0].note.as_ref().unwrap()).unwrap(),
        NOTE
    );
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    assert_eq!(view.seams.len(), 1, "the divider survives a reload");
    assert!(view.seams[0].note.is_some());
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Codex);
    flow.stop().await;
}

/// S38: the user's rules bind the fallback: with no model of the other agent allowed, the
/// session waits for its reset instead.
#[tokio::test]
async fn the_users_rules_bind_the_agent_fallback() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-agent-fallback-rules",
        Options::default(),
        logging(&log, |turn| match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Done on Codex."),
        }),
    )
    .await;
    std::fs::write(
        flow.manager.models_rules_path(),
        "# My rules\n\n- allow codex: gpt-7\n",
    )
    .unwrap();
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    flow.core.update_settings(settings).await.unwrap();

    flow.say("Do the thing.").await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while !moments(&flow)
        .await
        .iter()
        .any(|text| text.contains("Your message waits"))
    {
        assert!(tokio::time::Instant::now() < deadline, "no wait");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        seen(&log)
            .iter()
            .all(|turn| turn.provider == ProviderKind::Claude),
        "{:?}",
        seen(&log)
    );
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Claude);
    flow.stop().await;
}

/// S38, D34: the fallback takes the first `Fallback` row of the other agent the user's rules
/// allow, in the routing's order: its first row's effort is over their ceiling, so the next
/// row's model and effort go on, never the CLI's own default.
#[tokio::test]
async fn the_agent_fallback_takes_the_next_fallback_row_the_rules_allow() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-agent-fallback-allowed",
        Options::default(),
        logging(&log, |turn| match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Done on Codex."),
        }),
    )
    .await;
    std::fs::write(
        flow.manager.models_rules_path(),
        "# My rules\n\n- effort-ceiling: high\n\n\
         | Work | Agent | Model | Effort | Reason |\n|---|---|---|---|---|\n\
         | Fallback | codex | default | xhigh | Mine. |\n\
         | Fallback | codex | gpt-test | medium | Then this. |\n",
    )
    .unwrap();
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    flow.core.update_settings(settings).await.unwrap();

    flow.say("Do the thing.").await;
    flow.until("a turn on Codex", |_| {
        seen(&log)
            .iter()
            .any(|turn| turn.provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let on = orchestrator(&flow);
    assert_eq!(on.provider, ProviderKind::Codex);
    assert_eq!(
        (on.model.as_deref(), on.effort.as_deref()),
        (Some("gpt-test"), Some("medium")),
        "{on:?}"
    );
    flow.stop().await;
}

/// D34: with no `Fallback` row of another agent the user's rules allow, a session at its limit
/// waits for the reset rather than moving on a model no row names.
#[tokio::test]
async fn with_no_fallback_row_the_rules_allow_a_session_waits_for_the_reset() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-agent-fallback-none",
        Options::default(),
        logging(&log, |turn| match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Done on Codex."),
        }),
    )
    .await;
    std::fs::write(
        flow.manager.models_rules_path(),
        "# My rules\n\n- effort-ceiling: high\n\n\
         | Work | Agent | Model | Effort | Reason |\n|---|---|---|---|---|\n\
         | Fallback | codex | default | xhigh | Mine. |\n",
    )
    .unwrap();
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    flow.core.update_settings(settings).await.unwrap();

    flow.say("Do the thing.").await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while !moments(&flow)
        .await
        .iter()
        .any(|text| text.contains("Your message waits"))
    {
        assert!(tokio::time::Instant::now() < deadline, "no wait");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        seen(&log)
            .iter()
            .all(|turn| turn.provider == ProviderKind::Claude),
        "{:?}",
        seen(&log)
    );
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Claude);
    flow.stop().await;
}

/// S38: a session waiting for its agent's reset goes on with another agent once the switch is
/// turned on.
#[tokio::test]
async fn turning_the_switch_on_moves_a_waiting_session_to_another_agent() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-agent-fallback-later",
        Options::default(),
        logging(&log, |turn| match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Done on Codex."),
        }),
    )
    .await;
    flow.say("Do the thing.").await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while !moments(&flow)
        .await
        .iter()
        .any(|text| text.contains("Your message waits"))
    {
        assert!(tokio::time::Instant::now() < deadline, "no wait");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    flow.core.update_settings(settings).await.unwrap();
    // As the daemon does when the switch turns on (`wakes_waiting_work`).
    flow.manager.retry_waiting_work().await;
    flow.until("a turn on Codex", |_| {
        seen(&log)
            .iter()
            .any(|turn| turn.provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let codex = seen(&log).into_iter().last().unwrap();
    assert!(codex.input.contains("Do the thing."), "{}", codex.input);
    assert_eq!(orchestrator(&flow).provider, ProviderKind::Codex);
    flow.stop().await;
}

/// The messages a session's last seam gave its successor, and those of them the thread shows
/// already.
fn moved(flow: &Flow) -> (Vec<String>, Vec<String>) {
    let file =
        crate::manager::seam::read_seam(&flow.manager.session_dir(&flow.conversation).record());
    let seam = file.seams.last().expect("a seam");
    (seam.messages.clone(), seam.end.shown.clone())
}

/// S38 (#562): the message whose turn the limit cut moves to the other agent, which is given it
/// again; the seam records the thread shows it already, so it isn't drawn twice.
#[tokio::test]
async fn a_message_moved_to_another_agent_is_shown_already() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-agent-fallback-once",
        Options::default(),
        logging(&log, |turn| match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Done on Codex."),
        }),
    )
    .await;
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    flow.core.update_settings(settings).await.unwrap();

    flow.say("Do the thing.").await;
    flow.until("a turn on Codex", |_| {
        seen(&log)
            .iter()
            .any(|turn| turn.provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let codex = seen(&log).into_iter().last().unwrap();
    assert!(codex.input.contains("Do the thing."), "{}", codex.input);
    let (messages, shown) = moved(&flow);
    assert_eq!(messages.len(), 1);
    assert_eq!(shown, messages);
    flow.stop().await;
}

/// S38 (#562): a session waiting for its reset moves to the other agent when the switch turns
/// on; the message it waited with is recorded as shown already.
#[tokio::test]
async fn a_waiting_message_moved_to_another_agent_is_shown_already() {
    let log = Log::default();
    let flow = Flow::start(
        "accounts-agent-fallback-later-once",
        Options::default(),
        logging(&log, |turn| match turn.provider {
            ProviderKind::Claude => Reply::limited(),
            ProviderKind::Codex => Reply::text("Done on Codex."),
        }),
    )
    .await;
    flow.say("Do the thing.").await;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
    while !moments(&flow)
        .await
        .iter()
        .any(|text| text.contains("Your message waits"))
    {
        assert!(tokio::time::Instant::now() < deadline, "no wait");
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let mut settings = flow.core.settings();
    settings.switch_providers = true;
    flow.core.update_settings(settings).await.unwrap();
    flow.manager.retry_waiting_work().await;
    flow.until("a turn on Codex", |_| {
        seen(&log)
            .iter()
            .any(|turn| turn.provider == ProviderKind::Codex)
    })
    .await;
    flow.settled().await;
    let (messages, shown) = moved(&flow);
    assert_eq!(messages.len(), 1);
    assert_eq!(shown, messages);
    flow.stop().await;
}
