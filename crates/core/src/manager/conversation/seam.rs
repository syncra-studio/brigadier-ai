//! The seam in the conversation driver (S22, [`crate::manager::seam`]): the nudge, closing the
//! predecessor, starting the successor from its hand-off, the held messages and recovery.
//!
//! - **Nudge.** Past the threshold, with no background job or open permission card, the one
//!   message is steered into the running turn; with none running it starts a turn of its own.
//!   `seam.json` says `nudged` before it is sent.
//! - **Hold.** While nudged nothing else reaches the thread: a user message waits in the queue
//!   (persisted), anything else in memory, for the successor. If the note turn is still going
//!   a minute after a message came, it is stopped and the seam closes without waiting.
//! - **Close.** The turn that read the nudge ends: with a note or without, the seam closes
//!   (`closed`) and the predecessor's CLI goes.
//! - **Successor.** It starts at once, carrying the messages that waited (or, with none, told
//!   to carry on from the note; after the user's Stop in the note turn, their next message
//!   starts it): the turn writes `starting` with a session id reserved for it
//!   and those messages, starts a fresh CLI under that id, and sends the hand-off with them;
//!   once sent, the seam is complete and the generation one more.
//! - **Death.** "Prompt is too long", or a CLI that can't be resumed, ends the generation
//!   without a note: the successor starts the same way, told to check git first.
//! - **Another agent.** Every account of the session's agent at its limit, with "Switch to
//!   another agent automatically" on (S38, D23): the generation ends without a note and the
//!   successor starts on another agent, with the turn's messages.

use std::sync::Arc;

use brigadier_providers::{Origin, ProviderKind, TurnInput};

use super::{Cli, ConvLive, Envelope};
use crate::manager::SessionManager;
use crate::manager::seam::{
    Boot, Phase, SeamEnd, SeamFile, TailBudget, bootstrap, complete, handoff_line, note_path,
    nudge, read_seam, start_path, write_seam,
};
use crate::manager::session_dir::read_lineage;
use crate::model::{ConversationId, ConversationKind, Message, ModelChoice, Setup};
use crate::now_ms;
use crate::work::RunState;
use crate::work::{CardState, InjectionKind};

/// The seam's in-memory part of a conversation's state.
#[derive(Default)]
pub(super) struct SeamLive {
    /// A nudge is being sent.
    nudging: bool,
    /// The nudge goes as a turn of its own, this text, once none runs.
    nudge_due: Option<String>,
    /// `seam.json` says `nudged` (or is about to): nothing but the note turn reaches the CLI.
    pub(super) held: bool,
    /// The CLI took the nudge in (its stream said it): the turn it is in ends the generation.
    seen: bool,
    /// The note turn is being stopped: when it ends, the seam closes whatever it wrote.
    force: bool,
    /// Counts the hold timers armed; only the newest acts.
    hold_timer: u64,
    hold_armed: bool,
    /// The session id the next CLI starts under (a successor's, reserved in `seam.json`).
    pub(super) reserved: Option<String>,
    /// The seam closed and its successor starts by itself, when no message starts it first.
    pub(super) carry_on: bool,
    /// The request whose turn the generation died in ("prompt is too long"): it still works,
    /// and the successor that carries on by itself serves it.
    pub(super) carried_request: Option<String>,
    /// The context a successor's CLI first reported, which its generation grows from before
    /// its seam (none for the first generation); `None` until looked up in `seam.json`.
    pub(super) base: Option<Option<i64>>,
}

/// A successor about to start: its seam, how its predecessor ended and what it carries.
pub(super) struct Successor {
    n: u32,
    end: SeamEnd,
    messages: Vec<String>,
}

/// Why a generation ended without its note when the CLI said nothing better.
const NO_NOTE: &str = "it ended its turn without writing one";
const HELD_TOO_LONG: &str = "it was still working a minute after a message came";
const RESTARTED: &str = "Brigadier restarted before it wrote one";
const NOT_RESUMED: &str = "its CLI session could not be resumed";
const TOO_LONG: &str = "its context was too long for the model";
const USER_STOPPED: &str = "the user stopped it";

impl SessionManager {
    /// Whether conversation `conv` goes on at seams: a session's thread (a Chat compacts).
    pub(in crate::manager) fn seam_on(&self, conv: &ConvLive) -> bool {
        conv.kind == ConversationKind::Session
    }

