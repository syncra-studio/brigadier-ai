//! Orchestrator rebirth (PLAN.md §2, §6 Phase 4): the orchestrator never compacts. When its
//! context passes the prepare threshold, the outgoing CLI's session is forked in the
//! background and the fork writes a handoff note while the orchestrator keeps serving the user.
//! Between two turns (once the note is ready, or sooner when the context passes the swap
//! threshold with too little room left to wait), its CLI is closed and the next turn starts a fresh one with a briefing
//! prepended: the note, this conversation's decision ledger from the Brain, the live board, a
//! Brain digest and the last messages verbatim.
//!
//! The user sees nothing of it: no thread event, notice or run state marks a rebirth, and the
//! briefing tells the new CLI never to mention it. The Inspector's rebirth log
//! ([`OrchestratorEntry::Rebirth`]) has the note, the briefing and its sections.
//!
//! A conversation whose CLI session cannot be resumed (lost files, a failed resume) is started
//! the same way, from a Recovery briefing without a note.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::time::Duration;

use brigadier_brain::{
    BrainCaps, BrainQuery, Node, NodeFilter, NodeKind, NodeState, Origin as NodeOrigin,
};
use brigadier_providers::{
    Access, Origin, ProviderEvent, ProviderKind, Role as MessageAuthor, SessionSpec, Started,
    ToolSet, TurnInput,
};
use tokio::sync::{oneshot, watch};

use super::brains::{LEDGER_MAX, brain_error, cut, one_line};
use super::usage::TokenOwner;
use super::{SessionManager, blocking, prompts};
use crate::knowledge::{BriefingSection, RebirthRecord, RebirthTrigger};
use crate::model::{
    ConversationId, DomainEvent, Environment, Message, MessageRole, ModelChoice, Setup, streams,
};
use crate::now_ms;
use crate::routing::TokenMeter;
use crate::work::{ApprovalSubject, CardState, OrchestratorEntry, PlanState, TaskState};

/// About four bytes per token.
const BYTES_PER_TOKEN: usize = 4;
/// The briefing aims at this many tokens …
const BRIEFING_TARGET: usize = 20_000 * BYTES_PER_TOKEN;
/// … and never passes this unless the decision lines alone need it.
const BRIEFING_MAX: usize = 25_000 * BYTES_PER_TOKEN;

/// A verbatim exchange: a user message and what followed it.
struct Exchange {
    text: String,
    messages: u32,
}
/// Section budgets.
const HANDOFF_BUDGET: usize = 3_000 * BYTES_PER_TOKEN;
const DECISIONS_BUDGET: usize = 4_000 * BYTES_PER_TOKEN;
const STATE_BUDGET: usize = 2_000 * BYTES_PER_TOKEN;
const BRAIN_BUDGET: usize = 5_000 * BYTES_PER_TOKEN;
/// Exchanges (a user message and what followed it) carried verbatim at least …
const MIN_EXCHANGES: usize = 6;
/// … and, only when the briefing overflows, at least this many.
const MIN_EXCHANGES_OVERFLOW: usize = 3;
/// A single reply or Brigadier message is carried up to this size …
const MESSAGE_BYTES: usize = 6_000;
/// … and a user message up to this one (a long paste is cut, with a marker).
const USER_MESSAGE_BYTES: usize = 16_000;
/// Messages of the branch looked at for the verbatim tail.
const RECENT_SCAN: usize = 200;
/// How long the fork may take to write its note.
const HANDOFF_TIME: Duration = Duration::from_secs(180);
/// How long a swap waits for a note whose fork starts only once the old CLI closed.
pub(super) const HANDOFF_AFTER_CLOSE: Duration = Duration::from_secs(190);
/// A turn that cannot run on the old CLI any more waits this long for a note still being
/// written, then starts without it.
pub(super) const HANDOFF_WAIT: Duration = Duration::from_secs(30);
/// Room a context past the swap threshold must keep for one more large turn on its old CLI.
const TURN_ROOM: i64 = 40_000;
/// Orchestrator log events read per page while counting generations.
const LOG_PAGE: u32 = 500;

const HANDOFF_PROMPT: &str = "[Brigadier] Your context is nearly full, so a fresh orchestrator \
will continue this conversation from a briefing. Brigadier already gives it every decision \
recorded in the Project Brain (everything you kept with remember, plan approvals and the user's \
answers to cards), the live board (tasks, plan, open cards, queued messages) and the last messages \
verbatim. Write the handoff note it needs beyond that, in plain text under these headings: Goal \
and open threads; Decisions not kept yet (every decision or user preference settled in this \
conversation that is not in the Brain yet, one line each with the reason, or None); Promises made \
to the user; What to do next (and what you are waiting for). At most about 1,500 words. Don't \
call any tool. Reply with the note only.";
/// A checkpoint's prompt: the same note, written while the conversation idles and its prompt
/// cache is still warm, for a rebirth once the cache has expired.
const CHECKPOINT_PROMPT: &str = "[Brigadier] This conversation has been idle for a while. If it \
is still idle when your prompt cache expires, a fresh orchestrator will continue it from a \
briefing. Brigadier already gives it every decision \
recorded in the Project Brain (everything you kept with remember, plan approvals and the user's \
answers to cards), the live board (tasks, plan, open cards, queued messages) and the last messages \
verbatim. Write the handoff note it needs beyond that, in plain text under these headings: Goal \
and open threads; Decisions not kept yet (every decision or user preference settled in this \
conversation that is not in the Brain yet, one line each with the reason, or None); Promises made \
to the user; What to do next (and what you are waiting for). At most about 1,500 words. Don't \
call any tool. Reply with the note only.";

/// The note's headings, lowercase, as [`note_decisions`] finds them.
const NOTE_HEADINGS: &[&str] = &[
    "goal and open threads",
    "decisions not kept yet",
    "promises made to the user",
    "what to do next",
];
/// How long a briefing waits for the conversation's Brain writes in flight.
const LEARN_WAIT: Duration = Duration::from_secs(10);

