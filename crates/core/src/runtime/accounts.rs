//! The user's extra CLI accounts at run time: an adapter per account, its login and quota as
//! last checked, and which account work starts on ([`crate::accounts::select`]).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use brigadier_providers::claude::Claude;
use brigadier_providers::codex::Codex;
use brigadier_providers::{Provider, ProviderKind, ProviderStatus, QuotaSnapshot};
use brigadier_store::Retention;

use super::{Runtime, new_event};
use crate::accounts::{AccountRef, Candidate};
use crate::model::{AccountEntry, AccountView, AccountsView, DomainEvent, streams};
use crate::{Error, Result, now_ms};

/// One extra account, live.
pub(super) struct AccountLive {
    pub entry: AccountEntry,
    pub provider: Arc<dyn Provider>,
    pub status: Option<ProviderStatus>,
    pub checked_at_ms: Option<i64>,
    pub checking: bool,
    pub error: Option<String>,
}

/// Snapshots of Settings → Accounts kept in the event log.
const ACCOUNT_CHECKS_KEPT: u32 = 20;

/// Tests: makes the stand-in CLI of an extra account.
#[cfg(test)]
pub(crate) type FakeAccounts = Arc<dyn Fn(&AccountRef) -> Arc<dyn Provider> + Send + Sync>;

impl Runtime {
    /// Where extra accounts' CLI homes are: `<data dir>/accounts/<id>`.
    pub fn account_home(&self, id: &str) -> PathBuf {
        self.platform.paths().data_dir.join("accounts").join(id)
    }

    /// The adapter `account` runs on. A removed account has none.
    pub fn provider_for(&self, account: &AccountRef) -> Result<Arc<dyn Provider>> {
        match &account.account {
            None => Ok(self.provider(account.provider)),
            Some(id) if self.is_removing(id) => Err(Error::Invalid(
                "that account is being removed in Settings → Accounts".into(),
            )),
            Some(id) => self
                .accounts()
                .get(id)
                .filter(|live| live.entry.provider == account.provider)
                .map(|live| live.provider.clone())
                .ok_or_else(|| {
                    Error::Invalid("that account was removed in Settings → Accounts".into())
                }),
        }
    }

    /// The adapter of the extra account whose CLI home is `home`, while it is set up.
    pub(super) fn account_by_home(&self, home: &str) -> Option<Arc<dyn Provider>> {
        self.accounts()
            .values()
            .find(|live| self.account_home(&live.entry.id) == std::path::Path::new(home))
            .map(|live| live.provider.clone())
    }

    /// The homes of `provider`'s extra accounts set up now.
    pub fn account_homes(&self, provider: ProviderKind) -> Vec<PathBuf> {
        self.accounts()
            .values()
            .filter(|live| live.entry.provider == provider)
            .map(|live| self.account_home(&live.entry.id))
            .collect()
    }

    pub(super) fn accounts(&self) -> std::sync::MutexGuard<'_, HashMap<String, AccountLive>> {
        self.accounts
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Brings the live accounts in line with settings: a new one gets its home and adapter
    /// and is checked; a removed one is dropped (its home goes with
    /// [`Self::forget_account`]). Then each provider's lead is chosen again.
    pub async fn sync_accounts(self: &Arc<Self>) {
        let settings = self.core.settings();
        let mut added = Vec::new();
        {
            let mut accounts = self.accounts();
            accounts.retain(|id, _| settings.accounts.iter().any(|entry| entry.id == *id));
            for entry in &settings.accounts {
                if let Some(live) = accounts.get_mut(&entry.id) {
                    live.entry = entry.clone();
                } else {
                    added.push(entry.clone());
                }
            }
        }
        for entry in added {
            match self.make_account(&entry).await {
                Ok(provider) => {
                    self.accounts().insert(
                        entry.id.clone(),
                        AccountLive {
                            entry: entry.clone(),
                            provider,
                            status: None,
                            checked_at_ms: None,
                            checking: false,
                            error: None,
                        },
                    );
                    self.refresh_account(entry.id.clone());
                }
                Err(err) => {
                    tracing::warn!(account = %entry.id, error = %err, "could not set up an account")
                }
            }
        }
        self.update_leads().await;
        self.publish_accounts().await;
    }