    pub(in crate::manager) async fn read_seam_of(&self, id: &ConversationId) -> SeamFile {
        let record = self.session_dir(id).record();
        crate::manager::blocking(move || Ok(read_seam(&record)))
            .await
            .unwrap_or_default()
    }

    pub(super) async fn write_seam_of(
        &self,
        id: &ConversationId,
        file: SeamFile,
    ) -> crate::Result<()> {
        let record = self.session_dir(id).record();
        crate::manager::blocking(move || write_seam(&record, &file)).await
    }

    /// The CLI said how full its context is: past the threshold, the nudge.
    pub(super) async fn seam_context(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        used: i64,
        window: Option<i64>,
    ) {
        let base = self.seam_base(conv, used).await;
        if used >= self.seam_config().threshold(window, base) {
            self.seam_nudge(conv, Some(cli), used, true).await;
        }
    }

    /// What a successor's generation grows from: its first reported context, kept in
    /// `seam.json` so a restart doesn't take a later one.
    async fn seam_base(&self, conv: &Arc<ConvLive>, used: i64) -> Option<i64> {
        if let Some(base) = conv.state.lock().await.seam.base {
            return base;
        }
        let mut file = self.read_seam_of(&conv.id).await;
        let generation = file.generation;
        let base = match (&file.phase, file.seams.last_mut()) {
            (Phase::Live, Some(seam)) if seam.n + 1 == generation => match seam.first_context {
                Some(first) => Some(first),
                None => {
                    seam.first_context = Some(used);
                    self.write_seam_of(&conv.id, file).await.ok();
                    Some(used)
                }
            },
            // The successor reports before it is recorded: `seam_started` keeps this.
            (Phase::Starting { .. }, _) => Some(used),
            _ => None,
        };
        conv.state.lock().await.seam.base = Some(base);
        base
    }

    /// Asks for the hand-off once per generation: steered into the running turn when
    /// `steer` and one runs, else as a turn of its own. Not while the CLI runs jobs in the
    /// background or a permission card is open: their ends report the context again.
    pub(super) async fn seam_nudge(
        &self,
        conv: &Arc<ConvLive>,
        cli: Option<&Arc<Cli>>,
        used: i64,
        steer: bool,
    ) {
        {
            let mut state = conv.state.lock().await;
            if state.seam.nudging || state.seam.held || state.seam.nudge_due.is_some() {
                return;
            }
            state.seam.nudging = true;
        }
        let sent = self.send_nudge(conv, cli, used, steer).await;
        let mut state = conv.state.lock().await;
        state.seam.nudging = false;
        if sent == Some(false) {
            drop(state);
            self.kick(conv);
        }
    }

    /// `Some(true)`: steered in; `Some(false)`: due as a turn of its own; `None`: not now.
    async fn send_nudge(
        &self,
        conv: &Arc<ConvLive>,
        cli: Option<&Arc<Cli>>,
        used: i64,
        steer: bool,
    ) -> Option<bool> {
        let mut file = self.read_seam_of(&conv.id).await;
        if file.phase != Phase::Live
            || conv.has_cli_jobs().await
            || self.open_permission(&conv.id).await
        {
            return None;
        }
        let record = self.session_dir(&conv.id).record();
        let note = note_path(&record, file.generation);
        if let Some(dir) = note.parent()
            && let Err(err) = std::fs::create_dir_all(dir)
        {
            crate::logging::private_trace!(conv.private_logs, warn, "could not make the hand-off folder"; conversation = %conv.id, error = %err);
        }
        // With no CLI up (a move asked for after a restart), the one the lineage ends with.
        let predecessor = match cli {
            Some(cli) => Some(cli.session.native_id()),
            None => read_lineage(&record)
                .last()
                .map(|session| session.session_id.clone()),
        };
        file.phase = Phase::Nudged {
            at_ms: now_ms(),
            tokens: used,
            predecessor,
        };
        // Held before the file says so: a message that finds the seam nudged waits for the
        // successor, at most a minute.
        conv.state.lock().await.seam.held = true;
        if let Err(err) = self.write_seam_of(&conv.id, file).await {
            conv.state.lock().await.seam.held = false;
            crate::logging::private_trace!(conv.private_logs, warn, "could not record the seam"; conversation = %conv.id, error = %err);
            return None;
        }
        // So does one queued just before.
        if self
            .core
            .conversation_view(conv.id.clone(), 1)
            .await
            .is_ok_and(|view| !view.queue.items.is_empty())
        {
            self.arm_seam_hold(conv).await;
        }
        let mut text = nudge(&note);
        if self.seam_config().long.is_some() {
            text.push_str(crate::manager::seam::LONG_NOTE);
        }
        let mut state = conv.state.lock().await;
        let running = cli
            .zip(state.cli.as_ref())
            .is_some_and(|(cli, now)| Arc::ptr_eq(now, cli))
            && state.busy
            && !state.interrupted
            && !state.compacting;
        if steer
            && running
            && let Some(cli) = cli
            && cli
                .session
                .steer(TurnInput::text(text.clone()))
                .await
                .is_ok()
        {
            drop(state);
            crate::logging::private_trace!(conv.private_logs, info, "asked for the hand-off note"; conversation = %conv.id, used);
            return Some(true);
        }
        state.seam.nudge_due = Some(text);
        Some(false)
    }