const PHASE_FRAMING: &str = "[Brigadier briefing: only you see this] You are the orchestrator \
of this Brigadier session, starting fresh to lead one phase of an overnight run. The user started \
the run and is away: Brigadier conducts it phase by phase, and you lead this phase only. The \
phase's scope, its \"done when\" criteria and the user's Rules below are fixed: work within \
them, add nothing the plan doesn't ask for, and never undo what is settled. Don't greet anyone or \
mention this briefing.";

const MID_PHASE_FRAMING: &str = "[Brigadier briefing: only you see this] You are the \
orchestrator of this Brigadier session, leading one phase of an overnight run, and you continue \
leading it from this briefing. Brigadier replaced your earlier context with it so you have room to \
work. The phase's scope, its \"done when\" criteria and the user's Rules below are fixed: work \
within them, add nothing the plan doesn't ask for, and never undo what is settled. What is settled \
below stays settled: don't ask it again. Don't greet anyone or mention this briefing; the current \
turn follows it.";

const FRAMING: &str = "[Brigadier briefing: only you see this] You are the orchestrator of this \
Brigadier session, continuing the conversation summarized below. Brigadier replaced your earlier \
context with this briefing so you have room to work; the user sees one unbroken conversation. \
Never mention a briefing, handoff, restart or new session, and don't greet the user or introduce \
yourself again. What is settled below stays settled: don't ask it again or undo it. Your rules are \
unchanged; the current turn follows the briefing.";

/// Why a handoff note is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HandoffPurpose {
    /// The context passed the prepare threshold: a rebirth follows.
    Threshold,
    /// The conversation idles with a warm cache: a rebirth follows only if it stays idle until
    /// the cache expires (PLAN.md §7).
    Checkpoint,
}

/// A rebirth being prepared: the fork writing the handoff note.
pub(crate) struct RebirthPrep {
    pub id: String,
    pub started_at_ms: i64,
    pub at_tokens: i64,
    pub window: Option<i64>,
    pub old_native_id: Option<String>,
    /// The context passed the swap threshold.
    pub due: AtomicBool,
    /// When the fork finished (0 until then).
    ready_at_ms: AtomicI64,
    note: watch::Receiver<Option<Option<String>>>,
    /// Set while the fork waits for the old CLI to close: Codex refuses to fork a thread its
    /// live app-server still holds ("already has an active writer"). Dropped unsent, the fork
    /// never starts.
    after_close: std::sync::Mutex<Option<oneshot::Sender<()>>>,
    waits_for_close: bool,
}

impl RebirthPrep {
    /// The fork finished (with a note or without one).
    pub fn ready(&self) -> bool {
        self.note.borrow().is_some()
    }

    /// When the fork finished, once it has.
    pub fn ready_at_ms(&self) -> Option<i64> {
        Some(self.ready_at_ms.load(Ordering::Acquire)).filter(|at| *at > 0)
    }

    /// Whether the fork starts only once the old CLI closed ([`Self::closed`]).
    pub fn waits_for_close(&self) -> bool {
        self.waits_for_close
    }

    /// The old CLI closed: a fork waiting for that starts now.
    pub fn closed(&self) {
        let sender = self
            .after_close
            .lock()
            .ok()
            .and_then(|mut held| held.take());
        if let Some(sender) = sender {
            let _ = sender.send(());
        }
    }

    /// The note, waiting at most `limit` for it.
    pub async fn note(&self, limit: Duration) -> Option<String> {
        let mut note = self.note.clone();
        let _ = tokio::time::timeout(limit, note.wait_for(Option::is_some)).await;
        note.borrow().clone().flatten()
    }
}

/// How the next CLI session of a conversation starts over.
pub(crate) struct BriefingPlan {
    pub trigger: RebirthTrigger,
    pub prep: Option<Arc<RebirthPrep>>,
    pub at_tokens: i64,
    pub window: Option<i64>,
    /// When the swap began: the old CLI was retired from here on.
    pub swap_started_at_ms: i64,
    /// An overnight phase's briefing: the new CLI leads that phase from it alone, without the
    /// conversation's recent messages (earlier phases' talk must not set its scope).
    pub phase: Option<String>,
}

/// A briefing section while it is put together.
struct Part {
    name: &'static str,
    text: String,
    items: u32,
    truncated: bool,
    note: Option<String>,
}

impl Part {
    fn new(name: &'static str, text: String, items: u32) -> Self {
        Self {
            name,
            text,
            items,
            truncated: false,
            note: None,
        }
    }

    fn section(&self) -> BriefingSection {
        BriefingSection {
            name: self.name.into(),
            tokens: (self.text.len() / BYTES_PER_TOKEN) as u64,
            items: self.items,
            truncated: self.truncated,
            note: self.note.clone(),
        }
    }
}

