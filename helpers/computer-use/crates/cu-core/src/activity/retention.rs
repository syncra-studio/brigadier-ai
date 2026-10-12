//! Retention planner (brigadier-next decision D19).
//!
//! Pure: the store passes what it has, the planner returns what to delete
//! and when to run next. The host runs it at start, after a session ends and
//! on one one-shot timer at `next_run_ms`; it never sweeps periodically.
//!
//! Rules, in order:
//! 1. A session record and all its events and frames go 30 days after it
//!    ended. Live sessions never expire.
//! 2. A frame goes 7 days after capture.
//! 3. A session keeps at most `session_event_cap` events: the oldest prefix
//!    goes (gap-free seq stays gap-free from the new first event, C4).
//! 4. A session keeps at most `session_frame_bytes` of frames: full frames
//!    before thumbnails, oldest first.
//! 5. The machine keeps at most `machine_frame_bytes` of frames: frames of
//!    ended sessions before live ones, full frames before thumbnails, oldest
//!    first.
//!
//! Frames go before events: no rule except 1 and 3 deletes an event.

use std::collections::{BTreeMap, BTreeSet};

use super::model::SessionId;

pub const DAY_MS: u64 = 24 * 60 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub event_ttl_ms: u64,
    pub frame_ttl_ms: u64,
    pub machine_frame_bytes: u64,
    pub session_frame_bytes: u64,
    pub session_event_cap: u64,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        RetentionPolicy {
            event_ttl_ms: 30 * DAY_MS,
            frame_ttl_ms: 7 * DAY_MS,
            machine_frame_bytes: 2 * 1024 * 1024 * 1024,
            session_frame_bytes: 500 * 1024 * 1024,
            session_event_cap: 20_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: SessionId,
    /// `None` while the session is live.
    pub ended_at_ms: Option<u64>,
    /// Seq of the oldest stored event.
    pub first_seq: u64,
    /// Seq the next event will get (exclusive end).
    pub next_seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FrameKind {
    Full,
    Thumbnail,
}

/// One stored frame reference (an event's before or after image).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameRef {
    pub session: SessionId,
    pub seq: u64,
    pub kind: FrameKind,
    pub blob: String,
    pub captured_at_ms: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Plan {
    /// Delete the record, every event and every frame of these sessions.
    pub delete_sessions: Vec<SessionId>,
    /// Delete events with `seq < before_seq` of each session (and their frames).
    pub truncate_events: Vec<(SessionId, u64)>,
    /// Delete these frame references (the store removes a blob when its last
    /// reference goes).
    pub delete_frames: Vec<FrameRef>,
    /// When to run the planner again, if anything can expire.
    pub next_run_ms: Option<u64>,
}

pub fn plan(
    now_ms: u64,
    policy: &RetentionPolicy,
    sessions: &[SessionSummary],
    frames: &[FrameRef],
) -> Plan {
    let mut out = Plan::default();
    let mut gone_sessions: BTreeSet<&SessionId> = BTreeSet::new();
    let mut truncated: BTreeMap<&SessionId, u64> = BTreeMap::new();
    let mut next_run: Option<u64> = None;
    let mut wake_at = |at: u64| {
        next_run = Some(next_run.map_or(at, |current: u64| current.min(at)));
    };

    let live: BTreeSet<&SessionId> = sessions
        .iter()
        .filter(|s| s.ended_at_ms.is_none())
        .map(|s| &s.id)
        .collect();

    // Rules 1 and 3.
    for session in sessions {
        if let Some(ended) = session.ended_at_ms {
            let expires = ended.saturating_add(policy.event_ttl_ms);
            if expires <= now_ms {
                gone_sessions.insert(&session.id);
                out.delete_sessions.push(session.id.clone());
                continue;
            }
            wake_at(expires);
        }
        let stored = session.next_seq.saturating_sub(session.first_seq);
        if stored > policy.session_event_cap {
            let before = session.next_seq - policy.session_event_cap;
            truncated.insert(&session.id, before);
            out.truncate_events.push((session.id.clone(), before));
        }
    }

    // Frames that survive rules 1 and 3 (those rules delete them implicitly).
    let mut kept: Vec<&FrameRef> = Vec::new();
    for frame in frames {
        if gone_sessions.contains(&frame.session) {
            continue;
        }
        if let Some(&before) = truncated.get(&frame.session) {
            if frame.seq < before {
                continue;
            }
        }
        // Rule 2.
        let expires = frame.captured_at_ms.saturating_add(policy.frame_ttl_ms);
        if expires <= now_ms {
            out.delete_frames.push(frame.clone());
            continue;
        }
        kept.push(frame);
    }

    // Rule 4: per-session byte cap.
    let mut by_session: BTreeMap<&SessionId, Vec<&FrameRef>> = BTreeMap::new();
    for frame in kept.iter().copied() {
        by_session.entry(&frame.session).or_default().push(frame);
    }
    let mut dropped: BTreeSet<(SessionId, u64, FrameKind, String)> = BTreeSet::new();
    for (_, mut list) in by_session {
        let mut total: u64 = list.iter().map(|f| f.bytes).sum();
        if total <= policy.session_frame_bytes {
            continue;
        }
        list.sort_by_key(|a| (a.kind, a.captured_at_ms, a.seq));
        for frame in list {
            if total <= policy.session_frame_bytes {
                break;
            }
            total -= frame.bytes;
            dropped.insert(frame_key(frame));
            out.delete_frames.push(frame.clone());
        }
    }
    kept.retain(|f| !dropped.contains(&frame_key(f)));

    // Rule 5: machine byte cap.
    let mut total: u64 = kept.iter().map(|f| f.bytes).sum();
    if total > policy.machine_frame_bytes {
        let mut order = kept.clone();
        order.sort_by(|a, b| {
            let a_live = live.contains(&a.session);
            let b_live = live.contains(&b.session);
            (a_live, a.kind, a.captured_at_ms, a.seq).cmp(&(
                b_live,
                b.kind,
                b.captured_at_ms,
                b.seq,
            ))
        });
        for frame in order {
            if total <= policy.machine_frame_bytes {
                break;
            }
            total -= frame.bytes;
            dropped.insert(frame_key(frame));
            out.delete_frames.push(frame.clone());
        }
        kept.retain(|f| !dropped.contains(&frame_key(f)));
    }

    for frame in &kept {
        wake_at(frame.captured_at_ms.saturating_add(policy.frame_ttl_ms));
    }
    out.next_run_ms = next_run;
    out
}

fn frame_key(frame: &FrameRef) -> (SessionId, u64, FrameKind, String) {
    (
        frame.session.clone(),
        frame.seq,
        frame.kind,
        frame.blob.clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::model::Profile;

    fn sid(n: &str) -> SessionId {
        SessionId::new(Profile::Native, n)
    }

    fn session(n: &str, ended: Option<u64>, first: u64, next: u64) -> SessionSummary {
        SessionSummary {
            id: sid(n),
            ended_at_ms: ended,
            first_seq: first,
            next_seq: next,
        }
    }

    fn frame(n: &str, seq: u64, kind: FrameKind, at: u64, bytes: u64) -> FrameRef {
        FrameRef {
            session: sid(n),
            seq,
            kind,
            blob: format!("{n}-{seq}-{kind:?}"),
            captured_at_ms: at,
            bytes,
        }
    }

    const NOW: u64 = 100 * DAY_MS;

    #[test]
    fn ended_sessions_expire_after_thirty_days_live_never() {
        let p = RetentionPolicy::default();
        let sessions = [
            session("old", Some(NOW - 31 * DAY_MS), 0, 10),
            session("recent", Some(NOW - 29 * DAY_MS), 0, 10),
            session("live", None, 0, 10),
        ];
        let plan = plan(NOW, &p, &sessions, &[]);
        assert_eq!(plan.delete_sessions, vec![sid("old")]);
        assert_eq!(plan.next_run_ms, Some(NOW + DAY_MS));
    }

    #[test]
    fn frames_expire_after_seven_days_and_go_with_their_session() {
        let p = RetentionPolicy::default();
        let sessions = [session("a", None, 0, 5), session("gone", Some(0), 0, 5)];
        let frames = [
            frame("a", 1, FrameKind::Thumbnail, NOW - 8 * DAY_MS, 10),
            frame("a", 2, FrameKind::Thumbnail, NOW - 6 * DAY_MS, 10),
            frame("gone", 1, FrameKind::Thumbnail, NOW - DAY_MS, 10),
        ];
        let plan = plan(NOW, &p, &sessions, &frames);
        assert_eq!(plan.delete_sessions, vec![sid("gone")]);
        assert_eq!(plan.delete_frames, vec![frames[0].clone()]);
        assert_eq!(plan.next_run_ms, Some(NOW + DAY_MS));
    }

    #[test]
    fn event_cap_truncates_oldest_prefix() {
        let p = RetentionPolicy {
            session_event_cap: 100,
            ..RetentionPolicy::default()
        };
        let sessions = [session("a", None, 0, 250)];
        let frames = [
            frame("a", 10, FrameKind::Thumbnail, NOW, 1),
            frame("a", 200, FrameKind::Thumbnail, NOW, 1),
        ];
        let plan = plan(NOW, &p, &sessions, &frames);
        assert_eq!(plan.truncate_events, vec![(sid("a"), 150)]);
        // the frame of a truncated event goes with it, not as a separate delete
        assert!(plan.delete_frames.is_empty());
    }

    #[test]
    fn machine_cap_prefers_ended_then_full_then_oldest() {
        let p = RetentionPolicy {
            machine_frame_bytes: 30,
            session_frame_bytes: 1_000,
            ..RetentionPolicy::default()
        };
        let sessions = [
            session("live", None, 0, 10),
            session("done", Some(NOW - DAY_MS), 0, 10),
        ];
        let frames = [
            frame("live", 1, FrameKind::Full, NOW - 3000, 10),
            frame("live", 2, FrameKind::Thumbnail, NOW - 2000, 10),
            frame("done", 1, FrameKind::Thumbnail, NOW - 1000, 10),
            frame("done", 2, FrameKind::Full, NOW - 500, 10),
            frame("live", 3, FrameKind::Thumbnail, NOW - 100, 10),
        ];
        let plan = plan(NOW, &p, &sessions, &frames);
        assert_eq!(
            plan.delete_frames,
            vec![frames[3].clone(), frames[2].clone()]
        );
    }

    #[test]
    fn session_cap_drops_full_frames_first() {
        let p = RetentionPolicy {
            session_frame_bytes: 15,
            ..RetentionPolicy::default()
        };
        let sessions = [session("a", None, 0, 10)];
        let frames = [
            frame("a", 1, FrameKind::Thumbnail, NOW - 300, 5),
            frame("a", 2, FrameKind::Full, NOW - 200, 10),
            frame("a", 3, FrameKind::Thumbnail, NOW - 100, 5),
        ];
        let plan = plan(NOW, &p, &sessions, &frames);
        assert_eq!(plan.delete_frames, vec![frames[1].clone()]);
    }

    #[test]
    fn caps_hold_for_random_inputs() {
        let mut seed: u64 = 42;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let p = RetentionPolicy {
            machine_frame_bytes: 5_000,
            session_frame_bytes: 2_000,
            session_event_cap: 50,
            ..RetentionPolicy::default()
        };
        for _ in 0..200 {
            let session_count = 1 + next() % 6;
            let mut sessions = Vec::new();
            let mut frames = Vec::new();
            for s in 0..session_count {
                let name = format!("s{s}");
                let ended = if next() % 2 == 0 {
                    None
                } else {
                    Some(NOW - (next() % (40 * DAY_MS)))
                };
                let first = next() % 20;
                let next_seq = first + next() % 120;
                sessions.push(session(&name, ended, first, next_seq));
                for seq in first..next_seq {
                    if next() % 3 == 0 {
                        let kind = if next() % 2 == 0 {
                            FrameKind::Full
                        } else {
                            FrameKind::Thumbnail
                        };
                        frames.push(frame(
                            &name,
                            seq,
                            kind,
                            NOW - next() % (10 * DAY_MS),
                            1 + next() % 300,
                        ));
                    }
                }
            }
            let plan = plan(NOW, &p, &sessions, &frames);
            let deleted: BTreeSet<_> = plan.delete_frames.iter().map(frame_key).collect();
            let gone: BTreeSet<_> = plan.delete_sessions.iter().cloned().collect();
            let cut: BTreeMap<_, _> = plan.truncate_events.iter().cloned().collect();
            let remaining: Vec<&FrameRef> = frames
                .iter()
                .filter(|f| !deleted.contains(&frame_key(f)))
                .filter(|f| !gone.contains(&f.session))
                .filter(|f| cut.get(&f.session).is_none_or(|before| f.seq >= *before))
                .collect();
            let total: u64 = remaining.iter().map(|f| f.bytes).sum();
            assert!(total <= p.machine_frame_bytes);
            for f in &remaining {
                assert!(f.captured_at_ms + p.frame_ttl_ms > NOW);
            }
            for s in &sessions {
                let per: u64 = remaining
                    .iter()
                    .filter(|f| f.session == s.id)
                    .map(|f| f.bytes)
                    .sum();
                assert!(per <= p.session_frame_bytes);
                if s.ended_at_ms.is_none() {
                    assert!(!gone.contains(&s.id), "live session deleted");
                }
                let stored_after = s.next_seq
                    - cut
                        .get(&s.id)
                        .copied()
                        .unwrap_or(s.first_seq)
                        .max(s.first_seq);
                assert!(gone.contains(&s.id) || stored_after <= p.session_event_cap);
            }
            // deleted frames are never double counted
            assert_eq!(deleted.len(), plan.delete_frames.len());
        }
    }
}
