//! A thread that waited for its usage limit says where the limit reset and its work went on
//! (#128): a step under the request that waited, which survives a reload as the board does.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;

use super::{Flow, Options, Reply};
use crate::board::Board;
use crate::manager::prompts::CONTINUE_AFTER_LIMIT;
use crate::model::Setup;
use crate::work::{OrchestratorStepKind, RequestState};

/// The thread's first turn hits its limit and waits. Once the limit reset, it goes on, and the
/// request shows the reset.
#[tokio::test]
async fn a_turn_that_waited_for_its_limit_shows_where_it_reset() {
    let (flow, turns) = waiting_flow("quota-resume").await;
    let conversation = flow.core.conversation(&flow.conversation).unwrap();
    let wait = conversation.quota_wait.unwrap();
    let request = flow
        .board()
        .await
        .requests
        .values()
        .next()
        .unwrap()
        .id
        .clone();
    // What it waits for and where, kept for after a restart.
    assert_eq!(wait.request.as_ref(), Some(&request));
    assert_eq!(
        wait.model.map(|model| (model.provider, model.model)),
        Some((
            ProviderKind::Claude,
            conversation.setup.unwrap().choice().model.clone()
        ))
    );
    let board = resume(&flow).await;
    assert_eq!(resumed(&board), [(Some(request), ProviderKind::Claude)]);
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    flow.stop().await;
}

/// The user picked another Claude model while the turn waited: it goes on on that one, which
/// isn't the limit it waited for resetting, so no divider.
#[tokio::test]
async fn another_model_of_the_same_provider_is_no_reset() {
    let (flow, turns) = waiting_flow("quota-resume-other-model").await;
    let mut setup = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .setup
        .unwrap();
    let waited = setup.choice().model.clone();
    let other = if waited.as_deref() == Some("sonnet") {
        "opus"
    } else {
        "sonnet"
    };
    match &mut setup {
        Setup::Session { orchestrator, .. } => orchestrator.model = Some(other.into()),
        Setup::Chat { model, .. } => model.model = Some(other.into()),
    }
    flow.manager
        .set_setup(flow.conversation.clone(), setup)
        .await
        .unwrap();
    let board = resume(&flow).await;
    assert_eq!(resumed(&board), []);
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    flow.stop().await;
}

/// The five-hour window reset while the weekly one still holds the model: the look then
/// names no reset, and the turn goes on only at the weekly reset, which still shows.
#[tokio::test]
async fn a_reset_behind_a_second_window_still_shows() {
    let (flow, turns) = waiting_flow("quota-resume-two-windows").await;
    let request = flow
        .board()
        .await
        .requests
        .values()
        .next()
        .unwrap()
        .id
        .clone();
    flow.until("the five-hour window to reset", |_| {
        crate::now_ms() > waiting_reset(&flow)
    })
    .await;
    let weekly = crate::now_ms() + 1500;
    flow.manager.runtime.monitor().note(
        &crate::accounts::AccountRef::new(ProviderKind::Claude, None),
        &brigadier_providers::QuotaSnapshot {
            provider: ProviderKind::Claude,
            windows: Vec::new(),
            limit: Some(brigadier_providers::LimitHit {
                kind: brigadier_providers::LimitKind::UsageWindow,
                window: Some("seven_day".into()),
                resets_at_ms: Some(weekly),
            }),
            observed_at_ms: crate::now_ms(),
            source: brigadier_providers::QuotaSource::Read,
        },
        crate::now_ms(),
    );
    flow.manager.retry_waiting_conversations().await;
    assert!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .quota_wait
            .is_some(),
        "the weekly window still holds it"
    );
    flow.until("the weekly window to reset", |_| crate::now_ms() > weekly)
        .await;
    let board = resume(&flow).await;
    assert_eq!(resumed(&board), [(Some(request), ProviderKind::Claude)]);
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    flow.stop().await;
}

