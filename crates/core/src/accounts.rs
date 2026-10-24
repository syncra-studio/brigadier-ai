//! Which of a provider's accounts work runs on.
//!
//! Each provider has the user's own login (the one their terminal uses) and any extra accounts
//! added in Settings → Accounts ([`crate::model::AccountEntry`], each a CLI home of its own:
//! `brigadier_providers::accounts`). [`select`] (with [`ready`] under it) is the one rule for which of them new work
//! starts on, which one a conversation moves to when its account hits a limit, and whose quota
//! routing and the Usage page show for the provider.

use brigadier_providers::{ProviderKind, QuotaSnapshot, QuotaWindow};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::{OwnAccountName, Settings};

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

/// `incoming` settings with their accounts as an edit may change them: a name, the default
/// (one per provider) and nothing else. Accounts are added and removed by their own requests
/// (adding makes the account's home; removing signs it out first). An account picked with Use
/// (a new default) pins its provider: its accounts no longer switch on their own.
pub fn edited(current: &Settings, mut incoming: Settings) -> Settings {
    let mut accounts = current.accounts.clone();
    for account in &mut accounts {
        if let Some(edit) = incoming.accounts.iter().find(|edit| edit.id == account.id) {
            account.name = edit.name.trim().to_owned();
            account.default = edit.default;
        }
    }
    for provider in ProviderKind::ALL {
        let mut seen = false;
        for account in accounts
            .iter_mut()
            .filter(|a| a.provider == provider && a.default)
        {
            account.default = !std::mem::replace(&mut seen, true);
        }
    }
    let picked: Vec<ProviderKind> = ProviderKind::ALL
        .into_iter()
        .filter(|provider| {
            let id = |settings: &Settings| {
                settings
                    .accounts
                    .iter()
                    .find(|account| account.provider == *provider && account.default)
                    .map(|account| account.id.clone())
            };
            id(current)
                != accounts
                    .iter()
                    .find(|account| account.provider == *provider && account.default)
                    .map(|account| account.id.clone())
        })
        .collect();
    for provider in picked {
        if !incoming.manual_accounts.contains(&provider) {
            incoming.manual_accounts.push(provider);
        }
    }
    incoming.accounts = accounts;
    // An own login's name, trimmed; an empty one goes back to the email, and each agent keeps
    // its first.
    let mut names: Vec<OwnAccountName> = Vec::new();
    for entry in std::mem::take(&mut incoming.own_account_names) {
        let name = entry.name.trim();
        if !name.is_empty() && !names.iter().any(|kept| kept.provider == entry.provider) {
            names.push(OwnAccountName {
                provider: entry.provider,
                name: name.to_owned(),
            });
        }
    }
    incoming.own_account_names = names;
    incoming
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

/// The provider's account work starts on (for `model`, when it has windows of its own): as
/// [`ready`] picks it, else (no account ready) the default, which then reads as limited, so
/// routing moves the work to another provider.
///
/// An account an earlier choice already left at its limit is passed in `avoid`.
pub fn select(
    settings: &Settings,
    provider: ProviderKind,
    model: Option<&str>,
    candidates: &[Candidate],
    avoid: &[AccountRef],
    now: i64,
) -> AccountRef {
    ready(settings, provider, model, candidates, avoid, now)
        .unwrap_or_else(|| default_account(settings, provider))
}

/// The account work goes to among those that can take it, `None` when none can: the first of
/// [`order`]. With switching off, the account picked with Use (the default) while it can;
/// once it can't, or with switching on, the first of the smart order: another account of the
/// provider goes before another provider (#505).
pub fn ready(
    settings: &Settings,
    provider: ProviderKind,
    model: Option<&str>,
    candidates: &[Candidate],
    avoid: &[AccountRef],
    now: i64,
) -> Option<AccountRef> {
    if !settings.switches_accounts(provider) {
        let default = default_account(settings, provider);
        if candidates.iter().any(|candidate| {
            candidate.account == default && can_take(candidate, provider, model, avoid)
        }) {
            return Some(default);
        }
    }
    smart_order(settings, provider, model, candidates, avoid, now)
        .into_iter()
        .next()
}

/// The account picked as `provider`'s default: an extra one, else the user's own login.
pub fn default_account(settings: &Settings, provider: ProviderKind) -> AccountRef {
    AccountRef::new(
        provider,
        settings
            .default_account(provider)
            .map(|account| account.id.clone()),
    )
}

/// Whether `candidate` can take work for `model` now: signed in, not in `avoid`, no limit hit
/// and no window that limits it at the share routing counts as used up.
fn can_take(
    candidate: &Candidate,
    provider: ProviderKind,
    model: Option<&str>,
    avoid: &[AccountRef],
) -> bool {
    candidate.account.provider == provider
        && candidate.logged_in
        && !avoid.contains(&candidate.account)
        && candidate
            .quota
            .as_ref()
            .is_none_or(|quota| headroom(quota, model).is_some() && !used_up(quota, model))
}

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// What decides a ready account's place: the day its weekly window resets (counted from now;
/// unknown last), then how much is left in its tightest window (5-hour or weekly; unknown
/// counts as full).
fn rank(candidate: &Candidate, model: Option<&str>, now: i64) -> (Option<i64>, f64) {
    let day = candidate
        .quota
        .as_ref()
        .and_then(|quota| weekly_reset(quota, model))
        .map(|at| (at - now).max(0) / DAY_MS);
    let left = candidate
        .quota
        .as_ref()
        .and_then(|quota| headroom(quota, model))
        .unwrap_or(100.0);
    (day, left)
}

/// The accounts that can take work, in the order work goes to them (switching on, D23): the
/// one whose weekly window resets on the soonest day first, since its quota is lost first;
/// within a day, the one nearest its limit (least left in its tightest window, 5-hour or
/// weekly), so it drains while the other stays whole; then the earliest added. An unknown
/// reset sorts last.
pub fn smart_order(
    settings: &Settings,
    provider: ProviderKind,
    model: Option<&str>,
    candidates: &[Candidate],
    avoid: &[AccountRef],
    now: i64,
) -> Vec<AccountRef> {
    let mut ready: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| can_take(candidate, provider, model, avoid))
        .collect();
    ready.sort_by(|a, b| {
        let ((a_day, a_left), (b_day, b_left)) = (rank(a, model, now), rank(b, model, now));
        a_day
            .is_none()
            .cmp(&b_day.is_none())
            .then_with(|| a_day.cmp(&b_day))
            .then_with(|| a_left.total_cmp(&b_left))
            .then_with(|| added(settings, &a.account).cmp(&added(settings, &b.account)))
    });
    ready
        .into_iter()
        .map(|candidate| candidate.account.clone())
        .collect()
}

