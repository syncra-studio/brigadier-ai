//! `close_tab` (Q90): the thread closes one of the session's tabs. A tab an agent opened, with
//! nothing unsaved in it, closes at once. A tab of the user's, one with unsaved edits, or a
//! terminal running something (or whose state can't be told) closes only after the user says
//! yes on a card. The window knows whose a tab is and what it holds; the daemon knows whether
//! a terminal's shell runs a command.

use std::time::Duration;

use brigadier_providers::{ApprovalDecision, ApprovalKind, ApprovalRequest};

use super::SessionManager;
use super::cards::CardAnswer;
use super::cards::OWN_APPROVAL;
use crate::app_call::{AppAnswer, AppOp, CHAT_TAB};
use crate::model::Setup;
use crate::work::ApprovalSubject;
use crate::{ConversationId, Error, Result};

/// How long the window may take to close a tab.
const CLOSE_WAIT: Duration = Duration::from_secs(10);

/// Why closing a tab asks the user first.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CloseRisks {
    pub users: bool,
    pub unsaved: bool,
    /// A command runs in the terminal: `Some(true)`; whether one does can't be told: `None`.
    pub running: Option<bool>,
}

impl CloseRisks {
    pub(crate) fn any(self) -> bool {
        self.users || self.unsaved || self.running != Some(false)
    }

    /// The card's question, in plain words.
    pub(crate) fn question(self, title: &str) -> String {
        let mut why = Vec::new();
        if self.users {
            why.push("It's a tab you opened.");
        }
        if self.unsaved {
            why.push("It has edits that aren't saved yet: closing loses them.");
        }
        match self.running {
            Some(true) => why.push("A command is running in it: closing stops it."),
            None => why.push(
                "Whether a command runs in it can't be told: closing stops anything running.",
            ),
            Some(false) => {}
        }
        format!("Close the tab “{title}”? {}", why.join(" "))
    }
}

impl SessionManager {
    /// `close_tab`'s answer, in plain words.
    pub(crate) async fn close_tab(&self, id: &ConversationId, tab: &str) -> Result<String> {
        let tab = tab.trim();
        if tab.is_empty() {
            return Err(Error::Invalid(
                "name the tab, as `brigadier app state` lists it".into(),
            ));
        }
        if tab == CHAT_TAB {
            return Err(Error::Invalid(
                "The conversation's own tab never closes.".into(),
            ));
        }
        if !matches!(
            self.core.conversation(id)?.setup,
            Some(Setup::Session { .. })
        ) {
            return Err(Error::Invalid("A Chat has no tabs to close.".into()));
        }
        // A terminal named as terminal_run and read_terminal name it means its tab.
        let tab = self.terminal_tab(id, tab).unwrap_or_else(|| tab.to_owned());
        let tab = tab.as_str();
        let label = self.browser_label(id, None).await;
        let close = |confirmed| AppOp::CloseTab {
            tab: tab.to_owned(),
            confirmed,
        };
        let (title, risks) = match self
            .app
            .call(id, None, &label, close(false), CLOSE_WAIT)
            .await
        {
            AppAnswer::TabClosed { tab } => return Ok(self.closed(id, &tab)),
            AppAnswer::CloseAsks {
                title,
                kind,
                users,
                unsaved,
                ..
            } => {
                let running = if kind == "terminal" {
                    self.terminal_running(id, tab)
                } else {
                    Some(false)
                };
                (
                    title,
                    CloseRisks {
                        users,
                        unsaved,
                        running,
                    },
                )
            }
            AppAnswer::Failed { message } => return Err(Error::Invalid(message)),
            _ => {
                return Err(Error::Invalid(
                    "Brigadier's window answered something else.".into(),
                ));
            }
        };
        if risks.any() {
            self.confirm_close(id, &risks.question(&title)).await?;
        }
        match self
            .app
            .call(id, None, &label, close(true), CLOSE_WAIT)
            .await
        {
            AppAnswer::TabClosed { tab } => Ok(self.closed(id, &tab)),
            AppAnswer::Failed { message } => Err(Error::Invalid(message)),
            _ => Err(Error::Invalid(
                "Brigadier's window answered something else.".into(),
            )),
        }
    }

    /// Whether a command runs in the terminal tab `tab`: `None` when it can't be told. A tab
    /// whose shell is gone runs nothing.
    fn terminal_running(&self, id: &ConversationId, tab: &str) -> Option<bool> {
        let host = self.terminal_host.get()?;
        host.list(&id.0)
            .into_iter()
            .find(|terminal| terminal.key.as_deref() == Some(tab) || terminal.id == tab)
            .map_or(Some(false), |terminal| terminal.busy)
    }

    /// The tab of the terminal named `name` (`terminal-3`), when it has one.
    fn terminal_tab(&self, id: &ConversationId, name: &str) -> Option<String> {
        let host = self.terminal_host.get()?;
        host.list(&id.0)
            .into_iter()
            .find(|terminal| terminal.id == name)?
            .key
            .filter(|key| key.starts_with("terminal:"))
    }

    fn closed(&self, id: &ConversationId, tab: &str) -> String {
        self.browser.forget_tab(id, tab);
        format!("Closed {tab}.")
    }

    /// The user's yes on a card for closing a tab.
    async fn confirm_close(&self, id: &ConversationId, question: &str) -> Result<()> {
        let request = ApprovalRequest {
            id: format!("{OWN_APPROVAL}{}", uuid::Uuid::now_v7()),
            kind: ApprovalKind::Tool,
            tool: "close_tab".into(),
            command: None,
            cwd: None,
            paths: Vec::new(),
            reason: Some(question.to_owned()),
            escalation: false,
            input: None,
            grant: None,
        };
        let (_, answer) = self
            .open_approval(id, None, ApprovalSubject::Cli { request })
            .await?;
        match answer.await {
            Ok(CardAnswer::Decision(ApprovalDecision::Allow | ApprovalDecision::AllowSimilar)) => {
                Ok(())
            }
            Ok(CardAnswer::Decision(ApprovalDecision::Deny { message })) if !message.is_empty() => {
                Err(Error::Invalid(format!(
                    "The user said no, so the tab stays open: {message}"
                )))
            }
            _ => Err(Error::Invalid(
                "The user said no, so the tab stays open.".into(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_asks_for_the_user_s_tab_unsaved_edits_and_a_terminal_that_runs_or_can_t_be_told() {
        let quiet = CloseRisks {
            running: Some(false),
            ..CloseRisks::default()
        };
        assert!(!quiet.any());
        assert!(
            CloseRisks {
                users: true,
                ..quiet
            }
            .any()
        );
        assert!(
            CloseRisks {
                unsaved: true,
                ..quiet
            }
            .any()
        );
        assert!(
            CloseRisks {
                running: Some(true),
                ..quiet
            }
            .any()
        );
        assert!(
            CloseRisks {
                running: None,
                ..quiet
            }
            .any(),
            "unknown counts as running"
        );
        assert_eq!(
            CloseRisks {
                users: true,
                running: None,
                ..quiet
            }
            .question("npm run dev"),
            "Close the tab “npm run dev”? It's a tab you opened. Whether a command runs in it can't be told: closing stops anything running."
        );
    }
}