    /// An account's home, made (and linked to the user's own) when missing, and its adapter.
    async fn make_account(&self, entry: &AccountEntry) -> Result<Arc<dyn Provider>> {
        #[cfg(test)]
        if let Some(fakes) = &self.fake_accounts {
            return Ok(fakes(&AccountRef::new(
                entry.provider,
                Some(entry.id.clone()),
            )));
        }
        let home = self.account_home(&entry.id);
        let main = brigadier_providers::accounts::main_home(entry.provider, &self.env)
            .ok_or_else(|| Error::Invalid("no home folder for the CLI's own files".into()))?;
        let (kind, made) = (entry.provider, home.clone());
        tokio::task::spawn_blocking(move || {
            brigadier_providers::accounts::prepare_home(kind, &main, &made)
        })
        .await
        .map_err(|err| Error::Invalid(err.to_string()))?
        .map_err(|err| Error::Invalid(format!("setting up the account's folder: {err}")))?;
        Ok(match entry.provider {
            ProviderKind::Claude => {
                Arc::new(Claude::for_account(self.platform.clone(), &self.env, &home))
            }
            ProviderKind::Codex => {
                Arc::new(Codex::for_account(self.platform.clone(), &self.env, &home))
            }
        })
    }

    /// Checks an extra account in the background: its login, then its quota (a read that
    /// costs none).
    pub fn refresh_account(self: &Arc<Self>, id: String) {
        {
            let mut accounts = self.accounts();
            let Some(live) = accounts.get_mut(&id) else {
                return;
            };
            if live.checking {
                return;
            }
            live.checking = true;
        }
        let runtime = self.clone();
        self.spawn(async move {
            runtime.check_account(&id).await;
            runtime.update_leads().await;
            runtime.publish_accounts().await;
        });
    }

    /// Checks every extra account again in the background.
    pub fn refresh_accounts(self: &Arc<Self>) {
        let ids: Vec<String> = self.accounts().keys().cloned().collect();
        for id in ids {
            self.refresh_account(id);
        }
    }

    async fn check_account(&self, id: &str) {
        let Some((provider, kind)) = self
            .accounts()
            .get(id)
            .map(|live| (live.provider.clone(), live.entry.provider))
        else {
            return;
        };
        let status = provider.status().await;
        let mut error = None;
        if status.logged_in {
            match provider.quota().await {
                Ok(quota) => {
                    self.monitor.note(
                        &AccountRef::new(kind, Some(id.to_owned())),
                        &quota,
                        now_ms(),
                    );
                }
                Err(err) => error = Some(format!("quota: {err}")),
            }
        }
        if let Some(live) = self.accounts().get_mut(id) {
            live.status = Some(status);
            live.checked_at_ms = Some(now_ms());
            live.checking = false;
            live.error = error;
        }
    }

    /// Reads the quota of every signed-in extra account (the poller's turn).
    pub(super) async fn poll_accounts(&self) {
        let ready: Vec<(String, ProviderKind, Arc<dyn Provider>)> = self
            .accounts()
            .iter()
            .filter(|(_, live)| {
                !live.checking && live.status.as_ref().is_some_and(|status| status.logged_in)
            })
            .map(|(id, live)| (id.clone(), live.entry.provider, live.provider.clone()))
            .collect();
        if ready.is_empty() {
            return;
        }
        for (id, kind, provider) in ready {
            match provider.quota().await {
                Ok(quota) => {
                    self.monitor
                        .note(&AccountRef::new(kind, Some(id)), &quota, now_ms());
                }
                Err(err) => {
                    tracing::debug!(account = %id, error = %err, "account quota read failed")
                }
            }
        }
        self.update_leads().await;
        self.publish_accounts().await;
    }