impl SessionManager {
    /// Starts writing the handoff note in a fork of the orchestrator's CLI session, whose
    /// context is `(tokens, window)`. The returned preparation is ready once the fork is done.
    pub(super) fn prepare_rebirth(
        &self,
        id: &ConversationId,
        provider: ProviderKind,
        choice: ModelChoice,
        native_id: Option<String>,
        (at_tokens, window): (i64, Option<i64>),
        purpose: HandoffPurpose,
    ) -> Arc<RebirthPrep> {
        let (done, note) = watch::channel(None);
        // A Codex thread can't be forked while its own app-server holds it: the swap closes
        // the old CLI first, then the fork writes the note.
        let waits_for_close =
            provider == ProviderKind::Codex && purpose == HandoffPurpose::Threshold;
        let (after_close, closed) = if waits_for_close {
            let (sender, receiver) = oneshot::channel();
            (Some(sender), Some(receiver))
        } else {
            (None, None)
        };
        let prep = Arc::new(RebirthPrep {
            id: uuid::Uuid::now_v7().to_string(),
            started_at_ms: now_ms(),
            at_tokens,
            window,
            old_native_id: native_id.clone(),
            due: AtomicBool::new(false),
            ready_at_ms: AtomicI64::new(0),
            note,
            after_close: std::sync::Mutex::new(after_close),
            waits_for_close,
        });
        // Weak: a preparation dropped before its CLI closed drops the sender, which ends this.
        let (manager, id, ready) = (self.arc(), id.clone(), Arc::downgrade(&prep));
        self.spawn(async move {
            if let Some(closed) = closed
                && closed.await.is_err()
            {
                return;
            }
            let written = match native_id {
                Some(native_id) => {
                    manager
                        .write_handoff(&id, provider, choice, native_id, purpose)
                        .await
                }
                None => None,
            };
            tracing::info!(conversation = %id, bytes = written.as_ref().map_or(0, String::len), ?purpose, "handoff note written");
            // Its decisions are kept now, even if a swap that could not wait went ahead
            // without the note: every later briefing carries them. A checkpoint's are kept
            // only if its rebirth happens (the briefing keeps them then).
            if purpose == HandoffPurpose::Threshold
                && let Some(note) = &written
            {
                let generation = manager.rebirths(&id).await + 1;
                manager.keep_handoff_decisions(&id, note, generation).await;
            }
            if let Some(ready) = ready.upgrade() {
                ready.ready_at_ms.store(now_ms(), Ordering::Release);
            }
            let _ = done.send(Some(written));
        });
        prep
    }

