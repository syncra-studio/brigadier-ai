//! Reports one fact to the user when an accepted thread turn is silent. A running
//! command or tool call is left alone; the watchdog never steers or interrupts the CLI.
use super::*;
use crate::manager::watchdog::Timing;
use crate::manager::watchdog::progress::Progress;
use crate::work::CardState;
use brigadier_providers::NoticeLevel;

#[derive(Debug, Default)]
pub(super) struct ThreadWatch {
    progress: Progress,
    last_ms: i64,
    reported: Option<i64>,
}

fn due(watch: &ThreadWatch, timing: &Timing, now: i64, waiting: bool) -> bool {
    !waiting
        && !watch.progress.waiting()
        && watch.progress.command().is_none()
        && watch.reported.is_none()
        && now - watch.last_ms >= timing.silent
}

/// A turn is watched only after acceptance, with no stop, quota or hand-over under way.
fn eligible(state: &ConvState) -> bool {
    (state.busy || state.unowned_turn)
        && state.landed
        && !state.awaiting_start
        && !state.interrupted
        && !state.closing
        && !state.held
        && !state.waiting
        && state.hand_over.is_none()
        && state.limit_hit.is_none()
        && !state.compacting
}

fn waiting_on_board(board: &crate::board::Board, request: &Option<String>) -> bool {
    board
        .approvals
        .values()
        .any(|c| c.state == CardState::Pending)
        || board.questions.values().any(|c| c.is_open())
        || board.tasks.values().any(|t| {
            t.request_id == *request
                && (t.quota_wait.is_some()
                    || matches!(
                        t.state,
                        crate::work::TaskState::Starting | crate::work::TaskState::Running
                    ))
        })
}

// The event pump still calls these recovery hooks. Silence no longer owns or restarts
// a turn; normal process-exit handling remains in the event pump.
pub(super) const CARRY_ON: &str = prompts::CONTINUE_ON_ACCOUNT;

impl ThreadWatch {
    pub(super) fn acknowledge(&mut self) {}
    pub(super) fn take_carry_note(&mut self) -> bool {
        false
    }
    pub(super) fn recovering(&self) -> bool {
        false
    }
    pub(super) fn begin(&mut self, now: i64) {
        self.progress = Progress::default();
        self.last_ms = now;
        self.reported = None;
    }
    pub(super) fn observe(&mut self, event: &ProviderEvent, now: i64) {
        if self.progress.observe(event, now) {
            self.last_ms = now;
            self.reported = None;
        }
    }
}

impl SessionManager {
    pub(in crate::manager) async fn note_thread_progress(
        &self,
        conv: &ConvLive,
        cli: &Arc<Cli>,
        event: Option<&ProviderEvent>,
    ) {
        let Some(event) = event else {
            return;
        };
        let mut state = conv.state.lock().await;
        if state
            .cli
            .as_ref()
            .is_some_and(|current| Arc::ptr_eq(current, cli))
        {
            if matches!(event, ProviderEvent::TurnStarted { .. }) && !state.busy {
                state.watchdog.begin(now_ms());
            }
            state.watchdog.observe(event, now_ms());
        }
    }

    /// Called from the event pump's timer, and with injected time by flow tests.
    pub(crate) async fn watch_thread(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>, now: i64) {
        let timing = Timing::current();
        let Ok(board) = self.core.board(&conv.id).await else {
            return;
        };
        let text = {
            let mut state = conv.state.lock().await;
            if !state.cli.as_ref().is_some_and(|c| Arc::ptr_eq(c, cli))
                || !cli.session.is_running()
                || !eligible(&state)
            {
                return;
            }
            let waiting = waiting_on_board(&board, &state.request);
            if !due(&state.watchdog, &timing, now, waiting) {
                if waiting || state.watchdog.progress.waiting() {
                    state.watchdog.last_ms = now;
                    state.watchdog.reported = None;
                }
                return;
            }
            let text = format!(
                "Thread: silent {} min, last command {}",
                (now - state.watchdog.last_ms) / 60_000,
                state
                    .watchdog
                    .progress
                    .last_command(now)
                    .as_deref()
                    .unwrap_or("none")
            );
            state.watchdog.reported = Some(now);
            text
        };
        self.notice(&conv.id, NoticeLevel::Warning, &text).await;
    }

    pub(super) async fn finish_thread_stall(&self, _conv: &Arc<ConvLive>) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_silent_thread_reports_once_until_progress() {
        let timing = Timing::scaled(180);
        let mut watch = ThreadWatch::default();
        watch.begin(0);
        assert!(!due(&watch, &timing, 179_999, false));
        assert!(due(&watch, &timing, 180_000, false));
        watch.reported = Some(180_000);
        assert!(!due(&watch, &timing, 299_999, false));
        assert!(!due(&watch, &timing, 3_000_000, false));
        watch.observe(
            &ProviderEvent::ReasoningDelta {
                item_id: "r".into(),
                text: "progress".into(),
            },
            3_000_000,
        );
        assert!(due(&watch, &timing, 3_180_000, false));
        assert!(!due(&watch, &timing, 3_000_000, true));
    }

    #[test]
    fn watchdog_thread_build_output_and_quiet_builds_have_no_false_stop() {
        let timing = Timing::scaled(180);
        let mut watch = ThreadWatch::default();
        let event = ProviderEvent::Command {
            item_id: "c".into(),
            command: "cargo build".into(),
            cwd: None,
            status: ItemStatus::InProgress,
            exit_code: None,
            output: None,
            duration_ms: None,
            kept: None,
        };
        watch.observe(&event, 0);
        assert!(!due(&watch, &timing, 30 * 60_000 - 1, false));
        for minute in 1..=120 {
            watch.observe(
                &ProviderEvent::CommandOutputDelta {
                    item_id: "c".into(),
                    text: format!("Compiled crate {minute}"),
                },
                minute * 60_000,
            );
            assert!(!due(&watch, &timing, minute * 60_000, false));
        }
        assert!(!due(&watch, &timing, 150 * 60_000, false));
    }
    /// The CLI's own turn for a stopped request's job is watched like any turn: its silence is
    /// reported, as turns still owed wait for its end (#221).
    #[test]
    fn watchdog_watches_a_turn_of_no_requests() {
        let unowned = ConvState {
            unowned_turn: true,
            landed: true,
            ..ConvState::default()
        };
        assert!(eligible(&unowned));
        assert!(!eligible(&ConvState {
            unowned_turn: false,
            ..unowned
        }));
    }

    #[test]
    fn watchdog_thread_waiting_for_quota_or_stopping_is_never_touched() {
        let working = || ConvState {
            busy: true,
            landed: true,
            ..ConvState::default()
        };
        assert!(eligible(&working()));
        for change in [
            (|s: &mut ConvState| s.waiting = true) as fn(&mut ConvState),
            |s| s.interrupted = true,
            |s| s.closing = true,
            |s| s.awaiting_start = true,
            |s| s.landed = false,
            |s| s.held = true,
            |s| {
                s.hand_over = Some(HandOver {
                    request: None,
                    stopped: false,
                })
            },
        ] {
            let mut state = working();
            change(&mut state);
            assert!(!eligible(&state));
        }
    }
}
