//! The merge record (WORKSPACES-PLAN.md phase 3): what was checked of the code a merge takes,
//! each check with the commit it ran on, so the merge message shows a review of an older
//! commit as stale and a check that never ran as missing. Read from what the session recorded
//! since its last merge; it never holds or gates a merge.

use std::path::PathBuf;

use super::{SessionManager, blocking, git_error};
use crate::board::Board;
use crate::model::ConversationId;
use crate::work::{
    FindingStatus, MergeCheck, MergeCi, MergeRecord, MergeReview, MergeVerifier,
    OrchestratorStepKind, ReviewKind, WorkerRole,
};

impl SessionManager {
    /// The record of the merge of `head` from `branch` in `id`'s `checkout`. `auto`: it merged
    /// on its own, and the record says so when it is the project's first such merge.
    pub(super) async fn merge_record(
        &self,
        id: &ConversationId,
        checkout: &str,
        branch: &str,
        head: &str,
        auto: bool,
    ) -> Option<Box<MergeRecord>> {
        let board = self.core.board(id).await.ok()?;
        let (git, path, commit) = (self.git.clone(), PathBuf::from(checkout), head.to_owned());
        let head_tree = blocking(move || {
            let repo = git.open(&path).map_err(git_error)?;
            repo.resolve(&format!("{commit}^{{tree}}"))
                .map_err(git_error)
        })
        .await
        .ok()
        .map(|tree| tree.0);
        let mut record = record_of(&board, branch, head, head_tree.as_deref());
        if auto
            && let Some(project) = self
                .core
                .conversation(id)
                .ok()
                .and_then(|conversation| conversation.project_id)
        {
            record.first_auto = match self.core.mark_auto_merged(&project).await {
                Ok(first) => first,
                Err(err) => {
                    tracing::warn!(conversation = %id, error = %err, "could not record the project's first merge on its own");
                    false
                }
            };
        }
        Some(Box::new(record))
    }
}

