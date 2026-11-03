//! Pure launch decisions, shared with host-side regression tests.
pub(crate) fn executable_name(target: &str) -> String {
    let target = target.trim().trim_matches('"').to_ascii_lowercase();
    let name = target.rsplit(['\\', '/']).next().unwrap_or(&target);
    match name.strip_suffix(".exe").unwrap_or(name) {
        "microsoft edge" => "msedge".into(),
        "google chrome" => "chrome".into(),
        "file explorer" => "explorer".into(),
        other => other.into(),
    }
}

pub(crate) fn may_restore_foreground(
    prior_valid: bool,
    input_unchanged: bool,
    foreground_is_prior: bool,
    foreground_is_target: bool,
) -> bool {
    prior_valid && input_unchanged && !foreground_is_prior && foreground_is_target
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn hand_back_only_launch_activation_without_user_input() {
        assert!(may_restore_foreground(true, true, false, true));
        assert!(!may_restore_foreground(true, false, false, true));
        assert!(!may_restore_foreground(true, true, false, false));
        assert!(!may_restore_foreground(false, true, false, true));
        assert!(!may_restore_foreground(true, true, true, true));
    }
    #[test]
    fn identifies_shell_delegates_and_browser_names() {
        assert_eq!(executable_name("Microsoft Edge"), "msedge");
        assert_eq!(executable_name(r"C:\Program Files\Chrome.EXE"), "chrome");
        assert_eq!(executable_name("File Explorer"), "explorer");
    }
}

/// A shared frame host is a window container, never the app's process identity.
pub(crate) fn confirmed_hosted_app_pid(
    activation_pid: u32,
    frame_host_pid: u32,
    child_pid: Option<u32>,
) -> Option<u32> {
    child_pid.filter(|child| *child != 0 && *child != frame_host_pid && *child == activation_pid)
}

pub(crate) fn is_shared_frame_host(executable: &str) -> bool {
    executable_name(executable) == "applicationframehost"
}

