//! The build lease and the heat escalation, as a pure state machine: given the heavy commands
//! Brigadier's CLIs run now and the machine's load, what to stop, what to let go on, and what
//! the threads should say.
//!
//! - **One build at a time.** The first heavy command takes the daemon-wide lease; one that
//!   starts while another holds it, or while the machine is strained, is stopped where it
//!   stands (never asked about, denied or killed) and goes on, oldest first, once the lease is
//!   free and the machine eased. The lease belongs to the command, not to its worker: it is
//!   given back the moment the command ends or leaves its worker's tree, so a worker waiting
//!   on another one (a nested review) never holds it, and a crashed worker can't keep it.
//! - **Bounded.** A command holding the lease for [`LEASE_MAX`] is taken for a server or
//!   watcher the classifier missed: it keeps running and no longer holds the lease.
//! - **Critical heat.** At critical heat for [`CRITICAL_HOLD`], the newest running build is
//!   paused, then another every [`STEP`], until the heat drops back to serious or below; then
//!   every paused build goes on, in reverse order.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use brigadier_sandbox::MachineLoad;
use serde::{Deserialize, Serialize};

/// How long a command may hold the lease before it counts as long-running.
pub(crate) const LEASE_MAX: Duration = Duration::from_secs(10 * 60);
/// How long critical heat lasts before builds are paused.
pub(crate) const CRITICAL_HOLD: Duration = Duration::from_secs(60);
/// Time between two pauses while the heat stays critical.
pub(crate) const STEP: Duration = Duration::from_secs(20);

/// A process by its pid and start time, so a later process reusing the pid is never taken
/// for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Proc {
    pub pid: u32,
    /// Start time, in ms since the Unix epoch.
    pub started_ms: i64,
}

/// A heavy command one of Brigadier's CLIs runs now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    /// Its topmost heavy process.
    pub root: Proc,
    /// The CLI's owner in the cleanup ledger (`task:<id>`, `orch:<id>`, …).
    pub owner: String,
    /// How a thread row names it.
    pub command: String,
}

