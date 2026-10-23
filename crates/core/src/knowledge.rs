//! What the core records about the Project Brain and the context engine: Chat memories,
//! orchestrator rebirths, and the overview the Inspector reads.

use brigadier_brain::{BrainStats, EmbedderStatus};
use brigadier_index::IndexStatus;
use brigadier_providers::ProviderKind;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::model::ProjectId;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum RebirthTrigger {
    /// Its context passed the rebirth threshold.
    Threshold,
    /// Its CLI session could not be resumed (lost files, a failed resume).
    Recovery,
    /// Its prompt cache had expired when the next turn came: resuming would have sent the
    /// whole history again at the cache-write price (PLAN.md §7).
    CacheExpired,
    /// Its turn was stopped near its model's limit and the request continued in the fresh
    /// session (BUILD-PLAN D2.3b).
    MidTurn,
}

/// A rebirth's trigger as stored, older names included: an earlier run's per-phase restart
/// (`phase`) reads as `recovery`, so the thread engine's first start can still delete it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum StoredRebirthTrigger {
    Threshold,
    Recovery,
    CacheExpired,
    MidTurn,
    Phase,
}

impl<'de> Deserialize<'de> for RebirthTrigger {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(match StoredRebirthTrigger::deserialize(deserializer)? {
            StoredRebirthTrigger::Threshold => Self::Threshold,
            StoredRebirthTrigger::Recovery | StoredRebirthTrigger::Phase => Self::Recovery,
            StoredRebirthTrigger::CacheExpired => Self::CacheExpired,
            StoredRebirthTrigger::MidTurn => Self::MidTurn,
        })
    }
}

/// One part of a rebirth briefing.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct BriefingSection {
    /// `framing`, `constraints`, `handoff`, `decisions`, `state`, `brain`, `recent`, `search`.
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

/// Below this context an orchestrator whose cache expired just resumes: re-caching it is
/// cheap enough, and it keeps its whole context (SESSION-FIXES-PLAN Q5, the user's answer).
const COLD_REBIRTH_MIN_TOKENS: i64 = 150_000;

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
const WORKER_HANDOFF_MAX_TOKENS: i64 = 200_000;

/// The worker context at which a worker is handed over to a fresh session (PLAN.md §7): 200k
/// tokens (SESSION-FIXES-PLAN.md Q6.4) or 70% of its model's context window, whichever comes
/// first. A debug build takes
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
/// task makes. Never later than `cap`, below where the CLI itself would act on a full context
/// (BUILD-PLAN D2.3).
pub fn worker_handoff_due(tokens: i64, start: Option<i64>, at: i64, cap: i64) -> bool {
    tokens >= at.max(start.unwrap_or(0) + at / 2).min(cap)
}

/// The room Claude keeps in its window for the reply (its CLI's default most output tokens): a
/// request whose context leaves less is refused.
const CLAUDE_OUTPUT_ROOM: i64 = 32_000;

/// Where a CLI itself acts on a full context, in tokens (BUILD-PLAN D2.3): Codex compacts at
/// about 90% of the window, whatever its limit is set to; Claude, with its own compaction off,
/// refuses the request ("prompt is too long") once the context leaves less than its reply's room
/// (a live run on a 200k window was refused between about 175k and 180k).
pub fn context_ceiling(provider: ProviderKind, window: Option<i64>) -> i64 {
    let size = window.filter(|size| *size > 0).unwrap_or(DEFAULT_WINDOW);
    match provider {
        ProviderKind::Codex => size * 9 / 10,
        ProviderKind::Claude => (size - CLAUDE_OUTPUT_ROOM).max(size / 2),
    }
}

