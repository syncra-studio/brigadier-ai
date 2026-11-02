//! Foreground observation decisions; no Win32 calls, tested on every host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Observation {
    pub activated: bool,
    pub uncertain_broker: bool,
    pub foreground_seen: bool,
    pub restored: bool,
    pub monitoring: bool,
    pub interrupted: bool,
    pub user_input: bool,
    /// The front was handed back and Windows refused every route.
    pub refused: bool,
    /// A launched window that opened over the user's window was put under it.
    pub placed_behind: bool,
}

impl Observation {
    pub(crate) fn report(self) -> (&'static str, bool) {
        if self.activated
            && self.foreground_seen
            && !self.restored
            && self.refused
            && !self.user_input
            && !self.interrupted
        {
            return ("The launched app took the front, and Windows refused to give it back to your window: the app is in front of your window now. Click your window to continue. The app itself opened.", true);
        }
        if self.user_input {
            return ("Real user input ended automatic focus restoration; any recorded activation may reflect your own actions. Keeping the original window in front is unconfirmed.", true);
        }
        if self.interrupted {
            return ("The captured window is no longer valid; automatic restoration stopped. Keeping it in front is unconfirmed.", true);
        }
        if !self.activated && self.uncertain_broker {
            return ("A Windows broker activation event could not be attributed to this launch. Keeping your window in front is unconfirmed.", true);
        }
        if self.activated && !self.foreground_seen && self.restored && self.placed_behind {
            return ("Windows reported an app activation event, but the app was never seen in front: your window was in front at every check, including at reply time. The app's window opened on top of yours and was placed behind your window.", false);
        }
        if self.activated && !self.foreground_seen && self.restored {
            return ("Windows reported an app activation event, but the app was never seen in front: your window was in front at every check, including at reply time.", false);
        }
        if self.activated && !self.foreground_seen {
            return ("Windows reported an app activation event, but the app was not confirmed in front. Keeping your window in front is unconfirmed; monitoring remains bounded.", true);
        }
        match (self.activated, self.restored, self.monitoring, self.interrupted) {
            (true, true, _, _) => (
                "App activation was observed; your previous window was confirmed in front at reply time. A focus change occurred.", false),
            (true, false, _, _) => (
                "App activation was observed; your previous window could not be confirmed in front. Opening and keeping your window in front are separate results.", true),
            (false, _, _, true) => (
                "Monitoring stopped after user input or the previous window closed; keeping your window in front is unconfirmed.", true),
            (false, _, true, false) if self.placed_behind => (
                "No app activation has been recorded before this reply. The app's window opened on top of yours and was placed behind your window. Monitoring continues for up to 8 seconds after the call; this does not guarantee a flash-free launch.", false),
            (false, _, true, false) => (
                "No app activation has been recorded before this reply. Monitoring continues for up to 8 seconds after the call; this does not guarantee a flash-free launch.", false),
            _ => ("Window activation observation was unavailable or its bounded watch expired; preservation is unconfirmed.", true),
        }
    }
}

impl Observation {
    /// The sentence an input action's reply carries when the target app had
    /// the front during it (a displaced watch starts with it already there);
    /// None when no activation was seen or the user's own input may explain it.
    pub(crate) fn action_note(self) -> Option<&'static str> {
        if !self.activated || !self.foreground_seen || self.user_input || self.interrupted {
            return None;
        }
        Some(if self.restored {
            "The app's window had the front during this action; your window was put back in front."
        } else {
            "The app's window had the front during this action and Windows did not give it back: it is in front of your window now. Click your window to continue."
        })
    }
}

/// Facts for [`may_inherit_prior`] about one live input watch.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct InheritFacts {
    /// The front window's process is one of the watch's targets.
    pub target: bool,
    pub monitoring: bool,
    pub restore_allowed: bool,
    /// The watch saw a target window take the front.
    pub activated: bool,
    /// No physical input since the watch started.
    pub input_unchanged: bool,
    pub prior_valid: bool,
    pub prior_is_target: bool,
}

/// A later action may treat a live watch's window as the user's only when the
/// app took the front from it during that watch and the user has not touched
/// the mouse or keyboard since: then the app's front window is not the user's.
pub(crate) fn may_inherit_prior(facts: InheritFacts) -> bool {
    facts.target
        && facts.monitoring
        && facts.restore_allowed
        && facts.activated
        && facts.input_unchanged
        && facts.prior_valid
        && !facts.prior_is_target
}

