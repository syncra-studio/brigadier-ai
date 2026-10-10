//! The build lease and the heat escalation, as a pure state machine: given the heavy commands
//! Brigadier's CLIs run now and the machine's load, what to stop, what to let go on, and what
//! the threads should say.
//!
//! - **One build at a time.** The first heavy command takes the daemon-wide lease; one that
//!   starts while another holds it, or while the machine is strained, is stopped where it
//!   stands (never asked about, denied or killed) and goes on, oldest first, once the lease is
//!   free and the machine eased (memory warnings alone hold at most [`MEMORY_WARNING_HOLD`]).
//!   The lease belongs to the command, not to its worker: it is given back the moment the
//!   command ends or leaves its worker's tree, so a worker waiting on another one (a nested
//!   review) never holds it, and a crashed worker can't keep it.
//! - **Bounded.** A command holding the lease for [`LEASE_MAX`] is taken for a server or
//!   watcher the classifier missed: it keeps running and no longer holds the lease. One that
//!   used no CPU for [`IDLE`] while others wait is blocked on something (the network, or a
//!   lock a waiting command took in the moment before it was stopped, such as cargo's on a
//!   shared target folder): it keeps running too, and the next command goes on.
//! - **Critical heat.** At critical heat for [`CRITICAL_HOLD`], the newest running build is
//!   paused, then another every [`STEP`], until the heat drops back to serious or below; then
//!   every paused build goes on, in reverse order.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use brigadier_sandbox::{MachineLoad, MemoryPressure};
use serde::{Deserialize, Serialize};

use crate::model::MachineStepReason;

/// Maximum delay for a new build held only by a memory warning. Heat and critical memory
/// pressure never time out; an occupied lease still waits for its holder.
pub(crate) const MEMORY_WARNING_HOLD: Duration = Duration::from_secs(2 * 60);
/// How long a command may hold the lease before it counts as long-running.
pub(crate) const LEASE_MAX: Duration = Duration::from_secs(10 * 60);
/// How long the lease holder may use no CPU while others wait before the next one goes on.
pub(crate) const IDLE: Duration = Duration::from_secs(30);
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
    /// Its processes used CPU since the last look (or started), so it isn't blocked.
    pub active: bool,
}

