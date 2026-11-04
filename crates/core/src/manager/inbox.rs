//! The inbox's auto-done sweep: a conversation idle for 3 days, or whose branch's pull request
//! merged, is marked done. It runs in the hibernation timer and adds no GitHub polling: the
//! pull request is the one the board last saw.

use crate::board::Board;
use crate::model::{Conversation, Lifecycle};
use crate::now_ms;
use crate::thread_view::CliTurn;
use crate::work::{PullRequestState, RunState};

use super::SessionManager;

/// How long a conversation stays idle before it is marked done.
const IDLE_FOR_DONE_MS: i64 = 3 * 24 * 60 * 60 * 1000;

/// The sweep runs every this many hibernation ticks (a minute each), so each conversation is
/// read at most once every 10 minutes.
pub(super) const SWEEP_EVERY_TICKS: u32 = 10;

/// Whether the sweep marks `conversation` done now, with `board` its board, `turn` where its
/// CLI's turn stands, and `decided_at_ms` when it was last marked done or brought back (0:
/// never).
fn auto_done(
    conversation: &Conversation,
    board: &Board,
    turn: CliTurn,
    decided_at_ms: i64,
    now: i64,
) -> bool {
    let last_end = turn
        .turn_ended_at_ms
        .unwrap_or(0)
        .max(conversation.created_at_ms);
    // Done, or brought back by hand: that stands until the thread does something after it.
    if decided_at_ms >= last_end {
        return false;
    }
    if let Some(activity) = board.activity(&conversation.id, turn)
        && (activity.turn_open
            || matches!(activity.run, RunState::Starting | RunState::Running)
            || !activity.tasks.is_empty()
            || !activity.approvals.is_empty()
            || !activity.questions.is_empty()
            || !activity.sign_ins.is_empty())
    {
        return false;
    }
    let merged = board
        .pull_request
        .as_ref()
        .is_some_and(|pr| pr.state == PullRequestState::Merged);
    // A merge marks it done once; brought back after, only 3 idle days do again.
    (merged && decided_at_ms == 0)
        || now - conversation.updated_at_ms.max(last_end) >= IDLE_FOR_DONE_MS
}

