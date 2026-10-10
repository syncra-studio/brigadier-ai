//! What each task category is called, which vendor its ties go to, and matching model names
//! and effort levels.

use brigadier_providers::{ModelInfo, ProviderKind};

use crate::TaskCategory;

/// One task category: the vendor its ties go to, and its name in a sentence.
pub(crate) struct Row {
    /// Vendor order when both score the same.
    pub(crate) order: [ProviderKind; 2],
    /// What the task is, for the reason sentence ("for review").
    pub(crate) purpose: &'static str,
}

const CLAUDE_FIRST: [ProviderKind; 2] = [ProviderKind::Claude, ProviderKind::Codex];
const CODEX_FIRST: [ProviderKind; 2] = [ProviderKind::Codex, ProviderKind::Claude];

/// See the crate docs for the tie order.
pub(crate) fn row(category: TaskCategory) -> &'static Row {
    match category {
        TaskCategory::Scout => &Row {
            order: CODEX_FIRST,
            purpose: "scouting",
        },
        TaskCategory::Research => &Row {
            order: CODEX_FIRST,
            purpose: "research",
        },
        TaskCategory::Implement => &Row {
            order: CLAUDE_FIRST,
            purpose: "implementation",
        },
        TaskCategory::Review => &Row {
            order: CODEX_FIRST,
            purpose: "review",
        },
        TaskCategory::Merge => &Row {
            order: CLAUDE_FIRST,
            purpose: "conflict resolution",
        },
        TaskCategory::Verify => &Row {
            order: CODEX_FIRST,
            purpose: "verification",
        },
        TaskCategory::Operate => &Row {
            order: CLAUDE_FIRST,
            purpose: "operating apps on screen",
        },
        TaskCategory::Chat => &Row {
            order: CLAUDE_FIRST,
            purpose: "chat",
        },
        TaskCategory::Orchestrate => &Row {
            order: CLAUDE_FIRST,
            purpose: "orchestration",
        },
    }
}

// ----- model names ------------------------------------------------------------------------

/// The words of a model id: `gpt-5.6-sol` → `gpt`, `5.6`, `sol`; `opus[1m]` → `opus`, `1m`.
pub(crate) fn words(id: &str) -> impl Iterator<Item = &str> {
    id.split(|c: char| !(c.is_ascii_alphanumeric() || c == '.'))
        .filter(|word| !word.is_empty())
}

pub(crate) fn has_word(id: &str, word: &str) -> bool {
    words(id).any(|candidate| candidate.eq_ignore_ascii_case(word))
}

/// Whether a model name (as asked for, without a catalog entry) names a Fable model.
pub(crate) fn names_fable(name: &str) -> bool {
    has_word(name, "fable")
}

/// Fable models are never routed to (the user's rule), whatever the catalog says.
pub(crate) fn is_fable(model: &ModelInfo) -> bool {
    has_word(&model.id, "fable")
        || has_word(&model.display_name, "fable")
        || model
            .resolved
            .as_deref()
            .is_some_and(|resolved| has_word(resolved, "fable"))
}

// ----- reasoning effort -------------------------------------------------------------------

/// The highest effort Brigadier ever routes to.
pub(crate) const MAX_EFFORT: &str = "high";

/// Effort levels by strength; `None` for a level Brigadier doesn't know.
pub(crate) fn rank(effort: &str) -> Option<u8> {
    Some(match effort.to_ascii_lowercase().as_str() {
        "none" => 0,
        "minimal" => 1,
        "low" => 2,
        "medium" => 3,
        "high" => 4,
        "xhigh" => 5,
        "max" => 6,
        "ultra" => 7,
        _ => return None,
    })
}

/// The effort to run at: `wanted`, capped at [`MAX_EFFORT`] and at the levels a model accepts
/// (the strongest accepted level not above it, else the weakest accepted one; `None`: no
/// catalog to check against). `None` when nothing is wanted, the level is unknown, or the model
/// has no effort control. Returns whether the level was lowered.
pub(crate) fn fit_effort_in(
    efforts: Option<&[String]>,
    wanted: Option<&str>,
) -> (Option<String>, bool) {
    let Some(wanted) = wanted else {
        return (None, false);
    };
    let Some(wanted_rank) = rank(wanted) else {
        return (None, false);
    };
    let cap = rank(MAX_EFFORT).unwrap_or(u8::MAX);
    let target = wanted_rank.min(cap);
    let Some(efforts) = efforts else {
        // No catalog: pass a standard level through, capped.
        let effort = if wanted_rank > cap {
            MAX_EFFORT.to_owned()
        } else {
            wanted.to_ascii_lowercase()
        };
        return (Some(effort), wanted_rank > cap);
    };
    if efforts.is_empty() {
        return (None, false);
    }
    let accepted = || {
        efforts
            .iter()
            .filter_map(|effort| rank(effort).map(|rank| (rank, effort)))
            .filter(|(rank, _)| *rank <= cap)
    };
    let chosen = accepted()
        .filter(|(rank, _)| *rank <= target)
        .max_by_key(|(rank, _)| *rank)
        .or_else(|| accepted().min_by_key(|(rank, _)| *rank));
    match chosen {
        Some((rank, effort)) => (Some(effort.clone()), rank < wanted_rank),
        None => (None, false),
    }
}
