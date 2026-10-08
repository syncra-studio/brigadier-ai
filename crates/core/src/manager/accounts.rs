//! Settings → Accounts: adding and removing the user's extra CLI accounts. Signing one in is
//! the CLI's own sign-in, run by the daemon in a terminal with the account's home.

use brigadier_providers::ProviderKind;

use super::SessionManager;
use crate::accounts::AccountRef;
use crate::model::{AccountEntry, Settings};
use crate::{Error, Result, now_ms};

impl SessionManager {
    /// Adds an extra account of `provider`: its home is made, linked to the user's own, and
    /// ready for the CLI's own sign-in.
    pub async fn add_account(&self, provider: ProviderKind) -> Result<AccountEntry> {
        let entry = AccountEntry {
            id: format!("acct-{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            provider,
            name: String::new(),
            default: false,
            added_at_ms: now_ms(),
        };
        let mut settings = self.core.settings();
        settings.accounts.push(entry.clone());
        self.core.update_settings(settings).await?;
        self.runtime.sync_accounts().await;
        if !self
            .runtime
            .account_homes(provider)
            .contains(&self.runtime.account_home(&entry.id))
        {
            return Err(Error::Invalid(
                "the account's folder couldn't be set up".into(),
            ));
        }
        // The folders the user trusts are trusted on the new account too.
        if provider == ProviderKind::Claude {
            self.reconcile_trust().await;
        }
        Ok(entry)
    }

    /// Removes extra account `id`: the CLI signs it out of its own home, settings forget it
    /// (a default or a default model on it falls back to the user's own login), and its home
    /// goes. Refused while a chat's turn or a worker runs on it; nothing new starts on it
    /// meanwhile.
    pub async fn remove_account(&self, id: &str) -> Result<Settings> {
        let entry = self
            .core
            .settings()
            .accounts
            .into_iter()
            .find(|entry| entry.id == id)
            .ok_or_else(|| Error::NotFound(format!("account {id}")))?;
        let _removing = self.runtime.start_removing(id)?;
        let account = AccountRef::new(entry.provider, Some(id.to_owned()));
        if self.account_in_use(&account).await {
            return Err(Error::Invalid(
                "A chat or worker is working on this account. Wait for it to finish, or stop it, then remove the account."
                    .into(),
            ));
        }
        if let Err(err) = self.runtime.sign_out(&entry).await {
            tracing::warn!(account = id, error = %err, "the account's CLI didn't sign out");
        }
        let mut settings = self.core.settings();
        settings.accounts.retain(|entry| entry.id != id);
        for choice in [
            settings.default_orchestrator.as_mut(),
            settings.default_chat_model.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            if choice.account.as_deref() == Some(id) {
                choice.account = None;
            }
        }
        let settings = self.core.update_settings(settings).await?;
        self.runtime.sync_accounts().await;
        self.runtime.remove_account_home(id).await?;
        tracing::info!(account = id, provider = %entry.provider, "account removed");
        Ok(settings)
    }

    /// Whether a chat's turn or a worker runs on `account`. A chat's idle CLI on it is
    /// closed: its next turn resumes the session on another account.
    async fn account_in_use(&self, account: &AccountRef) -> bool {
        let convs: Vec<_> = self.convs_lock().values().cloned().collect();
        for conv in convs {
            let Some(cli) = conv.live_cli().await.filter(|cli| cli.account == *account) else {
                continue;
            };
            conv.retire_cli(&cli).await;
            if conv
                .live_cli()
                .await
                .is_some_and(|now| std::sync::Arc::ptr_eq(&now, &cli))
            {
                return true;
            }
        }
        let tasks: Vec<_> = self.tasks_lock().values().cloned().collect();
        for task in tasks {
            if task.cli().await.is_some_and(|cli| cli.account == *account) {
                return true;
            }
        }
        false
    }
}
