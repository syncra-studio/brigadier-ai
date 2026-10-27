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