/// What a thread row says about a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Note {
    /// It waits for the machine to cool down (or free memory) before it starts.
    WaitingToCool(MachineStepReason),
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
        proc: Proc,
        owner: String,
        command: String,
        note: Note,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Running; `since` it last started running, `quiet_since` it last used no CPU.
    Running {
        since: Instant,
        quiet_since: Option<Instant>,
    },
    /// Running without the lease (it held it past [`LEASE_MAX`], or idle past [`IDLE`] while
    /// others waited): left alone.
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
    /// When this waiting build first saw warning-only memory pressure.
    warning_since: Option<Instant>,
    /// What holds this waiting build, as its thread last read it (heat or memory).
    held_for: Option<MachineStepReason>,
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
        for (proc, seen) in &current {
            if let Some(build) = self.builds.get_mut(proc)
                && let State::Running { quiet_since, .. } = &mut build.state
            {
                *quiet_since = if seen.active {
                    None
                } else {
                    Some(quiet_since.unwrap_or(now))
                };
            }
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
            let held_for = load
                .builds_held()
                .then(|| MachineStepReason::from_load(load));
            let state = if self.lease.is_none() && held_for.is_none() && !waiting_before {
                self.lease = Some(seen.root);
                State::Running {
                    since: now,
                    quiet_since: None,
                }
            } else {
                actions.push(Action::Stop(seen.root));
                actions.push(Action::Note {
                    proc: seen.root,
                    owner: seen.owner.clone(),
                    command: seen.command.clone(),
                    note: match held_for {
                        Some(reason) => Note::WaitingToCool(reason),
                        None => Note::WaitingForBuild,
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
                    warning_since: None,
                    held_for,
                },
            );
        }
        // The lease holder keeps running without the lease once it ran too long, or sat idle
        // while others wait.
        let waiting = self
            .builds
            .values()
            .any(|build| build.state == State::Waiting);
        if let Some(holder) = self.lease
            && let Some(build) = self.builds.get_mut(&holder)
            && let State::Running { since, quiet_since } = build.state
            && (now.duration_since(since) >= LEASE_MAX
                || (waiting && quiet_since.is_some_and(|quiet| now.duration_since(quiet) >= IDLE)))
        {
            build.state = State::LongRunning;
            self.lease = None;
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
                    proc,
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
        // A persistent warning must not starve builds on an otherwise healthy machine.
        // Reset the grace period whenever heat or critical memory also holds them.
        let warning_only = load.memory == MemoryPressure::Warning && !load.workers_held();
        for build in self
            .builds
            .values_mut()
            .filter(|build| build.state == State::Waiting)
        {
            if warning_only {
                build.warning_since.get_or_insert(now);
            } else {
                build.warning_since = None;
            }
        }
        // The oldest waiting command takes a free lease once the machine eased, or its
        // memory-warning grace period elapsed. Critical memory and heat keep holding it.
        if self.lease.is_none()
            && let Some((&proc, build)) = self
                .builds
                .iter_mut()
                .filter(|(_, build)| build.state == State::Waiting)
                .min_by_key(|(_, build)| build.seq)
            && (!load.builds_held()
                || (warning_only
                    && build
                        .warning_since
                        .is_some_and(|since| now.duration_since(since) >= MEMORY_WARNING_HOLD)))
        {
            build.state = State::Running {
                since: now,
                quiet_since: None,
            };
            self.lease = Some(proc);
            actions.push(Action::Continue(proc));
        }
        // One still waiting for the machine, held now for another reason: its thread says so.
        if load.builds_held() {
            let reason = MachineStepReason::from_load(load);
            for (&proc, build) in &mut self.builds {
                if build.state == State::Waiting
                    && build.held_for.is_some_and(|held_for| held_for != reason)
                {
                    build.held_for = Some(reason);
                    actions.push(Action::Note {
                        proc,
                        owner: build.owner.clone(),
                        command: build.command.clone(),
                        note: Note::WaitingToCool(reason),
                    });
                }
            }
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

    /// A command that couldn't be stopped runs on, without the lease, left alone.
    pub(crate) fn left_running(&mut self, proc: Proc) {
        if let Some(build) = self.builds.get_mut(&proc) {
            build.state = State::LongRunning;
        }
        if self.lease == Some(proc) {
            self.lease = None;
        }
    }

    /// The owners of the commands stopped now.
    pub(crate) fn held_owners(&self) -> Vec<String> {
        self.builds
            .values()
            .filter(|build| matches!(build.state, State::Waiting | State::Paused { .. }))
            .map(|build| build.owner.clone())
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
                State::Running {
                    since: now,
                    quiet_since: None,
                }
            };
            actions.push(Action::Continue(proc));
            actions.push(Action::Note {
                proc,
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
        memory: MemoryPressure::Normal,
    };
    const HOT: MachineLoad = MachineLoad {
        heat: Heat::Serious,
        memory: MemoryPressure::Normal,
    };
    const TIGHT: MachineLoad = MachineLoad {
        heat: Heat::Nominal,
        memory: MemoryPressure::Warning,
    };
    const CRITICAL: MachineLoad = MachineLoad {
        heat: Heat::Critical,
        memory: MemoryPressure::Normal,
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
            active: true,
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
    fn a_lease_holder_idle_while_others_wait_lets_the_next_go_on() {
        let mut builds = Builds::default();
        let t0 = Instant::now();
        let quiet = |pid: u32| Seen {
            active: false,
            ..seen(pid, "task:a")
        };
        // Idle with nobody waiting: it keeps the lease.
        builds.tick(vec![quiet(10)], CALM, t0);
        builds.tick(vec![quiet(10)], CALM, t0 + IDLE * 2);
        assert_eq!(builds.lease, Some(proc(10)));
        // Someone waits; the holder works again, then goes quiet (blocked on a lock the
        // waiting one took): after IDLE of quiet, the waiting one goes on; neither is stopped.
        builds.tick(
            vec![seen(10, "task:a"), seen(20, "task:b")],
            CALM,
            t0 + IDLE * 2 + Duration::from_secs(2),
        );
        let t1 = t0 + IDLE * 2 + Duration::from_secs(4);
        assert!(
            builds
                .tick(vec![quiet(10), seen(20, "task:b")], CALM, t1)
                .is_empty()
        );
        assert!(
            builds
                .tick(
                    vec![quiet(10), seen(20, "task:b")],
                    CALM,
                    t1 + IDLE - Duration::from_secs(1)
                )
                .is_empty()
        );
        let actions = builds.tick(vec![quiet(10), seen(20, "task:b")], CALM, t1 + IDLE);
        assert_eq!(continues(&actions), vec![20]);
        assert!(stops(&actions).is_empty());
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
        for strained in [
            HOT,
            TIGHT,
            MachineLoad {
                memory: MemoryPressure::Critical,
                ..CALM
            },
        ] {
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
                vec![(
                    "task:b".into(),
                    Note::WaitingToCool(MachineStepReason::from_load(strained))
                )]
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
    fn memory_warning_yields_after_two_minutes_but_heat_and_critical_memory_do_not() {
        let t0 = Instant::now();
        for load in [
            TIGHT,
            HOT,
            CRITICAL,
            MachineLoad {
                memory: MemoryPressure::Critical,
                ..CALM
            },
        ] {
            let mut builds = Builds::default();
            assert_eq!(
                stops(&builds.tick(vec![seen(10, "task:a")], load, t0)),
                vec![10]
            );
            assert!(
                builds
                    .tick(
                        vec![seen(10, "task:a")],
                        load,
                        t0 + MEMORY_WARNING_HOLD - Duration::from_secs(1)
                    )
                    .is_empty()
            );
            let actions = builds.tick(vec![seen(10, "task:a")], load, t0 + MEMORY_WARNING_HOLD);
            assert_eq!(
                continues(&actions),
                if load == TIGHT { vec![10] } else { vec![] }
            );
            // Memory warning's admitted build continues running even after the heat pause timer.
            assert!(
                builds
                    .tick(
                        vec![seen(10, "task:a")],
                        load,
                        t0 + MEMORY_WARNING_HOLD + CRITICAL_HOLD
                    )
                    .is_empty()
            );
        }
    }

    #[test]
    fn warning_timeout_keeps_the_lease_and_resets_after_stronger_pressure() {
        let t0 = Instant::now();
        let all = || vec![seen(10, "task:a"), seen(20, "task:b")];
        let mut builds = Builds::default();
        builds.tick(vec![seen(10, "task:a")], CALM, t0);
        builds.tick(all(), TIGHT, t0);
        assert!(continues(&builds.tick(all(), TIGHT, t0 + MEMORY_WARNING_HOLD)).is_empty());
        let critical = MachineLoad {
            memory: MemoryPressure::Critical,
            ..CALM
        };
        builds.tick(all(), critical, t0 + MEMORY_WARNING_HOLD);
        let t1 = t0 + MEMORY_WARNING_HOLD + Duration::from_secs(1);
        assert!(builds.tick(vec![seen(20, "task:b")], TIGHT, t1).is_empty());
        assert_eq!(
            continues(&builds.tick(vec![seen(20, "task:b")], TIGHT, t1 + MEMORY_WARNING_HOLD)),
            vec![20]
        );
    }

    #[test]
    fn a_build_held_for_heat_then_memory_gets_a_row_for_each() {
        let hot_and_tight = MachineLoad {
            memory: MemoryPressure::Warning,
            ..HOT
        };
        let mut builds = Builds::default();
        let t0 = Instant::now();
        let actions = builds.tick(vec![seen(20, "task:b")], hot_and_tight, t0);
        assert_eq!(stops(&actions), vec![20]);
        assert_eq!(
            notes(&actions),
            vec![(
                "task:b".into(),
                Note::WaitingToCool(MachineStepReason::Heat)
            )]
        );
        // Still held for the same reason: no new row.
        let actions = builds.tick(
            vec![seen(20, "task:b")],
            hot_and_tight,
            t0 + Duration::from_secs(2),
        );
        assert!(actions.is_empty());
        // The heat drops; the memory warning keeps holding it, and its thread says so once.
        let actions = builds.tick(vec![seen(20, "task:b")], TIGHT, t0 + Duration::from_secs(4));
        assert_eq!(
            notes(&actions),
            vec![(
                "task:b".into(),
                Note::WaitingToCool(MachineStepReason::Memory)
            )]
        );
        assert!(continues(&actions).is_empty());
        let actions = builds.tick(vec![seen(20, "task:b")], TIGHT, t0 + Duration::from_secs(6));
        assert!(actions.is_empty());
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
