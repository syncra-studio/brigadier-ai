//! Session reducer: the only code that changes `cu_session` records.
//!
//! `SessionBook::apply(ctx, mint, command)` validates a command against the
//! invariants below and either rejects it with no state change (`Err`) or
//! commits the change and returns the events to append. It does no I/O: the
//! daemon persists the returned events and the changed record in one
//! transaction, then runs the engine for an `Execute` outcome.
//!
//! - C1 user stop wins: after `Stop` (or `StopAgent`) commits, every later
//!   call on that session is refused and never reaches the engine; a `Start`
//!   with the same label from the same actor is refused until the user
//!   allows the agent again. `Pause` refuses calls until `Resume`.
//! - C2 idempotent calls: a call retried with the same idempotency key gets
//!   the first result (`Replay`) or `CallInFlight`, never a second `Execute`.
//! - C3 identity is stamped: records and events take actor, class and
//!   attribution from the `Caller` the daemon authenticated, never from args.
//! - C4 gap-free log: every session's event `seq` runs 0, 1, 2, ... .
//! - C5 no clear text: call args are redacted when the call begins, so the
//!   stored copy never holds typed text.
//! - C6 ended is terminal: an ended session never becomes live again; a new
//!   `Start` with the same label creates a new session id.
//!
//! `MarkIdle`, `ExpireIdle` and `HostRestart` are host-internal: the
//! dispatcher never maps an agent request to them.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use serde_json::Value;

use super::model::{
    CallKind, CallOutcome, Caller, ClickPoint, EndReason, Event, EventKind, Profile, RecordingMode,
    Scope, SessionId, SessionRecord, SessionStatus, Target,
};
use super::redact::{redact_args_with, RedactPolicy};

/// How long a finished call's result is replayed for its idempotency key.
pub const CALL_CACHE_TTL_MS: u64 = 10 * 60 * 1000;
/// Most finished results kept per session for idempotent replay.
pub const CALL_CACHE_CAP: usize = 4096;

/// Request context the daemon fills from the authenticated connection.
#[derive(Debug, Clone, Copy)]
pub struct Ctx<'a> {
    pub now_ms: u64,
    /// Transaction id of the request; every event it causes carries it.
    pub tx: &'a str,
    pub caller: &'a Caller,
}

/// A new session's unique id part and cursor color, minted by the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minted {
    pub unique: String,
    pub color: String,
}

/// Images and geometry the engine produced for one call.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CallFrames {
    pub before_frame: Option<String>,
    pub after_frame: Option<String>,
    pub click_point: Option<ClickPoint>,
    pub ax_digest: Option<String>,
}

/// Handle for a call the engine is running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CallTicket(pub u64);

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Start {
        label: String,
        scope: Option<Scope>,
    },
    End {
        id: SessionId,
    },
    Stop {
        id: SessionId,
    },
    Pause {
        id: SessionId,
    },
    Resume {
        id: SessionId,
    },
    /// Stop every live session of `actor` and refuse its new sessions.
    StopAgent {
        actor: String,
    },
    /// Undo `StopAgent` and label stops for `actor`.
    AllowAgent {
        actor: String,
    },
    SetRecording {
        id: SessionId,
        mode: RecordingMode,
    },
    BeginCall {
        id: SessionId,
        kind: CallKind,
        tool: String,
        args: Value,
        target: Option<Target>,
        idempotency_key: Option<String>,
    },
    FinishCall {
        id: SessionId,
        ticket: CallTicket,
        outcome: CallOutcome,
        duration_ms: u64,
        frames: CallFrames,
    },
    MarkIdle {
        id: SessionId,
    },
    ExpireIdle {
        id: SessionId,
    },
    HostRestart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Reject {
    #[error("unknown_session")]
    UnknownSession,
    #[error("session_ended")]
    SessionEnded,
    #[error("session_stopped")]
    SessionStopped,
    #[error("session_paused")]
    SessionPaused,
    #[error("agent_stopped")]
    AgentStopped,
    #[error("user_required")]
    UserRequired,
    #[error("not_owner")]
    NotOwner,
    #[error("call_in_flight")]
    CallInFlight,
    #[error("unknown_ticket")]
    UnknownTicket,
}