/// [`SessionManager::merge_record`] from the session's board: what it recorded since its
/// last merge, read against `head` (whose files are `head_tree`).
pub(super) fn record_of(
    board: &Board,
    branch: &str,
    head: &str,
    head_tree: Option<&str>,
) -> MergeRecord {
    let last = board
        .orchestrator_steps
        .iter()
        .filter(|step| matches!(step.kind, OrchestratorStepKind::Merged { .. }))
        .max_by_key(|step| step.position);
    let (since, since_ms) = last.map_or((i64::MIN, i64::MIN), |step| (step.position, step.at_ms));
    let checks = board
        .thread_checks
        .iter()
        .filter(|check| check.position > since)
        .map(|check| MergeCheck {
            command: check.command.clone(),
            status: check.status.clone(),
            on_head: head_tree.is_some() && check.tree.as_deref() == head_tree,
        })
        .collect();
    let mut reviews: Vec<_> = board
        .reviews
        .values()
        .filter(|review| review.kind == ReviewKind::Code && review.started_at_ms > since_ms)
        .collect();
    reviews.sort_by_key(|review| (review.started_at_ms, review.number));
    let reviews = reviews
        .into_iter()
        .map(|review| {
            let count = |settled: fn(&FindingStatus) -> bool| {
                review
                    .items
                    .iter()
                    .filter(|item| settled(&item.status))
                    .count() as u32
            };
            MergeReview {
                reviewer: review.reviewer,
                model: review.reviewer_model.clone(),
                from: review.base.clone(),
                to: review.tip.clone(),
                state: review.state.clone(),
                fixed: count(|status| matches!(status, FindingStatus::Fixed { .. })),
                dismissed: count(|status| matches!(status, FindingStatus::Dismissed { .. })),
                open: count(|status| {
                    matches!(status, FindingStatus::Open { .. } | FindingStatus::New)
                }),
                on_head: review.tip == head,
            }
        })
        .collect();
    let ci = board
        .orchestrator_steps
        .iter()
        .rev()
        .filter(|step| step.position > since)
        .find_map(|step| match &step.kind {
            OrchestratorStepKind::CiWatch {
                branch: watched,
                sha,
                result,
                ..
            } if watched == branch => Some(MergeCi {
                sha: sha.clone(),
                result: result.clone(),
                on_head: sha == head,
            }),
            _ => None,
        });
    let proofs = board
        .orchestrator_steps
        .iter()
        .filter(|step| {
            step.position > since && matches!(step.kind, OrchestratorStepKind::Proof { .. })
        })
        .count() as u32;
    let verifier = board
        .tasks
        .values()
        .filter(|task| task.role == Some(WorkerRole::Verifier) && task.created_at_ms > since_ms)
        .filter_map(|task| Some((task, task.fresh_commit.clone()?)))
        .max_by_key(|(task, _)| task.created_at_ms)
        .map(|(task, commit)| MergeVerifier {
            on_head: commit == head,
            commit,
            result: task.report.as_ref().and_then(|report| report.checks),
        });
    MergeRecord {
        head: head.to_owned(),
        checks,
        reviews,
        ci,
        proofs,
        verifier,
        first_auto: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::board::ThreadCheck;
    use crate::work::{Finding, OrchestratorStep, ReviewRun, ReviewState};

    fn step(kind: OrchestratorStepKind, position: i64) -> OrchestratorStep {
        OrchestratorStep {
            request_id: None,
            kind,
            at_ms: position,
            position,
        }
    }

    fn check(command: &str, tree: &str, position: i64) -> ThreadCheck {
        ThreadCheck {
            command: command.into(),
            workdir: String::new(),
            tree: Some(tree.into()),
            status: "exit 0".into(),
            position,
        }
    }

    fn ci(sha: &str, position: i64) -> OrchestratorStep {
        step(
            OrchestratorStepKind::CiWatch {
                repo: "o/n".into(),
                branch: "work".into(),
                sha: sha.into(),
                waiting: false,
                runs: Vec::new(),
                result: Some("passed".into()),
            },
            position,
        )
    }

    /// Each check names the commit it ran on: a review or CI of an older commit reads stale,
    /// checks of the files the head has read current, and what ran before the last merge
    /// doesn't count for this one.
    #[test]
    fn a_merge_record_marks_what_ran_on_an_older_commit_and_leaves_out_the_last_merge() {
        let mut board = Board::default();
        assert_eq!(
            record_of(&board, "work", "c2", Some("t2")),
            MergeRecord {
                head: "c2".into(),
                ..MergeRecord::default()
            },
            "nothing ran: every part is missing"
        );
        board.thread_checks = vec![
            check("cargo test", "t0", 1),
            check("pnpm typecheck", "t1", 3),
            check("pnpm test", "t2", 4),
        ];
        board.orchestrator_steps = vec![
            ci("c0", 1),
            step(
                OrchestratorStepKind::Merged {
                    branch: "work".into(),
                    base: "main".into(),
                    commits: 1,
                    asked_in: None,
                    upstream: None,
                    head: Some("c0".into()),
                    merge_commit: Some("m0".into()),
                    pr: None,
                    auto: false,
                    requests: Vec::new(),
                    record: None,
                },
                2,
            ),
            ci("c1", 5),
        ];
        let review = |id: &str, tip: &str, at: i64, items: Vec<FindingStatus>| ReviewRun {
            id: id.into(),
            conversation_id: ConversationId("s".into()),
            request_id: None,
            task_id: None,
            kind: ReviewKind::Code,
            base: "c0".into(),
            tip: tip.into(),
            author: brigadier_providers::ProviderKind::Claude,
            reviewer: brigadier_providers::ProviderKind::Codex,
            reviewer_model: Some("gpt".into()),
            notify: crate::work::ReviewFor::Orchestrator,
            state: if items.is_empty() {
                ReviewState::Clean
            } else {
                ReviewState::Findings {
                    count: items.len() as u32,
                }
            },
            started_at_ms: at,
            ended_at_ms: Some(at),
            findings: None,
            number: 1,
            items: items
                .into_iter()
                .enumerate()
                .map(|(n, status)| Finding {
                    id: format!("R1.{n}"),
                    severity: None,
                    title: "A finding".into(),
                    location: None,
                    status,
                    high: false,
                })
                .collect(),
            landing: None,
            answer_waits: false,
        };
        board.reviews.insert(
            "r1".into(),
            review(
                "r1",
                "c1",
                3,
                vec![
                    FindingStatus::Fixed {
                        task: None,
                        why: None,
                        checked: None,
                    },
                    FindingStatus::Dismissed { why: "fine".into() },
                    FindingStatus::New,
                ],
            ),
        );
        board
            .reviews
            .insert("r0".into(), review("r0", "c0", 1, Vec::new()));
        let record = record_of(&board, "work", "c2", Some("t2"));
        assert_eq!(
            record
                .checks
                .iter()
                .map(|check| (check.command.as_str(), check.on_head))
                .collect::<Vec<_>>(),
            [("pnpm typecheck", false), ("pnpm test", true)]
        );
        assert_eq!(
            record.reviews.len(),
            1,
            "the review before the last merge is left out"
        );
        let review = &record.reviews[0];
        assert_eq!(
            (review.from.as_str(), review.to.as_str(), review.on_head),
            ("c0", "c1", false)
        );
        assert_eq!((review.fixed, review.dismissed, review.open), (1, 1, 1));
        assert_eq!(
            record.ci,
            Some(MergeCi {
                sha: "c1".into(),
                result: Some("passed".into()),
                on_head: false,
            })
        );
        assert_eq!((record.proofs, record.verifier), (0, None));
        // Reviewed and checked up to the head: current.
        let record = record_of(&board, "work", "c1", Some("t1"));
        assert!(record.reviews[0].on_head && record.ci.unwrap().on_head);
        assert!(record.checks[0].on_head);
    }
}
