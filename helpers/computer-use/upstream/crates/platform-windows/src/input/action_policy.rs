//! Portable identities for exact native button commands.
pub fn common_dialog_accept(dialog_class: &str, button_class: &str, control_id: i32) -> bool {
    dialog_class == "#32770" && button_class.eq_ignore_ascii_case("button") && control_id == 1
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
    fn only_the_common_dialog_default_button_uses_idok() {
        assert!(common_dialog_accept("#32770", "Button", 1));
        assert!(!common_dialog_accept("Notepad", "Button", 1));
        assert!(!common_dialog_accept("#32770", "Edit", 1));
        assert!(!common_dialog_accept("#32770", "Button", 2));
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
