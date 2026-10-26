//! Durable session PRs, their timeline, and explicitly named non-PR waits.
use crate::work::PullRequest;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PrIdentity {
    pub host: String,
    pub owner: String,
    pub repo: String,
    pub number: u32,
}
impl PrIdentity {
    pub fn key(&self) -> String {
        format!("{}/{}/{}/{}", self.host, self.owner, self.repo, self.number)
    }
    pub fn url(&self) -> String {
        format!(
            "https://{}/{}/{}/pull/{}",
            self.host, self.owner, self.repo, self.number
        )
    }
    /// Canonical HTTPS PR links only. No credentials, ports, query strings or fragments.
    pub fn parse(url: &str) -> Option<Self> {
        let rest = url.trim().strip_prefix("https://")?;
        let parts: Vec<_> = rest.trim_end_matches('/').split('/').collect();
        if parts.len() != 5 || parts[3] != "pull" {
            return None;
        }
        let safe = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                && s != "."
                && s != ".."
        };
        if !parts[..3].iter().all(|p| safe(p)) || !parts[0].contains('.') {
            return None;
        }
        let number = parts[4].parse::<u32>().ok().filter(|n| *n > 0)?;
        Some(Self {
            host: parts[0].to_ascii_lowercase(),
            owner: parts[1].to_ascii_lowercase(),
            repo: parts[2].to_ascii_lowercase(),
            number,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TrackedPullRequest {
    pub identity: PrIdentity,
    /// Named account provenance, never a token. None requires explicit identity resolution.
    pub account: Option<String>,
    pub viewer: Option<String>,
    pub pull_request: Option<PullRequest>,
    pub touched_at_ms: i64,
    pub observed_at_ms: Option<i64>,
    pub actor: String,
    pub action: String,
    pub status: String,
    pub complete: bool,
    pub forgotten: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PrHistoryEvent {
    pub id: String,
    pub at_ms: i64,
    pub identity: PrIdentity,
    pub actor: String,
    pub action: String,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WaitingOn {
    pub id: String,
    pub text: String,
    pub actor: String,
    pub created_at_ms: i64,
    pub due_at_ms: Option<i64>,
    pub timezone: String,
    pub by: Option<String>,
    pub then: Option<String>,
    pub link: Option<String>,
    pub done_at_ms: Option<i64>,
    pub wake_delivered_at_ms: Option<i64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TrackedWork {
    pub pull_requests: Vec<TrackedPullRequest>,
    pub pr_history: Vec<PrHistoryEvent>,
    pub waiting_on: Vec<WaitingOn>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AddWaitingOn {
    pub text: String,
    pub by: Option<String>,
    pub timezone: Option<String>,
    pub then: Option<String>,
    pub link: Option<String>,
}