/// Why an account stands where it does in [`order`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OrderReason {
    /// Picked with Use (switching off): work goes here while it can take it.
    Picked,
    /// Its weekly window resets on an earlier day than the next one's: its quota is lost first.
    ResetsSooner { resets_at_ms: i64 },
    /// Its weekly window resets the same day as the next one's, and it has less left in its
    /// tightest window (`window`, the window's label): it drains first.
    NearerLimit { window: String, used_percent: f64 },
    /// Level with the next one: it was added first.
    Added,
    /// The last one that can take work.
    Last,
    /// It can't take work now (a limit, or a window used up): until `ready_at_ms`, when known.
    AtLimit {
        #[ts(optional)]
        #[serde(default, skip_serializing_if = "Option::is_none")]
        ready_at_ms: Option<i64>,
    },
    /// Not signed in.
    SignedOut,
}

/// One account's place in the order work goes to its provider's accounts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AccountOrder {
    pub account: AccountRef,
    /// 1 for the account work goes to first; `None` for one that can't take work now.
    pub place: Option<u32>,
    pub reason: OrderReason,
}

/// Every account of `provider` in the order work goes to them, each with why: the picked one
/// first with switching off, then [`smart_order`]'s, then those that can't take work now.
pub fn order(
    settings: &Settings,
    provider: ProviderKind,
    model: Option<&str>,
    candidates: &[Candidate],
    now: i64,
) -> Vec<AccountOrder> {
    let mut accounts = smart_order(settings, provider, model, candidates, &[], now);
    let picked = !settings.switches_accounts(provider);
    let default = default_account(settings, provider);
    if picked && let Some(at) = accounts.iter().position(|account| *account == default) {
        let first = accounts.remove(at);
        accounts.insert(0, first);
    }
    let find = |account: &AccountRef| {
        candidates
            .iter()
            .find(|candidate| candidate.account == *account)
    };
    let count = accounts.len();
    let mut placed: Vec<AccountOrder> = accounts
        .iter()
        .enumerate()
        .map(|(at, account)| {
            let reason = if picked && *account == default {
                OrderReason::Picked
            } else if at + 1 == count {
                OrderReason::Last
            } else {
                let (this, next) = (find(account), find(&accounts[at + 1]));
                match (this, next) {
                    (Some(this), Some(next)) => ahead(this, next, model, now),
                    _ => OrderReason::Added,
                }
            };
            AccountOrder {
                account: account.clone(),
                place: Some(at as u32 + 1),
                reason,
            }
        })
        .collect();
    let mut out: Vec<&Candidate> = candidates
        .iter()
        .filter(|candidate| {
            candidate.account.provider == provider && !accounts.contains(&candidate.account)
        })
        .collect();
    out.sort_by_key(|candidate| added(settings, &candidate.account));
    placed.extend(out.into_iter().map(|candidate| AccountOrder {
        account: candidate.account.clone(),
        place: None,
        reason: if candidate.logged_in {
            OrderReason::AtLimit {
                ready_at_ms: candidate
                    .quota
                    .as_ref()
                    .and_then(|quota| ready_again_at(quota, model)),
            }
        } else {
            OrderReason::SignedOut
        },
    }));
    placed
}

