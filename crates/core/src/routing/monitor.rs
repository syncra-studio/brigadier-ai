//! The quota monitor: every provider's usage windows as the CLIs report them, merged over
//! time, with the sample history the rolling estimates are made from.
//!
//! - **Sources.** A read (`get_usage`, `account/rateLimits/read`) replaces what is known; a
//!   running session's rate-limit events update the windows they name
//!   ([`QuotaSnapshot::merge`]).
//! - **Limits lift on time.** A used-up window's limit counts until its reset time; then the
//!   window reads as unused until the next read says otherwise. A spend control or a credits
//!   stop is lifted only by a fresh read.
//! - **History.** A sample is kept when a window's use changes, and every half hour while it
//!   does not, in memory for the estimates and in `routing.sqlite` across launches.
//! - **Polling.** The runtime reads each logged-in provider every 5 minutes while sessions
//!   report activity, every 30 minutes otherwise, and just after a known reset.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use brigadier_providers::{LimitHit, LimitKind, ProviderKind, QuotaSnapshot, QuotaSource};

use crate::accounts::AccountRef;
use brigadier_router::QuotaSample;

use super::store::{HISTORY_MS, RoutingStore, StoredSample};

/// A provider that reported activity this recently counts as in use (polled more often).
const BUSY_FOR_MS: i64 = 10 * 60 * 1000;
const POLL_BUSY: Duration = Duration::from_secs(5 * 60);
const POLL_IDLE: Duration = Duration::from_secs(30 * 60);
/// A read this long after a known reset sees the new window.
const AFTER_RESET: Duration = Duration::from_secs(30);
const MIN_POLL: Duration = Duration::from_secs(30);
/// An unchanged window still gets a sample this often, so a quiet stretch shows as flat.
const SAMPLE_UNCHANGED_MS: i64 = 30 * 60 * 1000;
/// Changes smaller than this (in percentage points) are not new samples.
const SAMPLE_EPSILON: f64 = 0.05;

#[derive(Default)]
struct Tracked {
    quota: Option<QuotaSnapshot>,
    /// Samples per window id, oldest first, for the last [`HISTORY_MS`].
    history: HashMap<String, VecDeque<QuotaSample>>,
    /// When a running session last reported this provider's quota.
    last_event_ms: i64,
    /// When this account's quota was last read or reported.
    read_at_ms: Option<i64>,
    /// Development builds: a limit injected to exercise fallback, held until its reset.
    #[cfg(debug_assertions)]
    injected: Option<LimitHit>,
}

/// Every account's quota is kept apart ([`AccountRef`]); a provider's quota, as routing and
/// the Usage page see it, is its lead account's: the one new work starts on
/// ([`crate::accounts::select`], set with [`QuotaMonitor::set_lead`]).
pub struct QuotaMonitor {
    store: Option<Arc<RoutingStore>>,
    state: Mutex<HashMap<AccountRef, Tracked>>,
    leads: Mutex<HashMap<ProviderKind, AccountRef>>,
}

impl QuotaMonitor {
    /// A monitor over `store`, with the history it holds loaded.
    pub async fn load(store: Option<Arc<RoutingStore>>, now_ms: i64) -> Arc<Self> {
        let mut state: HashMap<AccountRef, Tracked> = HashMap::new();
        if let Some(store) = &store {
            if let Err(err) = store.prune(now_ms).await {
                tracing::warn!(error = %err, "could not prune the quota history");
            }
            match store.samples_since(now_ms - HISTORY_MS).await {
                Ok(samples) => {
                    for stored in samples {
                        state
                            .entry(stored.account.clone())
                            .or_default()
                            .history
                            .entry(stored.window)
                            .or_default()
                            .push_back(stored.sample);
                    }
                }
                Err(err) => tracing::warn!(error = %err, "could not load the quota history"),
            }
        }
        Arc::new(Self {
            store,
            state: Mutex::new(state),
            leads: Mutex::new(HashMap::new()),
        })
    }