pub(crate) fn may_hand_back(
    prior_valid: bool,
    input_unchanged: bool,
    target: bool,
    still_front: bool,
) -> bool {
    prior_valid && input_unchanged && target && still_front
}

/// A launched window that Windows let take the front may be minimized once so
/// that Windows itself activates the user's window, which must be next below
/// it. A process without foreground rights may still minimize a window, while
/// SetForegroundWindow, AttachThreadInput and SwitchToThisWindow are refused
/// (Windows 11, 2026-11-01). Maximized windows are left alone: they could not
/// be restored maximized without activation.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct MinimizeFacts {
    pub visible: bool,
    pub iconic: bool,
    pub zoomed: bool,
    pub minimizable: bool,
    pub topmost: bool,
    pub tool: bool,
    pub prior_next: bool,
    pub prior_hung: bool,
}

pub(crate) fn may_minimize_for_hand_back(facts: MinimizeFacts) -> bool {
    facts.visible
        && !facts.iconic
        && !facts.zoomed
        && facts.minimizable
        && !facts.topmost
        && !facts.tool
        && facts.prior_next
        && !facts.prior_hung
}

/// A launched window shown without activation still opens on top of the
/// z-order. Place it under the user's window while that window is in front.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct CoverFacts {
    pub visible: bool,
    pub root: bool,
    pub iconic: bool,
    pub topmost: bool,
    pub tool: bool,
    pub target: bool,
}

pub(crate) fn place_behind_prior(facts: CoverFacts) -> bool {
    facts.visible && facts.root && !facts.iconic && !facts.topmost && !facts.tool && facts.target
}

pub(crate) fn expired(elapsed_ms: u128, since_end_ms: Option<u128>) -> bool {
    elapsed_ms >= 30_000 || since_end_ms.is_some_and(|ms| ms >= 8_000)
}

// Moving the pointer and typing into the user's current window must not
// disarm protection. Yield when the user explicitly selects another window.
pub(crate) fn switch_key(vk: u32, alt: bool, ctrl: bool, injected: bool) -> bool {
    !injected
        && (matches!(vk, 0x5B | 0x5C) || (alt && matches!(vk, 0x09 | 0x1B)) || (ctrl && vk == 0x1B))
}

pub(crate) fn switch_click(injected: bool, clicked_prior: bool) -> bool {
    !injected && !clicked_prior
}

// Any event of the user's own, pointer motion included, interrupts typing.
// Windows flags every SendInput event as injected, ours and other programs'.
pub(crate) fn own_input(injected: bool) -> bool {
    !injected
}

pub(crate) fn launch_names(mut expected: Vec<String>) -> Vec<String> {
    for name in expected.clone() {
        match name.as_str() {
            "calculator" | "calc" | "calculatorapp" => expected.push("calculatorapp".into()),
            "task manager" | "taskmgr" => expected.push("taskmgr".into()),
            "windows powershell" | "powershell" | "pwsh" | "windows terminal" => {
                expected.extend(["powershell".into(), "pwsh".into(), "windowsterminal".into()])
            }

            _ => {}
        }
    }
    expected.retain(|name| name != "applicationframehost");
    expected.sort_unstable();
    expected.dedup();
    expected
}

/// Attribution can arrive after a broker's brief foreground event. Keep
/// bounded evidence until its real pid is returned; never infer activation
/// from calling SetForegroundWindow successfully on an already-front window.
#[derive(Default)]
pub(crate) struct ActivationEvidence {
    known: Vec<u32>,
    pending: Vec<u32>,
    pub(crate) activated: bool,
}

impl ActivationEvidence {
    pub(crate) fn observe(&mut self, pid: u32, name_matches: bool) -> bool {
        if pid == 0 {
            return false;
        }
        if name_matches || self.known.contains(&pid) {
            self.activated = true;
            return true;
        }
        if self.pending.len() < 64 && !self.pending.contains(&pid) {
            self.pending.push(pid);
        }
        false
    }

    pub(crate) fn is_known(&self, pid: u32) -> bool {
        pid != 0 && self.known.contains(&pid)
    }