    /// The fork's handoff note: the orchestrator's own CLI session, forked, asked once. Its
    /// events reach neither the thread nor the orchestrator log; its files belong to the
    /// orchestrator's cleanup-ledger owner.
    async fn write_handoff(
        &self,
        id: &ConversationId,
        provider: ProviderKind,
        choice: ModelChoice,
        native_id: String,
        purpose: HandoffPurpose,
    ) -> Option<String> {
        let conversation = self.core.conversation(id).ok()?;
        let project = conversation
            .project_id
            .as_ref()
            .and_then(|project| self.core.project(project).ok());
        let preferences = self.memory_lines(super::brain_jobs::MEMORY_BYTES).await;
        let spec = SessionSpec {
            cwd: self.owned_dir("orch", &id.0),
            model: choice.model.clone(),
            effort: choice.effort.clone(),
            fast: choice.fast == Some(true),
            origin: Origin::Fork { native_id },
            access: Access::ReadOnly,
            append_system_prompt: Some(prompts::orchestrator(
                &conversation,
                project.as_ref(),
                &preferences,
                self.overnight
                    .active
                    .get(id)
                    .and_then(|active| active.workspace)
                    .as_ref(),
                self.core.settings().short_replies,
            )),
            mcp_servers: Vec::new(),
            tools: ToolSet::None,
            env: Vec::new(),
            unset_env: Vec::new(),
            low_priority: false,
            record_to: None,
            redactor: None,
            // The orchestrator's live CLI works in the same folder (a fork finds its session by
            // folder): sweeping the folder when the fork ends would end that CLI too, mid-turn.
            // The fork's own process tree still ends with it.
            owned_cwd: false,
            auto_compact: false,
            allowed_models: None,
            auto_review: false,
            omit_ai_coauthors: false,
        };
        let owner = format!("orch:{id}");
        let Started {
            session,
            mut events,
        } = match self.runtime.start_hosted(&owner, provider, spec).await {
            Ok(started) => started,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not fork the orchestrator for its handoff note");
                return None;
            }
        };
        // A forked Codex thread may carry its parent's totals: its first report is a baseline.
        let meter = TokenMeter::new(provider == ProviderKind::Codex);
        let written = async {
            let prompt = match purpose {
                HandoffPurpose::Threshold => HANDOFF_PROMPT,
                HandoffPurpose::Checkpoint => CHECKPOINT_PROMPT,
            };
            session.send(TurnInput::text(prompt)).await.ok()?;
            let mut parts: Vec<String> = Vec::new();
            while let Some(event) = events.recv().await {
                match event {
                    ProviderEvent::Message {
                        role: MessageAuthor::Assistant,
                        text,
                        ..
                    } => parts.push(text),
                    ProviderEvent::RateLimits { quota } => {
                        self.runtime.note_quota_snapshot(quota).await;
                    }
                    ProviderEvent::Usage { total, last } => {
                        self.note_tokens(
                            &meter,
                            provider,
                            choice.model.as_deref(),
                            TokenOwner::Conversation(id),
                            &total,
                            last.as_ref(),
                        )
                        .await
                    }
                    ProviderEvent::ApprovalRequested { request } => {
                        let _ = session
                            .answer(
                                request.id,
                                brigadier_providers::ApprovalDecision::Deny {
                                    message: "Declined: write the note only.".into(),
                                },
                            )
                            .await;
                    }
                    ProviderEvent::TurnCompleted { .. } | ProviderEvent::Exited { .. } => break,
                    _ => {}
                }
            }
            let note = parts.join("\n\n").trim().to_owned();
            (!note.is_empty()).then_some(note)
        };
        let note = tokio::time::timeout(HANDOFF_TIME, written)
            .await
            .ok()
            .flatten();
        session.close().await;
        note
    }

    /// The briefing a fresh orchestrator CLI starts with, and its rebirth record (without the
    /// new CLI's id). `carried` are the user messages of the turn it prefixes.
    pub(super) async fn briefing(
        &self,
        id: &ConversationId,
        provider: ProviderKind,
        model: Option<String>,
        plan: &BriefingPlan,
        carried: &[Message],
    ) -> (String, RebirthRecord) {
        let generation = self.rebirths(id).await + 1;
        if let Some(phase) = &plan.phase {
            return self
                .phase_briefing(id, provider, model, plan, phase, generation)
                .await;
        }
        let handoff = match &plan.prep {
            Some(prep) => prep.note(Duration::ZERO).await,
            None => None,
        };
        // The ledger misses nothing: writes in flight (a report, a card's answer) land first,
        // and what the note settled beyond the Brain is a decision node already (kept again
        // here in case that failed).
        self.learned(id, LEARN_WAIT).await;
        let from_note = match &handoff {
            Some(note) => self.keep_handoff_decisions(id, note, generation).await,
            None => 0,
        };
        let mut handoff_part = Part::new("handoff", String::new(), 0);
        if let Some(note) = &handoff {
            handoff_part.text = format!(
                "[Handoff note from your earlier context]\n{}",
                cut(note, HANDOFF_BUDGET)
            );
            handoff_part.items = 1;
            handoff_part.truncated = note.len() > HANDOFF_BUDGET;
        }
        let (mut decisions, decisions_in_full) = self.decision_part(id).await;
        if from_note > 0
            && let Some(note) = &mut decisions.note
        {
            note.push_str(&format!("; {from_note} kept from the handoff note"));
        }
        let state = self.state_part(id).await;
        let search = Part::new(
            "search",
            "[Older context] search_transcript searches this conversation's whole transcript; query_brain finds decisions (by id or words), reports and findings; read_report gives a task's report in full.".into(),
            0,
        );
        let mut brain = self.brain_part(id, carried).await;
        // A lead reborn mid-phase (its context full, its cache expired, its CLI lost) keeps
        // leading it: the phase's briefing as it is now goes with its handoff, and only this
        // phase's messages are carried.
        let led = self.led_phase(id).await;
        let (framing, phase) = match &led {
            Some((brief, _)) => (MID_PHASE_FRAMING, Part::new("phase", brief.clone(), 1)),
            None => (FRAMING, Part::new("phase", String::new(), 0)),
        };
        let exchanges = self
            .recent_exchanges(id, carried, led.as_ref().map(|(_, since)| *since))
            .await;
        let (target, max) = briefing_budget();

        let fixed = framing.len()
            + phase.text.len()
            + handoff_part.text.len()
            + decisions.text.len()
            + state.text.len()
            + search.text.len();
        let total_exchanges = exchanges.len();
        // Newest first: as many exchanges as the target leaves room for, at least six.
        let mut taken = 0;
        let mut recent_bytes = 0;
        for exchange in &exchanges {
            let room = target.saturating_sub(fixed + brain.text.len());
            if taken >= MIN_EXCHANGES && recent_bytes + exchange.text.len() > room {
                break;
            }
            taken += 1;
            recent_bytes += exchange.text.len();
        }
        let mut overflow_note = None;
        if fixed + brain.text.len() + recent_bytes > max {
            // Over the cap: the Brain digest shrinks first, then the verbatim tail (down to
            // three exchanges). Decision lines are never dropped.
            let room = max.saturating_sub(fixed + recent_bytes);
            if room < brain.text.len() {
                brain.text = cut(&brain.text, room);
                brain.truncated = true;
                brain.note = Some("cut to fit the briefing".into());
            }
            while taken > MIN_EXCHANGES_OVERFLOW.min(total_exchanges)
                && fixed + brain.text.len() + recent_bytes > max
            {
                taken -= 1;
                recent_bytes -= exchanges[taken].text.len();
            }
            if taken < MIN_EXCHANGES.min(total_exchanges) {
                overflow_note = Some(format!(
                    "overflow: {taken} exchanges carried instead of {MIN_EXCHANGES}"
                ));
            }
        }
        let mut chosen: Vec<&Exchange> = exchanges[..taken].iter().collect();
        chosen.reverse();
        let mut recent = Part::new(
            "recent",
            if chosen.is_empty() {
                String::new()
            } else {
                format!(
                    "[The conversation's latest messages, verbatim]\n{}",
                    chosen
                        .iter()
                        .map(|exchange| exchange.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n\n")
                )
            },
            chosen.iter().map(|exchange| exchange.messages).sum(),
        );
        recent.truncated = taken < total_exchanges;
        recent.note = overflow_note.or_else(|| {
            recent
                .truncated
                .then(|| format!("{taken} of {total_exchanges} exchanges"))
        });

        let framing = Part::new("framing", framing.into(), 0);
        let parts = [
            framing,
            phase,
            handoff_part,
            decisions,
            state,
            brain,
            recent,
            search,
        ];
        let text = parts
            .iter()
            .filter(|part| !part.text.is_empty())
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
            + "\n\n[End of briefing]";
        let sections: Vec<BriefingSection> = parts
            .iter()
            .filter(|part| !part.text.is_empty())
            .map(Part::section)
            .collect();
        let briefing_blob = self
            .core
            .store()
            .blobs()
            .put(text.clone().into_bytes())
            .await
            .map(|hash| hash.to_string())
            .unwrap_or_default();
        let handoff_blob = match handoff {
            Some(note) => self
                .core
                .store()
                .blobs()
                .put(note.into_bytes())
                .await
                .ok()
                .map(|hash| hash.to_string()),
            None => None,
        };
        let part = |name: &str| parts.iter().find(|part| part.name == name);
        let record = RebirthRecord {
            id: plan
                .prep
                .as_ref()
                .map_or_else(|| uuid::Uuid::now_v7().to_string(), |prep| prep.id.clone()),
            generation,
            trigger: plan.trigger,
            provider,
            model,
            at_tokens: plan.at_tokens,
            window_tokens: plan.window,
            prepare_started_at_ms: plan
                .prep
                .as_ref()
                .map_or(plan.swap_started_at_ms, |prep| prep.started_at_ms),
            handoff_ready_at_ms: plan.prep.as_ref().and_then(|prep| prep.ready_at_ms()),
            swap_started_at_ms: Some(plan.swap_started_at_ms),
            swapped_at_ms: now_ms(),
            handoff_blob,
            briefing_blob,
            briefing_tokens: (text.len() / BYTES_PER_TOKEN) as u64,
            sections,
            decisions: part("decisions").map_or(0, |part| part.items),
            decisions_in_full,
            recent_messages: part("recent").map_or(0, |part| part.items),
            old_native_id: plan
                .prep
                .as_ref()
                .and_then(|prep| prep.old_native_id.clone()),
            new_native_id: None,
        };
        (text, record)
    }

    /// A phase lead's briefing: the phase itself (scope, criteria, Rules, what earlier phases
    /// verified, the user's own words since Start), the session's settled decisions, the live
    /// board and the Brain. No handoff note and no recent messages.
    async fn phase_briefing(
        &self,
        id: &ConversationId,
        provider: ProviderKind,
        model: Option<String>,
        plan: &BriefingPlan,
        phase: &str,
        generation: u32,
    ) -> (String, RebirthRecord) {
        self.learned(id, LEARN_WAIT).await;
        let (decisions, decisions_in_full) = self.decision_part(id).await;
        let state = self.state_part(id).await;
        let mut brain = self.brain_part(id, &[]).await;
        if brain.text.len() > BRAIN_BUDGET {
            brain.text = cut(&brain.text, BRAIN_BUDGET);
            brain.truncated = true;
        }
        let parts = [
            Part::new("framing", PHASE_FRAMING.into(), 0),
            Part::new("phase", phase.to_owned(), 1),
            decisions,
            state,
            brain,
            Part::new(
                "search",
                "[Older context] search_transcript searches this conversation's whole transcript; query_brain finds decisions, reports and findings; read_report gives a task's report in full.".into(),
                0,
            ),
        ];
        let text = parts
            .iter()
            .filter(|part| !part.text.is_empty())
            .map(|part| part.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
            + "\n\n[End of briefing]";
        let sections: Vec<BriefingSection> = parts
            .iter()
            .filter(|part| !part.text.is_empty())
            .map(Part::section)
            .collect();
        let briefing_blob = self
            .core
            .store()
            .blobs()
            .put(text.clone().into_bytes())
            .await
            .map(|hash| hash.to_string())
            .unwrap_or_default();
        let record = RebirthRecord {
            id: uuid::Uuid::now_v7().to_string(),
            generation,
            trigger: plan.trigger,
            provider,
            model,
            at_tokens: plan.at_tokens,
            window_tokens: plan.window,
            prepare_started_at_ms: plan.swap_started_at_ms,
            handoff_ready_at_ms: None,
            swap_started_at_ms: Some(plan.swap_started_at_ms),
            swapped_at_ms: now_ms(),
            handoff_blob: None,
            briefing_blob,
            briefing_tokens: (text.len() / BYTES_PER_TOKEN) as u64,
            sections,
            decisions: parts[2].items,
            decisions_in_full,
            recent_messages: 0,
            old_native_id: None,
            new_native_id: None,
        };
        (text, record)
    }

    /// Keeps the decisions a handoff note lists as nodes of the conversation (idempotent: a
    /// line's node is keyed by its text). Returns how many it kept.
    async fn keep_handoff_decisions(
        &self,
        id: &ConversationId,
        note: &str,
        generation: u32,
    ) -> u32 {
        match self
            .keep_note_decisions(id, note_decisions(note), generation)
            .await
        {
            Ok(kept) => kept,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not keep the handoff note's decisions");
                0
            }
        }
    }

    /// Every settled decision of this conversation as a line with its node id; full bodies,
    /// newest first, while the budget lasts.
    async fn decision_part(&self, id: &ConversationId) -> (Part, u32) {
        let mut read = self.session_decisions(id).await;
        for pause in [100, 500] {
            if read.is_ok() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(pause)).await;
            read = self.session_decisions(id).await;
        }
        let nodes = match read {
            Ok(nodes) => nodes,
            Err(err) => {
                tracing::warn!(conversation = %id, error = %err, "could not read the decision ledger");
                let mut part = Part::new(
                    "decisions",
                    "[Decisions settled in this conversation] The ledger could not be read just now. Before acting on anything settled earlier, look it up with query_brain or search_transcript.".into(),
                    0,
                );
                part.truncated = true;
                part.note = Some(format!("ledger unreadable: {err}"));
                return (part, 0);
            }
        };
        if nodes.is_empty() {
            return (Part::new("decisions", String::new(), 0), 0);
        }
        let lines: Vec<String> = nodes.iter().map(decision_line).collect();
        let mut used: usize = lines.iter().map(|line| line.len() + 1).sum();
        let mut bodies: Vec<Option<String>> = vec![None; nodes.len()];
        let mut in_full = 0;
        for (index, node) in nodes.iter().enumerate().rev() {
            let body = node.body.trim();
            if body.is_empty() || body == node.title {
                in_full += 1;
                continue;
            }
            let body = format!("    {}", body.replace('\n', "\n    "));
            if used + body.len() + 1 > DECISIONS_BUDGET {
                continue;
            }
            used += body.len() + 1;
            bodies[index] = Some(body);
            in_full += 1;
        }
        let mut text = String::from(
            "[Decisions settled in this conversation, oldest first; all still hold unless replaced]\n",
        );
        for (line, body) in lines.iter().zip(&bodies) {
            text.push_str(line);
            text.push('\n');
            if let Some(body) = body {
                text.push_str(body);
                text.push('\n');
            }
        }
        let mut part = Part::new("decisions", text.trim_end().to_owned(), nodes.len() as u32);
        part.truncated = in_full < nodes.len();
        part.note = Some(format!(
            "{in_full} of {} in full; the rest as lines (query_brain by id){}",
            nodes.len(),
            if nodes.len() >= LEDGER_MAX as usize {
                "; the ledger's newest lines only"
            } else {
                ""
            }
        ));
        (part, in_full as u32)
    }

    /// The live board, exactly (no model wrote it).
    async fn state_part(&self, id: &ConversationId) -> Part {
        let mut text = String::from("[Where things stand now]\n");
        let mut items = 0;
        if let Ok(conversation) = self.core.conversation(id)
            && let Some(Setup::Session { environment, .. }) = &conversation.setup
        {
            // What applies now: an overnight run works on its own branch under Approve for me.
            let place = match self
                .overnight
                .active
                .get(id)
                .and_then(|active| active.workspace)
            {
                Some(run) => format!(
                    "the overnight run's branch `{}` (from `{}`)",
                    run.branch, run.base
                ),
                None => match environment {
                    Environment::LocalCheckout { branch } => {
                        format!("local checkout on `{branch}`")
                    }
                    Environment::NewWorktree { branch, base, .. } => {
                        format!("worktree on `{branch}` (from `{base}`)")
                    }
                },
            };
            text.push_str(&format!(
                "Environment: {place}. Permission: {:?}.{}\n",
                self.permission(id),
                if self.plan_mode(id) {
                    " Plan mode is on."
                } else {
                    ""
                }
            ));
        }
        let Ok(board) = self.core.board(id).await else {
            return Part::new("state", text, 0);
        };
        // What waits on the user comes first: the task list is what a long board cuts.
        for question in board
            .questions
            .values()
            .filter(|question| question.answer.is_none())
        {
            items += 1;
            text.push_str(&format!(
                "Waiting for the user's answer: {}\n",
                one_line(&question.text, 300)
            ));
        }
        for approval in board
            .approvals
            .values()
            .filter(|approval| approval.state == CardState::Pending)
        {
            items += 1;
            let what = match &approval.subject {
                ApprovalSubject::Cli { request } => format!("a worker's {} request", request.tool),
                ApprovalSubject::OutwardCommand { argv, .. } => {
                    format!("running `{}`", argv.join(" "))
                }
                ApprovalSubject::Landing { branch, .. } => format!("landing a task on `{branch}`"),
                ApprovalSubject::FinishSession { branch, base, .. } => {
                    format!("merging `{branch}` into `{base}`")
                }
                ApprovalSubject::Action { action, .. } => action.clone(),
                ApprovalSubject::Outline { title, .. } => format!("starting the plan “{title}”"),
            };
            text.push_str(&format!(
                "Waiting for the user's approval: {}\n",
                one_line(&what, 300)
            ));
        }
        for item in board.sorted_waiting() {
            items += 1;
            text.push_str(&format!(
                "Waiting for the user to do (listed under Waiting on you): {}\n",
                one_line(&item.what, 300)
            ));
        }
        if !board.queue.items.is_empty() {
            text.push_str("The user's queued messages (they reach you later, one by one):\n");
            for item in &board.queue.items {
                items += 1;
                text.push_str(&format!(
                    "- {}\n",
                    one_line(
                        &crate::sessions::display_text(&item.text, &item.attachments),
                        300
                    )
                ));
            }
        }
        let mut plans = board.sorted_plans();
        plans.retain(|plan| matches!(plan.state, PlanState::Proposed | PlanState::Approved { .. }));
        if let Some(plan) = plans.last() {
            items += 1;
            let state = match &plan.state {
                PlanState::Proposed => "waiting for the user",
                _ => "its phases",
            };
            text.push_str(&format!("Plan \"{}\" ({state}):\n", plan.title));
            for (number, step) in plan.steps.iter().enumerate() {
                let task = step
                    .task_id
                    .as_ref()
                    .and_then(|task| board.tasks.get(task))
                    .map(|task| format!(" → task-{}", task.number))
                    .unwrap_or_default();
                text.push_str(&format!(
                    "  {}. {}{task} ({:?})\n",
                    number + 1,
                    step.title,
                    step.stage
                ));
            }
        }
        let mut tasks: Vec<_> = board.tasks.values().collect();
        tasks.sort_by_key(|task| task.number);
        if !tasks.is_empty() {
            text.push_str("Tasks:\n");
        }
        let open = tasks.iter().filter(|task| !task.state.is_final()).count();
        // Every open task, and the latest finished ones.
        let finished_shown = 12usize.saturating_sub(open.min(12));
        let finished: Vec<_> = tasks.iter().filter(|task| task.state.is_final()).collect();
        let skip_finished = finished.len().saturating_sub(finished_shown);
        let mut finished_seen = 0;
        for task in &tasks {
            if task.state.is_final() {
                finished_seen += 1;
                if finished_seen <= skip_finished {
                    continue;
                }
            }
            items += 1;
            let mut line = format!(
                "- task-{} ({:?}, {:?}): {}",
                task.number, task.kind, task.state, task.title
            );
            if let Some(report) = &task.report {
                line.push_str(&format!(" — reported: {}", one_line(&report.summary, 240)));
            }
            if let Some(landed) = &task.landed {
                line.push_str(&format!(" — landed {}", &landed[..landed.len().min(10)]));
            }
            if task.state == TaskState::Blocked
                && let Some(reason) = &task.blocked_reason
            {
                line.push_str(&format!(" — blocked: {}", one_line(reason, 160)));
            }
            text.push_str(&line);
            text.push('\n');
        }
        if skip_finished > 0 {
            text.push_str(&format!(
                "({skip_finished} earlier finished tasks not listed)\n"
            ));
        }
        let mut part = Part::new("state", cut(text.trim_end(), STATE_BUDGET), items);
        part.truncated = text.len() > STATE_BUDGET;
        part
    }

    /// What the Brain knows that frames the work: the project's stack, recipe, modules and
    /// conventions, the user's preferences, and the nodes matching the latest messages.
    async fn brain_part(&self, id: &ConversationId, carried: &[Message]) -> Part {
        let Some(project) = self.core.conversation(id).ok().and_then(|c| c.project_id) else {
            return Part::new("brain", String::new(), 0);
        };
        let Ok(project) = self.project_brain(&project).await else {
            return Part::new("brain", String::new(), 0);
        };
        let recent_text: String = match carried.last() {
            Some(message) => cut(&message.text, 600),
            None => self
                .core
                .list_messages(id.clone(), None, 4)
                .await
                .ok()
                .and_then(|page| {
                    page.messages
                        .iter()
                        .rev()
                        .find(|message| message.role == MessageRole::User)
                        .map(|message| cut(&message.text, 600))
                })
                .unwrap_or_default(),
        };
        let brain = project.brain.clone();
        let digest = blocking(move || {
            let structure = brain
                .nodes(&NodeFilter {
                    kinds: vec![
                        NodeKind::Convention,
                        NodeKind::Module,
                        NodeKind::Service,
                        NodeKind::Contract,
                    ],
                    session_id: None,
                    current_only: true,
                    text: None,
                    limit: Some(2_000),
                })
                .map_err(brain_error)?;
            let related = if recent_text.trim().is_empty() {
                None
            } else {
                brain
                    .query(&BrainQuery {
                        text: recent_text,
                        kinds: Vec::new(),
                        limit: Some(8),
                        max_tokens: Some(1_200),
                        files: false,
                        history: false,
                        caps: Some(BrainCaps::default()),
                        page: None,
                    })
                    .ok()
            };
            Ok((structure, related))
        })
        .await;
        let Ok((structure, related)) = digest else {
            return Part::new("brain", String::new(), 0);
        };
        let mut text = String::from("[What the Project Brain knows]\n");
        let mut items = 0;
        let (key, rest): (Vec<&Node>, Vec<&Node>) = structure
            .iter()
            .partition(|node| node.title.starts_with("Stack") || node.title.starts_with("Recipe"));
        // One stack and one recipe: earlier passes may have left others under other titles.
        // The fresh one, else the newest (the list is newest first).
        let key = ["Stack", "Recipe"].into_iter().filter_map(|prefix| {
            let of_prefix = || {
                key.iter()
                    .filter(move |node| node.title.starts_with(prefix))
            };
            of_prefix()
                .find(|node| is_fresh(node))
                .or_else(|| of_prefix().next())
        });
        for node in key {
            items += 1;
            text.push_str(&format!(
                "{}\n{}\n",
                node.title,
                cut(node.body.trim(), 1_500)
            ));
        }
        for kind in [
            NodeKind::Module,
            NodeKind::Service,
            NodeKind::Contract,
            NodeKind::Convention,
        ] {
            let mut of_kind: Vec<&&Node> = rest.iter().filter(|node| node.kind == kind).collect();
            // What holds now first; a stale one says so.
            of_kind.sort_by_key(|node| !is_fresh(node));
            if of_kind.is_empty() {
                continue;
            }
            text.push_str(&format!("{kind:?}s:\n"));
            for node in of_kind {
                items += 1;
                let described = node.provenance.origin != NodeOrigin::Index;
                let outdated = if is_fresh(node) {
                    ""
                } else {
                    " (may be outdated)"
                };
                if described {
                    text.push_str(&format!(
                        "- {}{outdated}: {}\n",
                        node.title,
                        one_line(&node.body, 200)
                    ));
                } else {
                    text.push_str(&format!("- {}{outdated}\n", node.title));
                }
            }
        }
        let preferences = self.memory_lines(1_500).await;
        if !preferences.is_empty() {
            text.push_str("The user's preferences:\n");
            for preference in &preferences {
                items += 1;
                text.push_str(&format!("- {preference}\n"));
            }
        }
        if let Some(related) = related.filter(|answer| !answer.hits.is_empty()) {
            items += related.hits.len() as u32;
            text.push_str("Related to the latest messages:\n");
            text.push_str(related.text.trim());
            text.push('\n');
        }
        let mut part = Part::new("brain", cut(text.trim_end(), BRAIN_BUDGET), items);
        part.truncated = text.len() > BRAIN_BUDGET;
        part
    }

    /// The branch's latest exchanges (a user message and what followed it), newest first,
    /// each as the text carried verbatim. `carried` are left out: the turn carries them.
    async fn recent_exchanges(
        &self,
        id: &ConversationId,
        carried: &[Message],
        since_ms: Option<i64>,
    ) -> Vec<Exchange> {
        let branch = match self.core.head(id).await {
            Ok(Some(head)) => self.core.branch(id, &head).await.unwrap_or_default(),
            _ => Vec::new(),
        };
        let carried: Vec<&str> = carried.iter().map(|m| m.id.as_str()).collect();
        let start = branch.len().saturating_sub(RECENT_SCAN);
        let mut exchanges: Vec<Exchange> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        let close = |current: &mut Vec<String>, exchanges: &mut Vec<Exchange>| {
            exchanges.push(Exchange {
                text: current.join("\n\n"),
                messages: current.len() as u32,
            });
            current.clear();
        };
        for message in &branch[start..] {
            if carried.contains(&message.id.as_str())
                || since_ms.is_some_and(|since| message.created_at_ms < since)
            {
                continue;
            }
            let who = match message.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "You",
                MessageRole::System => "Brigadier",
            };
            if message.role == MessageRole::User && !current.is_empty() {
                close(&mut current, &mut exchanges);
            }
            let full = self.full_words(message).await;
            let max = if message.role == MessageRole::User {
                USER_MESSAGE_BYTES
            } else {
                MESSAGE_BYTES
            };
            current.push(format!("{who}: {}", carried_text(&full, max)));
        }
        if !current.is_empty() {
            close(&mut current, &mut exchanges);
        }
        exchanges.reverse();
        exchanges
    }

    /// Rebirths of this conversation so far.
    async fn rebirths(&self, id: &ConversationId) -> u32 {
        let mut count = 0;
        let mut after = 0;
        loop {
            let Ok(page) = self
                .core
                .store()
                .read_stream_since(streams::orchestrator(id), after, LOG_PAGE)
                .await
            else {
                return count;
            };
            let Some(last) = page.last() else {
                return count;
            };
            after = last.stream_seq;
            let full = page.len() as u32 == LOG_PAGE;
            count += page
                .iter()
                .filter(|stored| stored.kind == "orchestrator.logged")
                .filter(|stored| {
                    matches!(
                        serde_json::from_str::<DomainEvent>(stored.payload.get()),
                        Ok(DomainEvent::OrchestratorLogged {
                            entry: OrchestratorEntry::Rebirth { .. },
                            ..
                        })
                    )
                })
                .count() as u32;
            if !full {
                return count;
            }
        }
    }
}

