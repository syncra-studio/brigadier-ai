//! The app call (Q90): what an agent asks of Brigadier's own window, which the app does there
//! as the user's click would, and what comes back. The browser tool's calls are one kind; the
//! rest open, show and read what the session has on screen. A call never brings the window
//! forward or switches the session the user is on.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::browser::{BrowserOp, BrowserResult};
use crate::{ConversationId, TaskId};

/// A call an agent made of the app's window, for the app to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct AppCall {
    /// Answered with `appAnswer` and this id.
    pub id: String,
    pub conversation_id: ConversationId,
    /// The worker that made it; the thread's own when absent.
    pub task_id: Option<TaskId>,
    /// The agent as the user knows it: its thread's or task's title.
    pub label: String,
    pub op: AppOp,
}

/// What a call asks of the app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "app", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AppOp {
    /// A Browser tab's call (Q74).
    Browser { op: BrowserOp },
}

impl AppOp {
    /// What can't be used when no window answers, as the agent hears it.
    pub(crate) fn what(&self) -> &'static str {
        match self {
            Self::Browser { .. } => "its browser",
        }
    }
}

/// What the app answers a call with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "app", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum AppAnswer {
    Browser {
        result: BrowserResult,
    },
    /// Nothing was done: why, in plain words.
    Failed {
        message: String,
    },
}
