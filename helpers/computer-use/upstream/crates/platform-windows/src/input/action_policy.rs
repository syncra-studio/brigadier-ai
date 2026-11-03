//! Portable identities for exact native button commands.
/// The dialog command a common dialog's button sends: IDOK (1) or IDCANCEL (2).
pub fn common_dialog_command(
    dialog_class: &str,
    button_class: &str,
    control_id: i32,
) -> Option<i32> {
    (dialog_class == "#32770"
        && button_class.eq_ignore_ascii_case("button")
        && matches!(control_id, 1 | 2))
    .then_some(control_id)
}

pub fn caption_close(
    button: bool,
    native_hit: isize,
    automation_id: &str,
    top_right: bool,
) -> bool {
    button
        && (native_hit == 20
            || ((automation_id.eq_ignore_ascii_case("CloseButton")
                || automation_id.eq_ignore_ascii_case("Close"))
                && top_right))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_common_dialog_ok_and_cancel_buttons_send_their_command() {
        assert_eq!(common_dialog_command("#32770", "Button", 1), Some(1));
        assert_eq!(common_dialog_command("#32770", "Button", 2), Some(2));
        assert_eq!(common_dialog_command("Notepad", "Button", 1), None);
        assert_eq!(common_dialog_command("#32770", "Edit", 1), None);
        assert_eq!(common_dialog_command("#32770", "Button", 3), None);
    }
    #[test]
    fn native_caption_or_winui_close_identity_is_required() {
        assert!(caption_close(true, 20, "", false));
        assert!(caption_close(true, 1, "CloseButton", true));
        assert!(caption_close(true, 1, "Close", true));
        assert!(!caption_close(true, 1, "Close", false));
        assert!(!caption_close(true, 1, "CloseButton", false));
        assert!(!caption_close(true, 1, "CloseTab", true));
        assert!(!caption_close(false, 20, "CloseButton", true));
    }
}