/// What the briefing aims at and its cap, in bytes. A debug build takes
/// `BRIGADIER_BRIEFING_TOKENS` as the cap (aiming at four fifths of it), so the overflow path
/// can be tried on a short conversation.
fn briefing_budget() -> (usize, usize) {
    if cfg!(debug_assertions)
        && let Some(tokens) = std::env::var("BRIGADIER_BRIEFING_TOKENS")
            .ok()
            .and_then(|value| value.trim().parse::<usize>().ok())
            .filter(|tokens| *tokens > 0)
    {
        let max = tokens * BYTES_PER_TOKEN;
        return (max * 4 / 5, max);
    }
    (BRIEFING_TARGET, BRIEFING_MAX)
}

/// A message carried verbatim, up to `max` bytes; a longer one says it was cut.
fn carried_text(text: &str, max: usize) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    format!(
        "{}\n[cut here: {} more bytes; search_transcript finds the rest]",
        cut(text, max),
        text.len() - max
    )
}

/// The lines under a handoff note's "Decisions not kept yet" heading, without list markers
/// ("None" is no decision).
fn note_decisions(note: &str) -> Vec<String> {
    let mut decisions = Vec::new();
    let mut inside = false;
    for line in note.lines() {
        let item = list_item(line.trim());
        let bare = item
            .trim_start_matches(['#', '*', '_', ' '])
            .trim_end_matches(['*', '_', ':', ' '])
            .to_lowercase();
        if NOTE_HEADINGS
            .iter()
            .any(|heading| bare.starts_with(heading))
        {
            inside = bare.starts_with(NOTE_HEADINGS[1]);
            // "Decisions not kept yet: Use tabs in the Makefile." has one on the same line.
            let inline = item
                .split_once(':')
                .map(|(_, rest)| rest.trim_start_matches(['*', '_', ' ']).trim())
                .unwrap_or_default();
            if inside && !inline.is_empty() && !says_none(inline) {
                decisions.push(inline.to_owned());
            }
            continue;
        }
        if !inside || item.is_empty() || says_none(item) {
            continue;
        }
        decisions.push(item.to_owned());
    }
    decisions
}