/// Why `this` goes before `next` in [`smart_order`].
fn ahead(this: &Candidate, next: &Candidate, model: Option<&str>, now: i64) -> OrderReason {
    let ((this_day, this_left), (next_day, _)) = (rank(this, model, now), rank(next, model, now));
    if let Some(day) = this_day
        && next_day.is_none_or(|next| day < next)
        && let Some(at) = this
            .quota
            .as_ref()
            .and_then(|quota| weekly_reset(quota, model))
    {
        return OrderReason::ResetsSooner { resets_at_ms: at };
    }
    let (_, next_left) = rank(next, model, now);
    if this_left < next_left
        && let Some(window) = this.quota.as_ref().and_then(|quota| tightest(quota, model))
    {
        return OrderReason::NearerLimit {
            window: window.label.clone(),
            used_percent: window.used_percent,
        };
    }
    OrderReason::Added
}

/// The window that applies to `model` with the least left.
fn tightest<'a>(quota: &'a QuotaSnapshot, model: Option<&str>) -> Option<&'a QuotaWindow> {
    quota
        .windows
        .iter()
        .filter(|window| applies(window, model))
        .max_by(|a, b| a.used_percent.total_cmp(&b.used_percent))
}

/// Whether a window limits `model`: a window of every model, or one scoped to it.
fn applies(window: &QuotaWindow, model: Option<&str>) -> bool {
    window
        .model
        .as_deref()
        .is_none_or(|scope| model.is_some_and(|model| limits(scope, model)))
}

const FIVE_HOURS: i64 = 5 * 60;
pub(crate) const WEEK: i64 = 7 * 24 * 60;

/// How long a window runs, in minutes: as reported, else as its id says (Claude's
/// `five_hour` and `seven_day…`, Codex's usual `primary` and `secondary`, a bucket's own
/// prefixed `bucket.primary`). Codex may report its weekly window as `primary`, so a reported
/// length wins.
pub(crate) fn span(window: &QuotaWindow) -> Option<i64> {
    let slot = window.id.rsplit('.').next().unwrap_or(&window.id);
    window.window_minutes.or(match slot {
        "five_hour" | "primary" => Some(FIVE_HOURS),
        "secondary" => Some(WEEK),
        _ if slot.starts_with("seven_day") => Some(WEEK),
        _ => None,
    })
}

/// Whether a window that applies to `model` is at the share routing counts as used up
/// (`brigadier_router::forecast::LIMITED_USED`, 97%): new work would stop at once, and routing
/// would read the whole provider as limited while this account leads it.
fn used_up(quota: &QuotaSnapshot, model: Option<&str>) -> bool {
    quota.windows.iter().any(|window| {
        applies(window, model) && window.used_percent >= brigadier_router::forecast::LIMITED_USED
    })
}

