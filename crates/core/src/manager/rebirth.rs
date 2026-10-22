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
use crate::tools::Role;
use crate::work::{ApprovalSubject, CardState, OrchestratorEntry, PlanState, TaskState};

/// About four bytes per token.
const BYTES_PER_TOKEN: usize = 4;
/// The briefing aims at this many tokens …
const BRIEFING_TARGET: usize = 20_000 * BYTES_PER_TOKEN;
/// … and passes this only when what it must carry needs more: the framing, the handoff note
/// and the state (each within its own budget), every decision whole, and the newest
/// [`MIN_EXCHANGES_OVERFLOW`] exchanges, however long ([`fit`]).
const BRIEFING_MAX: usize = 25_000 * BYTES_PER_TOKEN;

/// A verbatim exchange: a user message and what followed it.
struct Exchange {
    text: String,
    messages: u32,
    /// Its first message's number (`#N`, as search_transcript shows it).
    first: i64,
    /// The user's message it starts with, as carried when the rest of it isn't (issue #216).
    words: Option<String>,
}
/// Section budgets.
const HANDOFF_BUDGET: usize = 3_000 * BYTES_PER_TOKEN;
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
/// Past the verbatim exchanges, the user's own messages of older ones are carried without the
/// replies, up to this much of the target: what the user said can't be looked up as easily as
/// what the thread read, ran or wrote, which the replies hold and search_transcript finds
/// (issue #216). They come before more verbatim exchanges beyond [`MIN_EXCHANGES`] …
const WORDS_BUDGET: usize = 3_000 * BYTES_PER_TOKEN;
/// … each up to this size.
const WORDS_MESSAGE_BYTES: usize = 1_500;
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

const HANDOFF_PROMPT: &str = "[Brigadier] Your context is nearly full, so a fresh session \
will continue this conversation from a briefing. Brigadier already gives it every decision \
recorded in the Project Brain (everything you kept with remember, plan approvals and the user's \
answers to cards), the live board (tasks, plan, open cards, queued messages) and the last messages \
verbatim. Write the handoff note it needs beyond that, in plain text under these headings: Goal \
and open threads; Decisions not kept yet (every decision or user preference settled in this \
conversation that is not in the Brain yet, one line each with the reason and, for what the user \
settled, their words in double quotes, or None); Ruled out (each approach tried or rejected, and \
why); Failed checks (each with its command and exact error line, and whether it was fixed); Exact \
identifiers in use (commit hashes, paths with line numbers, ids, numbers with their units, copied \
exactly); Promises made to the user; What to do next (and what you are waiting for). Keep word for \
word what can't be looked up again: the user's decisions, corrections, rules and the exact numbers, \
names and paths they gave. For what can be read or run again, give where it is instead of a copy: \
path:line, commit hash, output id, message #N, decision id. Give every claim where it came from, and \
never call something done without the check that shows it. At most about 1,500 words. Don't call \
any tool. Reply with the note only.";
/// A checkpoint's prompt: the same note, written while the conversation idles and its prompt
/// cache is still warm, for a rebirth once the cache has expired.
const CHECKPOINT_PROMPT: &str = "[Brigadier] This conversation has been idle for a while. If it \
is still idle when your prompt cache expires, a fresh session will continue it from a \
briefing. Brigadier already gives it every decision \
recorded in the Project Brain (everything you kept with remember, plan approvals and the user's \
answers to cards), the live board (tasks, plan, open cards, queued messages) and the last messages \
verbatim. Write the handoff note it needs beyond that, in plain text under these headings: Goal \
and open threads; Decisions not kept yet (every decision or user preference settled in this \
conversation that is not in the Brain yet, one line each with the reason and, for what the user \
settled, their words in double quotes, or None); Ruled out (each approach tried or rejected, and \
why); Failed checks (each with its command and exact error line, and whether it was fixed); Exact \
identifiers in use (commit hashes, paths with line numbers, ids, numbers with their units, copied \
exactly); Promises made to the user; What to do next (and what you are waiting for). Keep word for \
word what can't be looked up again: the user's decisions, corrections, rules and the exact numbers, \
names and paths they gave. For what can be read or run again, give where it is instead of a copy: \
path:line, commit hash, output id, message #N, decision id. Give every claim where it came from, and \
never call something done without the check that shows it. At most about 1,500 words. Don't call \
any tool. Reply with the note only.";

