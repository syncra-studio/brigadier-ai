//! A watchdog for a turn the thread CLI has accepted. Recovery preserves the native session
//! and uses `continuing`, just like an account switch, so accepted user messages aren't resent.
use super::*;
use crate::manager::watchdog::progress::{CommandKind, Progress};
use crate::manager::watchdog::{Timing, spoken};
use crate::work::CardState;
use brigadier_providers::NoticeLevel;

#[derive(Debug, Default)]
pub(super) struct ThreadWatch {
    progress: Progress,
    last_ms: i64,
    nudged: Option<i64>,
    recovered: HashSet<String>,
    action: Option<Action>,
    interrupting: bool,
    acknowledged: bool,
    carry_note: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Continue,
    Fail,
}

fn unanswered(watch: &ThreadWatch, timing: &Timing) -> bool {
    watch.nudged.is_some_and(|at| {
        at >= watch.last_ms
            || (watch.progress.command() == Some(CommandKind::Server)
                && watch
                    .progress
                    .command_deadline(timing)
                    .is_some_and(|deadline| deadline <= at))
    })
}

/// Pure deadline decision, with legitimate waits supplied by the caller.
fn due(watch: &ThreadWatch, timing: &Timing, now: i64, waiting: bool) -> bool {
    if waiting || watch.progress.waiting() || watch.action.is_some() {
        return false;
    }
    if let Some(nudged) = watch.nudged.filter(|_| unanswered(watch, timing)) {
        return now - nudged >= timing.after_nudge;
    }
    let deadline = watch
        .progress
        .command()
        .map_or(timing.silent, |k| k.deadline(timing));
    watch
        .progress
        .command_deadline(timing)
        .map_or(now - watch.last_ms >= deadline, |at| now >= at)
}

/// A turn is watched only after acceptance, with no stop, quota or hand-over under way.
fn eligible(state: &ConvState) -> bool {
    state.busy
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
        || board
            .plans
            .values()
            .any(|p| matches!(p.state, crate::work::PlanState::Proposed))
        || board.tasks.values().any(|t| {
            t.request_id == *request
                && (t.quota_wait.is_some()
                    || matches!(
                        t.state,
                        crate::work::TaskState::Starting | crate::work::TaskState::Running
                    ))
        })
}

pub(super) const CARRY_ON: &str = "[Brigadier] The previous turn stopped responding. Carry on from this session's transcript. Check what already happened; do not repeat completed actions.";

impl ThreadWatch {
    pub(super) fn acknowledge(&mut self) {
        if self.interrupting {
            self.acknowledged = true;
        }
    }
    pub(super) fn take_carry_note(&mut self) -> bool {
        std::mem::take(&mut self.carry_note)
    }
    pub(super) fn recovering(&self) -> bool {
        self.action.is_some()
    }
    pub(super) fn begin(&mut self, now: i64) {
        self.progress = Progress::default();
        self.last_ms = now;
        self.nudged = None;
        self.acknowledged = false;
    }
    pub(super) fn observe(&mut self, event: &ProviderEvent, now: i64) {
        if self.progress.observe(event, now) {
            self.last_ms = now;
            if matches!(event, ProviderEvent::TurnStarted { .. }) {
                self.nudged = None;
            }
        }
    }
}