impl SessionManager {
    /// Marks done each listed conversation idle for 3 days or whose pull request merged, while
    /// nothing of it runs or waits for the user.
    pub(super) async fn sweep_done(&self) {
        let now = now_ms();
        for conversation in self.core.visible_catalog().conversations {
            if conversation.lifecycle == Lifecycle::Archived
                || conversation.side_of.is_some()
                || conversation.tab_chat.is_some()
            {
                continue;
            }
            let id = conversation.id.clone();
            if let Ok(conv) = self.conv(&id)
                && (conv.is_busy().await || conv.has_cli_jobs().await)
            {
                continue;
            }
            let turn = self.cli_turn(&id);
            // A chat has no pull request: its board is read only once it could be idle long
            // enough.
            let idle_from = conversation
                .updated_at_ms
                .max(turn.turn_ended_at_ms.unwrap_or(0));
            if conversation.project_id.is_none() && now - idle_from < IDLE_FOR_DONE_MS {
                continue;
            }
            let Ok(board) = self.core.board(&id).await else {
                continue;
            };
            if auto_done(
                &conversation,
                &board,
                turn,
                self.core.done_decided_at(&id),
                now,
            ) && let Err(err) = self.core.set_done(id.clone(), true).await
            {
                tracing::debug!(conversation = %id, error = %err, "could not mark done");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::Core;
    use crate::model::{ConversationId, ConversationKind};
    use crate::sessions::Origin;
    use crate::work::{Approval, ApprovalSubject, CardId, CardState, PullRequest};

    const NOW: i64 = 10 * IDLE_FOR_DONE_MS;

    fn conversation(updated_at_ms: i64) -> Conversation {
        serde_json::from_value(serde_json::json!({
            "id": "c1", "kind": "session", "projectId": "p", "title": "t",
            "pinnedAtMs": null, "createdAtMs": 1, "updatedAtMs": updated_at_ms
        }))
        .unwrap()
    }

    fn ended(at_ms: i64) -> CliTurn {
        CliTurn {
            open: false,
            last_end: None,
            turn_ended_at_ms: Some(at_ms),
        }
    }

    fn with_pr(state: PullRequestState) -> Board {
        let pr: PullRequest = serde_json::from_value(serde_json::json!({
            "number": 7, "title": "t", "url": "u", "state": "open"
        }))
        .unwrap();
        Board {
            pull_request: Some(Box::new(PullRequest { state, ..pr })),
            ..Board::default()
        }
    }

    #[test]
    fn the_inbox_sweep_marks_3_idle_days_and_a_merged_pr_done() {
        let recent = NOW - 60_000;
        let old = NOW - IDLE_FOR_DONE_MS;
        let idle = conversation(old);
        assert!(auto_done(&idle, &Board::default(), ended(old), 0, NOW));
        assert!(
            !auto_done(&idle, &Board::default(), ended(recent), 0, NOW),
            "counted from the last turn end too"
        );
        assert!(!auto_done(
            &conversation(recent),
            &Board::default(),
            ended(old),
            0,
            NOW
        ));
        let busy = conversation(recent);
        assert!(auto_done(
            &busy,
            &with_pr(PullRequestState::Merged),
            ended(recent),
            0,
            NOW
        ));
        for state in [PullRequestState::Closed, PullRequestState::Open] {
            assert!(
                !auto_done(&busy, &with_pr(state), ended(recent), 0, NOW),
                "{state:?} is not merged"
            );
        }
    }

    #[test]
    fn the_inbox_sweep_leaves_a_working_or_waiting_session() {
        let old = NOW - IDLE_FOR_DONE_MS;
        let idle = conversation(old);
        let open = CliTurn {
            open: true,
            ..ended(old)
        };
        assert!(!auto_done(&idle, &Board::default(), open, 0, NOW));
        let running = Board {
            run: RunState::Running,
            ..with_pr(PullRequestState::Merged)
        };
        assert!(!auto_done(&idle, &running, ended(old), 0, NOW));
        let mut asking = with_pr(PullRequestState::Merged);
        let approval = CardId("a1".into());
        asking.approvals.insert(
            approval.clone(),
            Approval {
                id: approval,
                conversation_id: idle.id.clone(),
                task_id: None,
                request_id: None,
                position: 1,
                subject: ApprovalSubject::Action {
                    action: "deploy".into(),
                    details: String::new(),
                },
                state: CardState::Pending,
                created_at_ms: old,
                resolved_at_ms: None,
            },
        );
        assert!(!auto_done(&idle, &asking, ended(old), 0, NOW));
    }

    #[test]
    fn the_inbox_sweep_respects_a_choice_until_the_thread_moves_on() {
        let old = NOW - 2 * IDLE_FOR_DONE_MS;
        let idle = conversation(old);
        let merged = with_pr(PullRequestState::Merged);
        // Done, or brought back by hand, with nothing after it: left alone.
        assert!(!auto_done(
            &idle,
            &Board::default(),
            ended(old),
            old + 1,
            NOW
        ));
        assert!(!auto_done(&idle, &merged, ended(old), old + 1, NOW));
        // A turn after it ended 3 days ago: done again; the merge alone doesn't.
        assert!(auto_done(
            &idle,
            &Board::default(),
            ended(old + 2),
            old + 1,
            NOW
        ));
        let later = NOW - 60_000;
        assert!(!auto_done(
            &conversation(later),
            &merged,
            ended(later),
            old,
            NOW
        ));
    }

    /// A folder in the temp directory, removed when dropped.
    struct Temp(PathBuf);

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[tokio::test]
    async fn inbox_done_snooze_and_seen_survive_a_reload() {
        let dir =
            Temp(std::env::temp_dir().join(format!("brigadier-inbox-{}", uuid::Uuid::now_v7())));
        let store = brigadier_store::Store::open(brigadier_store::StoreConfig {
            db_path: dir.0.join("test.db"),
            blobs_dir: dir.0.join("blobs"),
            readers: 1,
        })
        .unwrap();
        let core = Core::load(store.clone()).await.unwrap();
        let id = core
            .create_conversation(
                ConversationId::generate(),
                ConversationKind::Chat,
                None,
                None,
                None,
                Origin::default(),
            )
            .await
            .unwrap()
            .id;
        assert_eq!(core.done_decided_at(&id), 0);
        let done = core.set_done(id.clone(), true).await.unwrap();
        assert!(done.done_at_ms.is_some());
        let decided = core.done_decided_at(&id);
        assert!(decided > 0);
        core.snooze(id.clone(), Some(5_000)).await.unwrap();
        core.mark_seen(id.clone(), 40).await.unwrap();
        let seen = core.mark_seen(id.clone(), 30).await.unwrap();
        assert_eq!(seen.seen_at_ms, Some(40), "seen only moves forward");

        let reloaded = Core::load(store.clone()).await.unwrap();
        let conversation = reloaded.conversation(&id).unwrap();
        assert_eq!(conversation.done_at_ms, done.done_at_ms);
        assert_eq!(conversation.snoozed_until_ms, Some(5_000));
        assert!(conversation.snoozed_at_ms.is_some());
        assert_eq!(conversation.seen_at_ms, Some(40));
        assert_eq!(reloaded.done_decided_at(&id), decided);

        reloaded.set_done(id.clone(), false).await.unwrap();
        reloaded.snooze(id.clone(), None).await.unwrap();
        let reloaded = Core::load(store).await.unwrap();
        let conversation = reloaded.conversation(&id).unwrap();
        assert_eq!(conversation.done_at_ms, None);
        assert_eq!(conversation.snoozed_until_ms, None);
        assert!(
            reloaded.done_decided_at(&id) >= decided,
            "bringing it back is a choice the sweep respects too"
        );
    }
}