    /// What is known of `provider`'s accounts, for choosing among them.
    pub fn account_candidates(&self, provider: ProviderKind) -> Vec<Candidate> {
        let now = now_ms();
        let own = AccountRef::own(provider);
        let own_logged_in = self
            .overview(provider)
            .and_then(|overview| overview.status)
            .is_some_and(|status| status.logged_in);
        let mut candidates = vec![Candidate {
            quota: self.monitor.current_for(&own, now),
            account: own,
            logged_in: own_logged_in,
        }];
        let settings = self.core.settings();
        let accounts = self.accounts();
        for entry in settings
            .accounts
            .iter()
            .filter(|entry| entry.provider == provider && !self.is_removing(&entry.id))
        {
            let account = AccountRef::new(provider, Some(entry.id.clone()));
            candidates.push(Candidate {
                quota: self.monitor.current_for(&account, now),
                logged_in: accounts
                    .get(&entry.id)
                    .and_then(|live| live.status.as_ref())
                    .is_some_and(|status| status.logged_in),
                account,
            });
        }
        candidates
    }

    /// The account new work on `provider` (for `model`) starts on.
    pub fn launch_account(&self, provider: ProviderKind, model: Option<&str>) -> AccountRef {
        crate::accounts::select(
            &self.core.settings(),
            provider,
            model,
            &self.account_candidates(provider),
            &[],
        )
    }

    /// The account work on `choice` runs on: the one it names while that is still there,
    /// otherwise as [`Self::launch_account`] says.
    pub fn account_for(&self, choice: &crate::model::ModelChoice) -> AccountRef {
        match choice.account.as_deref() {
            Some(crate::accounts::OWN) => AccountRef::own(choice.provider),
            Some(id)
                if !self.is_removing(id)
                    && self
                        .accounts()
                        .get(id)
                        .is_some_and(|live| live.entry.provider == choice.provider) =>
            {
                AccountRef::new(choice.provider, Some(id.to_owned()))
            }
            _ => self.launch_account(choice.provider, choice.model.as_deref()),
        }
    }

    /// Whether `account` is signed in, as its CLI last said.
    pub fn signed_in(&self, account: &AccountRef) -> bool {
        let status = match &account.account {
            None => self
                .overview(account.provider)
                .and_then(|overview| overview.status),
            Some(id) => self.accounts().get(id).and_then(|live| live.status.clone()),
        };
        status.is_some_and(|status| status.logged_in)
    }

    /// Whether new work of `provider` has a signed-in account to start on: its lead (the
    /// computer's own login, unless another account leads).
    pub fn provider_signed_in(&self, provider: ProviderKind) -> bool {
        self.signed_in(&self.monitor.lead(provider))
    }

    /// How the app names `account` to the user, as Settings → Accounts does: the user's own
    /// login is "this computer's login"; an extra account its name, else its email.
    pub fn account_label(&self, account: &AccountRef) -> String {
        let Some(id) = &account.account else {
            return "this computer's login".into();
        };
        let (name, status) = match self.accounts().get(id) {
            Some(live) => (live.entry.name.clone(), live.status.clone()),
            None => (String::new(), None),
        };
        let email = status.and_then(|status| status.email);
        match (name.trim(), email) {
            (name, _) if !name.is_empty() => name.to_owned(),
            (_, Some(email)) if !email.is_empty() => email,
            _ => "a new account".into(),
        }
    }