    /// Whether a permission card of the conversation's own waits on the user.
    async fn open_permission(&self, id: &ConversationId) -> bool {
        self.core.board(id).await.is_ok_and(|board| {
            board
                .approvals
                .values()
                .any(|approval| approval.state == CardState::Pending)
        })
    }

    /// A closed seam's successor that starts by itself, taken for the next turn: when nothing
    /// else is there to start it.
    /// With the request it serves, if the generation died in one's turn.
    pub(super) fn take_carry_on(
        state: &mut super::ConvState,
    ) -> Option<(Envelope, Option<String>)> {
        if !std::mem::take(&mut state.seam.carry_on) {
            return None;
        }
        let envelope = Envelope {
            kind: InjectionKind::Seam,
            label: "carry on".into(),
            task_id: None,
            text: crate::manager::seam::CARRY_ON.into(),
        };
        Some((envelope, state.seam.carried_request.take()))
    }

    /// The nudge due as a turn of its own, taken for the next turn.
    pub(super) fn take_due_nudge(state: &mut super::ConvState) -> Option<Envelope> {
        let text = state.seam.nudge_due.take()?;
        state.seam.seen = true;
        Some(Envelope {
            kind: InjectionKind::Seam,
            label: "hand-off note".into(),
            task_id: None,
            text,
        })
    }

    /// The CLI's stream showed a user message: the nudge, once taken in.
    pub(super) async fn seam_heard(&self, conv: &ConvLive, text: &str) {
        if text.starts_with(crate::thread_view::NUDGE_MARK) {
            conv.state.lock().await.seam.seen = true;
        }
    }

    /// A turn of the session's CLI is ending (before it is marked over): the nudge it read
    /// closes the seam, and past the threshold with none sent yet, one is due. Answers whether
    /// the seam closed (the next turn then retires the CLI).
    pub(super) async fn seam_turn_end(&self, conv: &Arc<ConvLive>, cli: &Arc<Cli>) -> bool {
        let file = self.read_seam_of(&conv.id).await;
        match file.phase {
            Phase::Nudged { at_ms, .. } => {
                let note = note_path(&self.session_dir(&conv.id).record(), file.generation);
                let (seen, force, stopped, why) = {
                    let state = conv.state.lock().await;
                    let stopped = state.interrupted && !state.seam.force;
                    let why = if state.limit_hit.is_some() {
                        "it hit its usage limit".to_owned()
                    } else if let Some(error) = &state.turn_error {
                        format!("its turn failed: {error}")
                    } else if stopped {
                        USER_STOPPED.to_owned()
                    } else {
                        NO_NOTE.to_owned()
                    };
                    (state.seam.seen, state.seam.force, stopped, why)
                };
                let written = note.is_file();
                if !(written || seen || force) {
                    // The nudge is still on its way into the CLI: its own turn takes it.
                    self.arm_seam_hold(conv).await;
                    return false;
                }
                let death = (!written).then(|| if force { HELD_TOO_LONG.into() } else { why });
                self.close_seam(conv, file, Some(at_ms), death, stopped)
                    .await
            }
            Phase::Live => {
                let (used, base, limited) = {
                    let state = conv.state.lock().await;
                    (
                        state.context,
                        state.seam.base.flatten(),
                        state.limit_hit.is_some(),
                    )
                };
                // A move to another CLI asked for in this turn comes now; one cut by a limit
                // goes with the limit's own hand-over.
                if file.moving.is_some() {
                    if !limited {
                        self.begin_move(conv, Some(cli)).await;
                    }
                } else if let Some((used, window)) = used
                    && self.long_mode(conv, cli.provider).is_none()
                    && used >= self.seam_config().threshold(window, base)
                {
                    self.seam_nudge(conv, Some(cli), used, false).await;
                }
                false
            }
            Phase::Closed { .. } | Phase::Starting { .. } => false,
        }
    }

