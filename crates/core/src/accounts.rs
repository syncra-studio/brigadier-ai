//! Which of a provider's accounts work runs on.
//!
//! Each provider has the user's own login (the one their terminal uses) and any extra accounts
//! added in Settings → Accounts ([`crate::model::AccountEntry`], each a CLI home of its own:
//! `brigadier_providers::accounts`). [`select`] is the one rule for which of them new work
//! starts on, which one a conversation moves to when its account hits a limit, and whose quota
//! routing and the Usage page show for the provider.

use brigadier_providers::{ProviderKind, QuotaSnapshot};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::Settings;

/// [`crate::model::ModelChoice::account`] for the user's own login, chosen over the default.
pub const OWN: &str = "own";

/// One login of a provider: the user's own (`account` absent) or an extra account's id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountRef {
    pub provider: ProviderKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub account: Option<String>,
}

impl AccountRef {
    /// The user's own login of `provider`.
    pub fn own(provider: ProviderKind) -> Self {
        Self {
            provider,
            account: None,
        }
    }

    pub fn new(provider: ProviderKind, account: Option<String>) -> Self {
        Self { provider, account }
    }

    /// How a [`crate::model::ModelChoice`] names it: [`OWN`] for the user's own login.
    pub fn choice_id(&self) -> String {
        self.account.clone().unwrap_or_else(|| OWN.to_owned())
    }

    /// Its folder name in the data directory's `accounts`, and the key its quota is kept
    /// under: empty for the user's own login.
    pub fn key(&self) -> &str {
        self.account.as_deref().unwrap_or("")
    }
}

/// What is known of one account when choosing.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub account: AccountRef,
    /// Signed in, as its CLI last said.
    pub logged_in: bool,
    /// Its quota now (limits past their reset lifted); unknown until first read.
    pub quota: Option<QuotaSnapshot>,
}

/// The provider's account work starts on (for `model`, when it has windows of its own): its
/// default account while that can take work; otherwise, with switching on, the signed-in
/// account with the most quota left; otherwise the default still (it then reads as limited,
/// and routing moves the work to another provider).
///
/// An account an earlier choice already left at its limit is passed in `avoid`.
pub fn select(
    settings: &Settings,
    provider: ProviderKind,
    model: Option<&str>,
    candidates: &[Candidate],
    avoid: &[AccountRef],
) -> AccountRef {
    let default = AccountRef::new(
        provider,
        settings
            .default_account(provider)
            .map(|account| account.id.clone()),
    );
    let known = |account: &AccountRef| candidates.iter().find(|c| c.account == *account);
    let ready = |candidate: &Candidate| {
        candidate.logged_in
            && !avoid.contains(&candidate.account)
            && candidate
                .quota
                .as_ref()
                .is_none_or(|quota| headroom(quota, model).is_some())
    };
    if known(&default).is_some_and(ready) {
        return default;
    }
    if !settings.switch_accounts {
        return default;
    }
    candidates
        .iter()
        .filter(|candidate| candidate.account.provider == provider && candidate.account != default)
        .filter(|candidate| ready(candidate))
        // The most left; unknown quota counts as full, and on a tie the earliest added wins.
        .max_by(|a, b| {
            let left = |c: &Candidate| c.quota.as_ref().and_then(|q| headroom(q, model));
            let (a_left, b_left) = (left(a).unwrap_or(100.0), left(b).unwrap_or(100.0));
            a_left
                .partial_cmp(&b_left)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| order(settings, &b.account).cmp(&order(settings, &a.account)))
        })
        .map(|candidate| candidate.account.clone())
        .unwrap_or(default)
}

/// How much is left (percent) in the tightest window that applies to `model`, `None` when the
/// account can't take work now (a limit, or such a window used up).
pub fn headroom(quota: &QuotaSnapshot, model: Option<&str>) -> Option<f64> {
    if quota.limit.is_some() {
        return None;
    }
    let left = quota
        .windows
        .iter()
        .filter(|window| {
            window
                .model
                .as_deref()
                .is_none_or(|only| model.is_some_and(|model| model == only))
        })
        .map(|window| 100.0 - window.used_percent)
        .fold(100.0_f64, f64::min);
    (left > 0.0).then_some(left)
}