    fn state(&self) -> MutexGuard<'_, HashMap<AccountRef, Tracked>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The account whose quota is `provider`'s (the user's own login until set).
    pub fn lead(&self, provider: ProviderKind) -> AccountRef {
        self.leads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&provider)
            .cloned()
            .unwrap_or_else(|| AccountRef::own(provider))
    }

    /// Makes `account` its provider's lead; answers whether that changed.
    pub fn set_lead(&self, account: AccountRef) -> bool {
        let mut leads = self.leads.lock().unwrap_or_else(PoisonError::into_inner);
        leads.insert(account.provider, account.clone()).as_ref() != Some(&account)
    }

    /// Takes in an account's report and answers what is known now. New samples are stored in
    /// the background.
    pub fn note(
        &self,
        account: &AccountRef,
        incoming: &QuotaSnapshot,
        now_ms: i64,
    ) -> QuotaSnapshot {
        let (current, samples) = {
            let mut state = self.state();
            let tracked = state.entry(account.clone()).or_default();
            if incoming.source == QuotaSource::Event {
                tracked.last_event_ms = now_ms;
            }
            tracked.read_at_ms = Some(incoming.observed_at_ms);
            match &mut tracked.quota {
                Some(known) => known.merge(incoming),
                None => tracked.quota = Some(incoming.clone()),
            }
            let samples = sample(tracked, account, now_ms);
            (current(tracked, now_ms), samples)
        };
        if let Some(store) = &self.store
            && !samples.is_empty()
        {
            let store = store.clone();
            tokio::spawn(async move {
                if let Err(err) = store.add_samples(samples).await {
                    tracing::warn!(error = %err, "could not store quota samples");
                }
            });
        }
        current.unwrap_or_else(|| incoming.clone())
    }

    /// What is known about a provider's quota now (its lead account's): limits past their
    /// reset lifted, windows past their reset read as unused.
    pub fn current(&self, provider: ProviderKind, now_ms: i64) -> Option<QuotaSnapshot> {
        self.current_for(&self.lead(provider), now_ms)
    }

    /// What is known about one account's quota now.
    pub fn current_for(&self, account: &AccountRef, now_ms: i64) -> Option<QuotaSnapshot> {
        self.state()
            .get(account)
            .and_then(|tracked| current(tracked, now_ms))
    }

    /// When `account`'s quota was last read or reported; `None` before the first.
    pub fn read_at(&self, account: &AccountRef) -> Option<i64> {
        self.state()
            .get(account)
            .and_then(|tracked| tracked.read_at_ms)
    }