    /// Ends generation `file.generation`: `closed`, with `death` when it left no note. Its
    /// successor starts at once, unless the user `stopped` the turn: then their next message
    /// starts it.
    async fn close_seam(
        &self,
        conv: &Arc<ConvLive>,
        mut file: SeamFile,
        nudged_at_ms: Option<i64>,
        death: Option<String>,
        stopped: bool,
    ) -> bool {
        let moved_to = match file.moving.take() {
            Some(to) => self.move_setup(conv, to).await,
            None => None,
        };
        let predecessor = match &file.phase {
            Phase::Nudged { predecessor, .. } => predecessor.clone(),
            _ => conv
                .state
                .lock()
                .await
                .cli
                .as_ref()
                .map(|cli| cli.session.native_id()),
        };
        let hot_tokens =
            conv.state
                .lock()
                .await
                .context
                .map(|(used, _)| used)
                .or(match file.phase {
                    Phase::Nudged { tokens, .. } if tokens > 0 => Some(tokens),
                    _ => None,
                });
        file.phase = Phase::Closed {
            end: SeamEnd {
                predecessor,
                hot_tokens,
                nudged_at_ms,
                closed_at_ms: now_ms(),
                death,
                stopped,
                shown: Vec::new(),
                moved_to,
            },
        };
        if let Err(err) = self.write_seam_of(&conv.id, file).await {
            crate::logging::private_trace!(conv.private_logs, warn, "could not record the seam"; conversation = %conv.id, error = %err);
            return false;
        }
        crate::logging::private_trace!(conv.private_logs, info, "the session's generation ended at its seam"; conversation = %conv.id);
        let mut state = conv.state.lock().await;
        state.seam.held = false;
        state.seam.seen = false;
        state.seam.force = false;
        state.seam.hold_armed = false;
        state.seam.hold_timer += 1;
        state.seam.carry_on = !stopped;
        true
    }

    /// The agent another one goes on with when every account of `from`'s is at its limit,
    /// with "Switch to another agent automatically" on (S38, D23): the first `Fallback` row of
    /// the user's routing, in its order, on another agent that is on and has an account that
    /// can take work, and that the user's rules allow (D34). `None`: the session waits for the
    /// reset (Q11).
    pub(super) fn fallback_agent(
        &self,
        conv: &ConvLive,
        from: &ModelChoice,
    ) -> Option<ModelChoice> {
        let settings = self.core.settings();
        if !settings.switch_providers || !self.seam_on(conv) {
            return None;
        }
        let now = now_ms();
        self.fallback_rows()
            .into_iter()
            .filter(|choice| {
                choice.provider != from.provider
                    && crate::routing::availability::provider_on(&settings, choice.provider)
                    && crate::accounts::ready(
                        &settings,
                        choice.provider,
                        None,
                        &self.runtime.account_candidates(choice.provider),
                        &[],
                        now,
                    )
                    .is_some()
            })
            .find(|choice| self.fallback_refusal(choice).is_none())
    }

    /// The model and effort a session goes on with on another agent, `provider` (D25, D34): the
    /// first `Fallback` row of the user's routing that names that agent and that the user's
    /// rules allow. Never a model or effort no row names; with none, why, in plain words.
    pub(crate) fn successor_model(&self, provider: ProviderKind) -> Result<ModelChoice, String> {
        let rows: Vec<ModelChoice> = self
            .fallback_rows()
            .into_iter()
            .filter(|choice| choice.provider == provider)
            .collect();
        let mut refused = None;
        for choice in rows {
            match self.fallback_refusal(&choice) {
                None => return Ok(choice),
                Some(rule) => {
                    refused.get_or_insert(rule);
                }
            }
        }
        let label = provider.label();
        Err(match refused {
            Some(rule) => format!(
                "your model rules refuse every Fallback row for {label} ({rule}); change the \
                 rows or the rules in Settings › Routing"
            ),
            None => format!(
                "no Fallback row in Settings › Routing names {label}; add one to continue there"
            ),
        })
    }