impl SessionManager {
    pub(super) async fn note_thread_progress(
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

    /// Called from the event pump's timer, and directly with injected time by flow tests.
    /// The event pump stays free to drain an interrupt's completion and streamed output.
    pub(crate) async fn watch_thread(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>, now: i64) {
        let timing = Timing::current();
        let Ok(board) = self.core.board(&conv.id).await else {
            return;
        };
        let (action, request, silent, server) = {
            let mut state = conv.state.lock().await;
            if !state.cli.as_ref().is_some_and(|c| Arc::ptr_eq(c, cli)) || !eligible(&state) {
                return;
            }
            let waiting = waiting_on_board(&board, &state.request);
            if !due(&state.watchdog, &timing, now, waiting) {
                // A legitimate wait must get a fresh allowance once it ends.
                if waiting || state.watchdog.progress.waiting() {
                    state.watchdog.last_ms = now;
                    state.watchdog.nudged = None;
                }
                return;
            }
            let silent = now - state.watchdog.last_ms;
            let request = state.request.clone();
            let server = state.watchdog.progress.command() == Some(CommandKind::Server);
            let action = if unanswered(&state.watchdog, &timing) {
                let action = if request
                    .as_ref()
                    .is_some_and(|r| state.watchdog.recovered.contains(r))
                {
                    Action::Fail
                } else {
                    Action::Continue
                };
                state.watchdog.action = Some(action);
                Some(action)
            } else {
                // Set before awaiting the provider; a CLI which won't read still gets recovered.
                state.watchdog.nudged = Some(now);
                None
            };
            (action, request, silent, server)
        };
        let text = match action {
            None if server => "A foreground server exceeded its deadline: asked the model to run it in the background or with a timeout.".into(),
            None => format!("No progress from the model for {}: asked it to carry on.", spoken(silent)),
            Some(Action::Continue) => "The model stayed silent after a nudge: interrupting and continuing in the same session.".into(),
            Some(Action::Fail) => "The model stopped responding again after continuing. The request has failed; its transcript and changes are kept.".into(),
        };
        self.notice(&conv.id, NoticeLevel::Warning, &text).await;
        self.decided_for_you(
            &conv.id,
            request.clone(),
            crate::work::DecisionSource::Orchestrator,
            text,
            "The accepted turn stopped making progress and has nothing it is waiting for.".into(),
        )
        .await;
        // Provider output and a newly opened wait can arrive while the notices are stored.
        // Recheck immediately before steering or interrupting, as the worker watchdog does.
        let board = self.core.board(&conv.id).await.ok();
        {
            let mut state = conv.state.lock().await;
            let same_cli = state.cli.as_ref().is_some_and(|c| Arc::ptr_eq(c, cli));
            let exited = action.is_some() && cli.ended.is_cancelled() && state.cli.is_none();
            let same = (same_cli || exited) && state.request == request;
            let fresh = state.watchdog.last_ms != now - silent;
            let waiting = board
                .as_ref()
                .is_none_or(|b| waiting_on_board(b, &state.request));
            if exited && state.interrupted && state.watchdog.action.is_some() {
                state.watchdog.interrupting = true;
                drop(state);
                self.finish_thread_stall(conv).await;
                return;
            }
            if !same || !eligible(&state) || fresh || waiting || state.watchdog.action != action {
                if same {
                    state.watchdog.action = None;
                    state.watchdog.nudged = None;
                }
                return;
            }
            state.watchdog.interrupting = action.is_some();
        }
        if action.is_none() {
            let text = if server {
                "[Brigadier] A foreground server has kept this turn open past its deadline. Stop it and run it in the background or with a timeout, then carry on.".into()
            } else {
                format!(
                    "[Brigadier] Nothing has come from you for {}. If a command is hanging, stop it and use a timeout. Otherwise carry on, or say what stops you.",
                    spoken(silent)
                )
            };
            let _ = tokio::time::timeout(
                Duration::from_secs(5),
                cli.session.steer(TurnInput::text(text)),
            )
            .await;
            return;
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), cli.session.interrupt()).await;
        // Most CLIs report Interrupted. If one cannot, close it; ensure_cli resumes the
        // saved native session. Closing is outside the pump, whose drain close_cli awaits.
        for _ in 0..50 {
            {
                let state = conv.state.lock().await;
                if state.watchdog.action.is_none() || state.watchdog.acknowledged {
                    return;
                }
            }
            if cli.ended.is_cancelled() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let closing = {
            let mut state = conv.state.lock().await;
            if state.watchdog.action.is_none() {
                return;
            }
            state.closing = true;
            state.cli.take()
        };
        if let Some(cli) = closing {
            cli.session.close().await;
            cli.ended.cancelled().await;
        }
        conv.state.lock().await.closing = false;
        self.finish_thread_stall(conv).await;
    }

    /// Consumes a watchdog interrupt rather than treating it as the user's Stop. Invoked
    /// from turn_completed, or after a CLI that ignored interrupt has been closed.
    pub(super) async fn finish_thread_stall(&self, conv: &Arc<ConvLive>) -> bool {
        let (action, request, stopped) = {
            let mut state = conv.state.lock().await;
            let Some(action) = state.watchdog.action.take() else {
                return false;
            };
            // A natural completion raced with the notice, before we asked for interruption.
            if !std::mem::take(&mut state.watchdog.interrupting) {
                return false;
            }
            // A user's stop always wins, including when the CLI exited during recovery.
            let stopped = state.interrupted;
            let request = state.request.take();
            state.busy = false;
            state.awaiting_start = false;
            state.landed = false;
            state.commands.clear();
            state.command_words.clear();
            state.holding.clear();
            state.watchdog.progress = Progress::default();
            state.watchdog.nudged = None;
            if action == Action::Continue && !stopped {
                if let Some(request) = &request {
                    state.watchdog.recovered.insert(request.clone());
                }
                let mut carried = std::mem::take(&mut state.in_turn);
                state
                    .continuing
                    .extend(carried.iter().map(|m| m.id.clone()));
                let carried_users = !carried.is_empty();
                state.watchdog.carry_note = carried_users;
                carried.append(&mut state.pending);
                state.pending = carried;
                state.carry_peers();
                // A turn started only by envelopes has no user message to requeue.
                if !carried_users {
                    state.inbox.push((
                        Envelope {
                            kind: InjectionKind::Reminder,
                            label: "continue after silence".into(),
                            task_id: None,
                            text: CARRY_ON.into(),
                        },
                        request.clone(),
                    ));
                }
            } else if let Some(request) = &request {
                state.outcomes.insert(request.clone(), if stopped { RequestState::Stopped } else { RequestState::Failed {
                    error: "The model stopped responding again after continuing. Your transcript and changes are kept.".into(),
                }});
            }
            (action, request, stopped)
        };
        self.return_held_replies(conv).await;
        self.release_held(conv).await;
        if stopped {
            self.set_run(&conv.id, RunState::Idle, None).await;
            self.settle_requests(&conv.id).await;
        } else if action == Action::Fail {
            self.set_run_for(&conv.id, RunState::Failed,
                Some("The model stopped responding again after continuing. Your transcript and changes are kept.".into()), request).await;
            self.settle_requests(&conv.id).await;
        } else {
            self.kick(conv);
        }
        true
    }
}

/// For the flow tests, which run on unix only.
#[cfg(all(test, unix))]
impl ConvLive {
    pub(crate) async fn watchdog_snapshot(&self) -> (bool, i64, Option<i64>) {
        let state = self.state.lock().await;
        (
            state.landed && !state.awaiting_start,
            state.watchdog.last_ms,
            state.watchdog.nudged,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_silent_thread_nudges_at_three_minutes_recovers_two_minutes_later() {
        let timing = Timing::scaled(180);
        let mut watch = ThreadWatch::default();
        watch.begin(0);
        assert!(!due(&watch, &timing, 179_999, false));
        assert!(due(&watch, &timing, 180_000, false));
        watch.nudged = Some(180_000);
        assert!(!due(&watch, &timing, 299_999, false));
        assert!(due(&watch, &timing, 300_000, false));
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
        assert!(due(&watch, &timing, 150 * 60_000, false));
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
