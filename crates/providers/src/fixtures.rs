//! Replay fixtures shipped with Brigadier: real CLI sessions recorded with
//! [`crate::record::Recorder`] (personal data scrubbed), for debugging adapter parsing from the
//! Inspector.

/// `(name, recording)` pairs.
pub const BUILTIN: &[(&str, &str)] = &[
    (
        "claude-thinking-read",
        include_str!("../fixtures/claude-thinking-read.jsonl"),
    ),
    (
        "claude-steer-interrupt-approvals",
        include_str!("../fixtures/claude-steer-interrupt-approvals.jsonl"),
    ),
    (
        "codex-steer-interrupt-escalation",
        include_str!("../fixtures/codex-steer-interrupt-escalation.jsonl"),
    ),
    (
        "claude-thread-reads",
        include_str!("../fixtures/claude-thread-reads.jsonl"),
    ),
    (
        "codex-thread-reads",
        include_str!("../fixtures/codex-thread-reads.jsonl"),
    ),
    // The CLIs' own plan mode and question tool (harness D7), part of each CLI version's
    // recording set (harness D9(c), D25).
    (
        "claude-plan-questions",
        include_str!("../fixtures/claude-plan-questions.jsonl"),
    ),
    (
        "codex-plan-questions",
        include_str!("../fixtures/codex-plan-questions.jsonl"),
    ),
];
