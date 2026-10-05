//! What the core records about the Project Brain and the context engine: Brain jobs (the
//! skeleton pass, idle-quota enrichment), Chat memories, orchestrator rebirths, and the
//! overview the Inspector reads.

use brigadier_brain::{BrainStats, EmbedderStatus};
use brigadier_index::IndexStatus;
use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::ProjectId;

/// A background job that deepens a project's Brain with a cheap model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum BrainJobKind {
    /// On project add: each module's purpose, the stack, conventions, the run/build/verify
    /// recipe.
    Skeleton,
    /// Spare quota before a usage window resets: stale nodes first, then gaps.
    Enrichment,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum BrainJobState {
    Running,
    Done,
    /// It yielded: the user started work on its provider, the window reset or ran hot, or the
    /// user stopped it.
    Stopped {
        reason: String,
    },
    Failed {
        error: String,
    },
}

/// A Brain job (full snapshot on each change).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainJob {
    pub id: String,
    pub project_id: ProjectId,
    pub kind: BrainJobKind,
    pub state: BrainJobState,
    pub provider: ProviderKind,
    pub model: Option<String>,
    pub started_at_ms: i64,
    pub ended_at_ms: Option<i64>,
    /// Nodes it recorded so far.
    pub nodes: u32,
    /// What it worked on, in a few words.
    pub note: String,
}

/// What a Chat's model saved to, or removed from, the Personal Brain in a turn (a Memory chip).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct MemoryChange {
    /// The Personal Brain node.
    pub node_id: String,
    pub text: String,
    /// The user removed it again.
    pub forgotten: bool,
    /// The request whose turn saved it.
    pub request_id: Option<String>,
    pub at_ms: i64,
}

/// Why an orchestrator was reborn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RebirthTrigger {
    /// Its context passed the rebirth threshold.
    Threshold,
    /// Its CLI session could not be resumed (lost files, a failed resume).
    Recovery,
    /// Its prompt cache had expired when the next turn came: resuming would have sent the
    /// whole history again at the cache-write price (PLAN.md §7).
    CacheExpired,
    /// An overnight run starts a phase: its lead begins from the phase's briefing alone.
    Phase,
}

/// One part of a rebirth briefing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BriefingSection {
    /// `framing`, `handoff`, `decisions`, `state`, `brain`, `recent`, `search`.
    pub name: String,
    /// About four bytes per token.
    pub tokens: u64,
    /// Entries it holds (decisions, messages, nodes, tasks).
    pub items: u32,
    /// It was cut to fit the budget; `note` says how.
    pub truncated: bool,
    pub note: Option<String>,
}

/// An orchestrator rebirth, as the Inspector's rebirth log shows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RebirthRecord {
    pub id: String,
    /// 1 for the first rebirth of the conversation.
    pub generation: u32,
    pub trigger: RebirthTrigger,
    pub provider: ProviderKind,
    pub model: Option<String>,
    /// The outgoing CLI's context when the handoff started.
    pub at_tokens: i64,
    pub window_tokens: Option<i64>,
    pub prepare_started_at_ms: i64,
    /// When the handoff note was ready; `None` without a note being prepared, and in records
    /// made before this was kept.
    #[serde(default)]
    pub handoff_ready_at_ms: Option<i64>,
    /// When the swap began: the next turn was due and the old CLI was retired. The time from
    /// the note being ready until then is spent waiting for that turn, not working. `None` in
    /// records made before this was kept.
    #[serde(default)]
    pub swap_started_at_ms: Option<i64>,
    /// When the briefing was ready for the new CLI.
    pub swapped_at_ms: i64,
    /// The outgoing orchestrator's handoff note (blob hash); `None` if it could not write one.
    pub handoff_blob: Option<String>,
    /// The whole briefing (blob hash).
    pub briefing_blob: String,
    pub briefing_tokens: u64,
    pub sections: Vec<BriefingSection>,
    /// Settled decisions of this conversation, all listed in the briefing.
    pub decisions: u32,
    /// Of those, the ones carried with their full text.
    pub decisions_in_full: u32,
    /// Messages carried verbatim.
    pub recent_messages: u32,
    pub old_native_id: Option<String>,
    /// The new CLI session's id, once it started.
    pub new_native_id: Option<String>,
}