impl Reject {
    pub fn code(self) -> String {
        self.to_string()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Started {
        id: SessionId,
        created: bool,
    },
    /// Run the engine, then send `FinishCall` with this ticket.
    Execute {
        ticket: CallTicket,
    },
    /// Idempotent retry: return this result without running the engine.
    Replay {
        outcome: CallOutcome,
    },
    /// The call was refused; the refusal is in the log when `events` has it.
    Refused {
        reason: Reject,
    },
    Done,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Applied {
    pub outcome: Outcome,
    pub events: Vec<Event>,
}

impl Applied {
    fn done(events: Vec<Event>) -> Self {
        Applied {
            outcome: Outcome::Done,
            events,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct InFlight {
    kind: CallKind,
    tool: String,
    args_redacted: Value,
    target: Option<Target>,
    idempotency_key: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum CacheState {
    InFlight,
    Done(CallOutcome),
}

#[derive(Debug, Clone, PartialEq, Default)]
struct CallCache {
    entries: HashMap<String, (CacheState, u64)>,
    order: VecDeque<String>,
}

impl CallCache {
    fn lookup(&mut self, key: &str, now_ms: u64) -> Option<&CacheState> {
        let expired = matches!(
            self.entries.get(key),
            Some((CacheState::Done(_), at)) if at.saturating_add(CALL_CACHE_TTL_MS) <= now_ms
        );
        if expired {
            self.entries.remove(key);
            self.order.retain(|k| k != key);
        }
        self.entries.get(key).map(|(state, _)| state)
    }

    fn begin(&mut self, key: String, now_ms: u64) {
        self.evict(now_ms);
        self.entries
            .insert(key.clone(), (CacheState::InFlight, now_ms));
        self.order.push_back(key);
    }

    fn finish(&mut self, key: &str, outcome: CallOutcome) {
        if let Some(entry) = self.entries.get_mut(key) {
            entry.0 = CacheState::Done(outcome);
        }
    }

    fn evict(&mut self, now_ms: u64) {
        while let Some(front) = self.order.front() {
            let Some((state, at)) = self.entries.get(front) else {
                self.order.pop_front();
                continue;
            };
            let over_cap = self.entries.len() >= CALL_CACHE_CAP;
            let expired = at.saturating_add(CALL_CACHE_TTL_MS) <= now_ms;
            if matches!(state, CacheState::Done(_)) && (over_cap || expired) {
                let key = self.order.pop_front().expect("front exists");
                self.entries.remove(&key);
            } else {
                break;
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
struct SessionState {
    record: SessionRecord,
    next_seq: u64,
    next_ticket: u64,
    in_flight: BTreeMap<CallTicket, InFlight>,
    cache: CallCache,
}

/// All sessions of one daemon profile.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionBook {
    profile: Profile,
    sessions: BTreeMap<SessionId, SessionState>,
    live_labels: HashMap<(String, String), SessionId>,
    stopped_labels: HashSet<(String, String)>,
    stopped_actors: HashSet<String>,
    redact_policy: RedactPolicy,
}

impl SessionBook {
    pub fn new(profile: Profile) -> Self {
        SessionBook {
            profile,
            sessions: BTreeMap::new(),
            live_labels: HashMap::new(),
            stopped_labels: HashSet::new(),
            stopped_actors: HashSet::new(),
            redact_policy: RedactPolicy::default(),
        }
    }

    pub fn redact_policy(&self) -> RedactPolicy {
        self.redact_policy
    }

    /// Set by a user-origin `activity_policy_set` only (the daemon checks).
    pub fn set_redact_policy(&mut self, policy: RedactPolicy) {
        self.redact_policy = policy;
    }

    /// Ids of live sessions.
    pub fn live_ids(&self) -> std::collections::BTreeSet<SessionId> {
        self.sessions
            .iter()
            .filter(|(_, s)| s.record.status.is_live())
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// The (record, next_seq) pair the store persists for `id`.
    pub fn persisted(&self, id: &SessionId) -> Option<(&SessionRecord, u64)> {
        self.sessions
            .get(id)
            .map(|state| (&state.record, state.next_seq))
    }

    /// Rebuilds a book from stored records and each session's next seq. Call
    /// `HostRestart` afterwards to close sessions the previous process left
    /// live.
    pub fn restore(profile: Profile, stored: Vec<(SessionRecord, u64)>) -> Self {
        let mut book = SessionBook::new(profile);
        for (record, next_seq) in stored {
            if record.status.is_live() {
                book.live_labels.insert(
                    (record.agent.actor.clone(), record.label.clone()),
                    record.id.clone(),
                );
            }
            book.sessions.insert(
                record.id.clone(),
                SessionState {
                    record,
                    next_seq,
                    next_ticket: 0,
                    in_flight: BTreeMap::new(),
                    cache: CallCache::default(),
                },
            );
        }
        book
    }

    pub fn profile(&self) -> Profile {
        self.profile
    }

    pub fn record(&self, id: &SessionId) -> Option<&SessionRecord> {
        self.sessions.get(id).map(|state| &state.record)
    }

    pub fn records(&self) -> impl Iterator<Item = &SessionRecord> {
        self.sessions.values().map(|state| &state.record)
    }

    pub fn next_seq(&self, id: &SessionId) -> Option<u64> {
        self.sessions.get(id).map(|state| state.next_seq)
    }

    pub fn live_session(&self, actor: &str, label: &str) -> Option<&SessionId> {
        self.live_labels.get(&(actor.to_owned(), label.to_owned()))
    }

    /// Drops a session the retention planner deleted.
    pub fn forget(&mut self, id: &SessionId) {
        if let Some(state) = self.sessions.remove(id) {
            let key = (state.record.agent.actor.clone(), state.record.label.clone());
            if self.live_labels.get(&key) == Some(id) {
                self.live_labels.remove(&key);
            }
        }
    }

    pub fn apply(
        &mut self,
        ctx: &Ctx<'_>,
        mint: &mut dyn FnMut(&str) -> Minted,
        command: Command,
    ) -> Result<Applied, Reject> {
        match command {
            Command::Start { label, scope } => self.start(ctx, mint, label, scope),
            Command::End { id } => self.end(ctx, &id),
            Command::Stop { id } => self.stop(ctx, &id),
            Command::Pause { id } => self.pause(ctx, &id),
            Command::Resume { id } => self.resume(ctx, &id),
            Command::StopAgent { actor } => self.stop_agent(ctx, &actor),
            Command::AllowAgent { actor } => {
                if !ctx.caller.is_user() {
                    return Err(Reject::UserRequired);
                }
                self.stopped_actors.remove(&actor);
                self.stopped_labels.retain(|(a, _)| a != &actor);
                Ok(Applied::done(Vec::new()))
            }
            Command::SetRecording { id, mode } => {
                let state = self.sessions.get_mut(&id).ok_or(Reject::UnknownSession)?;
                if !ctx.caller.is_user() && ctx.caller.identity.actor != state.record.agent.actor {
                    return Err(Reject::NotOwner);
                }
                state.record.recording = mode;
                Ok(Applied::done(Vec::new()))
            }
            Command::BeginCall {
                id,
                kind,
                tool,
                args,
                target,
                idempotency_key,
            } => self.begin_call(ctx, &id, kind, tool, args, target, idempotency_key),
            Command::FinishCall {
                id,
                ticket,
                outcome,
                duration_ms,
                frames,
            } => self.finish_call(ctx, &id, ticket, outcome, duration_ms, frames),
            Command::MarkIdle { id } => {
                let state = self.sessions.get_mut(&id).ok_or(Reject::UnknownSession)?;
                if state.record.status != SessionStatus::Active || !state.in_flight.is_empty() {
                    return Ok(Applied::done(Vec::new()));
                }
                state.record.status = SessionStatus::Idle;
                let event = push_event(state, ctx, EventKind::SessionIdle);
                Ok(Applied::done(vec![event]))
            }
            Command::ExpireIdle { id } => {
                let state = self.sessions.get(&id).ok_or(Reject::UnknownSession)?;
                if !matches!(
                    state.record.status,
                    SessionStatus::Active | SessionStatus::Idle
                ) {
                    return Ok(Applied::done(Vec::new()));
                }
                if !state.in_flight.is_empty() {
                    return Ok(Applied {
                        outcome: Outcome::Refused {
                            reason: Reject::CallInFlight,
                        },
                        events: Vec::new(),
                    });
                }
                let event = self.close(ctx, &id, EndReason::IdleTtl, EventKind::SessionEnd);
                Ok(Applied::done(vec![event]))
            }
            Command::HostRestart => {
                let live: Vec<SessionId> = self
                    .sessions
                    .iter()
                    .filter(|(_, state)| state.record.status.is_live())
                    .map(|(id, _)| id.clone())
                    .collect();
                let mut events = Vec::new();
                for id in live {
                    events.push(self.close(
                        ctx,
                        &id,
                        EndReason::HostRestart,
                        EventKind::SessionEnd,
                    ));
                }
                for state in self.sessions.values_mut() {
                    state.in_flight.clear();
                    state.cache = CallCache::default();
                }
                Ok(Applied::done(events))
            }
        }
    }

    fn start(
        &mut self,
        ctx: &Ctx<'_>,
        mint: &mut dyn FnMut(&str) -> Minted,
        label: String,
        scope: Option<Scope>,
    ) -> Result<Applied, Reject> {
        let actor = ctx.caller.identity.actor.clone();
        if self.stopped_actors.contains(&actor) {
            return Err(Reject::AgentStopped);
        }
        let key = (actor, label);
        if self.stopped_labels.contains(&key) {
            return Err(Reject::SessionStopped);
        }
        if let Some(id) = self.live_labels.get(&key).cloned() {
            if let Some(state) = self.sessions.get_mut(&id) {
                if state.record.status.is_live() {
                    if state.record.status == SessionStatus::Idle {
                        state.record.status = SessionStatus::Active;
                    }
                    return Ok(Applied {
                        outcome: Outcome::Started { id, created: false },
                        events: Vec::new(),
                    });
                }
            }
        }
        let minted = mint(&key.1);
        let id = SessionId::new(self.profile, &minted.unique);
        let record = SessionRecord {
            id: id.clone(),
            profile: self.profile,
            label: key.1.clone(),
            agent: ctx.caller.identity.clone(),
            origin: ctx.caller.origin,
            color: minted.color,
            started_at_ms: ctx.now_ms,
            ended_at_ms: None,
            last_action_at_ms: ctx.now_ms,
            status: SessionStatus::Active,
            delivery: Default::default(),
            targets: Vec::new(),
            scope: scope.unwrap_or_default(),
            counters: Default::default(),
            recording: RecordingMode::Events,
        };
        let mut state = SessionState {
            record,
            next_seq: 0,
            next_ticket: 0,
            in_flight: BTreeMap::new(),
            cache: CallCache::default(),
        };
        let event = push_event(&mut state, ctx, EventKind::SessionStart);
        self.sessions.insert(id.clone(), state);
        self.live_labels.insert(key, id.clone());
        Ok(Applied {
            outcome: Outcome::Started { id, created: true },
            events: vec![event],
        })
    }

    fn end(&mut self, ctx: &Ctx<'_>, id: &SessionId) -> Result<Applied, Reject> {
        let state = self.sessions.get(id).ok_or(Reject::UnknownSession)?;
        let owner = ctx.caller.identity.actor == state.record.agent.actor;
        if !owner && !ctx.caller.is_user() {
            return Err(Reject::NotOwner);
        }
        if !state.record.status.is_live() {
            return Ok(Applied::done(Vec::new()));
        }
        let reason = if owner {
            EndReason::AgentEnd
        } else {
            EndReason::UserStop
        };
        let event = self.close(ctx, id, reason, EventKind::SessionEnd);
        Ok(Applied::done(vec![event]))
    }

    fn stop(&mut self, ctx: &Ctx<'_>, id: &SessionId) -> Result<Applied, Reject> {
        if !ctx.caller.is_user() {
            return Err(Reject::UserRequired);
        }
        let state = self.sessions.get(id).ok_or(Reject::UnknownSession)?;
        if !state.record.status.is_live() {
            return Ok(Applied::done(Vec::new()));
        }
        let key = (state.record.agent.actor.clone(), state.record.label.clone());
        let event = self.close(ctx, id, EndReason::UserStop, EventKind::SessionStop);
        self.stopped_labels.insert(key);
        Ok(Applied::done(vec![event]))
    }

    fn pause(&mut self, ctx: &Ctx<'_>, id: &SessionId) -> Result<Applied, Reject> {
        if !ctx.caller.is_user() {
            return Err(Reject::UserRequired);
        }
        let state = self.sessions.get_mut(id).ok_or(Reject::UnknownSession)?;
        match state.record.status {
            SessionStatus::Ended { .. } => Err(Reject::SessionEnded),
            SessionStatus::Paused => Ok(Applied::done(Vec::new())),
            SessionStatus::Active | SessionStatus::Idle => {
                state.record.status = SessionStatus::Paused;
                let event = push_event(state, ctx, EventKind::SessionPause);
                Ok(Applied::done(vec![event]))
            }
        }
    }

    fn resume(&mut self, ctx: &Ctx<'_>, id: &SessionId) -> Result<Applied, Reject> {
        if !ctx.caller.is_user() {
            return Err(Reject::UserRequired);
        }
        let state = self.sessions.get_mut(id).ok_or(Reject::UnknownSession)?;
        match state.record.status {
            SessionStatus::Ended { .. } => Err(Reject::SessionEnded),
            SessionStatus::Paused => {
                state.record.status = SessionStatus::Active;
                let event = push_event(state, ctx, EventKind::SessionResume);
                Ok(Applied::done(vec![event]))
            }
            SessionStatus::Active | SessionStatus::Idle => Ok(Applied::done(Vec::new())),
        }
    }

    fn stop_agent(&mut self, ctx: &Ctx<'_>, actor: &str) -> Result<Applied, Reject> {
        if !ctx.caller.is_user() {
            return Err(Reject::UserRequired);
        }
        self.stopped_actors.insert(actor.to_owned());
        let live: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|(_, s)| s.record.agent.actor == actor && s.record.status.is_live())
            .map(|(id, _)| id.clone())
            .collect();
        let mut events = Vec::new();
        for id in live {
            events.push(self.close(ctx, &id, EndReason::UserStop, EventKind::SessionStop));
        }
        Ok(Applied::done(events))
    }

    #[allow(clippy::too_many_arguments)]
    fn begin_call(
        &mut self,
        ctx: &Ctx<'_>,
        id: &SessionId,
        kind: CallKind,
        tool: String,
        args: Value,
        target: Option<Target>,
        idempotency_key: Option<String>,
    ) -> Result<Applied, Reject> {
        let agent_stopped = self.stopped_actors.contains(&ctx.caller.identity.actor);
        let state = self.sessions.get_mut(id).ok_or(Reject::UnknownSession)?;
        if ctx.caller.identity.actor != state.record.agent.actor {
            return Err(Reject::NotOwner);
        }
        let args_redacted = redact_args_with(&tool, &args, self.redact_policy);
        let refusal = if agent_stopped {
            Some(Reject::AgentStopped)
        } else {
            match state.record.status {
                SessionStatus::Ended {
                    reason: EndReason::UserStop,
                } => Some(Reject::SessionStopped),
                SessionStatus::Ended { .. } => Some(Reject::SessionEnded),
                SessionStatus::Paused => Some(Reject::SessionPaused),
                SessionStatus::Active | SessionStatus::Idle => None,
            }
        };
        if let Some(reason) = refusal {
            let mut event = push_event(state, ctx, EventKind::PolicyReject);
            event.tool = Some(tool);
            event.args_redacted = args_redacted;
            event.target = target;
            event.reject = Some(reason.code());
            return Ok(Applied {
                outcome: Outcome::Refused { reason },
                events: vec![event],
            });
        }
        if let Some(key) = idempotency_key.as_deref() {
            match state.cache.lookup(key, ctx.now_ms) {
                Some(CacheState::Done(outcome)) => {
                    return Ok(Applied {
                        outcome: Outcome::Replay {
                            outcome: outcome.clone(),
                        },
                        events: Vec::new(),
                    });
                }
                Some(CacheState::InFlight) => {
                    return Ok(Applied {
                        outcome: Outcome::Refused {
                            reason: Reject::CallInFlight,
                        },
                        events: Vec::new(),
                    });
                }
                None => {}
            }
        }
        let ticket = CallTicket(state.next_ticket);
        state.next_ticket += 1;
        if let Some(key) = idempotency_key.clone() {
            state.cache.begin(key, ctx.now_ms);
        }
        if let Some(target) = target.as_ref() {
            if !state.record.targets.iter().any(|t| t.same_surface(target)) {
                state.record.targets.push(target.clone());
            }
        }
        state.record.status = SessionStatus::Active;
        state.record.last_action_at_ms = ctx.now_ms;
        state.in_flight.insert(
            ticket,
            InFlight {
                kind,
                tool,
                args_redacted,
                target,
                idempotency_key,
            },
        );
        Ok(Applied {
            outcome: Outcome::Execute { ticket },
            events: Vec::new(),
        })
    }

    fn finish_call(
        &mut self,
        ctx: &Ctx<'_>,
        id: &SessionId,
        ticket: CallTicket,
        outcome: CallOutcome,
        duration_ms: u64,
        frames: CallFrames,
    ) -> Result<Applied, Reject> {
        let state = self.sessions.get_mut(id).ok_or(Reject::UnknownSession)?;
        if ctx.caller.identity.actor != state.record.agent.actor {
            return Err(Reject::NotOwner);
        }
        let call = state
            .in_flight
            .remove(&ticket)
            .ok_or(Reject::UnknownTicket)?;
        if let Some(key) = call.idempotency_key.as_deref() {
            state.cache.finish(key, outcome.clone());
        }
        match call.kind {
            CallKind::Observe => state.record.counters.observes += 1,
            CallKind::Act => state.record.counters.acts += 1,
        }
        if !outcome.ok {
            state.record.counters.errors += 1;
        }
        state.record.counters.frames +=
            u64::from(frames.before_frame.is_some()) + u64::from(frames.after_frame.is_some());
        state.record.last_action_at_ms = ctx.now_ms;
        let kind = match call.kind {
            CallKind::Observe => EventKind::Observe,
            CallKind::Act => EventKind::Act,
        };
        let mut event = push_event(state, ctx, kind);
        event.tool = Some(call.tool);
        event.args_redacted = call.args_redacted;
        event.target = call.target;
        event.result = Some(CallOutcome {
            reply: Value::Null,
            ..outcome
        });
        event.duration_ms = Some(duration_ms);
        event.before_frame = frames.before_frame;
        event.after_frame = frames.after_frame;
        event.click_point = frames.click_point;
        event.ax_digest = frames.ax_digest;
        Ok(Applied {
            outcome: Outcome::Done,
            events: vec![event],
        })
    }

    /// Ends a live session. The caller checked it is live.
    fn close(
        &mut self,
        ctx: &Ctx<'_>,
        id: &SessionId,
        reason: EndReason,
        kind: EventKind,
    ) -> Event {
        let state = self.sessions.get_mut(id).expect("session exists");
        state.record.status = SessionStatus::Ended { reason };
        state.record.ended_at_ms = Some(ctx.now_ms);
        let key = (state.record.agent.actor.clone(), state.record.label.clone());
        let event = push_event(state, ctx, kind);
        if self.live_labels.get(&key) == Some(id) {
            self.live_labels.remove(&key);
        }
        event
    }
}

fn push_event(state: &mut SessionState, ctx: &Ctx<'_>, kind: EventKind) -> Event {
    let seq = state.next_seq;
    state.next_seq += 1;
    Event {
        session: state.record.id.clone(),
        seq,
        ts_ms: ctx.now_ms,
        tx: ctx.tx.to_owned(),
        kind,
        tool: None,
        actor: ctx.caller.identity.actor.clone(),
        origin: ctx.caller.origin,
        target: None,
        args_redacted: Value::Null,
        result: None,
        reject: None,
        duration_ms: None,
        click_point: None,
        before_frame: None,
        after_frame: None,
        ax_digest: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::model::{AgentClass, AgentIdentity, Attribution, Effect, Origin};
    use serde_json::json;

    fn identity(actor: &str, class: AgentClass) -> AgentIdentity {
        AgentIdentity {
            attribution: Attribution::ProcessTree,
            kind: "claude".to_owned(),
            class,
            actor: actor.to_owned(),
            on_behalf_of: Some("user_1".to_owned()),
            agent_id: None,
            terminal_id: Some(format!("term_{actor}")),
            acp_session: None,
            workspace_id: Some("ws_1".to_owned()),
            harness_session_id: None,
            proxy_pid: Some(100),
            proxy_pid_start: Some(1),
        }
    }

    fn agent(actor: &str) -> Caller {
        Caller {
            identity: identity(actor, AgentClass::Agent),
            origin: Origin::Mcp,
        }
    }

    fn user() -> Caller {
        Caller {
            identity: identity("user", AgentClass::User),
            origin: Origin::User,
        }
    }

    struct Harness {
        book: SessionBook,
        now: u64,
        minted: u64,
        log: BTreeMap<SessionId, Vec<Event>>,
    }

    impl Harness {
        fn new() -> Self {
            Harness {
                book: SessionBook::new(Profile::Native),
                now: 1_000,
                minted: 0,
                log: BTreeMap::new(),
            }
        }

        fn run(&mut self, caller: &Caller, command: Command) -> Result<Outcome, Reject> {
            self.now += 7;
            let before = self.book.clone();
            let ctx = Ctx {
                now_ms: self.now,
                tx: "tx",
                caller,
            };
            let minted = &mut self.minted;
            let mut mint = |_: &str| {
                *minted += 1;
                Minted {
                    unique: format!("u{minted}"),
                    color: "#ff8800".to_owned(),
                }
            };
            let result = self.book.apply(&ctx, &mut mint, command);
            match result {
                Ok(applied) => {
                    for event in applied.events {
                        self.log
                            .entry(event.session.clone())
                            .or_default()
                            .push(event);
                    }
                    Ok(applied.outcome)
                }
                Err(reject) => {
                    assert_eq!(self.book, before, "a rejected command changed state");
                    Err(reject)
                }
            }
        }

        fn start(&mut self, caller: &Caller, label: &str) -> SessionId {
            match self.run(
                caller,
                Command::Start {
                    label: label.to_owned(),
                    scope: None,
                },
            ) {
                Ok(Outcome::Started { id, .. }) => id,
                other => panic!("start failed: {other:?}"),
            }
        }

        fn call(
            &mut self,
            caller: &Caller,
            id: &SessionId,
            key: Option<&str>,
        ) -> Result<Outcome, Reject> {
            self.run(
                caller,
                Command::BeginCall {
                    id: id.clone(),
                    kind: CallKind::Act,
                    tool: "click".to_owned(),
                    args: json!({"x": 1, "y": 2}),
                    target: Some(Target {
                        app_name: Some("TextEdit".to_owned()),
                        pid: Some(9),
                        ..Target::default()
                    }),
                    idempotency_key: key.map(str::to_owned),
                },
            )
        }

        fn finish(&mut self, caller: &Caller, id: &SessionId, ticket: CallTicket, ok: bool) {
            let outcome = CallOutcome {
                ok,
                effect: Some(Effect::Confirmed),
                verified: true,
                error_code: None,
                reply: json!({"ticket": ticket.0}),
            };
            self.run(
                caller,
                Command::FinishCall {
                    id: id.clone(),
                    ticket,
                    outcome,
                    duration_ms: 3,
                    frames: CallFrames::default(),
                },
            )
            .expect("finish");
        }
    }

    #[test]
    fn start_is_idempotent_per_actor_and_label_and_stamps_identity() {
        let mut h = Harness::new();
        let a = agent("a");
        let id = h.start(&a, "research");
        assert_eq!(h.start(&a, "research"), id);
        let other = h.start(&agent("b"), "research");
        assert_ne!(other, id);
        let record = h.book.record(&id).unwrap();
        assert_eq!(record.agent, a.identity);
        assert_eq!(record.label, "research");
        assert!(id.as_str().starts_with("cu_n_"));
        assert_eq!(h.log[&id].len(), 1);
    }

    #[test]
    fn identity_comes_from_the_caller_not_the_args() {
        let mut h = Harness::new();
        let a = agent("a");
        let id = h.start(&a, "s");
        let Ok(Outcome::Execute { ticket }) = h.run(
            &a,
            Command::BeginCall {
                id: id.clone(),
                kind: CallKind::Act,
                tool: "click".to_owned(),
                args: json!({"actor": "mallory", "agent": {"kind": "mux"}, "x": 1}),
                target: None,
                idempotency_key: None,
            },
        ) else {
            panic!("expected execute")
        };
        h.finish(&a, &id, ticket, true);
        let event = h.log[&id].last().unwrap();
        assert_eq!(event.actor, "a");
        assert_eq!(h.book.record(&id).unwrap().agent.class, AgentClass::Agent);
        // another agent cannot act on, finish or end this session
        assert_eq!(h.call(&agent("b"), &id, None), Err(Reject::NotOwner));
        assert_eq!(
            h.run(&agent("b"), Command::End { id: id.clone() }),
            Err(Reject::NotOwner)
        );
    }

    #[test]
    fn user_stop_wins_and_blocks_restart_of_the_label() {
        let mut h = Harness::new();
        let a = agent("a");
        let id = h.start(&a, "s");
        let Ok(Outcome::Execute { ticket }) = h.call(&a, &id, None) else {
            panic!()
        };
        // an agent cannot stop, pause or resume
        assert_eq!(
            h.run(&a, Command::Stop { id: id.clone() }),
            Err(Reject::UserRequired)
        );
        assert_eq!(
            h.run(&a, Command::Pause { id: id.clone() }),
            Err(Reject::UserRequired)
        );
        // the claimed origin channel does not make an agent a user
        let spoof = Caller {
            identity: a.identity.clone(),
            origin: Origin::User,
        };
        assert_eq!(
            h.run(&spoof, Command::Stop { id: id.clone() }),
            Err(Reject::UserRequired)
        );
        h.run(&user(), Command::Stop { id: id.clone() }).unwrap();
        // the call already running finishes and is logged
        h.finish(&a, &id, ticket, true);
        assert_eq!(
            h.call(&a, &id, None),
            Ok(Outcome::Refused {
                reason: Reject::SessionStopped
            })
        );
        assert_eq!(
            h.run(
                &a,
                Command::Start {
                    label: "s".to_owned(),
                    scope: None
                }
            ),
            Err(Reject::SessionStopped)
        );
        let kinds: Vec<EventKind> = h.log[&id].iter().map(|e| e.kind).collect();
        assert_eq!(
            kinds,
            vec![
                EventKind::SessionStart,
                EventKind::SessionStop,
                EventKind::Act,
                EventKind::PolicyReject
            ]
        );
        h.run(
            &user(),
            Command::AllowAgent {
                actor: "a".to_owned(),
            },
        )
        .unwrap();
        let again = h.start(&a, "s");
        assert_ne!(again, id);
    }

    #[test]
    fn stop_agent_ends_every_session_and_refuses_new_ones() {
        let mut h = Harness::new();
        let a = agent("a");
        let one = h.start(&a, "one");
        let two = h.start(&a, "two");
        let b = h.start(&agent("b"), "one");
        h.run(
            &user(),
            Command::StopAgent {
                actor: "a".to_owned(),
            },
        )
        .unwrap();
        for id in [&one, &two] {
            assert_eq!(
                h.book.record(id).unwrap().status,
                SessionStatus::Ended {
                    reason: EndReason::UserStop
                }
            );
        }
        assert!(h.book.record(&b).unwrap().status.is_live());
        assert_eq!(
            h.run(
                &a,
                Command::Start {
                    label: "three".to_owned(),
                    scope: None
                }
            ),
            Err(Reject::AgentStopped)
        );
    }

    #[test]
    fn pause_refuses_calls_until_resume() {
        let mut h = Harness::new();
        let a = agent("a");
        let id = h.start(&a, "s");
        h.run(&user(), Command::Pause { id: id.clone() }).unwrap();
        assert_eq!(
            h.call(&a, &id, None),
            Ok(Outcome::Refused {
                reason: Reject::SessionPaused
            })
        );
        h.run(&user(), Command::Resume { id: id.clone() }).unwrap();
        assert!(matches!(h.call(&a, &id, None), Ok(Outcome::Execute { .. })));
    }

    #[test]
    fn retried_calls_replay_and_never_execute_twice() {
        let mut h = Harness::new();
        let a = agent("a");
        let id = h.start(&a, "s");
        let Ok(Outcome::Execute { ticket }) = h.call(&a, &id, Some("k1")) else {
            panic!()
        };
        assert_eq!(
            h.call(&a, &id, Some("k1")),
            Ok(Outcome::Refused {
                reason: Reject::CallInFlight
            })
        );
        h.finish(&a, &id, ticket, true);
        match h.call(&a, &id, Some("k1")) {
            Ok(Outcome::Replay { outcome }) => {
                assert_eq!(outcome.reply, json!({"ticket": ticket.0}))
            }
            other => panic!("expected replay, got {other:?}"),
        }
        // after the TTL the key may run again
        h.now += CALL_CACHE_TTL_MS;
        assert!(matches!(
            h.call(&a, &id, Some("k1")),
            Ok(Outcome::Execute { .. })
        ));
    }

    #[test]
    fn typed_text_never_reaches_the_log() {
        let mut h = Harness::new();
        let a = agent("a");
        let id = h.start(&a, "s");
        let Ok(Outcome::Execute { ticket }) = h.run(
            &a,
            Command::BeginCall {
                id: id.clone(),
                kind: CallKind::Act,
                tool: "type_text".to_owned(),
                args: json!({"pid": 9, "text": "correct horse battery"}),
                target: None,
                idempotency_key: None,
            },
        ) else {
            panic!()
        };
        h.finish(&a, &id, ticket, true);
        let stored = serde_json::to_string(&h.log[&id]).unwrap();
        assert!(!stored.contains("correct horse"), "{stored}");
        assert!(
            !stored.contains("\"ticket\""),
            "reply must not be persisted: {stored}"
        );
        assert!(stored.contains("\"length\":21"));
    }

    #[test]
    fn idle_expiry_waits_for_in_flight_calls_and_host_restart_ends_live_sessions() {
        let mut h = Harness::new();
        let a = agent("a");
        let id = h.start(&a, "s");
        let Ok(Outcome::Execute { ticket }) = h.call(&a, &id, None) else {
            panic!()
        };
        assert_eq!(
            h.run(&a, Command::ExpireIdle { id: id.clone() }),
            Ok(Outcome::Refused {
                reason: Reject::CallInFlight
            })
        );
        h.finish(&a, &id, ticket, false);
        h.run(&a, Command::MarkIdle { id: id.clone() }).unwrap();
        assert_eq!(h.book.record(&id).unwrap().status, SessionStatus::Idle);
        h.run(&a, Command::ExpireIdle { id: id.clone() }).unwrap();
        assert_eq!(
            h.book.record(&id).unwrap().status,
            SessionStatus::Ended {
                reason: EndReason::IdleTtl
            }
        );
        assert_eq!(h.book.record(&id).unwrap().counters.errors, 1);

        let live = h.start(&a, "s");
        assert_ne!(live, id, "an ended session is never reused");
        h.run(&a, Command::HostRestart).unwrap();
        assert_eq!(
            h.book.record(&live).unwrap().status,
            SessionStatus::Ended {
                reason: EndReason::HostRestart
            }
        );
    }

    /// Random command sequences from two agents and the user. Checks C1, C2,
    /// C4 and C6 after every step and that rejects never change state.
    #[test]
    fn invariants_hold_for_random_sequences() {
        let mut seed: u64 = 0x1234_5678_9abc_def1;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..300 {
            let mut h = Harness::new();
            let callers = [agent("a"), agent("b"), user()];
            let labels = ["x", "y"];
            let mut tickets: Vec<(Caller, SessionId, CallTicket)> = Vec::new();
            let mut executed: HashMap<(SessionId, String), u32> = HashMap::new();
            let mut stopped_at: HashMap<SessionId, usize> = HashMap::new();
            let mut ever_ended: HashSet<SessionId> = HashSet::new();
            for step in 0..60 {
                let caller = callers[(next() % 3) as usize].clone();
                let ids: Vec<SessionId> = h.book.records().map(|r| r.id.clone()).collect();
                let pick = |n: u64| ids.get((n as usize) % ids.len().max(1)).cloned();
                match next() % 9 {
                    0 | 1 => {
                        let _ = h.run(
                            &caller,
                            Command::Start {
                                label: labels[(next() % 2) as usize].to_owned(),
                                scope: None,
                            },
                        );
                    }
                    2 | 3 => {
                        if let Some(id) = pick(next()) {
                            let key = format!("k{}", next() % 3);
                            let result = h.call(&caller, &id, Some(&key));
                            if let Ok(Outcome::Execute { ticket }) = result {
                                assert!(!stopped_at.contains_key(&id), "C1: executed after stop");
                                *executed.entry((id.clone(), key)).or_default() += 1;
                                tickets.push((caller.clone(), id, ticket));
                            }
                        }
                    }
                    4 => {
                        if !tickets.is_empty() {
                            let (c, id, t) = tickets.remove((next() as usize) % tickets.len());
                            h.finish(&c, &id, t, next() % 4 != 0);
                        }
                    }
                    5 => {
                        if let Some(id) = pick(next()) {
                            if h.run(&caller, Command::Stop { id: id.clone() }).is_ok()
                                && caller.is_user()
                            {
                                stopped_at.entry(id).or_insert(step);
                            }
                        }
                    }
                    6 => {
                        if let Some(id) = pick(next()) {
                            let command = if next() % 2 == 0 {
                                Command::Pause { id }
                            } else {
                                Command::Resume { id }
                            };
                            let _ = h.run(&caller, command);
                        }
                    }
                    7 => {
                        if let Some(id) = pick(next()) {
                            let _ = h.run(&caller, Command::End { id });
                        }
                    }
                    _ => {
                        if let Some(id) = pick(next()) {
                            let _ = h.run(&caller, Command::ExpireIdle { id });
                        }
                    }
                }
                for record in h.book.records() {
                    if ever_ended.contains(&record.id) {
                        assert!(!record.status.is_live(), "C6: ended session came back");
                    }
                    if !record.status.is_live() {
                        ever_ended.insert(record.id.clone());
                    }
                }
            }
            // C2: one key ran at most once per TTL window (the harness never
            // advances past the TTL here: 60 steps x 7 ms).
            for ((_, key), count) in &executed {
                assert!(*count <= 1, "C2: key {key} executed {count} times");
            }
            // C4: gap-free seq per session.
            for (id, events) in &h.log {
                let seqs: Vec<u64> = events.iter().map(|e| e.seq).collect();
                let expected: Vec<u64> = (0..events.len() as u64).collect();
                assert_eq!(seqs, expected, "C4 for {id}");
                assert_eq!(h.book.next_seq(id), Some(events.len() as u64));
            }
        }
    }
}