/// The note's headings, lowercase, as [`note_decisions`] finds them.
const NOTE_HEADINGS: &[&str] = &[
    "goal and open threads",
    "decisions not kept yet",
    "ruled out",
    "failed checks",
    "exact identifiers in use",
    "promises made to the user",
    "what to do next",
];
/// How long a briefing waits for the conversation's Brain writes in flight.
const LEARN_WAIT: Duration = Duration::from_secs(10);

const FRAMING: &str = "[Brigadier briefing: only you see this] You lead this \
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

impl RebirthPrep {
    /// A checkpoint whose fork wrote `note` at `at_ms`, for the session `old_native_id` (one
    /// taken back from the log after a restart, or a test's).
    pub(crate) fn written(note: &str, old_native_id: &str, at_ms: i64) -> Arc<Self> {
        let (_, note) = watch::channel(Some(Some(note.to_owned())));
        Arc::new(Self {
            id: uuid::Uuid::now_v7().to_string(),
            started_at_ms: at_ms,
            at_tokens: 0,
            window: None,
            old_native_id: Some(old_native_id.to_owned()),
            due: AtomicBool::new(false),
            ready_at_ms: AtomicI64::new(at_ms),
            note,
            after_close: std::sync::Mutex::new(None),
            waits_for_close: false,
        })
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
    /// A turn stopped mid-request: where it stood (BUILD-PLAN D2.3b), carried first.
    pub stopped: Option<String>,
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

/// What a thread's CLI was started with, kept for its note fork (BUILD-PLAN B5).
pub(crate) struct ForkBase {
    pub spec: SessionSpec,
}

/// What a note fork is told when it calls any tool: the deny hook's reason and Brigadier's
/// tool server's answer to its grant.
pub(crate) const NOTE_FORK_REFUSAL: &str = "This is the read-only note fork of the thread: no \
tool runs here. Write the note from what you already know, in text.";

/// A Claude thread's note fork: the thread's own start (tools, MCP servers, model, effort,
/// instructions and settings), so its first call reads the thread's prefix from the cache.
/// Only its origin, its life (ephemeral, unrecorded) and its folders' ownership change. It
/// stays read-only, every tool call is refused before it runs by a client-side hook that leaves
/// the tool list as it is, and Brigadier's server answers it under the fork's own `grant`.
pub(crate) fn note_fork_spec(thread: &SessionSpec, native_id: String, grant: &str) -> SessionSpec {
    let mcp_servers = thread
        .mcp_servers
        .iter()
        .cloned()
        .map(|mut server| {
            for (name, value) in &mut server.env {
                if name == super::conversation::MCP_GRANT_ENV {
                    *value = grant.to_owned();
                }
            }
            server
        })
        .collect();
    // It runs no command, so it has no use for the thread's `brigadier` grant.
    let env = thread
        .env
        .iter()
        .filter(|(name, _)| {
            name != brigadier_cli::protocol::SESSION_ENV
                && name != brigadier_cli::protocol::GRANT_ENV
        })
        .cloned()
        .collect();
    SessionSpec {
        origin: Origin::Fork { native_id },
        ephemeral: true,
        access: Access::ReadOnly,
        mcp_servers,
        env,
        record_to: None,
        redactor: super::secrets::redactor(vec![grant.to_owned()]),
        // The thread's live CLI works in the same folder (a fork finds its session by folder):
        // sweeping the folder when the fork ends would end that CLI too, mid-turn. The fork's
        // own process tree still ends with it.
        owned_cwd: false,
        sweep_dir: None,
        // Its hook stores the output of commands, and no command runs here.
        output_hook: None,
        stop_hook: None,
        deny_tools: Some(NOTE_FORK_REFUSAL.into()),
        ..thread.clone()
    }
}

/// A note fork with no tools or MCP servers: a Codex thread's (its own shell and patch tools
/// stay, read-only), or one whose thread start isn't known.
fn plain_fork_spec(
    cwd: std::path::PathBuf,
    choice: &ModelChoice,
    native_id: String,
    prompt: String,
) -> SessionSpec {
    SessionSpec {
        cwd,
        model: choice.model.clone(),
        effort: choice.effort.clone(),
        fast: choice.fast == Some(true),
        origin: Origin::Fork { native_id },
        ephemeral: true,
        access: Access::ReadOnly,
        append_system_prompt: Some(prompt),
        mcp_servers: Vec::new(),
        tools: ToolSet::None,
        skill_plugins: Vec::new(),
        switched_off: Default::default(),
        add_dirs: Vec::new(),
        project: None,
        env: Vec::new(),
        unset_env: Vec::new(),
        low_priority: false,
        record_to: None,
        redactor: None,
        owned_cwd: false,
        sweep_dir: None,
        auto_compact: false,
        allowed_models: None,
        omit_ai_coauthors: false,
        output_hook: None,
        stop_hook: None,
        deny_tools: None,
        lean_worker: false,
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
        let started_at_ms = now_ms();
        let prep = Arc::new(RebirthPrep {
            id: uuid::Uuid::now_v7().to_string(),
            started_at_ms,
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
            let written = match &native_id {
                Some(native_id) => {
                    manager
                        .write_handoff(&id, provider, choice, native_id.clone(), purpose)
                        .await
                }
                None => None,
            };
            // A checkpoint outlives a daemon restart (BUILD-PLAN D1.4).
            if purpose == HandoffPurpose::Checkpoint
                && let (Some(native_id), Some(note)) = (&native_id, &written)
            {
                manager
                    .keep_checkpoint(&id, native_id, started_at_ms, note)
                    .await;
            }
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
        let _turn = self.background_turn();
        let owner = format!("orch:{id}");
        let live = self.convs_lock().get(id).cloned();
        let base = match live {
            Some(conv) => conv.fork_base().await,
            None => None,
        };
        let mut grant = None;
        let spec = match base.as_deref() {
            // A Claude fork starts as the thread did, so it reads the thread's whole start from
            // the cache; Brigadier's tools answer it under a grant of its own, which refuses them.
            Some(base) if provider == ProviderKind::Claude => {
                let issued = self.grants.issue(
                    &owner,
                    Role::NoteFork {
                        conversation_id: id.clone(),
                    },
                );
                let spec = note_fork_spec(&base.spec, native_id, &issued);
                grant = Some(issued);
                spec
            }
            // Codex is unchanged until its hooks can be shown to hold (BUILD-PLAN B5), and a
            // thread whose CLI hasn't started since the daemon did has nothing to copy.
            _ => {
                if provider == ProviderKind::Claude {
                    tracing::info!(conversation = %id, "no thread start to copy: the note fork starts on its own");
                }
                let conversation = self.core.conversation(id).ok()?;
                let preferences = self.memory_lines(super::brain_jobs::MEMORY_BYTES).await;
                // The thread's own instructions.
                let prompt = self
                    .thread_prompt(
                        &conversation,
                        &preferences,
                        self.recorded_workspace(id).as_ref(),
                        provider,
                    )
                    .await;
                plain_fork_spec(self.thread_cwd(id), &choice, native_id, prompt)
            }
        };
        let account = self.runtime.account_for(&choice);
        let revoke = |grants: &crate::tools::Grants| {
            if let Some(grant) = &grant {
                grants.revoke(std::slice::from_ref(grant));
            }
        };
        let Started {
            session,
            mut events,
        } = match self.runtime.start_hosted(&owner, &account, spec).await {
            Ok(started) => started,
            Err(err) => {
                revoke(&self.grants);
                tracing::warn!(conversation = %id, error = %err, "could not fork the orchestrator for its handoff note");
                return None;
            }
        };
        // A forked Codex thread may carry its parent's totals: its first report is a baseline.
        // Its first call should read the thread's last prompt back from the cache (BUILD-PLAN
        // B0); the thread's rows say it when its CLI is gone.
        let live = self.convs_lock().get(id).cloned();
        let thread_prompt = match live {
            Some(conv) => conv
                .live_cli()
                .await
                .and_then(|cli| cli.meter.last_prompt()),
            None => None,
        };
        let meter = TokenMeter::new(provider == ProviderKind::Codex)
            .on_account(account.account.clone())
            .starting_from(crate::routing::WriteCause::NoteFork, thread_prompt);
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
                        self.runtime
                            .note_quota_snapshot(&account, quota.clone())
                            .await;
                    }
                    ProviderEvent::Usage { total, last, calls } => {
                        self.note_tokens(
                            &meter,
                            provider,
                            choice.model.as_deref(),
                            TokenOwner::Handoff(id),
                            &total,
                            last.as_ref(),
                            &calls,
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
        revoke(&self.grants);
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
        // A compact index over every decision, the replaced ones too, on top of the active
        // ones' whole bodies (BUILD-PLAN D1.2, Q41.1): nothing is paged out.
        let index = self.decision_index_part(id).await;
        if from_note > 0
            && let Some(note) = &mut decisions.note
        {
            note.push_str(&format!("; {from_note} kept from the handoff note"));
        }
        let state = self.state_part(id).await;
        // Every constraint in force, verbatim, whatever else must be cut (BUILD-PLAN D1.1).
        let active = self.active_constraints(id).await;
        let mut constraints = Part::new(
            "constraints",
            super::constraints::briefing_text(&active),
            active.len() as u32,
        );
        constraints.truncated = active
            .iter()
            .any(|constraint| constraint.words.len() > super::constraints::WORDS_MAX);
        let search = Part::new(
            "search",
            "[Older context] Not all of this conversation is carried here: before you answer that something from it is unknown or ask the user again, look it up. search_transcript searches its whole transcript, your earlier commands, reads and tool calls too; query_brain finds decisions (by id or words), reports and findings; read_report gives a task's report in full.".into(),
            0,
        );
        let mut brain = self.brain_part(id, carried).await;
        let exchanges = self.recent_exchanges(id, carried).await;
        let (target, max) = briefing_budget();
        // A turn stopped mid-request: where it stood, carried whole (BUILD-PLAN D2.3b).
        let stopped = Part::new("stopped", plan.stopped.clone().unwrap_or_default(), 0);

        let fixed = FRAMING.len()
            + stopped.text.len()
            + constraints.text.len()
            + index.text.len()
            + handoff_part.text.len()
            + decisions.text.len()
            + state.text.len()
            + search.text.len();
        let total_exchanges = exchanges.len();
        let sizes: Vec<(usize, usize)> = exchanges
            .iter()
            .map(|exchange| {
                (
                    exchange.text.len(),
                    exchange.words.as_ref().map_or(0, |words| words.len() + 1),
                )
            })
            .collect();
        let fit = fit(fixed, brain.text.len(), &sizes, (target, max));
        let taken = fit.taken;
        if fit.brain < brain.text.len() {
            brain.text = cut(&brain.text, fit.brain);
            brain.truncated = true;
            brain.note = Some("cut to fit the briefing".into());
        }
        let mut overflow = Vec::new();
        if taken < MIN_EXCHANGES.min(total_exchanges) {
            overflow.push(format!(
                "{taken} exchanges carried instead of {MIN_EXCHANGES}"
            ));
        }
        if fit.over > 0 {
            overflow.push(format!(
                "~{} tokens past the cap of {}, for what it must carry",
                fit.over.div_ceil(BYTES_PER_TOKEN),
                max / BYTES_PER_TOKEN
            ));
        }
        let overflow_note =
            (!overflow.is_empty()).then(|| format!("overflow: {}", overflow.join("; ")));
        let mut chosen: Vec<&Exchange> = exchanges[..taken].iter().collect();
        chosen.reverse();
        // Older exchanges past the verbatim ones: the user's own words, oldest first.
        let said: Vec<&str> = exchanges[taken..taken + fit.words]
            .iter()
            .rev()
            .filter_map(|exchange| exchange.words.as_deref())
            .collect();
        let mut words = Part::new(
            "words",
            if said.is_empty() {
                String::new()
            } else {
                format!(
                    "[The user's earlier messages, verbatim; the replies between them aren't carried, search_transcript finds them by number]\n{}",
                    said.join("\n")
                )
            },
            said.len() as u32,
        );
        words.truncated = !said.is_empty() && taken + fit.words < total_exchanges;
        let mut recent = Part::new(
            "recent",
            if chosen.is_empty() {
                String::new()
            } else {
                format!(
                    "[The conversation's latest messages, verbatim]\n{}{}",
                    chosen
                        .iter()
                        .map(|exchange| exchange.text.as_str())
                        .collect::<Vec<_>>()
                        .join("\n\n"),
                    // Every cut names what it leaves out (BUILD-PLAN D1.1).
                    chosen
                        .first()
                        .filter(|oldest| oldest.first > 1)
                        .map(|oldest| format!(
                            "\n[Messages #1–#{} aren't carried here; search_transcript finds them{}]",
                            oldest.first - 1,
                            if said.is_empty() {
                                ""
                            } else {
                                ". The user's own words among them are above"
                            }
                        ))
                        .unwrap_or_default()
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

        let framing = Part::new("framing", FRAMING.into(), 0);
        let parts = [
            framing,
            stopped,
            constraints,
            handoff_part,
            index,
            decisions,
            state,
            brain,
            words,
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
            Ok(nodes) => crate::heap(|| self.sources_checked(id, nodes)).await,
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
        let mut part = Part::new("decisions", decision_text(&nodes), nodes.len() as u32);
        let ledger_full = nodes.len() >= LEDGER_MAX as usize;
        part.truncated = ledger_full;
        part.note = Some(format!(
            "all {} in full{}",
            nodes.len(),
            if ledger_full {
                "; the ledger's newest only"
            } else {
                ""
            }
        ));
        (part, nodes.len() as u32)
    }

    /// A worker's hand-off's decisions (BUILD-PLAN D2.4): the decision index, then every
    /// settled decision in full, as a rebirth briefing carries them; empty without any.
    pub(crate) async fn decision_index_text(&self, id: &ConversationId) -> String {
        let index = self.decision_index_part(id).await.text;
        let (settled, _) = self.decision_part(id).await;
        [index, settled.text]
            .into_iter()
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n")
    }

    /// The decision index: one line per decision, convention or contract of this conversation,
    /// replaced ones included, oldest first: its id, scope, status and what it replaced. Empty
    /// without any, or when the ledger can't be read (the decisions part says so).
    async fn decision_index_part(&self, id: &ConversationId) -> Part {
        let Some(project) = self.project_of(id) else {
            return Part::new("decision-index", String::new(), 0);
        };
        let brain = match self.project_brain(&project).await {
            Ok(project) => project.brain.clone(),
            Err(_) => return Part::new("decision-index", String::new(), 0),
        };
        let session = id.0.clone();
        let nodes = blocking(move || {
            brain
                .nodes(&NodeFilter {
                    kinds: vec![NodeKind::Decision, NodeKind::Convention, NodeKind::Contract],
                    session_id: Some(session),
                    current_only: false,
                    text: None,
                    limit: Some(LEDGER_MAX),
                })
                .map_err(brain_error)
        })
        .await;
        match nodes {
            Ok(mut nodes) if !nodes.is_empty() => {
                nodes.sort_by(|a, b| (a.created_at_ms, &a.id).cmp(&(b.created_at_ms, &b.id)));
                let text = decision_index(&nodes);
                let mut part = Part::new("decision-index", text, nodes.len() as u32);
                let replaced = nodes
                    .iter()
                    .filter(|node| matches!(node.state, NodeState::Superseded { .. }))
                    .count();
                part.note = Some(format!(
                    "{} active, {replaced} replaced",
                    nodes.len() - replaced
                ));
                part
            }
            _ => Part::new("decision-index", String::new(), 0),
        }
    }

    /// The live board, exactly (no model wrote it).
    async fn state_part(&self, id: &ConversationId) -> Part {
        let mut text = String::from("[Where things stand now]\n");
        let mut items = 0;
        if let Ok(conversation) = self.core.conversation(id)
            && let Some(Setup::Session { environment, .. }) = &conversation.setup
        {
            // What applies now: an overnight run works on its own branch under Accept edits.
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
                ApprovalSubject::Cli { request } if approval.task_id.is_none() => {
                    format!("your own {} request", request.tool)
                }
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
                ApprovalSubject::PeerMessage { title, .. } => {
                    format!("sending a message to the session “{title}”")
                }
                ApprovalSubject::Browser { question, .. } => question.clone(),
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
        // Each claim's sources are re-read for this briefing (BUILD-PLAN E1).
        let reader = self.sources_for(id, None);
        let digest = blocking(move || {
            let mut structure = brain
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
            brigadier_brain::check_sources(
                &mut structure.iter_mut().collect::<Vec<_>>(),
                &|source| reader.check(source),
            );
            let related = if recent_text.trim().is_empty() {
                None
            } else {
                brain
                    .query_with(
                        &BrainQuery {
                            text: recent_text,
                            kinds: Vec::new(),
                            limit: Some(8),
                            max_tokens: Some(1_200),
                            files: false,
                            history: false,
                            caps: Some(BrainCaps::default()),
                            page: None,
                        },
                        &|source| reader.check(source),
                    )
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
                        "- {} [{}]{outdated}: {}\n",
                        node.title,
                        brigadier_brain::source_words(node.kind, &node.provenance),
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
    async fn recent_exchanges(&self, id: &ConversationId, carried: &[Message]) -> Vec<Exchange> {
        let branch = match self.core.head(id).await {
            Ok(Some(head)) => self.core.branch(id, &head).await.unwrap_or_default(),
            _ => Vec::new(),
        };
        let carried: Vec<&str> = carried.iter().map(|m| m.id.as_str()).collect();
        let start = branch.len().saturating_sub(RECENT_SCAN);
        let mut exchanges: Vec<Exchange> = Vec::new();
        let mut current: Vec<String> = Vec::new();
        let mut first = 0;
        let mut words = None;
        let close = |current: &mut Vec<String>,
                     exchanges: &mut Vec<Exchange>,
                     first: i64,
                     words: &mut Option<String>| {
            exchanges.push(Exchange {
                text: current.join("\n\n"),
                messages: current.len() as u32,
                first,
                words: words.take(),
            });
            current.clear();
        };
        for message in &branch[start..] {
            if carried.contains(&message.id.as_str()) {
                continue;
            }
            let who = match message.role {
                MessageRole::User => "User".to_owned(),
                MessageRole::Assistant => "You".to_owned(),
                MessageRole::System => "Brigadier".to_owned(),
                MessageRole::Peer => self.core.peer_speaker(message),
            };
            // A peer's message starts an exchange as the user's does, but stays a peer's.
            let input = matches!(message.role, MessageRole::User | MessageRole::Peer);
            if input && !current.is_empty() {
                close(&mut current, &mut exchanges, first, &mut words);
            }
            let full = self.full_words(message).await;
            if current.is_empty() {
                first = message.seq;
                if message.role == MessageRole::User {
                    words = Some(format!(
                        "User (#{}): {}",
                        message.seq,
                        carried_text(&full, WORDS_MESSAGE_BYTES, message.seq)
                    ));
                }
            }
            let max = if input {
                USER_MESSAGE_BYTES
            } else {
                MESSAGE_BYTES
            };
            current.push(format!("{who}: {}", carried_text(&full, max, message.seq)));
        }
        if !current.is_empty() {
            close(&mut current, &mut exchanges, first, &mut words);
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
fn carried_text(text: &str, max: usize, seq: i64) -> String {
    if text.len() <= max {
        return text.to_owned();
    }
    let mut end = max;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}…\n[cut here: message #{seq}, bytes {end}–{} not shown; search_transcript finds the rest]",
        &text[..end],
        text.len()
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

/// The briefing's decisions part: every decision as its line, with its body in full (they
/// are never cut, SESSION-FIXES-PLAN Q5). What nobody checked (a worker's or your own claim)
/// is listed apart, each with its label, never as settled (BUILD-PLAN A11).
fn decision_text(nodes: &[Node]) -> String {
    let (claims, settled): (Vec<&Node>, Vec<&Node>) = nodes
        .iter()
        .partition(|node| brigadier_brain::unverified(node.kind, &node.provenance));
    let mut text = String::new();
    for (heading, nodes) in [
        (
            "[Decisions settled in this conversation, oldest first; all still hold unless replaced]",
            settled,
        ),
        (
            "[Claims kept in this conversation, oldest first: nobody checked them, so they are not settled; check one before relying on it, and remember with replaces corrects a wrong one]",
            claims,
        ),
    ] {
        if nodes.is_empty() {
            continue;
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(heading);
        text.push('\n');
        for node in nodes {
            text.push_str(&decision_line(node));
            text.push('\n');
            let body = node.body.trim();
            if !body.is_empty() && body != node.title {
                text.push_str(&format!("    {}\n", body.replace('\n', "\n    ")));
            }
        }
    }
    text.trim_end().to_owned()
}

/// The decision index's text for `nodes` (oldest first): one line each, `- <id> · <scope> ·
/// <status>[ · replaces <ids>] · <title>`.
fn decision_index(nodes: &[Node]) -> String {
    let mut text = String::from(
        "[Decision index: every decision, convention and contract of this conversation, oldest first, replaced ones too. Id · scope · status · what it replaced · what it is. The active ones follow in full; query_brain <id> gives a replaced one]\n",
    );
    for node in nodes {
        let scope = match node.kind {
            NodeKind::Decision => "session",
            _ => "project",
        };
        let status = match &node.state {
            // A replaced claim nobody checked still says so: what it said never reads as fact
            // (V2's false learned line).
            NodeState::Superseded { by, .. }
                if brigadier_brain::unverified(node.kind, &node.provenance) =>
            {
                format!(
                    "{}, replaced by {by}",
                    brigadier_brain::source_label(node.kind, &node.provenance)
                )
            }
            NodeState::Superseded { by, .. } => format!("replaced by {by}"),
            NodeState::Stale { .. } => "may be out of date".to_owned(),
            NodeState::Fresh if brigadier_brain::unverified(node.kind, &node.provenance) => {
                "claim, unverified".to_owned()
            }
            NodeState::Fresh => "holds".to_owned(),
        };
        let replaced: Vec<&str> = nodes
            .iter()
            .filter(|old| matches!(&old.state, NodeState::Superseded { by, .. } if *by == node.id))
            .map(|old| old.id.as_str())
            .collect();
        let replaces = if replaced.is_empty() {
            String::new()
        } else {
            format!(" · replaces {}", replaced.join(", "))
        };
        text.push_str(&format!(
            "- {} · {scope} · {status}{replaces} · {}\n",
            node.id,
            one_line(&node.title, 80)
        ));
    }
    text.trim_end().to_owned()
}

/// How much of a briefing's two flexible parts it carries.
#[derive(Debug, PartialEq, Eq)]
struct Fit {
    /// The newest exchanges carried verbatim.
    taken: usize,
    /// The exchanges past those whose user's words alone are carried.
    words: usize,
    /// Bytes of the Brain digest kept.
    brain: usize,
    /// Bytes the briefing passes the cap by: what it must carry needs them.
    over: usize,
}

/// Fits a briefing of `fixed` bytes that are never cut (the framing, handoff note, decisions,
/// state), a Brain digest of `brain` bytes and the `exchanges` (newest first: each one's size
/// verbatim and the size of its user's words alone) into `(target, max)`. As many exchanges
/// verbatim as the target leaves room for, at least six, after keeping room for the user's
/// words of the older ones ([`WORDS_BUDGET`]), which then fill what the target leaves. Over the
/// cap the exchanges shrink first, down to three, and then the Brain digest
/// (SESSION-FIXES-PLAN Q5). What it must carry, the fixed parts and those three exchanges
/// (an exchange holds several messages and may be long), can still pass the cap: by `over`.
fn fit(
    fixed: usize,
    brain: usize,
    exchanges: &[(usize, usize)],
    (target, max): (usize, usize),
) -> Fit {
    // The room the user's words of the exchanges from `from` on would take.
    let words_from = |from: usize| -> usize {
        exchanges
            .iter()
            .skip(from)
            .map(|(_, words)| words)
            .sum::<usize>()
            .min(WORDS_BUDGET)
    };
    let mut taken = 0;
    let mut recent = 0;
    for (size, _) in exchanges {
        let room = target.saturating_sub(fixed + brain + words_from(taken + 1));
        if taken >= MIN_EXCHANGES && recent + size > room {
            break;
        }
        taken += 1;
        recent += size;
    }
    while taken > MIN_EXCHANGES_OVERFLOW.min(exchanges.len()) && fixed + brain + recent > max {
        taken -= 1;
        recent -= exchanges[taken].0;
    }
    let brain = brain.min(max.saturating_sub(fixed + recent));
    let mut room = target
        .saturating_sub(fixed + brain + recent)
        .min(WORDS_BUDGET);
    let mut words = 0;
    for (_, size) in &exchanges[taken..] {
        if *size > room {
            break;
        }
        room -= size;
        words += 1;
    }
    // Not past the oldest one with words of the user's.
    while words > 0 && exchanges[taken + words - 1].1 == 0 {
        words -= 1;
    }
    Fit {
        taken,
        words,
        brain,
        over: (fixed + brain + recent).saturating_sub(max),
    }
}

/// A decision as one briefing line: its node id, what it says, where it came from and
/// whether anyone checked it.
fn decision_line(node: &Node) -> String {
    format!(
        "- [{}] {:?} [{}]: {} ({})",
        node.id,
        node.kind,
        brigadier_brain::source_words(node.kind, &node.provenance),
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

#[cfg(test)]
mod tests {
    use brigadier_brain::Provenance;

    use super::*;

    fn decision(n: usize, body: &str) -> Node {
        Node {
            id: format!("d{n}"),
            kind: NodeKind::Decision,
            key: None,
            title: format!("Decision {n}"),
            body: body.into(),
            state: NodeState::Fresh,
            provenance: Provenance {
                origin: NodeOrigin::User,
                session_id: None,
                task_id: None,
                job_id: None,
                worker: None,
                commit: None,
                recorded_at_ms: 0,
                claim: None,
            },
            files: Vec::new(),
            created_at_ms: 0,
            updated_at_ms: 0,
            expires_at_ms: None,
            embedded: false,
        }
    }

    /// A worker's claim the thread replaced keeps its label in the index, so its words never
    /// read as fact; a replaced user decision is just replaced.
    #[test]
    fn a_replaced_claim_keeps_its_label_in_the_decision_index() {
        let mut claim = decision(1, "The write is atomic.");
        claim.kind = NodeKind::Convention;
        claim.title = "writeDataFile writes atomically".into();
        claim.provenance.origin = NodeOrigin::Report;
        claim.provenance.claim = Some(brigadier_brain::Claim::default());
        claim.state = NodeState::Superseded {
            by: "d2".into(),
            reason: Some("the thread read the code".into()),
            since_ms: None,
        };
        let mut old = decision(3, "Keep it.");
        old.state = NodeState::Superseded {
            by: "d4".into(),
            reason: Some("replaced by the user".into()),
            since_ms: None,
        };
        let text = decision_index(&[
            claim,
            decision(2, "Not atomic."),
            old,
            decision(4, "Move it."),
        ]);
        assert!(
            text.contains(
                "- d1 · project · worker claim, unverified, replaced by d2 · writeDataFile writes atomically\n"
            ),
            "{text}"
        );
        assert!(
            text.contains("- d3 · session · replaced by d4 · Decision 3\n"),
            "{text}"
        );
    }

    #[test]
    fn every_decision_body_is_kept_whole() {
        // Far more than the old 4k-token budget for bodies.
        let nodes: Vec<Node> = (0..40)
            .map(|n| decision(n, &format!("Why {n}: {}", "because ".repeat(200))))
            .collect();
        let text = decision_text(&nodes);
        for node in &nodes {
            assert!(text.contains(&format!("[{}]", node.id)));
            assert!(text.contains(node.body.trim()), "{} cut", node.id);
        }
        assert!(text.len() > 4_000 * BYTES_PER_TOKEN, "past the old budget");
    }

    #[test]
    fn over_the_cap_exchanges_shrink_before_the_brain_digest() {
        let budget = (800, 1_000);
        // Fits the target: all six, the digest whole.
        assert_eq!(
            fit(300, 100, &[(50, 0); 6], budget),
            Fit {
                taken: 6,
                words: 0,
                brain: 100,
                over: 0
            }
        );
        // Over the cap: exchanges go first, down to three, and the digest stays whole.
        assert_eq!(
            fit(500, 200, &[(100, 0); 8], budget),
            Fit {
                taken: 3,
                words: 0,
                brain: 200,
                over: 0
            }
        );
        // Still over with three: only then is the digest cut.
        assert_eq!(
            fit(500, 400, &[(100, 0); 8], budget),
            Fit {
                taken: 3,
                words: 0,
                brain: 200,
                over: 0
            }
        );
        // Decisions alone past the cap: never cut, and the newest three exchanges still go.
        assert_eq!(
            fit(1_200, 400, &[(100, 0); 8], budget),
            Fit {
                taken: 3,
                words: 0,
                brain: 0,
                over: 500
            }
        );
        // The decisions fit, but the newest three exchanges are long: they all go, the digest
        // doesn't, and the briefing passes the cap by what they need.
        assert_eq!(
            fit(400, 100, &[(300, 0), (300, 0), (300, 0), (50, 0)], budget),
            Fit {
                taken: 3,
                words: 0,
                brain: 0,
                over: 300
            }
        );
        // Fewer exchanges than three: all of them.
        assert_eq!(
            fit(900, 100, &[(200, 0)], budget),
            Fit {
                taken: 1,
                words: 0,
                brain: 0,
                over: 100
            }
        );
    }

    #[test]
    fn older_user_words_come_before_more_verbatim_exchanges() {
        // Twenty exchanges of 4,000 bytes, each with 500 bytes of the user's words: past the six
        // verbatim ones, room is kept for the older ones' words, so all twenty are carried in
        // part (eleven whole, nine as the user's words) instead of twelve whole and eight lost.
        let exchanges = [(4_000, 500); 20];
        assert_eq!(
            fit(20_000, 10_000, &exchanges, (80_000, 100_000)),
            Fit {
                taken: 11,
                words: 9,
                brain: 10_000,
                over: 0
            }
        );
        // The words have a budget of their own: past it, older exchanges are left out.
        let long = [(4_000, 2_000); 20];
        let fitted = fit(20_000, 10_000, &long, (80_000, 100_000));
        assert_eq!(fitted.words, WORDS_BUDGET / 2_000);
        assert!(fitted.taken + fitted.words < 20);
        // Nothing older: no room is kept, the verbatim exchanges fill the target as before.
        assert_eq!(
            fit(300, 100, &[(50, 10); 6], (800, 1_000)),
            Fit {
                taken: 6,
                words: 0,
                brain: 100,
                over: 0
            }
        );
        // Over the cap: the exchanges shrink as before, and no words.
        assert_eq!(
            fit(500, 200, &[(100, 10); 8], (800, 1_000)),
            Fit {
                taken: 3,
                words: 0,
                brain: 200,
                over: 0
            }
        );
    }
}
