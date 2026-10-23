//! The Usage page's reads: each account of a provider on its own (its windows, when they were
//! read, whether work runs on it), its Activity over the last days, and a refresh that tells
//! when every account was checked afresh.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_providers::{ProviderKind, QuotaSnapshot, QuotaSource, QuotaWindow};

use super::{Flow, Options, Reply, Script, Turn};
use crate::accounts::AccountRef;
use crate::model::ProviderUsage;

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;

fn answering() -> Script {
    Arc::new(|_turn: Turn| Box::pin(async { Reply::text("Noted.") }))
}

/// `account`'s 5-hour and weekly windows at `used` percent, as read at `at_ms`.
fn read(flow: &Flow, account: &AccountRef, used: f64, at_ms: i64) {
    let window = |id: &str, minutes: i64| QuotaWindow {
        id: id.into(),
        label: id.into(),
        used_percent: used,
        resets_at_ms: Some(crate::now_ms() + HOUR_MS),
        window_minutes: Some(minutes),
        bucket: None,
        model: None,
    };
    flow.manager.runtime.monitor().note(
        account,
        &QuotaSnapshot {
            provider: account.provider,
            windows: vec![
                window("seven_day", 7 * 24 * 60),
                window("five_hour", 5 * 60),
            ],
            limit: None,
            observed_at_ms: at_ms,
            source: QuotaSource::Read,
        },
        at_ms,
    );
}

async fn claude(flow: &Flow) -> ProviderUsage {
    let view = flow.manager.usage_view().await.unwrap();
    view.providers
        .into_iter()
        .find(|usage| usage.provider == ProviderKind::Claude)
        .unwrap()
}

#[tokio::test]
async fn each_account_has_its_own_windows_and_the_time_they_were_read() {
    let flow = Flow::start("usage-accounts", Options::default(), answering()).await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let own = AccountRef::own(ProviderKind::Claude);
    let other = AccountRef::new(ProviderKind::Claude, Some("acct-b".into()));
    let now = crate::now_ms();
    read(&flow, &own, 30.0, now - 4 * 60 * 1000);
    read(&flow, &other, 85.0, now - 60 * 1000);

    let usage = claude(&flow).await;
    let accounts: Vec<_> = usage
        .accounts
        .iter()
        .map(|entry| entry.account.clone())
        .collect();
    assert_eq!(accounts, [own.clone(), other.clone()]);
    let used = |index: usize| -> Vec<(String, f64)> {
        let quota = usage.accounts[index].quota.as_ref().unwrap();
        quota
            .windows
            .iter()
            .map(|state| (state.window.id.clone(), state.window.used_percent))
            .collect()
    };
    assert_eq!(
        used(0),
        [("seven_day".into(), 30.0), ("five_hour".into(), 30.0)]
    );
    assert_eq!(
        used(1),
        [("seven_day".into(), 85.0), ("five_hour".into(), 85.0)]
    );
    // When each was read, not when the page asked.
    assert_eq!(usage.accounts[0].read_at_ms, Some(now - 4 * 60 * 1000));
    assert_eq!(usage.accounts[1].read_at_ms, Some(now - 60 * 1000));
    // An account never read says so.
    let codex = flow.manager.usage_view().await.unwrap().providers;
    let codex = codex
        .iter()
        .find(|usage| usage.provider == ProviderKind::Codex)
        .unwrap();
    assert_eq!(codex.accounts.len(), 1);
    flow.stop().await;
}

