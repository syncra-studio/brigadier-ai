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
];