/// Where an account stands in the list: the user's own login first, then the extra ones as
/// added.
fn order(settings: &Settings, account: &AccountRef) -> usize {
    match &account.account {
        None => 0,
        Some(id) => {
            1 + settings
                .accounts
                .iter()
                .position(|entry| entry.id == *id)
                .unwrap_or(usize::MAX - 1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AccountEntry;
    use brigadier_providers::{LimitHit, LimitKind, QuotaSource, QuotaWindow};

    fn settings(default: Option<&str>, switch: bool) -> Settings {
        Settings {
            accounts: ["work", "home"]
                .iter()
                .map(|id| AccountEntry {
                    id: (*id).into(),
                    provider: ProviderKind::Claude,
                    name: (*id).into(),
                    default: default == Some(*id),
                    added_at_ms: 0,
                })
                .collect(),
            switch_accounts: switch,
            ..Settings::default()
        }
    }

    fn quota(used: f64, limited: bool) -> QuotaSnapshot {
        QuotaSnapshot {
            provider: ProviderKind::Claude,
            windows: vec![QuotaWindow {
                id: "five_hour".into(),
                label: "5-hour".into(),
                used_percent: used,
                resets_at_ms: None,
                window_minutes: Some(300),
                bucket: None,
                model: None,
            }],
            limit: limited.then(|| LimitHit {
                kind: LimitKind::UsageWindow,
                window: Some("five_hour".into()),
                resets_at_ms: None,
            }),
            observed_at_ms: 0,
            source: QuotaSource::Read,
        }
    }

    fn candidate(id: Option<&str>, used: f64, limited: bool) -> Candidate {
        Candidate {
            account: AccountRef::new(ProviderKind::Claude, id.map(str::to_owned)),
            logged_in: true,
            quota: Some(quota(used, limited)),
        }
    }

    fn pick(settings: &Settings, candidates: &[Candidate]) -> Option<String> {
        select(settings, ProviderKind::Claude, None, candidates, &[]).account
    }

    #[test]
    fn the_default_account_takes_work_while_it_can() {
        let all = [
            candidate(None, 90.0, false),
            candidate(Some("work"), 10.0, false),
            candidate(Some("home"), 0.0, false),
        ];
        assert_eq!(pick(&settings(None, true), &all), None);
        assert_eq!(
            pick(&settings(Some("work"), true), &all),
            Some("work".into())
        );
    }

    #[test]
    fn a_limited_default_gives_way_to_the_account_with_the_most_left() {
        let all = [
            candidate(None, 100.0, true),
            candidate(Some("work"), 60.0, false),
            candidate(Some("home"), 20.0, false),
        ];
        assert_eq!(pick(&settings(None, true), &all), Some("home".into()));
        // Switching off: the default, limited as it is (routing then moves on).
        assert_eq!(pick(&settings(None, false), &all), None);
        // Every account used up: the default.
        let spent = [
            candidate(None, 100.0, true),
            candidate(Some("work"), 100.0, false),
            candidate(Some("home"), 30.0, true),
        ];
        assert_eq!(pick(&settings(None, true), &spent), None);
    }

    #[test]
    fn signed_out_and_avoided_accounts_are_passed_over() {
        let mut all = vec![
            candidate(None, 100.0, true),
            candidate(Some("work"), 0.0, false),
            candidate(Some("home"), 50.0, false),
        ];
        all[1].logged_in = false;
        assert_eq!(pick(&settings(None, true), &all), Some("home".into()));
        let avoid = [AccountRef::new(ProviderKind::Claude, Some("home".into()))];
        assert_eq!(
            select(
                &settings(None, true),
                ProviderKind::Claude,
                None,
                &all,
                &avoid
            )
            .account,
            None
        );
    }

    #[test]
    fn a_model_window_used_up_counts_only_for_that_model() {
        let mut own = candidate(None, 10.0, false);
        own.quota.as_mut().unwrap().windows.push(QuotaWindow {
            id: "seven_day_opus".into(),
            label: "Opus weekly".into(),
            used_percent: 100.0,
            resets_at_ms: None,
            window_minutes: None,
            bucket: None,
            model: Some("opus".into()),
        });
        let all = [own, candidate(Some("work"), 50.0, false)];
        let settings = settings(None, true);
        assert_eq!(
            select(&settings, ProviderKind::Claude, Some("opus"), &all, &[]).account,
            Some("work".into())
        );
        assert_eq!(
            select(&settings, ProviderKind::Claude, Some("sonnet"), &all, &[]).account,
            None
        );
    }
}