/// When the soonest weekly window that applies to `model` resets (Claude's `seven_day` and its
/// per-model ones, Codex's weekly one), if known.
fn weekly_reset(quota: &QuotaSnapshot, model: Option<&str>) -> Option<i64> {
    quota
        .windows
        .iter()
        .filter(|window| span(window) == Some(WEEK) && applies(window, model))
        .filter_map(|window| window.resets_at_ms)
        .min()
}

/// When an account that can't take work for `model` now can again: once the limit it hit and
/// every used-up window that applies have reset. `None` when one of those resets isn't known,
/// or nothing stops it.
pub fn ready_again_at(quota: &QuotaSnapshot, model: Option<&str>) -> Option<i64> {
    quota
        .windows
        .iter()
        .filter(|window| {
            applies(window, model)
                && window.used_percent >= brigadier_router::forecast::LIMITED_USED
        })
        .map(|window| window.resets_at_ms)
        .chain(quota.limit.as_ref().map(|limit| limit.resets_at_ms))
        .try_fold(None, |latest: Option<i64>, at| {
            at.map(|at| Some(latest.map_or(at, |latest| latest.max(at))))
        })
        .flatten()
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
        .filter(|window| applies(window, model))
        .map(|window| 100.0 - window.used_percent)
        .fold(100.0_f64, f64::min);
    (left > 0.0).then_some(left)
}

/// Whether a window scoped to `scope` limits `model` as a choice names it: by the same name,
/// or by the family word the name carries (Claude's `opus` window limits `opus[1m]` and
/// `claude-opus-5-5`), as `brigadier_router::window_applies` matches a catalog model.
fn limits(scope: &str, model: &str) -> bool {
    let words = |id: &str| -> Vec<String> {
        id.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect()
    };
    let scope = scope.trim();
    if scope.is_empty() || scope.eq_ignore_ascii_case(model) {
        return true;
    }
    match words(scope).as_slice() {
        [word] => words(model).contains(word),
        _ => false,
    }
}

/// An account's email as the app shows it outside Settings › Providers: the first three
/// characters of who, then dots, the domain's ending kept (`ste•••••@•••••.com`), like the
/// app's own `maskEmail`. Text without an `@` comes back as it is.
pub fn mask_email(email: &str) -> String {
    let Some((who, domain)) = email.rsplit_once('@') else {
        return email.to_owned();
    };
    let start: String = who.chars().take(3).collect();
    let ending = domain.rfind('.').map_or("", |dot| &domain[dot..]);
    format!("{start}•••••@•••••{ending}")
}

