//! What each task category is called, and matching model names and effort levels.

use crate::TaskCategory;

/// One task category: its name in a sentence.
pub(crate) struct Row {
    /// What the task is, for the reason sentence ("for review").
    pub(crate) purpose: &'static str,
}

/// The category's row.
pub(crate) fn row(category: TaskCategory) -> &'static Row {
    match category {
        TaskCategory::Scout => &Row {
            purpose: "scouting",
        },
        TaskCategory::Research => &Row {
            purpose: "research",
        },
        TaskCategory::Implement => &Row {
            purpose: "implementation",
        },
        TaskCategory::Review => &Row { purpose: "review" },
        TaskCategory::Merge => &Row {
            purpose: "conflict resolution",
        },
        TaskCategory::Verify => &Row {
            purpose: "verification",
        },
        TaskCategory::Operate => &Row {
            purpose: "operating apps on screen",
        },
        TaskCategory::Design => &Row {
            purpose: "designs and images",
        },
        TaskCategory::Chat => &Row { purpose: "chat" },
        TaskCategory::Orchestrate => &Row {
            purpose: "orchestration",
        },
        TaskCategory::GitText => &Row {
            purpose: "writing git text",
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

/// Family metadata used by the settings discovery view; it imposes no runtime restriction.
const EXPENSIVE: [&str; 2] = ["fable", "mythos"];

/// Whether a model name (as asked for, without a catalog entry) names an expensive model.
pub(crate) fn names_expensive(name: &str) -> bool {
    EXPENSIVE.iter().any(|word| has_word(name, word))
}

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