/// When an orchestrator's rebirth is prepared and when it must happen, in context tokens, for
/// the model it runs on now.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct RebirthThresholds {
    /// The handoff note is prepared once the context passes this.
    pub prepare_tokens: i64,
    /// The orchestrator is reborn before its next turn once the context passes this.
    pub swap_tokens: i64,
    pub window_tokens: Option<i64>,
}

/// How long an orchestrator's prompt cache lasts after its last request, where that is known
/// (PLAN.md §7). Claude keeps a subscription's main conversation cached for an hour. How long
/// Codex keeps it through the app-server is not measured yet, so a Codex orchestrator is never
/// reborn for an expired cache.
///
/// A debug build takes `BRIGADIER_CACHE_TTL_SECS` as Claude's, so a cold rebirth can be tried
/// in minutes.
pub fn cache_lifetime(provider: ProviderKind) -> Option<std::time::Duration> {
    match provider {
        ProviderKind::Claude => {
            if cfg!(debug_assertions)
                && let Some(secs) = std::env::var("BRIGADIER_CACHE_TTL_SECS")
                    .ok()
                    .and_then(|value| value.trim().parse::<u64>().ok())
                    .filter(|secs| *secs > 0)
            {
                return Some(std::time::Duration::from_secs(secs));
            }
            Some(std::time::Duration::from_secs(3_600))
        }
        ProviderKind::Codex => None,
    }
}

/// Below this context a cold resume costs less than a rebirth's briefing (about 25k tokens
/// written, plus its checkpoint): the orchestrator just resumes.
const COLD_REBIRTH_MIN_TOKENS: i64 = 60_000;

/// The least context at which an orchestrator on `provider` is reborn for an expired cache:
/// below the size threshold, of course. A debug build takes `BRIGADIER_COLD_REBIRTH_TOKENS`,
/// so a cold rebirth can be tried on a short conversation.
pub fn cold_rebirth_min_tokens(provider: ProviderKind, window: Option<i64>) -> i64 {
    if cfg!(debug_assertions)
        && let Some(tokens) = std::env::var("BRIGADIER_COLD_REBIRTH_TOKENS")
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .filter(|tokens| *tokens > 0)
    {
        return tokens;
    }
    COLD_REBIRTH_MIN_TOKENS.min(rebirth_thresholds(provider, window).prepare_tokens)
}

/// The most context a worker carries before it is handed over to a fresh session.
const WORKER_HANDOFF_MAX_TOKENS: i64 = 300_000;

/// The worker context at which a worker is handed over to a fresh session (PLAN.md §7): 300k
/// tokens or 70% of its model's context window, whichever comes first. A debug build takes
/// `BRIGADIER_WORKER_HANDOFF_TOKENS`, so a hand-off can be tried on a small task.
pub fn worker_handoff_tokens(window: Option<i64>) -> i64 {
    if cfg!(debug_assertions)
        && let Some(tokens) = std::env::var("BRIGADIER_WORKER_HANDOFF_TOKENS")
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .filter(|tokens| *tokens > 0)
    {
        return tokens;
    }
    let size = window.filter(|size| *size > 0).unwrap_or(DEFAULT_WINDOW);
    WORKER_HANDOFF_MAX_TOKENS.min(size * 7 / 10)
}