    /// Why the user's rules refuse a `Fallback` row's `choice` (`None`: they allow it).
    fn fallback_refusal(&self, choice: &ModelChoice) -> Option<String> {
        self.model_refusal(
            choice.provider,
            choice.model.as_deref(),
            choice.effort.as_deref(),
            Some(crate::manager::models::FALLBACK_WORK),
        )
        .unwrap_or_else(|err| Some(err.to_string()))
    }

    /// Every account of the session's agent (`from`) is at its limit: the generation ends
    /// without a note, the session's model becomes `to`, and the successor starts there from
    /// the hand-off with the turn's messages (`carried`) and those that waited. The divider
    /// says why.
    pub(super) async fn seam_to_agent(
        &self,
        conv: &Arc<ConvLive>,
        from: ProviderKind,
        to: ModelChoice,
        carried: Vec<Message>,
        landed: bool,
    ) {
        let Ok(conversation) = self.core.conversation(&conv.id) else {
            return;
        };
        let Some(mut setup) = conversation.setup.clone() else {
            return;
        };
        let Setup::Session { orchestrator, .. } = &mut setup else {
            return;
        };
        let death = format!(
            "usage limit on {} · continued on {}",
            from.label(),
            to.provider.label()
        );
        *orchestrator = to;
        if let Err(err) = self.core.set_setup(conv.id.clone(), setup).await {
            crate::logging::private_trace!(conv.private_logs, warn, "could not move the session to another agent"; conversation = %conv.id, error = %err);
            return;
        }
        let file = self.read_seam_of(&conv.id).await;
        let nudged = match &file.phase {
            Phase::Nudged { at_ms, .. } => Some(*at_ms),
            _ => None,
        };
        // A Stop during the hand-over leaves the successor to the user's next message, after a
        // restart too.
        let stopped = conv
            .state
            .lock()
            .await
            .hand_over
            .as_ref()
            .is_some_and(|over| over.stopped);
        if matches!(file.phase, Phase::Live | Phase::Nudged { .. }) {
            self.close_seam(conv, file, nudged, Some(death), stopped)
                .await;
        }
        // The messages the old CLI had taken are in the thread already: the successor is given
        // them again, and its first message doesn't show them twice (#562). Those of a turn
        // the limit cut (`landed`), or those that waited for the reset after one.
        let shown: Vec<String> = {
            let state = conv.state.lock().await;
            carried
                .iter()
                .filter(|_| landed)
                .map(|message| message.id.clone())
                .chain(state.continuing.iter().cloned())
                .collect()
        };
        let mut file = self.read_seam_of(&conv.id).await;
        if !shown.is_empty()
            && let Phase::Closed { end } = &mut file.phase
        {
            end.shown = shown;
            if let Err(err) = self.write_seam_of(&conv.id, file).await {
                crate::logging::private_trace!(conv.private_logs, warn, "could not record what the thread shows already"; conversation = %conv.id, error = %err);
            }
        }
        let stopped = {
            let mut state = conv.state.lock().await;
            // Stopped meanwhile: its messages stay in the transcript, as a stopped turn's do.
            let stopped = state.hand_over.take().is_some_and(|over| over.stopped);
            if stopped {
                state.seam.carry_on = false;
            } else {
                let mut pending = carried;
                pending.append(&mut state.pending);
                state.pending = pending;
                let envelopes = state.carry_envelopes(false);
                self.keep_carried(&conv.id, &envelopes).await;
            }
            stopped
        };
        conv.close_cli().await;
        if stopped {
            self.set_run(&conv.id, RunState::Idle, None).await;
        }
        self.kick(conv);
    }

    /// The CLI's context overflowed ("prompt is too long"): the generation ends without a note.
    pub(super) async fn seam_too_long(&self, conv: &Arc<ConvLive>) {
        let file = self.read_seam_of(&conv.id).await;
        let nudged = match &file.phase {
            Phase::Live => None,
            Phase::Nudged { at_ms, .. } => Some(*at_ms),
            Phase::Closed { .. } | Phase::Starting { .. } => return,
        };
        // Its request isn't over: the successor goes on with it.
        {
            let mut state = conv.state.lock().await;
            state.seam.carried_request = state.request.clone();
        }
        // The turn's end looks for the next turn, which retires the CLI first.
        self.close_seam(conv, file, nudged, Some(TOO_LONG.into()), false)
            .await;
    }

