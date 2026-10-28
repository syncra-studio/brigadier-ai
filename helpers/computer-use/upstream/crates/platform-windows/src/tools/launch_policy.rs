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
        assert!(is_settings_protocol_launch(false, &["ms-settings:about".into()]));
        assert!(!is_settings_protocol_launch(true, &["ms-settings:about".into()]));
        assert!(!is_settings_protocol_launch(false, &["https://example.com".into()]));
        assert!(!is_settings_protocol_launch(false, &[]));
        assert!(is_settings_process("SystemSettings.exe"));
        assert!(!is_settings_process("ApplicationFrameHost.exe"));
        assert!(!is_settings_process("SystemSettingsAdminFlows.exe"));
    }
}

pub(crate) fn is_notepad_target(target: &str) -> bool {
    executable_name(target) == "notepad"
        || target.to_ascii_lowercase().starts_with("microsoft.windowsnotepad_")
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
    [filename, stem].iter().any(|name| title == *name || title.strip_prefix(name).is_some_and(|tail| tail.starts_with(" - ")))
}

#[cfg(test)]
mod fresh_document_tests {
    use super::*;
    #[test]
    fn only_supported_apps_get_fresh_window_flags() {
        assert_eq!(fresh_window_argument("Microsoft Edge"), Some("--new-window"));
        assert_eq!(fresh_window_argument("chrome.exe"), Some("--new-window"));
        assert_eq!(fresh_window_argument("notepad"), None);
        assert!(is_notepad_target("Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"));
        assert!(is_notepad_target(r"C:\Windows\notepad.exe"));
        assert!(!is_notepad_target("Notepad++"));
    }
    #[test]
    fn restored_document_titles_cannot_match_the_new_file() {
        assert!(document_title_matches("brigadier-cu-123.txt - Notepad", "brigadier-cu-123.txt"));
        assert!(document_title_matches("brigadier-cu-123", "brigadier-cu-123.txt"));
        assert!(!document_title_matches("my resume - Notepad", "brigadier-cu-123.txt"));
        assert!(!document_title_matches("brigadier-cu-1234.txt", "brigadier-cu-123.txt"));
    }
}