/// Whether a worker session at `tokens` is handed over, given the hand-off size `at` and the
/// session's first context size `start` (a fresh session already carries its hand-off). A
/// session must grow by at least half the hand-off size since it started, so one that starts
/// near the size is not handed over again at once; there is no limit on how many hand-overs a
/// task makes.
pub fn worker_handoff_due(tokens: i64, start: Option<i64>, at: i64) -> bool {
    tokens >= at.max(start.unwrap_or(0) + at / 2)
}

/// A project's Brain at a glance (or the Personal Brain's), for the Inspector.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BrainOverview {
    /// `None` for the Personal Brain.
    pub project_id: Option<ProjectId>,
    pub stats: BrainStats,
    /// The project's code index (none for the Personal Brain or a project without a repo).
    pub index: Option<IndexStatus>,
    pub embedder: EmbedderStatus,
    /// Its Brain jobs, newest first.
    pub jobs: Vec<BrainJob>,
}

/// What an AGENTS.md export wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ConventionsExport {
    pub path: String,
    /// Conventions written.
    pub conventions: u32,
    /// The file did not exist before.
    pub created: bool,
}

/// The context window assumed when a CLI has not said its model's.
const DEFAULT_WINDOW: i64 = 200_000;

/// When an orchestrator on `provider` with a `window`-token model prepares its rebirth and when
/// it must happen (PLAN.md §6 Phase 4: at about 150–200k tokens).
///
/// Claude's auto-compact is off for orchestrators, so only the window bounds the swap. Codex
/// compacts on its own at 90% of the window whatever it is told, so a Codex orchestrator is
/// reborn well below that, with room left for one large turn.
///
/// A debug build takes `BRIGADIER_REBIRTH_TOKENS` as the prepare threshold (the swap follows a
/// quarter above it), so rebirths can be tried on a small budget.
pub fn rebirth_thresholds(provider: ProviderKind, window: Option<i64>) -> RebirthThresholds {
    let size = window.filter(|size| *size > 0).unwrap_or(DEFAULT_WINDOW);
    let (prepare_share, swap_share) = match provider {
        ProviderKind::Claude => (0.75, 0.90),
        ProviderKind::Codex => (0.65, 0.80),
    };
    let mut prepare_tokens = 150_000.min((size as f64 * prepare_share) as i64);
    let mut swap_tokens = 190_000.min((size as f64 * swap_share) as i64);
    if cfg!(debug_assertions)
        && let Some(tokens) = std::env::var("BRIGADIER_REBIRTH_TOKENS")
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .filter(|tokens| *tokens > 0)
    {
        prepare_tokens = tokens.min(prepare_tokens);
        swap_tokens = (tokens + tokens / 4).min(swap_tokens);
    }
    RebirthThresholds {
        prepare_tokens,
        swap_tokens,
        window_tokens: window,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_session_hands_over_at_the_size() {
        assert!(!worker_handoff_due(159_999, Some(15_000), 160_000));
        assert!(worker_handoff_due(160_000, Some(15_000), 160_000));
        assert!(worker_handoff_due(160_000, None, 160_000));
    }

    #[test]
    fn a_fresh_session_near_the_size_needs_headroom() {
        // Started at 28k with the size at 25k: not at once, only after growing by half of it.
        assert!(!worker_handoff_due(28_000, Some(28_000), 25_000));
        assert!(!worker_handoff_due(40_000, Some(28_000), 25_000));
        assert!(worker_handoff_due(40_500, Some(28_000), 25_000));
    }

    #[test]
    fn a_worker_hands_over_at_300k_or_70_percent_of_its_window_and_again_after() {
        if std::env::var_os("BRIGADIER_WORKER_HANDOFF_TOKENS").is_some() {
            return;
        }
        assert_eq!(worker_handoff_tokens(Some(1_000_000)), 300_000);
        assert_eq!(worker_handoff_tokens(Some(272_000)), 190_400);
        assert_eq!(worker_handoff_tokens(None), 140_000);
        // No limit on how many: a successor that started small hands over again.
        assert!(worker_handoff_due(300_000, Some(30_000), 300_000));
    }
}