    /// A message came while the note turn runs, or the turn ended before its CLI took the
    /// nudge in: a minute on, the seam closes whatever the CLI is doing.
    pub(super) async fn arm_seam_hold(&self, conv: &Arc<ConvLive>) {
        let timer = {
            let mut state = conv.state.lock().await;
            if !state.seam.held || state.seam.hold_armed {
                return;
            }
            state.seam.hold_armed = true;
            state.seam.hold_timer += 1;
            state.seam.hold_timer
        };
        let hold = self.seam_config().hold;
        let (manager, conv) = (self.arc(), conv.clone());
        self.spawn(async move {
            tokio::time::sleep(hold).await;
            let cli = {
                let mut state = conv.state.lock().await;
                if state.seam.hold_timer != timer || !state.seam.held {
                    return;
                }
                state.seam.force = true;
                state.cli.clone().filter(|_| state.busy)
            };
            match cli {
                // Its end closes the seam.
                Some(cli) => {
                    let _ = cli.session.interrupt().await;
                }
                None => {
                    let file = manager.read_seam_of(&conv.id).await;
                    let Phase::Nudged { at_ms, .. } = file.phase else {
                        return;
                    };
                    let written =
                        note_path(&manager.session_dir(&conv.id).record(), file.generation)
                            .is_file();
                    let death = (!written).then(|| HELD_TOO_LONG.to_owned());
                    // The next turn retires the CLI first.
                    if manager
                        .close_seam(&conv, file, Some(at_ms), death, false)
                        .await
                    {
                        manager.kick(&conv);
                    }
                }
            }
        });
    }

    /// Before the next turn: a closed seam's predecessor CLI, still up (a limit kept it), goes.
    pub(super) async fn seam_retire(&self, conv: &Arc<ConvLive>) {
        let open = {
            let state = conv.state.lock().await;
            state.cli.is_some() && !state.busy && !state.closing
        };
        if open
            && matches!(
                self.read_seam_of(&conv.id).await.phase,
                Phase::Closed { .. }
            )
        {
            conv.close_cli().await;
        }
    }

    /// The turn about to start goes to a successor when the seam is closed, or when the CLI
    /// could not be resumed (`reseed`): `starting` is written, with the session id reserved
    /// for it and the messages it carries, before its CLI exists.
    pub(super) async fn seam_successor(
        &self,
        conv: &Arc<ConvLive>,
        users: &[Message],
    ) -> Option<Successor> {
        let mut file = self.read_seam_of(&conv.id).await;
        let Phase::Closed { end } = file.phase.clone() else {
            return None;
        };
        let reserved = uuid::Uuid::new_v4().to_string();
        let provider = self
            .core
            .conversation(&conv.id)
            .ok()
            .and_then(|conversation| super::setup_choice(&conversation))
            .map_or(brigadier_providers::ProviderKind::Claude, |choice| {
                choice.provider
            });
        let messages: Vec<String> = users.iter().map(|message| message.id.clone()).collect();
        file.phase = Phase::Starting {
            end: end.clone(),
            successor: reserved.clone(),
            provider,
            messages: messages.clone(),
        };
        if let Err(err) = self.write_seam_of(&conv.id, file.clone()).await {
            crate::logging::private_trace!(conv.private_logs, warn, "could not record the successor's start"; conversation = %conv.id, error = %err);
            return None;
        }
        let mut state = conv.state.lock().await;
        state.seam.reserved = Some(reserved);
        state.seam.carry_on = false;
        // A message started it first: the request the generation died in ends.
        state.seam.carried_request = None;
        state.seam.base = None;
        state.fresh = true;
        // The predecessor's context is not the successor's.
        state.context = None;
        // Nothing is in the new session yet: whatever it carries goes in whole.
        state.continuing.clear();
        state.continuing_envelopes.clear();
        Some(Successor {
            n: file.generation,
            end,
            messages,
        })
    }