/// What a thread row says about a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Note {
    /// It waits for the machine to cool down (or free memory) before it starts.
    WaitingToCool,
    /// It waits for another build to finish.
    WaitingForBuild,
    /// It was paused to let the machine cool down.
    Paused,
    /// It goes on after a pause.
    Resumed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action {
    /// Stop the command's process tree.
    Stop(Proc),
    /// Let the command's process tree go on.
    Continue(Proc),
    /// Tell the command's owner's thread.
    Note {
        owner: String,
        command: String,
        note: Note,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Running; `since` it last started running.
    Running { since: Instant },
    /// Running past [`LEASE_MAX`]: left alone, holding no lease.
    LongRunning,
    /// Stopped before it ever ran far, until the lease is free and the machine eased.
    Waiting,
    /// Stopped for the heat; `order` among the paused, `long` whether it was long-running.
    Paused { order: u64, long: bool },
}

#[derive(Debug)]
struct Build {
    owner: String,
    command: String,
    /// Arrival order: the oldest waiting goes first, the newest running is paused first.
    seq: u64,
    state: State,
}

#[derive(Debug, Default)]
pub(crate) struct Builds {
    builds: HashMap<Proc, Build>,
    lease: Option<Proc>,
    next: u64,
    critical_since: Option<Instant>,
    last_pause: Option<Instant>,
}

impl Builds {
    /// Takes in what runs now and the machine's load at `now`; returns what to do, stops and
    /// continues in the order they are to happen.
    pub(crate) fn tick(&mut self, seen: Vec<Seen>, load: MachineLoad, now: Instant) -> Vec<Action> {
        let mut actions = Vec::new();
        // Ended, or left its CLI's tree (its shell gave up on it): forget it. One still stopped
        // goes on, so it can take whatever ended its shell and end too.
        let current: HashMap<Proc, Seen> = seen.into_iter().map(|seen| (seen.root, seen)).collect();
        let gone: Vec<Proc> = self
            .builds
            .keys()
            .filter(|proc| !current.contains_key(proc))
            .copied()
            .collect();
        for proc in gone {
            if let Some(build) = self.builds.remove(&proc)
                && matches!(build.state, State::Waiting | State::Paused { .. })
            {
                actions.push(Action::Continue(proc));
            }
            if self.lease == Some(proc) {
                self.lease = None;
            }
        }
        // The lease holder that ran too long keeps running without it.
        if let Some(holder) = self.lease
            && let Some(build) = self.builds.get_mut(&holder)
            && let State::Running { since } = build.state
            && now.duration_since(since) >= LEASE_MAX
        {
            build.state = State::LongRunning;
            self.lease = None;
        }
        // New commands: one runs if it may, the others wait.
        let mut new: Vec<Seen> = current
            .into_values()
            .filter(|seen| !self.builds.contains_key(&seen.root))
            .collect();
        new.sort_by_key(|seen| (seen.root.started_ms, seen.root.pid));
        for seen in new {
            let seq = self.next;
            self.next += 1;
            let waiting_before = self
                .builds
                .values()
                .any(|build| build.state == State::Waiting);
            let state = if self.lease.is_none() && !load.strained() && !waiting_before {
                self.lease = Some(seen.root);
                State::Running { since: now }
            } else {
                actions.push(Action::Stop(seen.root));
                actions.push(Action::Note {
                    owner: seen.owner.clone(),
                    command: seen.command.clone(),
                    note: if load.strained() {
                        Note::WaitingToCool
                    } else {
                        Note::WaitingForBuild
                    },
                });
                State::Waiting
            };
            self.builds.insert(
                seen.root,
                Build {
                    owner: seen.owner,
                    command: seen.command,
                    seq,
                    state,
                },
            );
        }
        if load.critical() {
            let since = *self.critical_since.get_or_insert(now);
            let due = now.duration_since(since) >= CRITICAL_HOLD
                && self
                    .last_pause
                    .is_none_or(|last| now.duration_since(last) >= STEP);
            if due && let Some(proc) = self.newest_running() {
                let order = self.next;
                self.next += 1;
                let build = self.builds.get_mut(&proc).expect("just found");
                build.state = State::Paused {
                    order,
                    long: build.state == State::LongRunning,
                };
                self.last_pause = Some(now);
                actions.push(Action::Stop(proc));
                actions.push(Action::Note {
                    owner: build.owner.clone(),
                    command: build.command.clone(),
                    note: Note::Paused,
                });
            }
        } else {
            self.critical_since = None;
            self.last_pause = None;
            actions.extend(self.resume_paused(now));
        }
        // The oldest waiting command takes a free lease once the machine eased.
        if self.lease.is_none()
            && !load.strained()
            && let Some((&proc, build)) = self
                .builds
                .iter_mut()
                .filter(|(_, build)| build.state == State::Waiting)
                .min_by_key(|(_, build)| build.seq)
        {
            build.state = State::Running { since: now };
            self.lease = Some(proc);
            actions.push(Action::Continue(proc));
        }
        actions
    }

    /// Every stopped command goes on (the daemon is quitting), and nothing is tracked after.
    pub(crate) fn release_all(&mut self) -> Vec<Action> {
        let mut stopped: Vec<(u64, Proc)> = self
            .builds
            .iter()
            .filter_map(|(proc, build)| match build.state {
                State::Paused { order, .. } => Some((order, *proc)),
                State::Waiting => Some((build.seq, *proc)),
                _ => None,
            })
            .collect();
        stopped.sort_by_key(|(order, _)| std::cmp::Reverse(*order));
        *self = Self::default();
        stopped
            .into_iter()
            .map(|(_, proc)| Action::Continue(proc))
            .collect()
    }

    /// The commands stopped now.
    #[cfg(test)]
    pub(crate) fn stopped(&self) -> Vec<Proc> {
        self.builds
            .iter()
            .filter(|(_, build)| matches!(build.state, State::Waiting | State::Paused { .. }))
            .map(|(proc, _)| *proc)
            .collect()
    }

    /// The running (or long-running) build that arrived last.
    fn newest_running(&self) -> Option<Proc> {
        self.builds
            .iter()
            .filter(|(_, build)| matches!(build.state, State::Running { .. } | State::LongRunning))
            .max_by_key(|(_, build)| build.seq)
            .map(|(proc, _)| *proc)
    }

    /// Lets every paused build go on, the last paused first.
    fn resume_paused(&mut self, now: Instant) -> Vec<Action> {
        let mut paused: Vec<(u64, Proc)> = self
            .builds
            .iter()
            .filter_map(|(proc, build)| match build.state {
                State::Paused { order, .. } => Some((order, *proc)),
                _ => None,
            })
            .collect();
        paused.sort_by_key(|(order, _)| std::cmp::Reverse(*order));
        let mut actions = Vec::new();
        for (_, proc) in paused {
            let build = self.builds.get_mut(&proc).expect("just listed");
            let State::Paused { long, .. } = build.state else {
                continue;
            };
            build.state = if long {
                State::LongRunning
            } else {
                State::Running { since: now }
            };
            actions.push(Action::Continue(proc));
            actions.push(Action::Note {
                owner: build.owner.clone(),
                command: build.command.clone(),
                note: Note::Resumed,
            });
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use brigadier_sandbox::Heat;

    use super::*;

    const CALM: MachineLoad = MachineLoad {
        heat: Heat::Nominal,
        memory_tight: false,
    };
    const HOT: MachineLoad = MachineLoad {
        heat: Heat::Serious,
        memory_tight: false,
    };
    const TIGHT: MachineLoad = MachineLoad {
        heat: Heat::Nominal,
        memory_tight: true,
    };
    const CRITICAL: MachineLoad = MachineLoad {
        heat: Heat::Critical,
        memory_tight: false,
    };

    fn proc(pid: u32) -> Proc {
        Proc {
            pid,
            started_ms: 1_000 + i64::from(pid),
        }
    }

    fn seen(pid: u32, owner: &str) -> Seen {
        Seen {
            root: proc(pid),
            owner: owner.into(),
            command: format!("cargo test #{pid}"),
        }
    }

    fn stops(actions: &[Action]) -> Vec<u32> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Stop(proc) => Some(proc.pid),
                _ => None,
            })
            .collect()
    }

    fn continues(actions: &[Action]) -> Vec<u32> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Continue(proc) => Some(proc.pid),
                _ => None,
            })
            .collect()
    }

    fn notes(actions: &[Action]) -> Vec<(String, Note)> {
        actions
            .iter()
            .filter_map(|action| match action {
                Action::Note { owner, note, .. } => Some((owner.clone(), *note)),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn concurrent_disjoint_workers_build_one_at_a_time() {
        let mut builds = Builds::default();
        let t0 = Instant::now();
        // Two workers start a build in the same moment: the older process runs.
        let actions = builds.tick(vec![seen(20, "task:b"), seen(10, "task:a")], CALM, t0);
        assert_eq!(stops(&actions), vec![20]);
        assert_eq!(
            notes(&actions),
            vec![("task:b".into(), Note::WaitingForBuild)]
        );
        // A third arrives: it waits behind the second.
        let actions = builds.tick(
            vec![seen(10, "task:a"), seen(20, "task:b"), seen(30, "task:c")],
            CALM,
            t0 + Duration::from_secs(2),
        );
        assert_eq!(stops(&actions), vec![30]);
        // The first ends: the oldest waiting goes on, and only it.
        let actions = builds.tick(
            vec![seen(20, "task:b"), seen(30, "task:c")],
            CALM,
            t0 + Duration::from_secs(4),
        );
        assert_eq!(continues(&actions), vec![20]);
        assert!(notes(&actions).is_empty());
        let actions = builds.tick(vec![seen(30, "task:c")], CALM, t0 + Duration::from_secs(6));
        assert_eq!(continues(&actions), vec![30]);
        assert_eq!(builds.stopped(), vec![]);
    }

    #[test]
    fn a_nested_review_never_waits_on_its_parent_holding_the_lease() {
        let mut builds = Builds::default();
        let t0 = Instant::now();
        // The parent worker builds, then asks for a review and waits on it: its build ended,
        // so the lease is free for the reviewer's build.
        builds.tick(vec![seen(10, "task:parent")], CALM, t0);
        let actions = builds.tick(
            vec![seen(11, "task:review")],
            CALM,
            t0 + Duration::from_secs(2),
        );
        assert_eq!(stops(&actions), Vec::<u32>::new());
        assert_eq!(builds.lease, Some(proc(11)));
        // While the parent's own build still runs, the reviewer's waits only for that build,
        // which needs nothing from the reviewer: it ends, and the reviewer's goes on.
        let mut builds = Builds::default();
        builds.tick(vec![seen(10, "task:parent")], CALM, t0);
        let actions = builds.tick(
            vec![seen(10, "task:parent"), seen(11, "task:review")],
            CALM,
            t0 + Duration::from_secs(2),
        );
        assert_eq!(stops(&actions), vec![11]);
        let actions = builds.tick(
            vec![seen(11, "task:review")],
            CALM,
            t0 + Duration::from_secs(4),
        );
        assert_eq!(continues(&actions), vec![11]);
    }

    #[test]
    fn a_command_that_runs_too_long_gives_the_lease_back() {
        let mut builds = Builds::default();
        let t0 = Instant::now();
        builds.tick(vec![seen(10, "task:a")], CALM, t0);
        builds.tick(
            vec![seen(10, "task:a"), seen(20, "task:b")],
            CALM,
            t0 + Duration::from_secs(1),
        );
        let actions = builds.tick(
            vec![seen(10, "task:a"), seen(20, "task:b")],
            CALM,
            t0 + LEASE_MAX + Duration::from_secs(1),
        );
        assert_eq!(continues(&actions), vec![20]);
        assert!(
            stops(&actions).is_empty(),
            "the long-running one is left alone"
        );
    }

    #[test]
    fn a_stopped_command_that_left_its_worker_goes_on_and_is_forgotten() {
        let mut builds = Builds::default();
        let t0 = Instant::now();
        builds.tick(vec![seen(10, "task:a"), seen(20, "task:b")], CALM, t0);
        // b's shell timed out and went away: its stopped command goes on (to end) at once.
        let actions = builds.tick(vec![seen(10, "task:a")], CALM, t0 + Duration::from_secs(2));
        assert_eq!(continues(&actions), vec![20]);
        assert_eq!(builds.stopped(), vec![]);
    }

    #[test]
    fn while_strained_new_builds_wait_and_running_ones_are_untouched() {
        for strained in [HOT, TIGHT] {
            let mut builds = Builds::default();
            let t0 = Instant::now();
            builds.tick(vec![seen(10, "task:a")], CALM, t0);
            let actions = builds.tick(
                vec![seen(10, "task:a"), seen(20, "task:b")],
                strained,
                t0 + Duration::from_secs(2),
            );
            assert_eq!(stops(&actions), vec![20], "only the new one");
            assert_eq!(
                notes(&actions),
                vec![("task:b".into(), Note::WaitingToCool)]
            );
            // a ends while still hot: b keeps waiting.
            let actions = builds.tick(
                vec![seen(20, "task:b")],
                strained,
                t0 + Duration::from_secs(4),
            );
            assert!(actions.is_empty(), "{actions:?}");
            // It clears: b starts.
            let actions = builds.tick(vec![seen(20, "task:b")], CALM, t0 + Duration::from_secs(6));
            assert_eq!(continues(&actions), vec![20]);
        }
    }

    #[test]
    fn critical_heat_held_a_minute_pauses_newest_first_and_resumes_in_reverse() {
        let mut builds = Builds::default();
        let t0 = Instant::now();
        let all = || vec![seen(10, "task:a"), seen(20, "task:b"), seen(30, "task:c")];
        builds.tick(vec![seen(10, "task:a")], CALM, t0);
        // Two more start while the first runs past its lease time: all three run.
        let t1 = t0 + LEASE_MAX + Duration::from_secs(1);
        builds.tick(vec![seen(10, "task:a"), seen(20, "task:b")], CALM, t1);
        // 20 now holds the lease; 30 waits for it.
        builds.tick(all(), CALM, t1 + Duration::from_secs(1));
        assert_eq!(builds.stopped(), vec![proc(30)]);
        // Critical, but not for a minute yet: nothing is paused.
        let t2 = t1 + Duration::from_secs(10);
        assert!(builds.tick(all(), CRITICAL, t2).is_empty());
        assert!(
            builds
                .tick(all(), CRITICAL, t2 + Duration::from_secs(59))
                .is_empty()
        );
        // A minute: the newest running (20) is paused.
        let actions = builds.tick(all(), CRITICAL, t2 + CRITICAL_HOLD);
        assert_eq!(stops(&actions), vec![20]);
        assert_eq!(notes(&actions), vec![("task:b".into(), Note::Paused)]);
        // Not another one before the step.
        assert!(
            builds
                .tick(all(), CRITICAL, t2 + CRITICAL_HOLD + Duration::from_secs(5))
                .is_empty()
        );
        let actions = builds.tick(all(), CRITICAL, t2 + CRITICAL_HOLD + STEP);
        assert_eq!(stops(&actions), vec![10]);
        // Nothing left to pause.
        assert!(
            builds
                .tick(all(), CRITICAL, t2 + CRITICAL_HOLD + STEP * 2)
                .is_empty()
        );
        // Back to serious: both go on, the last paused first; the waiting one still waits.
        let actions = builds.tick(all(), HOT, t2 + CRITICAL_HOLD + STEP * 3);
        assert_eq!(continues(&actions), vec![10, 20]);
        assert_eq!(
            notes(&actions),
            vec![
                ("task:a".into(), Note::Resumed),
                ("task:b".into(), Note::Resumed)
            ]
        );
        assert_eq!(builds.stopped(), vec![proc(30)]);
    }

    #[test]
    fn a_brief_spike_of_critical_heat_pauses_nothing() {
        let mut builds = Builds::default();
        let t0 = Instant::now();
        builds.tick(vec![seen(10, "task:a")], CALM, t0);
        builds.tick(
            vec![seen(10, "task:a")],
            CRITICAL,
            t0 + Duration::from_secs(1),
        );
        builds.tick(vec![seen(10, "task:a")], HOT, t0 + Duration::from_secs(40));
        let actions = builds.tick(
            vec![seen(10, "task:a")],
            CRITICAL,
            t0 + Duration::from_secs(70),
        );
        assert!(actions.is_empty(), "the minute starts over: {actions:?}");
    }

    #[test]
    fn quitting_lets_everything_stopped_go_on() {
        let mut builds = Builds::default();
        let t0 = Instant::now();
        builds.tick(vec![seen(10, "task:a"), seen(20, "task:b")], CALM, t0);
        builds.tick(
            vec![seen(10, "task:a"), seen(20, "task:b")],
            CRITICAL,
            t0 + Duration::from_secs(1),
        );
        builds.tick(
            vec![seen(10, "task:a"), seen(20, "task:b")],
            CRITICAL,
            t0 + Duration::from_secs(62),
        );
        let mut stopped = builds.stopped();
        stopped.sort_by_key(|proc| proc.pid);
        assert_eq!(stopped, vec![proc(10), proc(20)]);
        let mut resumed = continues(&builds.release_all());
        resumed.sort_unstable();
        assert_eq!(resumed, vec![10, 20]);
        assert!(builds.stopped().is_empty());
    }
}