/// The five-hour window reset while the weekly one still holds the account that hit the
/// limit, and another Claude account leads now: the turn goes on there, which isn't the limit
/// resetting, so no divider (#277).
#[tokio::test]
async fn another_account_taking_over_is_no_reset() {
    let (flow, turns) = waiting_flow("quota-resume-other-account").await;
    let own = crate::accounts::AccountRef::new(ProviderKind::Claude, None);
    assert_eq!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .quota_wait
            .unwrap()
            .account
            .as_ref(),
        Some(&own),
        "the wait names the account that hit the limit"
    );
    flow.until("the five-hour window to reset", |_| {
        crate::now_ms() > waiting_reset(&flow)
    })
    .await;
    flow.manager.runtime.monitor().note(
        &own,
        &brigadier_providers::QuotaSnapshot {
            provider: ProviderKind::Claude,
            windows: Vec::new(),
            limit: Some(brigadier_providers::LimitHit {
                kind: brigadier_providers::LimitKind::UsageWindow,
                window: Some("seven_day".into()),
                resets_at_ms: Some(crate::now_ms() + 3_600_000),
            }),
            observed_at_ms: crate::now_ms(),
            source: brigadier_providers::QuotaSource::Read,
        },
        crate::now_ms(),
    );
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    flow.manager.retry_waiting_conversations().await;
    let board = flow
        .until("the request done", |board| {
            board
                .requests
                .values()
                .any(|request| request.state == RequestState::Done)
        })
        .await;
    assert_eq!(resumed(&board), []);
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    flow.stop().await;
}

/// MA5: Continue now sends the waiting message at once, on the model it waited for, long
/// before the reset; it goes once.
#[tokio::test]
async fn continue_now_sends_the_waiting_message_at_once() {
    let (flow, turns) = limited_flow(
        "quota-continue-now",
        brigadier_providers::LimitKind::UsageWindow,
        Some(3_600_000),
    )
    .await;
    flow.manager
        .continue_waiting(&flow.conversation)
        .await
        .unwrap();
    flow.until("the request done", |board| {
        board
            .requests
            .values()
            .any(|request| request.state == RequestState::Done)
    })
    .await;
    assert!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .quota_wait
            .is_none()
    );
    flow.manager
        .continue_waiting(&flow.conversation)
        .await
        .unwrap();
    flow.settled().await;
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    flow.stop().await;
}

/// MA5: Don't resume leaves the thread idle: past the reset nothing goes on its own.
#[tokio::test]
async fn dont_resume_stays_idle_past_the_reset() {
    let (mut flow, turns) = waiting_flow("quota-dont-resume").await;
    let reset = waiting_reset(&flow);
    flow.manager.dont_resume(&flow.conversation).await.unwrap();
    assert!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .quota_wait
            .is_none()
    );
    // A restart before the reset doesn't bring the wait back.
    flow.restart().await;
    flow.until("the limit to reset", |_| crate::now_ms() > reset)
        .await;
    flow.manager.retry_waiting_conversations().await;
    flow.settled().await;
    assert_eq!(turns.load(Ordering::SeqCst), 1);
    assert_eq!(flow.board().await.run, crate::work::RunState::Idle);
    flow.stop().await;
}

/// MA5: the CLI had begun the turn the limit stopped, so its session holds the user's message
/// and what the turn did: at the reset the same turn goes on with a note to carry on, never
/// the message again, after a restart too.
#[tokio::test]
async fn the_limited_turn_carries_on_at_the_reset() {
    let (mut flow, turns, inputs) = limited_flow_n(
        "quota-carry-on",
        brigadier_providers::LimitKind::UsageWindow,
        Some(1500),
        1,
    )
    .await;
    flow.restart().await;
    resume(&flow).await;
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    let inputs = inputs.lock().unwrap().clone();
    assert!(inputs[0].contains(ASKED), "{inputs:#?}");
    assert!(inputs[1].contains(CONTINUE_AFTER_LIMIT), "{inputs:#?}");
    assert!(
        !inputs[1].contains(ASKED),
        "the message goes once: {inputs:#?}"
    );
    // The note is the CLI's input only, never a message of the thread's.
    let messages = flow.core.all_messages(&flow.conversation).await.unwrap();
    assert!(
        messages
            .iter()
            .all(|message| !message.text.contains(CONTINUE_AFTER_LIMIT)),
        "{messages:#?}"
    );
    flow.stop().await;
}

/// L43: repeated usage failures retain automatic recovery, rather than quietly requiring a click.
#[tokio::test]
async fn a_fifth_limit_keeps_automatic_recovery() {
    let (flow, turns, _) = limited_flow_n(
        "quota-five-limits",
        brigadier_providers::LimitKind::UsageWindow,
        Some(300),
        5,
    )
    .await;
    for resumed in 1..=4 {
        let reset = waiting_reset(&flow);
        flow.until("the limit to reset", |_| crate::now_ms() > reset)
            .await;
        flow.manager.retry_waiting_conversations().await;
        flow.until("the next limit", |_| {
            turns.load(Ordering::SeqCst) == resumed + 1 && waiting_reset(&flow) > reset
        })
        .await;
    }
    let wait = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .quota_wait
        .unwrap();
    let recovery = wait.recovery.unwrap();
    assert_ne!(recovery.mode, crate::work::QuotaRecoveryMode::ManualOnly);
    assert!(recovery.next_check_at_ms.unwrap() > crate::now_ms());
    resume(&flow).await;
    assert_eq!(turns.load(Ordering::SeqCst), 6);
    flow.stop().await;
}