    /// A session whose CLI could not be resumed goes on as at a seam with no note: its
    /// generation ends, and `cli`, already started fresh, is the successor.
    pub(super) async fn seam_unresumed(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        users: &[Message],
    ) -> Option<Successor> {
        let mut file = self.read_seam_of(&conv.id).await;
        let end = SeamEnd {
            hot_tokens: None,
            predecessor: read_lineage(&self.session_dir(&conv.id).record())
                .last()
                .map(|session| session.session_id.clone())
                .filter(|id| *id != cli.session.native_id()),
            nudged_at_ms: None,
            closed_at_ms: now_ms(),
            death: Some(NOT_RESUMED.into()),
            stopped: false,
            shown: Vec::new(),
            moved_to: None,
        };
        let messages: Vec<String> = users.iter().map(|message| message.id.clone()).collect();
        file.phase = Phase::Starting {
            end: end.clone(),
            successor: cli.session.native_id(),
            provider: cli.provider,
            messages: messages.clone(),
        };
        self.write_seam_of(&conv.id, file.clone()).await.ok()?;
        let mut state = conv.state.lock().await;
        state.seam.held = false;
        state.seam.base = None;
        state.context = None;
        state.continuing.clear();
        state.continuing_envelopes.clear();
        drop(state);
        Some(Successor {
            n: file.generation,
            end,
            messages,
        })
    }