/// The hard line, safely below [`context_ceiling`]: past it Brigadier stops a worker's turn and
/// hands it off, or stops a thread's turn and reborn it, without waiting for the turn to end
/// (BUILD-PLAN D2.3, D2.3b). A debug build takes `BRIGADIER_HARD_LINE_TOKENS`, so the line can
/// be crossed on a short task.
pub fn hard_line(provider: ProviderKind, window: Option<i64>) -> i64 {
    if cfg!(debug_assertions)
        && let Some(tokens) = std::env::var("BRIGADIER_HARD_LINE_TOKENS")
            .ok()
            .and_then(|value| value.trim().parse::<i64>().ok())
            .filter(|tokens| *tokens > 0)
    {
        return tokens;
    }
    context_ceiling(provider, window) * 85 / 100
}

/// A worker's hard line where its provider isn't known: the lower of the two.
pub fn worker_hard_line(window: Option<i64>) -> i64 {
    hard_line(ProviderKind::Claude, window).min(hard_line(ProviderKind::Codex, window))
}

/// Where a worker is first asked to wrap up for a hand-off: [`worker_handoff_tokens`], kept
/// below the hard line so the steer comes, and can come again, before the turn is stopped. An
/// unknown window has no hard line (see `worker_context`), so nothing to keep below.
pub fn worker_handoff_at(window: Option<i64>) -> i64 {
    let at = worker_handoff_tokens(window);
    match window {
        Some(_) => at.min(worker_hard_line(window) * 85 / 100),
        None => at,
    }
}

