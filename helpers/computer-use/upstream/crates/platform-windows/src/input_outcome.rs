//! Portable decisions for Windows input readback. A native API return is not an effect.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ControlState {
    pub toggle: Option<i32>,
    pub selected: Option<bool>,
    pub expanded: Option<i32>,
    pub value: Option<String>,
}

fn changed<T: PartialEq>(before: &Option<T>, after: &Option<T>) -> bool {
    matches!((before, after), (Some(a), Some(b)) if a != b)
}

pub fn observed_change(before: &ControlState, after: &ControlState) -> bool {
    changed(&before.toggle, &after.toggle)
        || changed(&before.selected, &after.selected)
        || changed(&before.expanded, &after.expanded)
        || changed(&before.value, &after.value)
}

pub fn summary(route: &str, closed: bool, changed: bool) -> String {
    if closed {
        format!("{route}: confirmed the target window or dialog closed.")
    } else if changed {
        format!("{route}: confirmed a control state change.")
    } else {
        format!("{route}: not confirmed; no window closure or readable control state change was observed. Read fresh state before retrying.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_or_unchanged_readback_never_confirms_a_click() {
        let empty = ControlState::default();
        assert!(!observed_change(&empty, &empty));
        let before = ControlState {
            selected: Some(false),
            ..empty.clone()
        };
        assert!(!observed_change(&before, &empty));
        assert!(!observed_change(&before, &before));
        assert!(observed_change(
            &before,
            &ControlState {
                selected: Some(true),
                ..empty
            }
        ));
        let pending = summary("PostMessage click", false, false);
        assert!(pending.contains("not confirmed"));
        assert!(!pending.contains("✅") && !pending.contains("Performed"));
    }
    #[test]
    fn close_and_save_require_an_observed_effect() {
        assert!(summary("WindowPattern.Close", true, false).contains("confirmed the target"));
        assert!(summary("Invoke Save", false, false).contains("not confirmed"));
        assert!(summary("Toggle", false, true).contains("confirmed a control"));
    }
}

/// Keyboard dispatch has no semantic readback. Neither S_OK nor a queued
/// message proves that an accelerator ran or a key reached the target.
pub fn dispatch_summary(action: &str, route: &str) -> String {
    format!(
        "Dispatched {action} via {route}; effect not confirmed. Read fresh state before retrying."
    )
}

#[cfg(test)]
mod key_tests {
    #[test]
    fn every_unobserved_key_route_reports_uncertainty() {
        for route in ["PostMessage", "SendInput", "UIA accelerator"] {
            let text = super::dispatch_summary("Ctrl+S", route);
            assert!(text.contains("not confirmed"), "{text}");
            assert!(
                !text.contains("✅") && !text.contains("Pressed") && !text.contains("Performed")
            );
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickPattern {
    WindowClose,
    Invoke,
    Toggle,
    Select,
    Expand,
}

pub fn pattern_order(titlebar_close: bool, expand_only: bool) -> &'static [ClickPattern] {
    use ClickPattern::*;
    if expand_only {
        &[Expand]
    } else if titlebar_close {
        &[WindowClose]
    } else {
        &[Invoke, Toggle, Select, Expand]
    }
}

pub fn window_closed(present_before: bool, present_after: bool) -> bool {
    present_before && !present_after
}

#[cfg(test)]
mod routing_tests {
    use super::*;
    #[test]
    fn close_expand_and_generic_click_use_their_own_patterns() {
        use ClickPattern::*;
        assert_eq!(pattern_order(true, false), &[WindowClose]);
        assert_eq!(pattern_order(true, true), &[Expand]);
        assert_eq!(
            pattern_order(false, false),
            &[Invoke, Toggle, Select, Expand]
        );
        assert_eq!(pattern_order(false, true), &[Expand]);
    }
    #[test]
    fn a_preexisting_missing_window_is_not_a_confirmed_close() {
        assert!(!window_closed(false, false));
        assert!(!window_closed(false, true));
        assert!(!window_closed(true, true));
        assert!(window_closed(true, false));
    }
}

pub fn close_button_identity(button: bool, in_titlebar: bool, name: &str, id: &str) -> bool {
    button
        && in_titlebar
        && (name.eq_ignore_ascii_case("close") || id.eq_ignore_ascii_case("close"))
}

#[cfg(test)]
mod titlebar_tests {
    #[test]
    fn a_client_close_button_or_close_tab_never_closes_the_whole_window() {
        use super::close_button_identity;
        assert!(close_button_identity(true, true, "Close", ""));
        assert!(close_button_identity(true, true, "Fermer", "Close"));
        assert!(!close_button_identity(true, false, "Close", "Close"));
        assert!(!close_button_identity(true, true, "Close Tab", "CloseTab"));
        assert!(!close_button_identity(false, true, "Close", "Close"));
    }
}