#[tokio::test]
async fn an_account_shows_as_working_only_while_work_runs_on_it() {
    let hold = Arc::new(tokio::sync::Notify::new());
    let started = Arc::new(Mutex::new(false));
    let script: Script = {
        let (hold, started) = (hold.clone(), started.clone());
        Arc::new(move |turn: Turn| {
            let (hold, started) = (hold.clone(), started.clone());
            Box::pin(async move {
                if turn.input.contains("Hold.") {
                    *started.lock().unwrap() = true;
                    hold.notified().await;
                }
                Reply::text("Done.")
            })
        })
    };
    let flow = Flow::start("usage-working", Options::default(), script).await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let working = |usage: &ProviderUsage| -> Vec<bool> {
        usage.accounts.iter().map(|entry| entry.working).collect()
    };
    assert_eq!(working(&claude(&flow).await), [false, false]);

    flow.say("Hold.").await;
    flow.until("the turn runs", |_| *started.lock().unwrap())
        .await;
    assert_eq!(working(&claude(&flow).await), [true, false]);

    hold.notify_waiters();
    flow.settled().await;
    assert_eq!(working(&claude(&flow).await), [false, false]);
    flow.stop().await;
}

#[tokio::test]
async fn activity_reads_each_accounts_samples_over_the_range_asked() {
    let flow = Flow::start("usage-activity", Options::default(), answering()).await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let own = AccountRef::own(ProviderKind::Claude);
    let other = AccountRef::new(ProviderKind::Claude, Some("acct-b".into()));
    let now = crate::now_ms();
    // Samples from long before the 5-hour window now running, then fresh ones.
    read(&flow, &own, 10.0, now - 20 * DAY_MS);
    read(&flow, &own, 40.0, now - 3 * DAY_MS);
    read(&flow, &own, 60.0, now - 60 * 1000);
    read(&flow, &other, 5.0, now - 60 * 1000);

    let samples = |days: u32| {
        let flow = &flow;
        let own = own.clone();
        async move {
            let activity = flow.manager.usage_activity(days).await;
            assert!(activity.since_ms <= now - i64::from(days) * DAY_MS + DAY_MS);
            let entry = activity
                .accounts
                .into_iter()
                .find(|entry| entry.account == own)
                .unwrap();
            // The shortest window first.
            let ids: Vec<_> = entry
                .history
                .iter()
                .map(|history| history.window_id.clone())
                .collect();
            assert_eq!(ids, ["five_hour", "seven_day"]);
            entry.history[0]
                .samples
                .iter()
                .map(|sample| sample.used_percent)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(samples(30).await, [10.0, 40.0, 60.0]);
    assert_eq!(samples(7).await, [40.0, 60.0]);

    // Each account's samples are its own.
    let activity = flow.manager.usage_activity(7).await;
    let theirs = activity
        .accounts
        .iter()
        .find(|entry| entry.account == other)
        .unwrap();
    let used: Vec<_> = theirs.history[0]
        .samples
        .iter()
        .map(|sample| sample.used_percent)
        .collect();
    assert_eq!(used, [5.0]);
    flow.stop().await;
}

#[tokio::test]
async fn a_refresh_waits_for_a_check_that_began_before_it() {
    let flow = Flow::start("usage-refresh", Options::default(), answering()).await;
    flow.add_accounts(&[(ProviderKind::Claude, "acct-b")], true)
        .await;
    let runtime = &flow.manager.runtime;
    let (reached, release) = (
        Arc::new(tokio::sync::Notify::new()),
        Arc::new(tokio::sync::Notify::new()),
    );
    *flow.behavior.hold_status.lock().unwrap() = Some((reached.clone(), release.clone()));
    // A check of the account is under way when the refresh is asked for.
    runtime.refresh_account("acct-b".into());
    reached.notified().await;
    let generation = runtime.refresh_all();
    let account_generation = || {
        runtime
            .accounts_view()
            .accounts
            .into_iter()
            .find(|view| view.account.account.as_deref() == Some("acct-b"))
            .unwrap()
            .checked_generation
    };
    // The earlier check finishes first: it began before the refresh, so it doesn't count.
    release.notify_one();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let views = runtime.accounts_view().accounts;
        if views
            .iter()
            .all(|view| view.checked_generation >= generation && !view.checking)
        {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "every account checked afresh: {views:#?}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(account_generation(), generation);
    flow.stop().await;
}