    pub(crate) fn bind(&mut self, pid: u32) {
        if pid == 0 {
            return;
        }
        if !self.known.contains(&pid) {
            self.known.push(pid);
        }
        self.activated |= self.pending.contains(&pid);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_users_own_input_interrupts_typing() {
        assert!(own_input(false));
        assert!(!own_input(true));
    }

    #[test]
    fn a_broker_pid_binds_a_brief_event_recorded_before_activation_returned() {
        let mut evidence = ActivationEvidence::default();
        assert!(!evidence.observe(42, false));
        assert!(!evidence.activated);
        evidence.bind(42);
        assert!(evidence.activated);
        assert!(evidence.observe(42, false));
    }

    #[test]
    fn unrelated_events_and_a_successful_noop_restore_are_not_activation_evidence() {
        let mut evidence = ActivationEvidence::default();
        evidence.observe(17, false);
        evidence.bind(42);
        assert!(!evidence.activated);
        assert!(!evidence.observe(0, true));
        assert!(evidence.observe(42, false));
        let mut named = ActivationEvidence::default();
        assert!(named.observe(99, true));
        assert!(named.activated);
    }

    #[test]
    fn pending_broker_evidence_is_bounded() {
        let mut evidence = ActivationEvidence::default();
        for pid in 1..=1000 {
            evidence.observe(pid, false);
        }
        assert_eq!(evidence.pending.len(), 64);
        evidence.bind(0);
        assert!(!evidence.activated);
    }

    #[test]
    fn launch_attribution_covers_reported_shell_and_store_delegates() {
        for (requested, host) in [
            ("calculator", "calculatorapp"),
            ("calc", "calculatorapp"),
            ("task manager", "taskmgr"),
            ("windows powershell", "windowsterminal"),
            ("chrome", "chrome"),
            ("notepad", "notepad"),
        ] {
            assert!(launch_names(vec![requested.into()])
                .iter()
                .any(|name| name == host));
        }
        assert!(launch_names(Vec::new()).is_empty());
        assert!(!launch_names(vec!["chrome".into()]).contains(&"windowsterminal".into()));
    }

    #[test]
    fn only_physical_window_selection_disarms_the_watch() {
        assert!(switch_click(false, false));
        assert!(!switch_click(false, true));
        assert!(!switch_click(true, false));
        assert!(switch_key(0x09, true, false, false));
        assert!(switch_key(0x5B, false, false, false));
        assert!(switch_key(0x1B, false, true, false));
        assert!(!switch_key(0x09, true, false, true));
        assert!(!switch_key(0x41, false, false, false));
        assert!(!switch_key(0x0D, false, false, false));
    }

    #[test]
    fn report_uses_activation_evidence_not_a_successful_noop_restore() {
        let unseen = Observation {
            restored: true,
            monitoring: true,
            ..Default::default()
        };
        assert!(unseen
            .report()
            .0
            .starts_with("No app activation has been recorded"));
        let observed = Observation {
            activated: true,
            foreground_seen: true,
            restored: true,
            ..unseen
        };
        assert!(observed.report().0.contains("A focus change occurred"));
        assert!(!observed.report().1);
        assert!(
            Observation {
                restored: false,
                ..observed
            }
            .report()
            .1
        );
    }

    #[test]
    fn unarmed_expired_and_user_interrupted_watches_never_claim_preservation() {
        assert!(Observation::default().report().1);
        assert!(
            Observation {
                interrupted: true,
                ..Default::default()
            }
            .report()
            .1
        );
        assert!(
            Observation {
                activated: true,
                restored: false,
                ..Default::default()
            }
            .report()
            .1
        );
    }

    #[test]
    fn never_undo_user_input_unrelated_windows_or_a_later_foreground_change() {
        assert!(may_hand_back(true, true, true, true));
        assert!(!may_hand_back(false, true, true, true));
        assert!(!may_hand_back(true, false, true, true));
        assert!(!may_hand_back(true, true, false, true));
        assert!(!may_hand_back(true, true, true, false));
    }

    #[test]
    fn late_splash_and_main_window_watch_has_two_finite_deadlines() {
        assert!(!expired(29_999, None));
        assert!(expired(30_000, None));
        assert!(!expired(9_000, Some(7_999)));
        assert!(expired(9_000, Some(8_000)));
        assert!(expired(30_000, Some(0)));
    }
}

/// Pure lease accounting shared by every native launch holder.
#[derive(Default)]
pub(crate) struct ForegroundLockState {
    pub holders: usize,
    pub locked: bool,
    pub generation: u64,
}
impl ForegroundLockState {
    pub fn add_holder(&mut self) {
        self.holders += 1;
    }
    pub fn release(&mut self, generation: u64) -> bool {
        if generation != self.generation || self.holders == 0 {
            return false;
        }
        self.holders -= 1;
        if self.holders != 0 {
            return false;
        }
        let unlock = self.locked;
        self.locked = false;
        unlock
    }
    pub fn revoke(&mut self) -> bool {
        self.generation = self.generation.wrapping_add(1);
        self.holders = 0;
        let unlock = self.locked;
        self.locked = false;
        unlock
    }
}
#[cfg(test)]
mod lease_tests {
    use super::*;
    #[test]
    fn nested_holders_release_only_last_and_revocation_invalidates_all() {
        let mut state = ForegroundLockState {
            locked: true,
            ..Default::default()
        };
        state.add_holder();
        state.add_holder();
        let old = state.generation;
        assert!(!state.release(old));
        assert!(state.locked);
        assert!(state.revoke());
        state.locked = true;
        state.add_holder();
        assert!(!state.release(old));
        assert!(state.locked);
        assert_eq!(state.holders, 1);
        assert!(state.release(state.generation));
        assert!(!state.release(state.generation));
    }
    #[test]
    fn broker_host_is_never_sufficient_launch_identity() {
        for app in ["calc", "notepad", "systemsettings"] {
            assert!(!launch_names(vec![app.into()]).contains(&"applicationframehost".into()));
        }
        assert!(Observation {
            activated: true,
            ..Default::default()
        }
        .report()
        .0
        .contains("not confirmed in front"));
    }
    #[test]
    fn an_activation_event_with_the_user_window_always_in_front_says_so() {
        let kept = Observation {
            activated: true,
            restored: true,
            ..Default::default()
        };
        let (text, failed) = kept.report();
        assert!(
            text.contains("your window was in front at every check"),
            "{text}"
        );
        assert!(!failed);
        let (covered, _) = Observation {
            placed_behind: true,
            ..kept
        }
        .report();
        assert!(covered.contains("placed behind your window"), "{covered}");
        let (lost, failed) = Observation {
            restored: false,
            ..kept
        }
        .report();
        assert!(lost.contains("not confirmed in front") && failed, "{lost}");
    }
}

/// A shared/unknown host PID alone does not identify the addressed input window.
pub(crate) fn input_window_matches(
    pid_matches: bool,
    exact_window: bool,
    shared_or_unknown_host: bool,
) -> bool {
    pid_matches && (!shared_or_unknown_host || exact_window)
}
#[cfg(test)]
mod input_host_tests {
    use super::*;
    #[test]
    fn shared_host_cannot_attribute_an_unrelated_frame_to_the_input() {
        assert!(!input_window_matches(true, false, true));
        assert!(input_window_matches(true, true, true));
        assert!(!input_window_matches(false, true, true));
        assert!(input_window_matches(true, false, false));
    }
}

/// Preserve queued pre-input WinEvents, including across the tick-count wrap.
/// Polling after interruption is never evidence of an app-caused activation.
pub(crate) fn record_before_input(unchanged: bool, event: Option<u32>, input: Option<u32>) -> bool {
    unchanged
        || event
            .is_some_and(|time| input.is_some_and(|first| (time.wrapping_sub(first) as i32) <= 0))
}
#[cfg(test)]
mod input_time_tests {
    use super::*;
    #[test]
    fn queued_pre_input_events_survive_zero_and_wrap_but_later_polls_do_not() {
        assert!(record_before_input(true, None, None));
        assert!(!record_before_input(false, None, Some(10)));
        assert!(record_before_input(false, Some(9), Some(10)));
        assert!(!record_before_input(false, Some(11), Some(10)));
        assert!(record_before_input(false, Some(u32::MAX), Some(0)));
        assert!(!record_before_input(false, Some(1), Some(u32::MAX)));
        assert!(Observation {
            user_input: true,
            activated: true,
            ..Default::default()
        }
        .report()
        .0
        .starts_with("Real user input ended"));
    }
}

#[cfg(test)]
mod launch_hand_back_tests {
    use super::*;