    /// Where work on `from` goes on when `from` hit its limit: another account of the same
    /// provider that can take it, when switching is on.
    pub fn switch_target(&self, from: &AccountRef, model: Option<&str>) -> Option<AccountRef> {
        let settings = self.core.settings();
        if !settings.switch_accounts {
            return None;
        }
        let mut candidates = self.account_candidates(from.provider);
        if model.is_none() {
            // The CLI's own default model, unnamed here: a model's window that is used up on
            // `from` may be what stopped it, so an account with that window used up as well
            // can't take the work either (else two such accounts would hand it back and forth).
            let used_up = |quota: &QuotaSnapshot| -> Vec<String> {
                quota
                    .windows
                    .iter()
                    .filter(|window| window.model.is_some() && window.used_percent >= 100.0)
                    .map(|window| window.id.clone())
                    .collect()
            };
            let stopped = self
                .monitor
                .current_for(from, now_ms())
                .map(|quota| used_up(&quota))
                .unwrap_or_default();
            candidates.retain(|candidate| {
                candidate.quota.as_ref().is_none_or(|quota| {
                    !used_up(quota).iter().any(|window| stopped.contains(window))
                })
            });
        }
        let next = crate::accounts::select(
            &settings,
            from.provider,
            model,
            &candidates,
            std::slice::from_ref(from),
        );
        let ready = candidates.iter().any(|candidate| {
            candidate.account == next
                && candidate.logged_in
                && candidate
                    .quota
                    .as_ref()
                    .is_none_or(|quota| crate::accounts::headroom(quota, model).is_some())
        });
        (next != *from && ready).then_some(next)
    }

    /// Chooses each provider's lead account again; a provider whose lead changed has its
    /// overview recorded again (its quota is the new lead's).
    pub(super) async fn update_leads(&self) {
        for kind in ProviderKind::ALL {
            let lead = self.launch_account(kind, None);
            if !self.monitor.set_lead(lead) {
                continue;
            }
            let overview = {
                let mut state = self.state();
                let Some(overview) = state.overviews.get_mut(&kind) else {
                    continue;
                };
                overview.quota = self.monitor.current(kind, now_ms());
                overview.clone()
            };
            self.record_overview(overview).await;
        }
    }

    /// Settings → Accounts as it stands.
    pub fn accounts_view(&self) -> AccountsView {
        let now = now_ms();
        let settings = self.core.settings();
        let mut accounts = Vec::new();
        for kind in ProviderKind::ALL {
            let own = AccountRef::own(kind);
            let overview = self.overview(kind);
            let checking = self.state().refreshing.contains(&kind);
            accounts.push(AccountView {
                quota: self.monitor.current_for(&own, now),
                quota_at_ms: self.monitor.read_at(&own),
                default: settings.default_account(kind).is_none(),
                name: String::new(),
                status: overview
                    .as_ref()
                    .and_then(|overview| overview.status.clone()),
                checked_at_ms: overview
                    .as_ref()
                    .and_then(|overview| overview.checked_at_ms),
                checking,
                error: overview.and_then(|overview| overview.error),
                account: own,
            });
            let live = self.accounts();
            for entry in settings
                .accounts
                .iter()
                .filter(|entry| entry.provider == kind)
            {
                let account = AccountRef::new(kind, Some(entry.id.clone()));
                let live = live.get(&entry.id);
                accounts.push(AccountView {
                    quota: self.monitor.current_for(&account, now),
                    quota_at_ms: self.monitor.read_at(&account),
                    default: entry.default,
                    name: entry.name.clone(),
                    status: live.and_then(|live| live.status.clone()),
                    checked_at_ms: live.and_then(|live| live.checked_at_ms),
                    checking: live.is_some_and(|live| live.checking),
                    error: live.and_then(|live| live.error.clone()),
                    account,
                });
            }
        }
        AccountsView { accounts }
    }

    /// Tells the app what Settings → Accounts shows now.
    pub(super) async fn publish_accounts(&self) {
        let event = DomainEvent::AccountsChecked {
            accounts: self.accounts_view(),
        };
        let result = async {
            let new = new_event(streams::ACCOUNTS, &event)?;
            self.core
                .store()
                .append_with(
                    vec![new],
                    Some(Retention {
                        keep_last: ACCOUNT_CHECKS_KEPT,
                    }),
                )
                .await?;
            Ok::<_, Error>(())
        }
        .await;
        if let Err(err) = result {
            tracing::warn!(error = %err, "could not record the accounts");
        }
    }