/// Where an account stands in the list: the user's own login first, then the extra ones as
/// added.
fn added(settings: &Settings, account: &AccountRef) -> usize {
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

    #[test]
    fn an_email_shows_only_its_start_and_ending() {
        assert_eq!(
            mask_email("stephen.golban@example.com"),
            "ste•••••@•••••.com"
        );
        assert_eq!(mask_email("me@mail.example.co.uk"), "me•••••@•••••.uk");
        assert_eq!(mask_email("Work account"), "Work account");
    }

    #[test]
    fn an_unknown_reset_is_left_out_as_the_app_reads_it() {
        let unknown = OrderReason::AtLimit { ready_at_ms: None };
        assert_eq!(
            serde_json::to_value(&unknown).unwrap(),
            serde_json::json!({"kind": "atLimit"})
        );
    }
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
            manual_accounts: if switch {
                vec![]
            } else {
                vec![ProviderKind::Claude]
            },
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
        select(settings, ProviderKind::Claude, None, candidates, &[], 0).account
    }

    /// `candidate` with a weekly window that resets in `hours` (unknown when `None`).
    fn weekly(id: Option<&str>, used: f64, hours: Option<i64>) -> Candidate {
        let mut candidate = candidate(id, used, false);
        candidate.quota.as_mut().unwrap().windows.push(QuotaWindow {
            id: "seven_day".into(),
            label: "Weekly".into(),
            used_percent: 10.0,
            resets_at_ms: hours.map(|hours| hours * 3_600_000),
            window_minutes: Some(7 * 24 * 60),
            bucket: None,
            model: None,
        });
        candidate
    }

    #[test]
    fn the_account_whose_week_resets_on_the_soonest_day_takes_work() {
        let all = [
            weekly(None, 0.0, Some(90)),
            weekly(Some("work"), 60.0, Some(20)),
            weekly(Some("home"), 0.0, Some(50)),
        ];
        assert_eq!(pick(&settings(None, true), &all), Some("work".into()));
        // Being the default wins nothing.
        assert_eq!(
            pick(&settings(Some("home"), true), &all),
            Some("work".into())
        );
        // Within the same day, the one nearest its limit (D23), then the earliest added.
        let tied = [
            weekly(None, 30.0, Some(20)),
            weekly(Some("work"), 40.0, Some(23)),
            weekly(Some("home"), 40.0, Some(21)),
        ];
        assert_eq!(pick(&settings(None, true), &tied), Some("work".into()));
        // A day later is later, however near its limit.
        let days = [
            weekly(None, 90.0, Some(30)),
            weekly(Some("work"), 10.0, Some(20)),
        ];
        assert_eq!(pick(&settings(None, true), &days), Some("work".into()));
    }

    #[test]
    fn the_order_says_why_each_account_stands_where_it_does() {
        let mut signed_out = weekly(Some("home"), 0.0, Some(1));
        signed_out.logged_in = false;
        let mut limited = candidate(Some("spare"), 100.0, true);
        let spent = limited.quota.as_mut().unwrap();
        spent.limit.as_mut().unwrap().resets_at_ms = Some(7_000);
        spent.windows[0].resets_at_ms = Some(7_000);
        let mut settings = settings(None, true);
        settings.accounts.push(AccountEntry {
            id: "spare".into(),
            provider: ProviderKind::Claude,
            name: "spare".into(),
            default: false,
            added_at_ms: 0,
        });
        let all = [
            weekly(None, 80.0, Some(20)),
            weekly(Some("work"), 10.0, Some(22)),
            signed_out,
            limited,
            weekly(Some("other"), 50.0, Some(60)),
        ];
        let order = order(&settings, ProviderKind::Claude, None, &all, 0);
        let seen: Vec<_> = order
            .iter()
            .map(|entry| {
                (
                    entry.account.account.as_deref(),
                    entry.place,
                    entry.reason.clone(),
                )
            })
            .collect();
        assert_eq!(
            seen,
            [
                (
                    None,
                    Some(1),
                    OrderReason::NearerLimit {
                        window: "5-hour".into(),
                        used_percent: 80.0
                    }
                ),
                (
                    Some("work"),
                    Some(2),
                    OrderReason::ResetsSooner {
                        resets_at_ms: 22 * 3_600_000
                    }
                ),
                (Some("other"), Some(3), OrderReason::Last),
                (Some("home"), None, OrderReason::SignedOut),
                (
                    Some("spare"),
                    None,
                    OrderReason::AtLimit {
                        ready_at_ms: Some(7_000)
                    }
                ),
            ]
        );
        // Switching off: the picked account first, then the order.
        let mut off = settings.clone();
        off.manual_accounts = vec![ProviderKind::Claude];
        off.accounts[1].default = true;
        // Picked but signed out: the order goes on without it (#505).
        let order = super::order(&off, ProviderKind::Claude, None, &all, 0);
        assert_eq!(order[0].account.account, None);
        let home = order
            .iter()
            .find(|entry| entry.account.account.as_deref() == Some("home"));
        assert_eq!(
            home.map(|entry| (entry.place, &entry.reason)),
            Some((None, &OrderReason::SignedOut))
        );
        off.accounts[1].default = false;
        off.accounts[0].default = true;
        let order = super::order(&off, ProviderKind::Claude, None, &all, 0);
        assert_eq!(
            (order[0].account.account.as_deref(), &order[0].reason),
            (Some("work"), &OrderReason::Picked)
        );
        assert_eq!(order[1].account.account, None);
    }

    #[test]
    fn an_account_at_99_percent_of_its_5_hour_window_is_skipped() {
        let all = [
            weekly(None, 99.0, Some(5)),
            weekly(Some("work"), 96.0, Some(20)),
            weekly(Some("home"), 0.0, Some(50)),
        ];
        assert_eq!(pick(&settings(None, true), &all), Some("work".into()));
        // At the share routing counts as used up (97%), of any window, it is skipped too:
        // routing would read the whole provider as limited while it leads.
        let all = [
            weekly(None, 99.0, Some(5)),
            weekly(Some("work"), 98.0, Some(20)),
            weekly(Some("home"), 0.0, Some(50)),
        ];
        assert_eq!(pick(&settings(None, true), &all), Some("home".into()));
        // Codex names its 5-hour window `primary` and its weekly one `secondary`.
        let codex = |id: Option<&str>, five_hour: f64, hours: i64| {
            let mut candidate = candidate(id, 0.0, false);
            candidate.quota.as_mut().unwrap().windows = vec![
                QuotaWindow {
                    id: "primary".into(),
                    label: "5-hour".into(),
                    used_percent: five_hour,
                    resets_at_ms: Some(3_600_000),
                    window_minutes: Some(300),
                    bucket: Some("codex".into()),
                    model: None,
                },
                QuotaWindow {
                    id: "secondary".into(),
                    label: "Weekly".into(),
                    used_percent: 10.0,
                    resets_at_ms: Some(hours * 3_600_000),
                    window_minutes: Some(7 * 24 * 60),
                    bucket: Some("codex".into()),
                    model: None,
                },
            ];
            candidate
        };
        let all = [
            codex(None, 99.5, 5),
            codex(Some("work"), 10.0, 30),
            codex(Some("home"), 10.0, 20),
        ];
        assert_eq!(pick(&settings(None, true), &all), Some("home".into()));
        // A weekly window Codex reports as `primary` is read by its length: the week, not a
        // 5-hour window.
        let weekly_primary = |id: Option<&str>, used: f64, hours: i64| {
            let mut candidate = codex(id, 0.0, hours);
            let windows = &mut candidate.quota.as_mut().unwrap().windows;
            windows.truncate(1);
            windows[0].used_percent = used;
            windows[0].window_minutes = Some(7 * 24 * 60);
            windows[0].resets_at_ms = Some(hours * 3_600_000);
            candidate
        };
        let all = [
            weekly_primary(None, 50.0, 5),
            weekly_primary(Some("work"), 10.0, 30),
        ];
        assert_eq!(pick(&settings(None, true), &all), None);
        let all = [
            weekly_primary(None, 98.0, 5),
            weekly_primary(Some("work"), 10.0, 30),
        ];
        assert_eq!(pick(&settings(None, true), &all), Some("work".into()));
    }

    /// The order consumes the monitor's snapshots, as runtime does: an exhausted 5-hour
    /// account has no place before its reset and rejoins without a fresh usage read.
    #[tokio::test]
    async fn a_five_hour_limit_rejoins_the_order_at_its_reset() {
        let monitor = crate::routing::QuotaMonitor::load(None, 0).await;
        let mut limited = weekly(None, 100.0, Some(20));
        let quota = limited.quota.as_mut().unwrap();
        quota.windows[0].resets_at_ms = Some(1_000);
        quota.limit = Some(LimitHit {
            kind: LimitKind::UsageWindow,
            window: Some("five_hour".into()),
            resets_at_ms: Some(1_000),
        });
        let other = weekly(Some("work"), 40.0, Some(60));
        for candidate in [&limited, &other] {
            monitor.note(&candidate.account, candidate.quota.as_ref().unwrap(), 0);
        }
        let settings = settings(None, true);
        for now in [0, 999, 1_000, 1_001] {
            let all: Vec<_> = [&limited, &other]
                .into_iter()
                .map(|candidate| Candidate {
                    quota: monitor.current_for(&candidate.account, now),
                    ..candidate.clone()
                })
                .collect();
            let placed = order(&settings, ProviderKind::Claude, None, &all, now);
            let own = placed
                .iter()
                .find(|entry| entry.account.account.is_none())
                .unwrap();
            if now < 1_000 {
                assert_eq!(own.place, None);
                assert_eq!(
                    own.reason,
                    OrderReason::AtLimit {
                        ready_at_ms: Some(1_000)
                    }
                );
                assert_eq!(placed[0].account.account.as_deref(), Some("work"));
            } else {
                assert_eq!(own.place, Some(1), "at {now}: {placed:?}");
                assert_eq!(placed[0].account.account, None);
                assert!(all[0].quota.as_ref().unwrap().limit.is_none());
            }
        }
    }

    #[test]
    fn an_unknown_weekly_reset_sorts_last() {
        let mut unread = candidate(Some("home"), 0.0, false);
        unread.quota = None;
        let all = [
            weekly(None, 0.0, None),
            weekly(Some("work"), 90.0, Some(150)),
            unread,
        ];
        assert_eq!(pick(&settings(None, true), &all), Some("work".into()));
    }

    #[test]
    fn with_switching_off_the_default_is_the_only_choice() {
        let all = [
            weekly(None, 0.0, Some(90)),
            weekly(Some("work"), 0.0, Some(20)),
            weekly(Some("home"), 0.0, Some(50)),
        ];
        assert_eq!(pick(&settings(None, false), &all), None);
        assert_eq!(
            pick(&settings(Some("home"), false), &all),
            Some("home".into())
        );
    }

    #[test]
    fn with_no_reset_known_the_account_nearest_its_limit_takes_work() {
        let all = [
            candidate(None, 100.0, true),
            candidate(Some("work"), 60.0, false),
            candidate(Some("home"), 20.0, false),
        ];
        assert_eq!(pick(&settings(None, true), &all), Some("work".into()));
        // Switching off with the default limited: another account that can take it (#505).
        assert_eq!(pick(&settings(None, false), &all), Some("work".into()));
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
                &avoid,
                0
            )
            .account,
            None
        );
    }

    #[test]
    fn use_pins_its_provider_and_a_rename_does_not() {
        let current = settings(None, true);
        let mut renamed = current.clone();
        renamed.accounts[0].name = "Work".into();
        assert!(edited(&current, renamed).manual_accounts.is_empty());
        let mut picked = current.clone();
        picked.accounts[1].default = true;
        let after = edited(&current, picked);
        assert_eq!(after.manual_accounts, [ProviderKind::Claude]);
        // Back to the computer's login is a pick too.
        let mut own = after.clone();
        own.manual_accounts.clear();
        own.accounts[1].default = false;
        assert_eq!(edited(&after, own).manual_accounts, [ProviderKind::Claude]);
        // The own login's name: trimmed, one per agent, and cleared back to its email.
        let mut named = current.clone();
        named.own_account_names = vec![
            OwnAccountName {
                provider: ProviderKind::Claude,
                name: "  Personal ".into(),
            },
            OwnAccountName {
                provider: ProviderKind::Claude,
                name: "Second".into(),
            },
            OwnAccountName {
                provider: ProviderKind::Codex,
                name: "   ".into(),
            },
        ];
        let named = edited(&current, named);
        assert_eq!(named.own_account_name(ProviderKind::Claude), "Personal");
        assert_eq!(named.own_account_name(ProviderKind::Codex), "");
        assert_eq!(named.own_account_names.len(), 1);
        // Naming the own login is no pick: switching stays on.
        assert!(named.manual_accounts.is_empty());
        let mut cleared = named.clone();
        cleared.own_account_names[0].name = String::new();
        assert!(edited(&named, cleared).own_account_names.is_empty());
    }

    #[test]
    fn each_agent_switches_its_own_accounts() {
        let all = [
            weekly(None, 0.0, Some(90)),
            weekly(Some("work"), 0.0, Some(90)),
            weekly(Some("home"), 0.0, Some(2)),
        ];
        let mut on = settings(Some("work"), true);
        // Another agent's switch off leaves Claude's choice to Brigadier.
        on.manual_accounts = vec![ProviderKind::Codex];
        assert_eq!(pick(&on, &all), Some("home".into()));
        let mut off = on.clone();
        off.manual_accounts.push(ProviderKind::Claude);
        assert_eq!(pick(&off, &all), Some("work".into()));
    }

    #[test]
    fn a_picked_account_takes_work_while_it_can_and_another_after() {
        let off = settings(Some("work"), false);
        let mut all = vec![
            candidate(None, 10.0, false),
            candidate(Some("work"), 20.0, false),
            candidate(Some("home"), 0.0, false),
        ];
        // The picked one while it has room, though another has more.
        assert_eq!(pick(&off, &all), Some("work".into()));
        // At its limit, another account of the provider before another provider (#505), in
        // the order: the one nearest its limit.
        all[1] = candidate(Some("work"), 100.0, true);
        assert_eq!(pick(&off, &all), None);
        let moved = ready(
            &off,
            ProviderKind::Claude,
            None,
            &all,
            &[AccountRef::own(ProviderKind::Claude)],
            0,
        );
        assert_eq!(
            moved.and_then(|account| account.account),
            Some("home".into())
        );
        // None ready: the picked one, read as limited.
        let spent = [
            candidate(None, 100.0, true),
            candidate(Some("work"), 100.0, true),
            candidate(Some("home"), 100.0, true),
        ];
        assert_eq!(pick(&off, &spent), Some("work".into()));
        assert_eq!(
            ready(&off, ProviderKind::Claude, None, &spent, &[], 0),
            None
        );
    }

    #[test]
    fn an_edit_renames_and_picks_the_default_but_adds_and_removes_nothing() {
        let current = settings(None, true);
        let mut incoming = current.clone();
        incoming.accounts[0].name = " Work ".into();
        incoming.accounts[0].default = true;
        incoming.accounts[1].default = true;
        incoming.accounts[1].provider = ProviderKind::Codex;
        incoming.accounts.push(AccountEntry {
            id: "new".into(),
            provider: ProviderKind::Claude,
            name: "new".into(),
            default: false,
            added_at_ms: 0,
        });
        let after = edited(&current, incoming.clone());
        let ids: Vec<_> = after.accounts.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["work", "home"]);
        assert_eq!(after.accounts[0].name, "Work");
        // One default per provider, and an account keeps its provider.
        assert!(after.accounts[0].default && !after.accounts[1].default);
        assert_eq!(after.accounts[1].provider, ProviderKind::Claude);
        incoming.accounts.clear();
        assert_eq!(edited(&current, incoming).accounts.len(), 2);
    }

    #[test]
    fn a_capped_account_is_ready_again_once_everything_that_stops_it_reset() {
        let mut capped = quota(100.0, true);
        assert_eq!(ready_again_at(&capped, None), None, "no reset known");
        capped.windows[0].resets_at_ms = Some(1_000);
        capped.limit.as_mut().unwrap().resets_at_ms = Some(1_000);
        capped.windows.push(QuotaWindow {
            id: "seven_day".into(),
            label: "Weekly".into(),
            used_percent: 100.0,
            resets_at_ms: Some(9_000),
            window_minutes: None,
            bucket: None,
            model: None,
        });
        // The week is used up too: only its reset frees the account.
        assert_eq!(ready_again_at(&capped, None), Some(9_000));
        capped.windows[1].used_percent = 40.0;
        assert_eq!(ready_again_at(&capped, None), Some(1_000));
        assert_eq!(
            ready_again_at(&quota(20.0, false), None),
            None,
            "nothing stops it"
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
        let all = [own, candidate(Some("work"), 5.0, false)];
        let settings = settings(None, true);
        assert_eq!(
            select(&settings, ProviderKind::Claude, Some("opus"), &all, &[], 0).account,
            Some("work".into())
        );
        assert_eq!(
            select(
                &settings,
                ProviderKind::Claude,
                Some("sonnet"),
                &all,
                &[],
                0
            )
            .account,
            None
        );
        // The same window limits the family under its other names.
        for name in ["opus[1m]", "claude-opus-5-5", "Opus"] {
            assert_eq!(
                select(&settings, ProviderKind::Claude, Some(name), &all, &[], 0).account,
                Some("work".into()),
                "{name}"
            );
        }
        assert_eq!(
            select(
                &settings,
                ProviderKind::Claude,
                Some("claude-sonnet-5-5"),
                &all,
                &[],
                0
            )
            .account,
            None
        );
    }
}