    fn kept_front() -> Observation {
        Observation {
            activated: true,
            foreground_seen: true,
            monitoring: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_refused_hand_back_says_the_app_is_in_front_of_your_window() {
        let (note, failed) = Observation {
            refused: true,
            ..kept_front()
        }
        .report();
        assert!(failed);
        assert!(note.contains("Windows refused to give it back to your window"));
        assert!(note.contains("Click your window"));
        // Restored, or ended by the user's own input: never the refusal.
        assert!(Observation {
            refused: true,
            restored: true,
            ..kept_front()
        }
        .report()
        .0
        .contains("A focus change occurred"));
        assert!(Observation {
            refused: true,
            user_input: true,
            ..kept_front()
        }
        .report()
        .0
        .starts_with("Real user input ended"));
        assert!(Observation {
            monitoring: true,
            placed_behind: true,
            ..Default::default()
        }
        .report()
        .0
        .contains("placed behind your window"));
        // Not attempted yet: the earlier unconfirmed wording stays.
        assert!(kept_front().report().0.contains("could not be confirmed"));
    }

    #[test]
    fn minimizing_is_only_for_a_plain_window_with_your_window_next_below() {
        let plain = MinimizeFacts {
            visible: true,
            minimizable: true,
            prior_next: true,
            ..Default::default()
        };
        assert!(may_minimize_for_hand_back(plain));
        for blocked in [
            MinimizeFacts {
                visible: false,
                ..plain
            },
            MinimizeFacts {
                iconic: true,
                ..plain
            },
            MinimizeFacts {
                zoomed: true,
                ..plain
            },
            MinimizeFacts {
                minimizable: false,
                ..plain
            },
            MinimizeFacts {
                topmost: true,
                ..plain
            },
            MinimizeFacts {
                tool: true,
                ..plain
            },
            MinimizeFacts {
                prior_next: false,
                ..plain
            },
            MinimizeFacts {
                prior_hung: true,
                ..plain
            },
        ] {
            assert!(!may_minimize_for_hand_back(blocked), "{blocked:?}");
        }
    }

    #[test]
    fn only_the_launched_apps_ordinary_windows_are_placed_behind_yours() {
        let window = CoverFacts {
            visible: true,
            root: true,
            target: true,
            ..Default::default()
        };
        assert!(place_behind_prior(window));
        for kept in [
            CoverFacts {
                target: false,
                ..window
            },
            CoverFacts {
                topmost: true,
                ..window
            },
            CoverFacts {
                tool: true,
                ..window
            },
            CoverFacts {
                iconic: true,
                ..window
            },
            CoverFacts {
                root: false,
                ..window
            },
            CoverFacts {
                visible: false,
                ..window
            },
        ] {
            assert!(!place_behind_prior(kept), "{kept:?}");
        }
    }

    #[test]
    fn bound_launch_pids_are_known_and_zero_never_is() {
        let mut evidence = ActivationEvidence::default();
        evidence.bind(42);
        assert!(evidence.is_known(42));
        assert!(!evidence.is_known(7));
        assert!(!evidence.is_known(0));
    }
}

#[cfg(test)]
mod input_dialog_hand_back_tests {
    use super::*;

    fn displaced() -> InheritFacts {
        InheritFacts {
            target: true,
            monitoring: true,
            restore_allowed: true,
            activated: true,
            input_unchanged: true,
            prior_valid: true,
            prior_is_target: false,
        }
    }

    #[test]
    fn the_users_window_is_inherited_only_when_the_app_took_the_front_without_input() {
        assert!(may_inherit_prior(displaced()));
        for blocked in [
            // Another app's watch says nothing about this app's dialog.
            InheritFacts {
                target: false,
                ..displaced()
            },
            InheritFacts {
                monitoring: false,
                ..displaced()
            },
            InheritFacts {
                restore_allowed: false,
                ..displaced()
            },
            // The app never took the front: the user may have chosen it.
            InheritFacts {
                activated: false,
                ..displaced()
            },
            // The user's own input since: their choice stands.
            InheritFacts {
                input_unchanged: false,
                ..displaced()
            },
            InheritFacts {
                prior_valid: false,
                ..displaced()
            },
            // The app's own window is never the window to give the front to.
            InheritFacts {
                prior_is_target: true,
                ..displaced()
            },
        ] {
            assert!(!may_inherit_prior(blocked), "{blocked:?}");
        }
    }

    #[test]
    fn an_action_reply_says_when_the_app_took_the_front_and_whether_it_came_back() {
        let took = Observation {
            activated: true,
            foreground_seen: true,
            monitoring: true,
            ..Default::default()
        };
        let kept = took.action_note().expect("kept front is reported");
        assert!(kept.contains("Click your window"));
        let back = Observation {
            restored: true,
            ..took
        }
        .action_note()
        .expect("restored front is reported");
        assert!(back.contains("put back in front"));
        // No activation, an unconfirmed event, or the user's own input: silent.
        assert_eq!(Observation::default().action_note(), None);
        assert_eq!(
            Observation {
                foreground_seen: false,
                ..took
            }
            .action_note(),
            None
        );
        assert_eq!(
            Observation {
                user_input: true,
                ..took
            }
            .action_note(),
            None
        );
        assert_eq!(
            Observation {
                interrupted: true,
                ..took
            }
            .action_note(),
            None
        );
    }
}