/// Whether a note's line says there is nothing to list ("None.", "None that I know of …",
/// "Nothing new").
fn says_none(item: &str) -> bool {
    let lower = item.to_lowercase();
    ["none", "nothing", "no new ", "no other ", "no further "]
        .iter()
        .any(|word| {
            lower.strip_prefix(word).is_some_and(|rest| {
                word.ends_with(' ') || !rest.starts_with(|c: char| c.is_alphanumeric())
            })
        })
}

/// A line without its list marker ("- ", "* ", "• ", "1. ", "2) ").
fn list_item(line: &str) -> &str {
    for marker in ["- ", "* ", "• ", "+ "] {
        if let Some(rest) = line.strip_prefix(marker) {
            return rest.trim();
        }
    }
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0
        && let Some(rest) = line[digits..]
            .strip_prefix(". ")
            .or_else(|| line[digits..].strip_prefix(") "))
    {
        return rest.trim();
    }
    line
}

/// A decision as one briefing line: its node id, what it says, where it came from.
fn decision_line(node: &Node) -> String {
    let origin = match node.provenance.origin {
        NodeOrigin::User => "the user",
        NodeOrigin::Report => "a worker's report",
        NodeOrigin::Orchestrator => "you",
        NodeOrigin::Index | NodeOrigin::Skeleton | NodeOrigin::Enrichment => "the Brain",
    };
    format!(
        "- [{}] {:?}: {} ({origin}, {})",
        node.id,
        node.kind,
        one_line(&node.title, 300),
        prompts::date_of(node.created_at_ms)
    )
}