/// MA5: a credits limit never schedules a resume, whatever reset it names; only Continue now
/// sends the message.
#[tokio::test]
async fn a_credits_limit_never_resumes_on_its_own() {
    let (flow, turns) = limited_flow(
        "quota-credits",
        brigadier_providers::LimitKind::Credits,
        Some(300),
    )
    .await;
    let wait = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .quota_wait
        .unwrap();
    assert_eq!(
        wait.limit_kind,
        Some(brigadier_providers::LimitKind::Credits)
    );
    // The card reads the limit from the CLI's own error, on its turn.
    let view = flow.manager.thread_view(&flow.conversation).unwrap();
    let limit = view.turns.last().and_then(|turn| turn.limit.clone());
    assert_eq!(
        limit.map(|limit| limit.kind),
        Some(brigadier_providers::LimitKind::Credits),
        "{:#?}",
        view.turns
    );
    let reset = waiting_reset(&flow);
    flow.until("its named reset to pass", |_| crate::now_ms() > reset + 200)
        .await;
    flow.manager.retry_waiting_conversations().await;
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    assert_eq!(turns.load(Ordering::SeqCst), 1, "it never went on its own");
    assert!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .quota_wait
            .is_some()
    );
    flow.manager
        .continue_waiting(&flow.conversation)
        .await
        .unwrap();
    flow.until("the request done", |board| {
        board
            .requests
            .values()
            .any(|request| request.state == RequestState::Done)
    })
    .await;
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    flow.stop().await;
}

/// Credits that ran out with no reset time named still wait, and Continue now sends the message.
#[tokio::test]
async fn credits_with_no_reset_time_wait_for_continue_now() {
    let (flow, turns) = limited_flow(
        "quota-credits-no-reset",
        brigadier_providers::LimitKind::Credits,
        None,
    )
    .await;
    let wait = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .quota_wait
        .unwrap();
    assert_eq!(wait.resets_at_ms, None);
    flow.manager
        .continue_waiting(&flow.conversation)
        .await
        .unwrap();
    flow.until("the request done", |board| {
        board
            .requests
            .values()
            .any(|request| request.state == RequestState::Done)
    })
    .await;
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    flow.stop().await;
}

/// A flow whose first turn hits a limit that resets in a moment; Codex is off, so nothing
/// stands in and the thread waits. Also counts the turns.
async fn waiting_flow(name: &str) -> (Flow, Arc<AtomicUsize>) {
    limited_flow(
        name,
        brigadier_providers::LimitKind::UsageWindow,
        Some(1500),
    )
    .await
}

/// [`waiting_flow`] with a limit of `kind` that resets in `resets_in_ms` (none: no reset time).
async fn limited_flow(
    name: &str,
    kind: brigadier_providers::LimitKind,
    resets_in_ms: Option<i64>,
) -> (Flow, Arc<AtomicUsize>) {
    let (flow, turns, _) = limited_flow_n(name, kind, resets_in_ms, 1).await;
    (flow, turns)
}

/// What the user asks in a [`limited_flow`].
const ASKED: &str = "Move the tax rules into their own module.";

/// [`limited_flow`] whose first `limits` turns hit the limit; also keeps each turn's input.
async fn limited_flow_n(
    name: &str,
    kind: brigadier_providers::LimitKind,
    resets_in_ms: Option<i64>,
    limits: usize,
) -> (Flow, Arc<AtomicUsize>, Arc<Mutex<Vec<String>>>) {
    let turns = Arc::new(AtomicUsize::new(0));
    let counted = turns.clone();
    let inputs = Arc::new(Mutex::new(Vec::new()));
    let seen = inputs.clone();
    let flow = Flow::start(
        name,
        Options {
            behavior: Arc::new(super::FakeBehavior {
                session_file: true,
                ..super::FakeBehavior::default()
            }),
            ..Options::default()
        },
        Arc::new(move |turn| {
            seen.lock().unwrap().push(turn.input.clone());
            let limited = counted.fetch_add(1, Ordering::SeqCst) < limits;
            Box::pin(async move {
                if limited {
                    Reply {
                        limit: Some(brigadier_providers::LimitHit {
                            kind,
                            window: Some("five_hour".into()),
                            resets_at_ms: resets_in_ms.map(|ms| crate::now_ms() + ms),
                        }),
                        ..Reply::default()
                    }
                } else {
                    Reply::text("Moved the tax rules.")
                }
            })
        }),
    )
    .await;
    let mut settings = flow.core.settings();
    settings.disabled_providers.push(ProviderKind::Codex);
    flow.core.update_settings(settings).await.unwrap();

    flow.say(ASKED).await;
    flow.until("the thread to wait for its limit", |_| {
        flow.core
            .conversation(&flow.conversation)
            .is_ok_and(|conversation| conversation.quota_wait.is_some())
    })
    .await;
    assert_eq!(resumed(&flow.board().await), [], "nothing reset yet");
    (flow, turns, inputs)
}

