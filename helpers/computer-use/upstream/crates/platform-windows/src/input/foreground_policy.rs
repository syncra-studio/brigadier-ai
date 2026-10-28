//! Foreground observation decisions; no Win32 calls, tested on every host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Observation {
    pub activated: bool,
    pub restored: bool,
    pub monitoring: bool,
    pub interrupted: bool,
}

impl Observation {
    pub(crate) fn report(self) -> (&'static str, bool) {
        match (self.activated, self.restored, self.monitoring, self.interrupted) {
            (true, true, _, _) => (
                "App activation was observed; your previous window was confirmed in front at reply time. A brief focus change occurred.", false),
            (true, false, _, _) => (
                "App activation was observed; your previous window could not be confirmed in front. Opening and foreground preservation are separate results.", true),
            (false, _, _, true) => (
                "Foreground monitoring stopped after user input or the previous window closed; foreground preservation is unconfirmed.", true),
            (false, _, true, false) => (
                "No app activation has been recorded before this reply. Monitoring continues for up to 8 seconds after the call; this does not guarantee a flash-free launch.", false),
            _ => ("Foreground observation was unavailable or its bounded watch expired; preservation is unconfirmed.", true),
        }
    }
}

pub(crate) fn may_hand_back(
    prior_valid: bool,
    input_unchanged: bool,
    target: bool,
    still_front: bool,
) -> bool {
    prior_valid && input_unchanged && target && still_front
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

pub(crate) fn launch_names(mut expected: Vec<String>) -> Vec<String> {
    for name in expected.clone() {
        match name.as_str() {
            "calculator" | "calc" | "calculatorapp" => {
                expected.extend(["calculatorapp".into(), "applicationframehost".into()])
            }
            "task manager" | "taskmgr" => expected.push("taskmgr".into()),
            "windows powershell" | "powershell" | "pwsh" | "windows terminal" => {
                expected.extend(["powershell".into(), "pwsh".into(), "windowsterminal".into()])
            }
            "notepad" | "systemsettings" => expected.push("applicationframehost".into()),
            _ => {}
        }
    }
    if expected.is_empty() {
        expected.push("applicationframehost".into());
    }
    expected.sort_unstable();
    expected.dedup();
    expected
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_attribution_covers_reported_shell_and_store_delegates() {
        for (requested, host) in [
            ("calculator", "calculatorapp"),
            ("calc", "applicationframehost"),
            ("task manager", "taskmgr"),
            ("windows powershell", "windowsterminal"),
            ("chrome", "chrome"),
            ("notepad", "applicationframehost"),
        ] {
            assert!(launch_names(vec![requested.into()])
                .iter()
                .any(|name| name == host));
        }
        assert_eq!(launch_names(Vec::new()), vec!["applicationframehost"]);
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
            restored: true,
            ..unseen
        };
        assert!(observed
            .report()
            .0
            .contains("A brief focus change occurred"));
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