/// Whether a rebirth should be prepared or is due after a turn, for a context of `used`
/// tokens: `(prepare, due)`.
pub(super) fn rebirth_needed(
    provider: ProviderKind,
    used: i64,
    window: Option<i64>,
) -> (bool, bool) {
    let thresholds = crate::knowledge::rebirth_thresholds(provider, window);
    (
        used >= thresholds.prepare_tokens,
        used >= thresholds.swap_tokens,
    )
}

/// Whether a context past the swap threshold must be reborn before the next turn even though
/// its note is still being written. A Claude CLI (auto-compaction off) with room for one more
/// large turn keeps working until the note is ready; Codex compacts on its own near its limit,
/// so it is reborn at the threshold.
pub(super) fn swap_now(
    provider: Option<ProviderKind>,
    context: Option<(i64, Option<i64>)>,
) -> bool {
    match (provider, context) {
        (Some(ProviderKind::Claude), Some((used, Some(window)))) => {
            window - used < TURN_ROOM.max(window / 5)
        }
        _ => true,
    }
}

impl RebirthPrep {
    /// Marks the rebirth due: the next turn is reborn once the note is ready, or at once when
    /// [`swap_now`] says the context cannot wait.
    pub fn set_due(&self) {
        self.due.store(true, Ordering::Release);
    }

    pub fn is_due(&self) -> bool {
        self.due.load(Ordering::Acquire)
    }
}

/// The node holds as recorded (not stale; superseded ones aren't listed).
fn is_fresh(node: &Node) -> bool {
    matches!(node.state, NodeState::Fresh)
}
