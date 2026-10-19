//! The browser tool (Q74): what an agent asks of a Browser tab in its session, which the app
//! runs on the tab's page, and what comes back. The tab is a normal Browser tab, with the
//! user's sign-ins; the app opens it in the background and the user can switch to it and watch.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{ConversationId, TaskId};

/// A call an agent made of its session's Browser tabs, for the app to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserCall {
    /// Answered with `browserAnswer` and this id.
    pub id: String,
    pub conversation_id: ConversationId,
    /// The worker that made it; the thread's own when absent.
    pub task_id: Option<TaskId>,
    /// The agent as the user knows it: its thread's or task's title.
    pub label: String,
    pub op: BrowserOp,
}

/// What a call asks of a tab. Every op but `open` names a tab of the session: an agent's for
/// acting, any for reading, a picture and `show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BrowserOp {
    /// Opens `url` in a Browser tab of the session, behind the one the user is on: the tab an
    /// agent already has on that page, else a new one. `request`: the request it is for, at
    /// whose end it closes unless `keep` (it is for the user).
    Open {
        url: String,
        #[serde(default)]
        keep: bool,
        #[serde(default)]
        request: Option<String>,
    },
    Navigate {
        tab: String,
        url: String,
    },
    Go {
        tab: String,
        action: BrowserGo,
    },
    /// The page as text, with a ref for each control.
    Read {
        tab: String,
    },
    /// Clicks the control with ref `target`.
    Click {
        tab: String,
        target: String,
        guard: BrowserGuard,
    },
    /// Types `text` into the field with ref `target`, then presses Enter when `submit`.
    Type {
        tab: String,
        target: String,
        text: String,
        submit: bool,
        guard: BrowserGuard,
    },
    /// A picture of the page as it shows.
    Screenshot {
        tab: String,
    },
    Close {
        tab: String,
    },
    /// Brings the tab to the front of its session (the user asked to see it).
    Show {
        tab: String,
    },
}

impl BrowserOp {
    /// The tab the op works on; none for `open`.
    pub fn tab(&self) -> Option<&str> {
        match self {
            Self::Open { .. } => None,
            Self::Navigate { tab, .. }
            | Self::Go { tab, .. }
            | Self::Read { tab }
            | Self::Click { tab, .. }
            | Self::Type { tab, .. }
            | Self::Screenshot { tab }
            | Self::Close { tab }
            | Self::Show { tab } => Some(tab),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BrowserGo {
    Back,
    Forward,
    Reload,
}

/// What may make an action ask the user first (Q67: money, credentials and anything sent in
/// the user's name ask, whoever asked for it).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserGuard {
    /// What the agent said the action does.
    pub declared: Option<BrowserSensitive>,
    /// The page on which the user said yes to this very action.
    pub approved: Option<BrowserApproved>,
}

/// The page a yes was given on: it holds there only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserApproved {
    pub generation: u64,
    pub url: String,
}

/// Why an action asks the user first: the first three the agent declares, the rest the page's
/// fields show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BrowserSensitive {
    /// Pays or commits money.
    Pay,
    /// Sends something in the user's name (a message, a post, a form to someone).
    Send,
    /// Signs in as the user.
    SignIn,
    /// Enters or submits a password.
    Credentials,
    /// Enters or submits a payment card.
    Card,
    /// Enters or submits a one-time code.
    OneTimeCode,
}

/// What the app answers a call with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "result",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BrowserResult {
    /// The tab's page after the call. `user`: the tab is the user's, read only.
    Page {
        tab: String,
        page: BrowserPageRead,
        #[serde(default)]
        user: bool,
    },
    /// The tab is in front of its session now.
    Shown {
        tab: String,
    },
    /// A PNG of the page, base64.
    Shot {
        tab: String,
        url: String,
        png: String,
    },
    Closed {
        tab: String,
    },
    /// The user is using the tab: nothing was done.
    Paused {
        tab: String,
    },
    /// The action needs the user's yes first: nothing was done.
    NeedsApproval {
        tab: String,
        url: String,
        /// The page it was asked on: a yes holds for it only.
        generation: u64,
        sensitive: BrowserSensitive,
        /// The control, as the page names it.
        what: String,
    },
    Failed {
        message: String,
    },
}

/// A tab's page, read for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrowserPageRead {
    pub url: String,
    pub title: String,
    /// Counts the tab's page loads: refs and a yes hold within one.
    pub generation: u64,
    pub loading: bool,
    /// The page's text, each control in it as `[ref] role "name"`.
    pub text: String,
}
