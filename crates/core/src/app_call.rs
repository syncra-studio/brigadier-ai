//! The app call (Q90): what an agent asks of Brigadier's own window, which the app does there
//! as the user's click would, and what comes back. The browser tool's calls are one kind; the
//! rest open, show and read what the session has on screen. A call never brings the window
//! forward or switches the session the user is on.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::browser::{BrowserOp, BrowserResult};
use crate::{ConversationId, TaskId};

/// The conversation's own tab, first in every session and never closed.
pub const CHAT_TAB: &str = "chat";

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
    /// What the window shows of the session now (`app_state`'s part from the app).
    State,
    /// Opens `target` in the session's tabs (`open_in_app`): in front when `show` (the user
    /// asked to see it) and the user is on the session; else behind, marked new.
    Open { target: OpenTarget, show: bool },
}

/// What `open_in_app` opens, checked and named by the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum OpenTarget {
    /// A file of the session's checkout, by its path there, at `line`.
    File {
        path: String,
        line: Option<u32>,
    },
    /// The agent's changes, in the Review tab.
    Diff {
        scope: DiffScope,
    },
    /// The Progress tab: the job step by step, with its reviews and checks.
    Progress,
    /// What a preview or a background run prints, in an Output tab.
    Output {
        source: OutputOf,
        id: String,
        title: String,
    },
    /// A page on the web, or a preview's address, in a Browser tab.
    Url {
        url: String,
    },
    /// A worker's panel in the Workers sidebar: never behind, so only in front.
    Worker {
        task: TaskId,
    },
    /// A plan (or a lead's outline) read full screen.
    Plan {
        id: String,
        outline: bool,
        title: String,
    },
    Page {
        id: String,
        title: String,
    },
}

/// Which of the agent's changes a diff shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum DiffScope {
    /// What the latest request that changed anything changed, every landing of it.
    LastTurn,
    /// The session's branch since it left its base.
    Branch,
    /// Everything not committed: staged, unstaged and new files.
    Uncommitted,
}

/// What an Output tab shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum OutputOf {
    Preview,
    Run,
}

impl AppOp {
    /// What can't be used when no window answers, as the agent hears it.
    pub(crate) fn what(&self) -> &'static str {
        match self {
            Self::Browser { .. } => "its browser",
            Self::State | Self::Open { .. } => "its tabs",
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
    State {
        view: WindowView,
    },
    /// `tab` shows the target (none for a panel), in front or behind. `kept`: a tab already
    /// showed it (the user's own, or the one in front) and was left as it was.
    Opened {
        tab: Option<String>,
        front: bool,
        kept: bool,
    },
    /// Nothing was done: why, in plain words.
    Failed {
        message: String,
    },
}

/// What the window shows of one session: whether it is on screen, its tabs and its panels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WindowView {
    /// The session is the one the window shows, and the window isn't hidden.
    pub on_screen: bool,
    /// The tab in front ("chat" for the conversation).
    pub front: String,
    /// The session's tabs after Chat, in order.
    pub tabs: Vec<TabView>,
    pub panels: OpenPanels,
}

/// One of a session's tabs, as an agent is told of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TabView {
    pub id: String,
    /// file, review, panel, browser, terminal, sideChat, page, mockup, plan, newTab, conflict.
    pub kind: String,
    pub title: String,
    /// An agent opened it; else it is the user's.
    pub agent: bool,
    /// A file's path; a conflict's file.
    pub path: Option<String>,
    pub line: Option<u32>,
    /// A Browser tab's address.
    pub url: Option<String>,
}

/// The panels open beside the session's tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct OpenPanels {
    pub files: bool,
    pub terminal: bool,
    pub workers: bool,
    pub context: bool,
}
