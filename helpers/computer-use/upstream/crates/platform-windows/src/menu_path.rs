//! Exact labels for native menu paths, including WinUI's detached popups.
pub fn label_matches(actual: &str, wanted: &str) -> bool {
    fn label(value: &str) -> &str {
        value
            .split('\t')
            .next()
            .unwrap_or(value)
            .trim()
            .trim_end_matches('…')
            .trim_end_matches("...")
            .trim_end()
    }
    label(actual) == label(wanted)
}

pub fn candidate(
    control: &str,
    actual: &str,
    wanted: &str,
    top: bool,
    in_menu: bool,
    in_document: bool,
) -> bool {
    !in_document
        && in_menu
        && matches!(control, "MenuItem" | "Button")
        && (!top || control == "MenuItem" || control == "Button")
        && label_matches(actual, wanted)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn winui_labels_match_without_shortcut_or_ellipsis() {
        assert!(label_matches("Save as…\tCtrl+Shift+S", "Save as"));
        assert!(label_matches("Save as...", "Save as…"));
        assert!(!label_matches("Save", "Save as"));
        assert!(!label_matches("Close tab", "Close window"));
    }
    #[test]
    fn detached_popup_does_not_need_the_menu_bars_ancestry() {
        assert!(candidate(
            "MenuItem", "Save as", "Save as", false, true, false
        ));
        assert!(candidate("Button", "File", "File", true, true, false));
        assert!(!candidate("Text", "File", "File", true, true, false));
        assert!(!candidate("Button", "File", "File", true, false, false));
        assert!(!candidate("MenuItem", "File", "File", true, true, true));
    }
}
