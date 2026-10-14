//! How the agent cursor moves by default.
//!
//! Brigadier's agent cursor has always glided quickly, so a short hop
//! between two buttons takes a fraction of a second. An action waits for
//! its cursor to arrive, and the driver's built-in default gives every move
//! at least 600 ms. The classic glide at its own speed is slower still: its
//! path loops round to arrive pointing up-left, so a 30-point hop takes
//! over a second. The classic glide timed by the target's distance and size
//! (Fitts' law: 0.3 s for a hop, under 0.8 s across the screen) is
//! therefore saved as the default, but only where none is saved yet, so a
//! choice made through `set_config` stays.

use std::path::Path;

use cursor_overlay::motion_defaults::{self, SavedMotionDefaults, STYLE_KEY, TIMING_KEY};
use serde_json::{json, Value};

/// The style and timing Brigadier's cursor uses unless one is saved.
const DEFAULTS: [(&str, &str); 2] = [(STYLE_KEY, "classic"), (TIMING_KEY, "fitts")];

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
    fn nothing_saved_takes_the_classic_glide_timed_by_distance() {
        let todo = missing(&SavedMotionDefaults::default());
        assert_eq!(
            todo,
            vec![(STYLE_KEY, json!("classic")), (TIMING_KEY, json!("fitts"))]
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
        assert_eq!(missing(&style_only), vec![(TIMING_KEY, json!("fitts"))]);
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
        assert_eq!(saved.timing, Some(MotionTiming::Fitts));
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn the_saved_glide_arrives_quickly_in_every_direction() {
        use cua_cursor_motion::{plan_move, MotionParams, MoveRequest, Pt};
        let params = MotionParams {
            style: MotionStyle::Classic,
            timing: MotionTiming::Fitts,
            ..MotionParams::default()
        };
        let from = Pt::new(500.0, 500.0);
        let arrival = |dx: f64, dy: f64| {
            let to = Pt::new(from.x + dx, from.y + dy);
            plan_move(
                &params,
                &MoveRequest {
                    from,
                    from_heading: std::f64::consts::FRAC_PI_4,
                    to,
                    end_heading: std::f64::consts::FRAC_PI_4,
                    target: Some([to.x - 10.0, to.y - 10.0, 20.0, 20.0]),
                    seed: "test|0".into(),
                    reduced_motion: false,
                },
            )
            .arrival_t
        };
        for (dx, dy) in [(30.0, 0.0), (0.0, 30.0), (-30.0, 0.0), (20.0, -20.0)] {
            assert!(arrival(dx, dy) < 0.35, "hop ({dx},{dy})");
        }
        for (dx, dy) in [(140.0, 140.0), (0.0, 300.0), (-300.0, 0.0), (1300.0, 100.0)] {
            assert!(arrival(dx, dy) < 0.8, "move ({dx},{dy})");
        }
    }
}
