//! How the agent cursor moves by default.
//!
//! Brigadier's agent cursor has always glided at its own speed, so a short
//! hop between two buttons takes a fraction of a second. The driver's
//! built-in default gives every move at least 600 ms, and an action waits
//! for its cursor to arrive. The classic glide at its natural speed is
//! therefore saved as the default, but only where none is saved yet, so a
//! choice made through `set_config` stays.

use std::path::Path;

use cursor_overlay::motion_defaults::{self, SavedMotionDefaults, STYLE_KEY, TIMING_KEY};
use serde_json::{json, Value};

/// The style and timing Brigadier's cursor uses unless one is saved.
const DEFAULTS: [(&str, &str); 2] = [(STYLE_KEY, "classic"), (TIMING_KEY, "native")];

/// The defaults still to save, given what is saved now.
fn missing(saved: &SavedMotionDefaults) -> Vec<(&'static str, Value)> {
    DEFAULTS
        .iter()
        .filter(|(key, _)| match *key {
            STYLE_KEY => saved.style.is_none(),
            _ => saved.timing.is_none(),
        })
        .map(|(key, value)| (*key, json!(value)))
        .collect()
}

/// Save Brigadier's cursor motion as the default where the driver's
/// settings (under its state folder) name none. Call it after
/// [`crate::configure_environment`] and before the registry is built.
/// Without a state folder (a test process, say) nothing is written.
pub fn seed_default() {
    if std::env::var_os("CUA_DRIVER_STATE_DIR").is_none_or(|dir| dir.is_empty()) {
        return;
    }
    let Some(path) = motion_defaults::default_config_path() else {
        return;
    };
    seed_at(&path);
}

fn seed_at(path: &Path) {
    for (key, value) in missing(&motion_defaults::load_from(path)) {
        if let Err(error) = motion_defaults::set_key(key, &value) {
            tracing::warn!(%error, key, "could not save the agent cursor's motion");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cursor_overlay::motion::{MotionStyle, MotionTiming};

    #[test]
    fn nothing_saved_takes_the_classic_glide_at_its_own_speed() {
        let todo = missing(&SavedMotionDefaults::default());
        assert_eq!(
            todo,
            vec![(STYLE_KEY, json!("classic")), (TIMING_KEY, json!("native"))]
        );
    }

    #[test]
    fn a_saved_choice_stays() {
        let saved = SavedMotionDefaults {
            style: Some(MotionStyle::Magnetic),
            timing: Some(MotionTiming::Calm),
            ..Default::default()
        };
        assert!(missing(&saved).is_empty());
        let style_only = SavedMotionDefaults {
            style: Some(MotionStyle::Magnetic),
            ..Default::default()
        };
        assert_eq!(missing(&style_only), vec![(TIMING_KEY, json!("native"))]);
    }

    #[test]
    fn the_values_are_ones_the_driver_accepts() {
        let path = std::env::temp_dir()
            .join(format!("cu-layer-motion-{}", std::process::id()))
            .join("config.json");
        for (key, value) in DEFAULTS {
            motion_defaults::set_key_at(&path, key, &json!(value)).expect(key);
        }
        let saved = motion_defaults::load_from(&path);
        assert_eq!(saved.style, Some(MotionStyle::Classic));
        assert_eq!(saved.timing, Some(MotionTiming::Native));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
