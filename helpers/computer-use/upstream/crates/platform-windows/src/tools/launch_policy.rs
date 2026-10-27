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