/// How much more a context grows, past a first wrap-up steer, before the steer is sent again:
/// a third of the way from where steering starts (`from`) to the hard line, at least 5k and at
/// most 50k tokens.
pub fn steer_stretch(from: i64, hard: i64) -> i64 {
    ((hard - from) / 3).clamp(5_000, 50_000)
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

/// A Claude window that holds the big thresholds below (SESSION-FIXES-PLAN Q5).
const BIG_WINDOW: i64 = 500_000;
/// On a big window a Claude orchestrator prepares its rebirth here (400k until the earlier-rebirth
/// experiment, BUILD-PLAN Phase X A10: 300k kept every answer for 35% fewer tokens; the user's
/// go-ahead, 2026-10-17) …
const BIG_PREPARE_TOKENS: i64 = 300_000;
/// … and swaps here, a quarter above, as A10 ran it.
const BIG_SWAP_TOKENS: i64 = 375_000;

/// When an orchestrator on `provider` with a `window`-token model prepares its rebirth and when
/// it must happen: on a Claude window of 500k or more at 300k and 375k (SESSION-FIXES-PLAN
/// Q5, moved by A10), otherwise at about 150–190k tokens or a share of a smaller window (PLAN.md §6 Phase 4).
///
/// Claude's auto-compact is off for orchestrators, so only the window bounds the swap. Codex
/// compacts on its own at 90% of the window whatever it is told, so a Codex orchestrator is
/// reborn well below that, with room left for one large turn.
///
/// A debug build takes `BRIGADIER_REBIRTH_TOKENS` as the prepare threshold (the swap follows a
/// quarter above it), both capped at the thresholds above, so rebirths can be tried on a small
/// budget (the earlier-rebirth experiment, A10, ran its 200k arm so). A release build ignores it.
pub fn rebirth_thresholds(provider: ProviderKind, window: Option<i64>) -> RebirthThresholds {
    let value = std::env::var("BRIGADIER_REBIRTH_TOKENS").ok();
    thresholds_with(
        provider,
        window,
        rebirth_override(cfg!(debug_assertions), value.as_deref()),
    )
}

/// The prepare threshold `BRIGADIER_REBIRTH_TOKENS` asks for: only in a `debug` build, and only
/// a positive number.
fn rebirth_override(debug: bool, value: Option<&str>) -> Option<i64> {
    value
        .filter(|_| debug)
        .and_then(|value| value.trim().parse::<i64>().ok())
        .filter(|tokens| *tokens > 0)
}

/// The thresholds of [`rebirth_thresholds`], with `tokens` from the debug override.
fn thresholds_with(
    provider: ProviderKind,
    window: Option<i64>,
    tokens: Option<i64>,
) -> RebirthThresholds {
    let size = window.filter(|size| *size > 0).unwrap_or(DEFAULT_WINDOW);
    let (prepare_share, swap_share) = match provider {
        ProviderKind::Claude => (0.75, 0.90),
        ProviderKind::Codex => (0.65, 0.80),
    };
    let (mut prepare_tokens, mut swap_tokens) =
        if provider == ProviderKind::Claude && size >= BIG_WINDOW {
            (BIG_PREPARE_TOKENS, BIG_SWAP_TOKENS)
        } else {
            (
                150_000.min((size as f64 * prepare_share) as i64),
                190_000.min((size as f64 * swap_share) as i64),
            )
        };
    if let Some(tokens) = tokens {
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
    fn a_hand_off_comes_before_the_cli_acts_on_a_full_context() {
        // A fresh session that started large would grow to 250k first: capped below the hard
        // line of a 200k-window Codex model (153k).
        let cap = hard_line(ProviderKind::Codex, Some(200_000));
        assert_eq!(cap, 153_000);
        assert!(!worker_handoff_due(140_000, Some(100_000), 140_000, cap));
        assert!(worker_handoff_due(153_000, Some(100_000), 140_000, cap));
        // Claude refuses once less than its reply's room is left.
        assert_eq!(
            context_ceiling(ProviderKind::Claude, Some(200_000)),
            168_000
        );
        assert_eq!(hard_line(ProviderKind::Claude, Some(200_000)), 142_800);
        assert_eq!(worker_hard_line(Some(200_000)), 142_800);
        assert_eq!(worker_hard_line(Some(1_000_000)), 765_000);
        assert_eq!(steer_stretch(140_000, 170_000), 10_000);
        assert_eq!(steer_stretch(200_000, 850_000), 50_000);
    }

    #[test]
    fn a_first_session_hands_over_at_the_size() {
        assert!(!worker_handoff_due(
            159_999,
            Some(15_000),
            160_000,
            i64::MAX
        ));
        assert!(worker_handoff_due(160_000, Some(15_000), 160_000, i64::MAX));
        assert!(worker_handoff_due(160_000, None, 160_000, i64::MAX));
    }

    #[test]
    fn a_fresh_session_near_the_size_needs_headroom() {
        // Started at 28k with the size at 25k: not at once, only after growing by half of it.
        assert!(!worker_handoff_due(28_000, Some(28_000), 25_000, i64::MAX));
        assert!(!worker_handoff_due(40_000, Some(28_000), 25_000, i64::MAX));
        assert!(worker_handoff_due(40_500, Some(28_000), 25_000, i64::MAX));
    }

    #[test]
    fn a_worker_hands_over_at_200k_or_70_percent_of_its_window_and_again_after() {
        if std::env::var_os("BRIGADIER_WORKER_HANDOFF_TOKENS").is_some() {
            return;
        }
        assert_eq!(worker_handoff_tokens(Some(1_000_000)), 200_000);
        assert_eq!(worker_handoff_tokens(Some(272_000)), 190_400);
        assert_eq!(worker_handoff_tokens(None), 140_000);
        // Asked below the hard line, so the steer has room before the stop.
        assert_eq!(worker_handoff_at(Some(1_000_000)), 200_000);
        assert_eq!(worker_handoff_at(Some(200_000)), 121_380);
        assert_eq!(worker_handoff_at(None), 140_000);
        // No limit on how many: a successor that started small hands over again.
        assert!(worker_handoff_due(200_000, Some(30_000), 200_000, i64::MAX));
    }

    #[test]
    fn a_claude_thread_on_a_big_window_is_reborn_at_300k() {
        // Without the debug override, whatever this process's environment says: what the
        // shipped app does.
        let at = |provider, window| {
            let thresholds = thresholds_with(provider, window, None);
            (thresholds.prepare_tokens, thresholds.swap_tokens)
        };
        assert_eq!(
            at(ProviderKind::Claude, Some(1_000_000)),
            (300_000, 375_000)
        );
        assert_eq!(at(ProviderKind::Claude, Some(500_000)), (300_000, 375_000));
        // A window that doesn't hold them keeps the shares.
        assert_eq!(at(ProviderKind::Claude, Some(200_000)), (150_000, 180_000));
        assert_eq!(at(ProviderKind::Claude, Some(400_000)), (150_000, 190_000));
        assert_eq!(at(ProviderKind::Claude, None), (150_000, 180_000));
        // Codex compacts on its own at 90%: its lower shares stay.
        assert_eq!(at(ProviderKind::Codex, Some(1_000_000)), (150_000, 190_000));
        assert_eq!(at(ProviderKind::Codex, Some(200_000)), (130_000, 160_000));
    }

    #[test]
    fn only_a_debug_build_moves_the_rebirth() {
        // A release build passes `debug: false`, so the variable changes nothing there.
        assert_eq!(rebirth_override(false, Some("200000")), None);
        assert_eq!(rebirth_override(true, Some(" 200000 ")), Some(200_000));
        assert_eq!(rebirth_override(true, Some("0")), None);
        assert_eq!(rebirth_override(true, Some("soon")), None);
        assert_eq!(rebirth_override(true, None), None);
        // The swap a quarter above, never past production's 300k and 375k.
        let at = |tokens| {
            let thresholds = thresholds_with(ProviderKind::Claude, Some(1_000_000), Some(tokens));
            (thresholds.prepare_tokens, thresholds.swap_tokens)
        };
        assert_eq!(at(200_000), (200_000, 250_000));
        assert_eq!(at(300_000), (300_000, 375_000));
        assert_eq!(at(400_000), (300_000, 375_000));
    }

    #[test]
    fn the_app_never_sets_the_rebirth_override() {
        // Only this file and the measuring tools (`tools/ab`, outside `crates` and `apps`) name
        // the variable, so nothing the app ships sets it for its daemon. Source files only: a
        // built daemon (CI puts one under the desktop app's `binaries/`) holds this file's name
        // of it.
        fn scan(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
            for entry in std::fs::read_dir(dir).unwrap().flatten() {
                let path = entry.path();
                let name = entry.file_name();
                if path.is_dir() {
                    if !matches!(
                        name.to_str(),
                        Some("target" | "node_modules" | "dist" | "gen" | ".git")
                    ) {
                        scan(&path, found);
                    }
                } else if path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| {
                        matches!(
                            ext,
                            "rs" | "ts"
                                | "tsx"
                                | "js"
                                | "mjs"
                                | "json"
                                | "toml"
                                | "sh"
                                | "py"
                                | "yml"
                                | "yaml"
                                | "plist"
                                | "html"
                                | "css"
                                | "md"
                        )
                    })
                    && std::fs::read(&path).is_ok_and(|bytes| {
                        bytes
                            .windows(b"BRIGADIER_REBIRTH_TOKENS".len())
                            .any(|window| window == b"BRIGADIER_REBIRTH_TOKENS")
                    })
                {
                    found.push(path);
                }
            }
        }
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut found = Vec::new();
        for dir in ["crates", "apps"] {
            scan(&root.join(dir), &mut found);
        }
        let this = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/knowledge.rs");
        let this = this.canonicalize().unwrap();
        let others: Vec<_> = found
            .iter()
            .filter(|path| path.canonicalize().ok().as_ref() != Some(&this))
            .collect();
        assert!(others.is_empty(), "{others:?}");
    }

    #[test]
    fn a_cold_rebirth_needs_150k_and_never_more_than_a_rebirth() {
        if std::env::var_os("BRIGADIER_REBIRTH_TOKENS").is_some()
            || std::env::var_os("BRIGADIER_COLD_REBIRTH_TOKENS").is_some()
        {
            return;
        }
        assert_eq!(
            cold_rebirth_min_tokens(ProviderKind::Claude, Some(1_000_000)),
            150_000
        );
        assert_eq!(
            cold_rebirth_min_tokens(ProviderKind::Claude, Some(200_000)),
            150_000
        );
        // A small window's rebirth comes first.
        assert_eq!(
            cold_rebirth_min_tokens(ProviderKind::Claude, Some(100_000)),
            75_000
        );
    }
}