/// Once the reset is past, the thread looks again (as its timer would, 45 s after the reset)
/// and its request finishes: the board then.
async fn resume(flow: &Flow) -> Board {
    flow.until("the limit to reset", |_| {
        crate::now_ms() > waiting_reset(flow)
    })
    .await;
    flow.manager.retry_waiting_conversations().await;
    flow.until("the request done", |board| {
        board
            .requests
            .values()
            .any(|request| request.state == RequestState::Done)
    })
    .await
}

/// The resets the board shows: under which request, for which provider.
fn resumed(board: &Board) -> Vec<(Option<String>, ProviderKind)> {
    board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match step.kind {
            OrchestratorStepKind::ProviderTransition {
                cause: crate::work::ProviderTransitionCause::QuotaReset,
                ref to,
                ..
            } => Some((step.request_id.clone(), to.provider)),
            _ => None,
        })
        .collect()
}

/// When the recorded wait's limit resets.
fn waiting_reset(flow: &Flow) -> i64 {
    flow.core
        .conversation(&flow.conversation)
        .ok()
        .and_then(|conversation| conversation.quota_wait)
        .and_then(|wait| wait.resets_at_ms)
        .unwrap_or(0)
}

#[tokio::test]
async fn an_unknown_usage_reset_is_a_durable_automatic_probe() {
    let (mut flow, turns, _) = limited_flow_n(
        "quota-unknown-probe",
        brigadier_providers::LimitKind::UsageWindow,
        None,
        usize::MAX,
    )
    .await;
    let wait = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .quota_wait
        .unwrap();
    let recovery = wait.recovery.as_ref().unwrap();
    assert_eq!(
        recovery.mode,
        crate::work::QuotaRecoveryMode::AutomaticProbe
    );
    assert!(recovery.next_check_at_ms.unwrap() > crate::now_ms());
    flow.crash().await;
    flow.until("restored quota wait", |_| {
        flow.core
            .conversation(&flow.conversation)
            .is_ok_and(|conversation| conversation.quota_wait.is_some())
    })
    .await;
    let restored = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .quota_wait
        .unwrap();
    assert_eq!(restored.request, wait.request);
    assert_eq!(restored.messages, wait.messages);
    assert!(restored.recovery.unwrap().next_check_at_ms.unwrap() > crate::now_ms());
    let before_stop = turns.load(Ordering::SeqCst);
    flow.manager.dont_resume(&flow.conversation).await.unwrap();
    flow.manager.retry_waiting_conversations().await;
    assert_eq!(turns.load(Ordering::SeqCst), before_stop);
    assert!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .quota_wait
            .is_none()
    );
    flow.stop().await;
}

#[tokio::test]
async fn a_manual_account_selection_wakes_pending_work_without_a_message() {
    let (flow, turns) = waiting_flow("quota-manual-account").await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-free")], true)
        .await;
    let mut setup = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .setup
        .unwrap();
    let from = setup.choice().clone();
    match &mut setup {
        Setup::Session { orchestrator, .. } => orchestrator.account = Some("acct-free".into()),
        Setup::Chat { model, .. } => model.account = Some("acct-free".into()),
    }
    let to = setup.choice().clone();
    // The shared updateSetup hook calls this helper after committing the setup.
    flow.core
        .set_setup(flow.conversation.clone(), setup)
        .await
        .unwrap();
    flow.manager
        .model_selected(&flow.conversation, from, to)
        .await
        .unwrap();
    flow.until("manual selection accepted", |board| board.orchestrator_steps.iter().any(|step| matches!(&step.kind,
        OrchestratorStepKind::ProviderTransition { cause: crate::work::ProviderTransitionCause::ManualSwitch, account, .. }
        if account.account.as_deref() == Some("acct-free")))).await;
    assert_eq!(turns.load(Ordering::SeqCst), 2);
    assert!(
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .quota_wait
            .is_none()
    );
    flow.stop().await;
}