    fn is_removing(&self, id: &str) -> bool {
        self.removing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(id)
    }

    /// Marks extra account `id` as being removed until the guard is dropped: no work starts
    /// on it meanwhile. Fails while it is already being removed.
    pub fn start_removing(self: &Arc<Self>, id: &str) -> Result<RemovingAccount> {
        let mut removing = self
            .removing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !removing.insert(id.to_owned()) {
            return Err(Error::Invalid(
                "that account is being removed already".into(),
            ));
        }
        Ok(RemovingAccount {
            runtime: self.clone(),
            id: id.to_owned(),
        })
    }

    /// Signs extra account `entry` out with its CLI's own command, run in its home, so the
    /// CLI clears the login it keeps. Brigadier never sees it.
    pub async fn sign_out(&self, entry: &AccountEntry) -> Result<()> {
        #[cfg(test)]
        if self.fake_accounts.is_some() {
            return Ok(());
        }
        let home = self.account_home(&entry.id);
        if !home.is_dir() {
            return Ok(());
        }
        let program = self.env.resolve(entry.provider).ok_or_else(|| {
            Error::Invalid(format!("{} is not installed", entry.provider.label()))
        })?;
        let env = self.env.for_account(entry.provider, &home);
        let mut command = tokio::process::Command::new(program);
        command
            .args(sign_out_args(entry.provider))
            .env_clear()
            .envs(env.vars())
            .current_dir(&home)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(30), command.output())
            .await
            .map_err(|_| Error::Invalid("signing out took too long".into()))?
            .map_err(|err| Error::Invalid(format!("couldn't sign out: {err}")))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(Error::Invalid(format!(
                "signing out failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            )))
        }
    }

    /// Removes a removed account's home: the folder trust Brigadier wrote in it is undone
    /// first, then the folder goes (its links, never what they point to).
    pub async fn remove_account_home(&self, id: &str) -> Result<()> {
        let home = self.account_home(id);
        for (owner, artifacts, _) in self.ledger.owners() {
            let inside: Vec<_> = artifacts
                .into_iter()
                .filter(|artifact| match artifact {
                    brigadier_providers::Artifact::CliTrust { file, .. } => {
                        Path::new(file).starts_with(&home)
                    }
                    _ => false,
                })
                .collect();
            if !inside.is_empty() {
                let left = self.ledger.release(&owner, inside).await;
                if !left.is_clean() {
                    tracing::warn!(owner, failures = ?left.failures, "folder trust in a removed account left in place");
                }
            }
        }
        tokio::task::spawn_blocking(move || brigadier_providers::accounts::remove_home(&home))
            .await
            .map_err(|err| Error::Invalid(err.to_string()))?
            .map_err(|err| Error::Invalid(format!("couldn't remove the account's folder: {err}")))
    }
}

/// An extra account being removed; dropping it ends that.
pub struct RemovingAccount {
    runtime: Arc<Runtime>,
    id: String,
}

impl Drop for RemovingAccount {
    fn drop(&mut self) {
        self.runtime
            .removing
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&self.id);
    }
}

/// The CLI's own sign-out (`claude auth logout`, `codex logout`; both in the installed CLIs'
/// `--help`).
fn sign_out_args(kind: ProviderKind) -> &'static [&'static str] {
    match kind {
        ProviderKind::Claude => &["auth", "logout"],
        ProviderKind::Codex => &["logout"],
    }
}

/// The CLI's own sign-in (`claude auth login`, `codex login`).
pub fn sign_in_args(kind: ProviderKind) -> &'static [&'static str] {
    match kind {
        ProviderKind::Claude => &["auth", "login"],
        ProviderKind::Codex => &["login"],
    }
}