pub(crate) fn termination_front_refusal(
    pid: u32,
    foreground_pid: Option<u32>,
    owner_pid: Option<u32>,
) -> Option<&'static str> {
    match (foreground_pid, owner_pid) {
        (Some(front), Some(owner)) if front != 0 && owner != 0 => {
            (pid == front || pid == owner).then_some(
                "Terminating this app could close your front window or its owned dialog. No termination was attempted.",
            )
        }
        _ => Some("Windows could not identify the front window and its owner. No termination was attempted."),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TerminationObservation {
    Terminated,
    AlreadyExited,
    ExitUnconfirmed,
    Rejected,
}

pub(crate) fn termination_observation(
    request_accepted: bool,
    exit_confirmed: bool,
) -> TerminationObservation {
    match (request_accepted, exit_confirmed) {
        (true, true) => TerminationObservation::Terminated,
        (false, true) => TerminationObservation::AlreadyExited,
        (true, false) => TerminationObservation::ExitUnconfirmed,
        (false, false) => TerminationObservation::Rejected,
    }
}

#[cfg(test)]
mod hosted_termination_tests {
    use super::*;
    #[test]
    fn termination_leaves_the_front_app_and_its_popup_owner_alive() {
        assert!(termination_front_refusal(42, Some(7), Some(7)).is_none());
        assert!(termination_front_refusal(42, Some(42), Some(42)).is_some());
        assert!(termination_front_refusal(42, Some(7), Some(42)).is_some());
        assert!(termination_front_refusal(42, None, Some(7)).is_some());
        assert!(termination_front_refusal(42, Some(7), Some(0)).is_some());
    }
    #[test]
    fn each_frame_maps_to_its_own_launched_app_without_owning_the_shared_host() {
        assert_eq!(
            confirmed_hosted_app_pid(29628, 28908, Some(29628)),
            Some(29628)
        );
        assert_eq!(
            confirmed_hosted_app_pid(1508, 28908, Some(1508)),
            Some(1508)
        );
        assert_eq!(confirmed_hosted_app_pid(29628, 28908, Some(1508)), None);
        assert_eq!(confirmed_hosted_app_pid(28908, 28908, Some(28908)), None);
        assert_eq!(confirmed_hosted_app_pid(29628, 28908, None), None);
        assert_eq!(confirmed_hosted_app_pid(0, 28908, Some(0)), None);
    }
    #[test]
    fn frame_host_is_protected_by_executable_identity_not_a_cached_pid() {
        assert!(is_shared_frame_host(
            r"C:\Windows\System32\ApplicationFrameHost.EXE"
        ));
        assert!(!is_shared_frame_host(
            r"C:\Program Files\WindowsApps\CalculatorApp.exe"
        ));
        assert!(!is_shared_frame_host("ApplicationFrameHostHelper.exe"));
    }
    #[test]
    fn a_queued_termination_or_wait_timeout_never_confirms_process_exit() {
        assert_eq!(
            termination_observation(true, false),
            TerminationObservation::ExitUnconfirmed
        );
        assert_eq!(
            termination_observation(false, false),
            TerminationObservation::Rejected
        );
        assert_eq!(
            termination_observation(true, true),
            TerminationObservation::Terminated
        );
        assert_eq!(
            termination_observation(false, true),
            TerminationObservation::AlreadyExited
        );
    }
}

/// Settings protocol launches often return pid 0; match its actual app identity,
/// never a shared frame host or a localized window title.
pub(crate) fn is_settings_protocol_launch(has_app: bool, urls: &[String]) -> bool {
    !has_app && urls.len() == 1 && urls[0].to_ascii_lowercase().starts_with("ms-settings:")
}

pub(crate) fn is_settings_process(executable: &str) -> bool {
    executable_name(executable) == "systemsettings"
}

#[cfg(test)]
mod settings_protocol_tests {
    use super::*;
    #[test]
    fn protocol_resolution_never_uses_an_unrelated_process_or_host() {
        assert!(is_settings_protocol_launch(
            false,
            &["ms-settings:about".into()]
        ));
        assert!(!is_settings_protocol_launch(
            true,
            &["ms-settings:about".into()]
        ));
        assert!(!is_settings_protocol_launch(
            false,
            &["https://example.com".into()]
        ));
        assert!(!is_settings_protocol_launch(false, &[]));
        assert!(is_settings_process("SystemSettings.exe"));
        assert!(!is_settings_process("ApplicationFrameHost.exe"));
        assert!(!is_settings_process("SystemSettingsAdminFlows.exe"));
    }
}

pub(crate) fn is_notepad_target(target: &str) -> bool {
    executable_name(target) == "notepad"
        || target
            .to_ascii_lowercase()
            .starts_with("microsoft.windowsnotepad_")
}

/// Windows Notepad keeps every window in one process and opens a file as a
/// tab in a window it already shows; it has no switch for a window of its own.
/// The windows this helper did not open belong to the user, so a launch while
/// one is open is refused rather than sent into it.
pub(crate) fn foreign_notepad_windows(open: &[u64], own: &[u64]) -> Vec<u64> {
    open.iter()
        .copied()
        .filter(|window| !own.contains(window))
        .collect()
}

/// Who opened the windows that block a launch, as the refusal states it. An
/// earlier launch that did not answer in time may still have opened a window
/// after its reply, so such a window cannot be called the user's for sure.
fn blocking_window_owner(app: &str, which: &str, unconfirmed_launch: bool) -> String {
    if unconfirmed_launch {
        format!("a window this helper cannot confirm it opened{which}. An earlier {app} launch by this helper did not answer in time, so that window may come from it or may be the user's")
    } else {
        format!("a window this helper did not open{which}")
    }
}

pub(crate) fn notepad_open_refusal(titles: &[String], unconfirmed_launch: bool) -> String {
    let named = titles
        .iter()
        .filter(|title| !title.is_empty())
        .map(|title| format!("{title:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let which = if named.is_empty() {
        String::new()
    } else {
        format!(" ({named})")
    };
    let owner = blocking_window_owner("Notepad", &which, unconfirmed_launch);
    let would = if unconfirmed_launch { "could" } else { "would" };
    format!("No app was launched: Notepad is already open in {owner}. Windows Notepad opens a file as a tab inside a Notepad window that is already open, so it {would} appear in the user's own window, and while Notepad is in front Windows lets it bring a new window in front of the user's work. Ask the user to close Notepad first, or use another editor.")
}

/// A window counts as the user's open window unless DWM reports it cloaked
/// and it is on no virtual desktop. Unknown facts count as open.
pub(crate) fn user_reachable(read: bool, cloaked: bool, on_a_desktop: Option<bool>) -> bool {
    !read || !cloaked || on_a_desktop != Some(false)
}

/// Windows Settings has a single window: `ms-settings:` changes the page in
/// the one already open. One the helper did not open belongs to the user.
pub(crate) fn settings_open_refusal(titles: &[String], unconfirmed_launch: bool) -> String {
    let named = titles
        .iter()
        .filter(|title| !title.is_empty())
        .map(|title| format!("{title:?}"))
        .collect::<Vec<_>>()
        .join(", ");
    let which = if named.is_empty() {
        String::new()
    } else {
        format!(" ({named})")
    };
    let owner = blocking_window_owner("Settings", &which, unconfirmed_launch);
    let would = if unconfirmed_launch { "could" } else { "would" };
    format!("No app was launched: Windows Settings is already open in {owner}. Settings has only one window, so opening a Settings page {would} change the page in the user's own Settings window. Ask the user to close Settings first.")
}

pub(crate) fn fresh_window_argument(target: &str) -> Option<&'static str> {
    match executable_name(target).as_str() {
        "chrome" | "msedge" | "brave" | "chromium" | "vivaldi" | "opera" => Some("--new-window"),
        "firefox" | "mozilla firefox" => Some("-new-window"),
        _ => None,
    }
}

pub(crate) fn document_title_matches(title: &str, filename: &str) -> bool {
    let title = title.trim().trim_start_matches('*');
    let stem = filename.strip_suffix(".txt").unwrap_or(filename);
    [filename, stem].iter().any(|name| {
        title == *name
            || title.strip_prefix(name).is_some_and(|tail| {
                // A window title ("x.txt - Notepad") or a Notepad tab's
                // accessible name ("x.txt. Unmodified.").
                tail.starts_with(" - ") || matches!(tail, ". Unmodified." | ". Modified.")
            })
    })
}

#[cfg(test)]
mod fresh_document_tests {
    use super::*;
    #[test]
    fn only_supported_apps_get_fresh_window_flags() {
        assert_eq!(
            fresh_window_argument("Microsoft Edge"),
            Some("--new-window")
        );
        assert_eq!(fresh_window_argument("chrome.exe"), Some("--new-window"));
        assert_eq!(fresh_window_argument("notepad"), None);
        assert!(is_notepad_target(
            "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"
        ));
        assert!(is_notepad_target(r"C:\Windows\notepad.exe"));
        assert!(!is_notepad_target("Notepad++"));
    }
    #[test]
    fn only_a_cloaked_window_on_no_desktop_is_not_the_users() {
        assert!(user_reachable(true, false, None));
        assert!(user_reachable(false, true, Some(false)));
        // Another virtual desktop: the user can switch to it.
        assert!(user_reachable(true, true, Some(true)));
        assert!(user_reachable(true, true, None));
        assert!(!user_reachable(true, true, Some(false)));
    }
    #[test]
    fn settings_windows_the_helper_did_not_open_block_a_launch() {
        let refusal = settings_open_refusal(&["Settings".into()], false);
        assert!(refusal.starts_with("No app was launched"));
        assert!(refusal.contains("(\"Settings\")"));
        assert!(refusal.contains("close Settings"));
        assert!(refusal.contains("this helper did not open"));
        assert!(!settings_open_refusal(&[String::new()], false).contains("()"));
    }
    #[test]
    fn notepad_windows_the_helper_did_not_open_block_a_launch() {
        assert!(foreign_notepad_windows(&[], &[]).is_empty());
        assert!(foreign_notepad_windows(&[7, 9], &[7, 9, 11]).is_empty());
        assert_eq!(foreign_notepad_windows(&[7, 8], &[7]), vec![8]);
        let refusal = notepad_open_refusal(&["notes.txt - Notepad".into()], false);
        assert!(refusal.starts_with("No app was launched"));
        assert!(refusal.contains("\"notes.txt - Notepad\""));
        assert!(refusal.contains("close Notepad"));
        assert!(refusal.contains("this helper did not open"));
        assert!(!notepad_open_refusal(&[String::new()], false).contains("()"));
    }
    #[test]
    fn a_window_after_an_unconfirmed_launch_is_not_called_the_users_for_sure() {
        for refusal in [
            notepad_open_refusal(&["Untitled - Notepad".into()], true),
            settings_open_refusal(&["Settings".into()], true),
        ] {
            assert!(refusal.starts_with("No app was launched"));
            assert!(refusal.contains("cannot confirm it opened"));
            assert!(refusal.contains("did not answer in time"));
            assert!(!refusal.contains("did not open"));
            assert!(!refusal.contains("would"));
        }
    }
    #[test]
    fn restored_document_titles_cannot_match_the_new_file() {
        assert!(document_title_matches(
            "brigadier-cu-123.txt - Notepad",
            "brigadier-cu-123.txt"
        ));
        assert!(document_title_matches(
            "brigadier-cu-123",
            "brigadier-cu-123.txt"
        ));
        assert!(!document_title_matches(
            "my resume - Notepad",
            "brigadier-cu-123.txt"
        ));
        assert!(!document_title_matches(
            "brigadier-cu-1234.txt",
            "brigadier-cu-123.txt"
        ));
        assert!(document_title_matches(
            "brigadier-cu-123.txt. Unmodified.",
            "brigadier-cu-123.txt"
        ));
        assert!(!document_title_matches(
            "brigadier-cu-123.txt.bak. Unmodified.",
            "brigadier-cu-123.txt"
        ));
    }
}