    /// One of an account's windows' samples since `since_ms`, oldest first.
    pub fn history(&self, account: &AccountRef, window: &str, since_ms: i64) -> Vec<QuotaSample> {
        self.state()
            .get(account)
            .and_then(|tracked| tracked.history.get(window))
            .map(|samples| {
                samples
                    .iter()
                    .filter(|sample| sample.at_ms >= since_ms)
                    .copied()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// How long until the providers should be read again.
    pub fn next_poll(&self, now_ms: i64) -> Duration {
        let state = self.state();
        let busy = state
            .values()
            .any(|tracked| now_ms - tracked.last_event_ms < BUSY_FOR_MS);
        let mut wait = if busy { POLL_BUSY } else { POLL_IDLE };
        let resets = state
            .values()
            .filter_map(|tracked| tracked.quota.as_ref())
            .flat_map(|quota| {
                quota
                    .windows
                    .iter()
                    .filter_map(|window| window.resets_at_ms)
                    .chain(quota.limit.as_ref().and_then(|limit| limit.resets_at_ms))
            })
            .filter(|at| *at > now_ms);
        for at in resets {
            let until =
                Duration::from_millis(u64::try_from(at - now_ms).unwrap_or(0)) + AFTER_RESET;
            wait = wait.min(until);
        }
        wait.max(MIN_POLL)
    }

    /// Takes in a limit a session's error reported (a CLI can say it is at its limit without
    /// a rate-limit notification): the provider counts as limited until the limit's reset (the
    /// named window's, when the error gave none), or, when neither is known, until a read shows
    /// it clear. A model's own window running out refuses that model only: the window reads as
    /// used up and the provider stays usable for its other models.
    pub fn note_limit(
        &self,
        account: &AccountRef,
        mut limit: LimitHit,
        now_ms: i64,
    ) -> QuotaSnapshot {
        let provider = account.provider;
        let incoming = {
            let state = self.state();
            let mut windows = state
                .get(account)
                .and_then(|tracked| tracked.quota.as_ref())
                .map(|quota| quota.windows.clone())
                .unwrap_or_default();
            let named = limit
                .window
                .as_deref()
                .and_then(|id| windows.iter().position(|window| window.id == id));
            if let Some(index) = named
                && limit.resets_at_ms.is_none()
            {
                limit.resets_at_ms = windows[index].resets_at_ms;
            }
            match named.filter(|index| windows[*index].model.is_some()) {
                Some(index) => {
                    // Only the scoped window, so the event lifts no other limit.
                    let mut window = windows.swap_remove(index);
                    window.used_percent = window.used_percent.max(100.0);
                    window.resets_at_ms = window.resets_at_ms.or(limit.resets_at_ms);
                    QuotaSnapshot {
                        provider,
                        windows: vec![window],
                        limit: None,
                        observed_at_ms: now_ms,
                        source: QuotaSource::Event,
                    }
                }
                None => QuotaSnapshot {
                    provider,
                    windows,
                    limit: Some(limit),
                    observed_at_ms: now_ms,
                    source: QuotaSource::Event,
                },
            }
        };
        self.note(account, &incoming, now_ms)
    }

    /// Development builds: makes `account` refuse work as `limit` says until its reset, so
    /// no read can clear it early.
    #[cfg(debug_assertions)]
    pub fn inject(&self, account: &AccountRef, limit: LimitHit) {
        self.state().entry(account.clone()).or_default().injected = Some(limit);
    }
}

/// The known quota with the time applied.
fn current(tracked: &Tracked, now_ms: i64) -> Option<QuotaSnapshot> {
    let mut quota = tracked.quota.clone()?;
    // A usage-window limit that gave no reset lifts at its window's.
    if let Some(limit) = &mut quota.limit
        && limit.kind == LimitKind::UsageWindow
        && limit.resets_at_ms.is_none()
        && let Some(id) = limit.window.as_deref()
    {
        limit.resets_at_ms = quota
            .windows
            .iter()
            .find(|window| window.id == id)
            .and_then(|window| window.resets_at_ms);
    }
    for window in &mut quota.windows {
        if window.resets_at_ms.is_some_and(|at| at <= now_ms) {
            window.used_percent = 0.0;
            window.resets_at_ms = None;
        }
    }
    if quota.limit.as_ref().is_some_and(|limit| {
        limit.kind == LimitKind::UsageWindow && limit.resets_at_ms.is_some_and(|at| at <= now_ms)
    }) {
        quota.limit = None;
    }
    #[cfg(debug_assertions)]
    if let Some(injected) = &tracked.injected
        && injected.resets_at_ms.is_none_or(|at| at > now_ms)
    {
        quota.limit = Some(injected.clone());
        if let Some(window) = quota
            .windows
            .iter_mut()
            .find(|window| injected.window.as_deref() == Some(window.id.as_str()))
        {
            window.used_percent = 100.0;
            window.resets_at_ms = injected.resets_at_ms;
        }
    }
    Some(quota)
}

/// New samples for the windows of `tracked`'s quota, added to its history.
fn sample(tracked: &mut Tracked, account: &AccountRef, now_ms: i64) -> Vec<StoredSample> {
    let Some(quota) = &tracked.quota else {
        return Vec::new();
    };
    // Development builds: an injected limit's used-up window is not real use, so it stays out
    // of the history the estimates and the balancing read.
    #[cfg(debug_assertions)]
    let injected = tracked
        .injected
        .as_ref()
        .filter(|limit| limit.resets_at_ms.is_none_or(|at| at > now_ms))
        .and_then(|limit| limit.window.as_deref());
    #[cfg(not(debug_assertions))]
    let injected: Option<&str> = None;
    let mut stored = Vec::new();
    for window in &quota.windows {
        // A window past its reset has no known use until a report says so.
        if injected == Some(window.id.as_str())
            || window.resets_at_ms.is_some_and(|at| at <= now_ms)
        {
            continue;
        }
        let history = tracked.history.entry(window.id.clone()).or_default();
        let due = history.back().is_none_or(|last| {
            (last.used_percent - window.used_percent).abs() >= SAMPLE_EPSILON
                || now_ms - last.at_ms >= SAMPLE_UNCHANGED_MS
        });
        if !due {
            continue;
        }
        let sample = QuotaSample {
            at_ms: now_ms,
            used_percent: window.used_percent,
        };
        history.push_back(sample);
        while history
            .front()
            .is_some_and(|first| first.at_ms < now_ms - HISTORY_MS)
        {
            history.pop_front();
        }
        stored.push(StoredSample {
            account: account.clone(),
            window: window.id.clone(),
            sample,
            resets_at_ms: window.resets_at_ms,
        });
    }
    stored
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_providers::QuotaWindow;

    fn read(provider: ProviderKind, used: f64, at_ms: i64) -> QuotaSnapshot {
        QuotaSnapshot {
            provider,
            windows: vec![QuotaWindow {
                id: "five_hour".into(),
                label: "5-hour".into(),
                used_percent: used,
                resets_at_ms: None,
                window_minutes: Some(300),
                bucket: None,
                model: None,
            }],
            limit: None,
            observed_at_ms: at_ms,
            source: QuotaSource::Read,
        }
    }

    fn used(quota: Option<QuotaSnapshot>) -> Option<f64> {
        quota.map(|quota| quota.windows[0].used_percent)
    }

    #[tokio::test]
    async fn each_account_keeps_its_own_quota_and_the_lead_is_the_providers() {
        let monitor = QuotaMonitor::load(None, 0).await;
        let own = AccountRef::own(ProviderKind::Claude);
        let work = AccountRef::new(ProviderKind::Claude, Some("work".into()));
        monitor.note(&own, &read(ProviderKind::Claude, 80.0, 10), 10);
        monitor.note(&work, &read(ProviderKind::Claude, 5.0, 20), 20);
        assert_eq!(used(monitor.current_for(&own, 30)), Some(80.0));
        assert_eq!(used(monitor.current_for(&work, 30)), Some(5.0));
        assert_eq!(
            (monitor.read_at(&own), monitor.read_at(&work)),
            (Some(10), Some(20))
        );
        // The user's own login leads until another account is made the lead.
        assert_eq!(used(monitor.current(ProviderKind::Claude, 30)), Some(80.0));
        assert!(monitor.set_lead(work.clone()));
        assert!(!monitor.set_lead(work.clone()));
        assert_eq!(used(monitor.current(ProviderKind::Claude, 30)), Some(5.0));
        // A limit on one account leaves the other free.
        monitor.note_limit(
            &own,
            LimitHit {
                kind: LimitKind::UsageWindow,
                window: Some("five_hour".into()),
                resets_at_ms: Some(1_000_000),
            },
            40,
        );
        assert!(monitor.current_for(&own, 50).unwrap().limit.is_some());
        assert!(monitor.current_for(&work, 50).unwrap().limit.is_none());
        assert_eq!(monitor.history(&work, "five_hour", 0).len(), 1);
    }
}