    /// The successor's CLI is up: its id is the one recorded (Codex picks its own), and its
    /// hand-off is written to disk. Answers the hand-off for its first input.
    pub(super) async fn seam_handoff(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        successor: &Successor,
    ) -> crate::Result<String> {
        let id = conv.id.clone();
        let mut file = self.read_seam_of(&id).await;
        if let Phase::Starting {
            successor: reserved,
            provider,
            ..
        } = &mut file.phase
            && (*reserved != cli.session.native_id() || *provider != cli.provider)
        {
            *reserved = cli.session.native_id();
            *provider = cli.provider;
            self.write_seam_of(&id, file).await?;
        }
        let dir = self.session_dir(&id);
        let key = id.short().to_owned();
        let blobs = self.core.store().blobs().root().to_path_buf();
        let (n, death) = (successor.n, successor.end.death.clone());
        let tail = TailBudget {
            reason: successor.end.reason(),
            hot_tokens: successor.end.hot_tokens,
            target_window: cli.session.context_window(),
        };
        let own = cli.session.native_id();
        crate::manager::blocking(move || {
            let record = dir.record();
            let lineage: Vec<_> = read_lineage(&record)
                .into_iter()
                .filter(|session| session.session_id != own)
                .collect();
            let files: Vec<_> = lineage
                .iter()
                .filter_map(|session| {
                    let file = session.file.as_ref()?;
                    Some((
                        session.provider,
                        brigadier_providers::session_file::locate(
                            session.provider,
                            std::path::Path::new(file),
                        ),
                    ))
                })
                .collect();
            let entries = crate::manager::records::transcript(&files);
            let note_at = note_path(&record, n);
            let note = std::fs::read_to_string(&note_at).ok();
            let start = start_path(&record, n);
            let text = bootstrap(&Boot {
                n,
                tail,
                key: &key,
                folder: dir.root(),
                lineage: &lineage,
                note_path: &note_at,
                note: note.as_deref(),
                start_path: &start,
                death: if note.is_some() {
                    None
                } else {
                    Some(death.as_deref().unwrap_or(NO_NOTE))
                },
                entries: &entries,
                blobs: &blobs,
            });
            if let Some(parent) = start.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|err| crate::Error::Invalid(err.to_string()))?;
            }
            std::fs::write(&start, &text).map_err(|err| crate::Error::Invalid(err.to_string()))?;
            Ok(text)
        })
        .await
    }

    /// The successor took its hand-off: the seam is complete and its generation current.
    pub(super) async fn seam_started(
        &self,
        conv: &Arc<ConvLive>,
        cli: &Arc<Cli>,
        successor: Successor,
    ) {
        let mut file = self.read_seam_of(&conv.id).await;
        let record = self.session_dir(&conv.id).record();
        complete(
            &mut file,
            &record,
            successor.n,
            cli.session.native_id(),
            cli.provider,
            successor.end,
            successor.messages,
        );
        if let Some(seam) = file.seams.last_mut() {
            seam.first_context = conv.state.lock().await.seam.base.flatten();
        }
        if let Err(err) = self.write_seam_of(&conv.id, file).await {
            crate::logging::private_trace!(conv.private_logs, warn, "could not record the successor"; conversation = %conv.id, error = %err);
        }
    }

    /// The successor's CLI did not take its first turn: the seam is closed again, and the next
    /// try reserves another id.
    pub(super) async fn seam_not_started(&self, conv: &Arc<ConvLive>, successor: Successor) {
        let mut file = self.read_seam_of(&conv.id).await;
        file.phase = Phase::Closed { end: successor.end };
        let _ = self.write_seam_of(&conv.id, file).await;
        conv.close_cli().await;
    }

    /// After a restart, before cut-off turns are marked: the seam goes on from `seam.json`. A
    /// nudged generation ends (with the note it wrote, or without); a successor that was
    /// starting is asked whether it took its hand-off, and only if it didn't do its messages
    /// go again, to the next successor.
    pub(in crate::manager) async fn recover_seam(&self, id: &ConversationId) {
        let Ok(conv) = self.conv(id) else {
            return;
        };
        if !self.seam_on(&conv) {
            return;
        }
        let record = self.session_dir(id).record();
        let mut file = self.read_seam_of(id).await;
        match file.phase.clone() {
            // A move to another CLI asked for in the turn the restart cut off comes now, from its
            // note: the accounts' state isn't read yet, and a limit hit in the note turn moves it
            // without one.
            Phase::Live if file.moving.is_some() => self.seam_nudge(&conv, None, 0, false).await,
            Phase::Live => {}
            // Its successor starts at once, unless the user stopped the turn that ended it.
            Phase::Closed { end } => conv.state.lock().await.seam.carry_on = !end.stopped,
            Phase::Nudged {
                at_ms,
                predecessor,
                tokens,
            } => {
                let written = note_path(&record, file.generation).is_file();
                let moved_to = match file.moving.take() {
                    Some(to) => self.move_setup(&conv, to).await,
                    None => None,
                };
                file.phase = Phase::Closed {
                    end: SeamEnd {
                        hot_tokens: (tokens > 0).then_some(tokens),
                        predecessor,
                        nudged_at_ms: Some(at_ms),
                        closed_at_ms: now_ms(),
                        death: (!written).then(|| RESTARTED.into()),
                        stopped: false,
                        shown: Vec::new(),
                        moved_to,
                    },
                };
                if self.write_seam_of(id, file).await.is_ok() {
                    conv.state.lock().await.seam.carry_on = true;
                }
            }
            Phase::Starting {
                end,
                successor,
                provider,
                messages,
            } => {
                let line = handoff_line(file.generation, id.short());
                if self.runtime.took_input(provider, &successor, &line).await {
                    let n = file.generation;
                    complete(&mut file, &record, n, successor, provider, end, messages);
                    let _ = self.write_seam_of(id, file).await;
                    // Its turn began in the successor, with its messages: the turn the restart
                    // cut off goes on with a word to check (D9 a), never sending them again.
                    self.core.cut_off_request_began(id).await;
                    return;
                }
                file.phase = Phase::Closed { end };
                if self.write_seam_of(id, file).await.is_err() {
                    return;
                }
                // Never delivered: they go again, as the user sent them, and their request
                // isn't marked cut off.
                let Ok(all) = self.core.all_messages(id).await else {
                    return;
                };
                if let Ok(board) = self.core.board(id).await
                    && board
                        .cut_off_request
                        .as_ref()
                        .is_some_and(|cut| messages.contains(&cut.request))
                {
                    self.core.take_cut_off_request(id).await;
                }
                let again: Vec<Message> = all
                    .into_iter()
                    .filter(|message| messages.contains(&message.id))
                    .collect();
                let mut state = conv.state.lock().await;
                // Its messages start the next successor; with none, it carries on by itself.
                state.seam.carry_on = again.is_empty();
                state.pending.extend(again);
            }
        }
    }

    /// The CLI session a session resumes: after a seam its successor, whatever the log says
    /// last (a restart may have come before the successor's start was logged).
    pub(super) async fn seam_resume(
        &self,
        conv: &ConvLive,
        provider: brigadier_providers::ProviderKind,
    ) -> Option<String> {
        if !self.seam_on(conv) {
            return None;
        }
        let file = self.read_seam_of(&conv.id).await;
        if file.phase != Phase::Live {
            return None;
        }
        file.seams
            .last()
            .filter(|seam| seam.provider == provider)
            .map(|seam| seam.successor.clone())
    }

    /// Where the next CLI starts: under the successor's reserved id, if one is.
    pub(super) fn seam_origin(reserved: Option<String>) -> Option<Origin> {
        reserved.map(|native_id| Origin::NewAs { native_id })
    }
}
