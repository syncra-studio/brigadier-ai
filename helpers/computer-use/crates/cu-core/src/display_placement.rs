//! Platform-neutral display placement policy.
//!
//! A [`DisplayPolicy`] is an ordered preference list of [`DisplaySelector`]s.
//! Platforms enumerate their active displays into [`DisplayInfo`] records and
//! call [`DisplayPolicy::resolve`] to pick a target, then [`place_rect`] to
//! compute the window frame on that target. The OS-specific move (AX on macOS)
//! stays in the platform crate; everything here is pure and unit-tested.
//!
//! The default policy is `["secondary"]`: when more than one display is
//! attached, windows the driver opens land on the first non-main display so
//! agent work stays off the user's primary display. With a single display it
//! resolves to nothing and windows are left where the OS put them.
//!
//! Config / wire shape: a string or an array of strings. Accepted selectors:
//!
//! | selector            | meaning                                                   |
//! |---------------------|-----------------------------------------------------------|
//! | `none` / `off`      | do not move; stops the list                               |
//! | `main` / `primary`  | the OS main display (macOS: the one with the menu bar)    |
//! | `secondary`         | the first non-main display in [`ordered_displays`] order  |
//! | `index:N`           | Nth display in [`ordered_displays`] order (0 = main)      |
//! | `id:N`              | platform display id (macOS `CGDirectDisplayID`)           |
//! | `uuid:X`            | stable platform display UUID, case-insensitive            |
//!
//! Selectors that match no attached display are skipped, so
//! `["uuid:…", "secondary"]` means "that monitor if connected, else any
//! secondary, else leave it".

use serde_json::Value;

/// Rectangle in global display points, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn intersection_area(&self, other: &Rect) -> f64 {
        let w = (self.x + self.width).min(other.x + other.width) - self.x.max(other.x);
        let h = (self.y + self.height).min(other.y + other.height) - self.y.max(other.y);
        if w > 0.0 && h > 0.0 {
            w * h
        } else {
            0.0
        }
    }
}

/// One active (non-mirrored) display.
#[derive(Clone, Debug, PartialEq)]
pub struct DisplayInfo {
    pub id: u32,
    pub uuid: Option<String>,
    pub bounds: Rect,
    /// Region usable by app windows (excludes menu bar / Dock where the
    /// platform reports them). Falls back to `bounds`.
    pub usable: Rect,
    pub is_main: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DisplaySelector {
    None,
    Main,
    Secondary,
    Index(usize),
    Id(u32),
    Uuid(String),
}

impl DisplaySelector {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let s = raw.trim();
        let lower = s.to_ascii_lowercase();
        match lower.as_str() {
            "none" | "off" => return Ok(Self::None),
            "main" | "primary" => return Ok(Self::Main),
            "secondary" => return Ok(Self::Secondary),
            _ => {}
        }
        if let Some(n) = lower.strip_prefix("index:") {
            return n
                .trim()
                .parse()
                .map(Self::Index)
                .map_err(|_| format!("invalid display index in `{s}`"));
        }
        if let Some(n) = lower.strip_prefix("id:") {
            return n
                .trim()
                .parse()
                .map(Self::Id)
                .map_err(|_| format!("invalid display id in `{s}`"));
        }
        if let Some(u) = lower.strip_prefix("uuid:") {
            let u = u.trim();
            if u.is_empty() {
                return Err(format!("empty display uuid in `{s}`"));
            }
            return Ok(Self::Uuid(u.to_owned()));
        }
        Err(format!(
            "unknown display selector `{s}` (expected none, main, secondary, index:N, id:N, or uuid:X)"
        ))
    }

    pub fn as_config_string(&self) -> String {
        match self {
            Self::None => "none".into(),
            Self::Main => "main".into(),
            Self::Secondary => "secondary".into(),
            Self::Index(n) => format!("index:{n}"),
            Self::Id(n) => format!("id:{n}"),
            Self::Uuid(u) => format!("uuid:{u}"),
        }
    }
}

/// Ordered display preference list. Empty behaves like `none`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DisplayPolicy {
    pub selectors: Vec<DisplaySelector>,
}

impl Default for DisplayPolicy {
    fn default() -> Self {
        Self {
            selectors: vec![DisplaySelector::Secondary],
        }
    }
}

impl DisplayPolicy {
    /// Parse a string or array-of-strings JSON value.
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let raw: Vec<&str> = match value {
            Value::String(s) => vec![s.as_str()],
            Value::Array(items) => items
                .iter()
                .map(|v| {
                    v.as_str()
                        .ok_or_else(|| "display selectors must be strings".to_owned())
                })
                .collect::<Result<_, _>>()?,
            Value::Null => {
                return Ok(Self {
                    selectors: vec![DisplaySelector::None],
                })
            }
            _ => return Err("display policy must be a string or an array of strings".into()),
        };
        let selectors = raw
            .into_iter()
            .map(DisplaySelector::parse)
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { selectors })
    }

    pub fn to_json(&self) -> Value {
        Value::Array(
            self.selectors
                .iter()
                .map(|s| Value::String(s.as_config_string()))
                .collect(),
        )
    }

    /// Pick the target display, or `None` to leave windows where they are.
    pub fn resolve<'a>(&self, displays: &'a [DisplayInfo]) -> Option<&'a DisplayInfo> {
        let ordered = ordered_displays(displays);
        for selector in &self.selectors {
            let hit = match selector {
                DisplaySelector::None => return None,
                DisplaySelector::Main => ordered.iter().find(|d| d.is_main),
                DisplaySelector::Secondary => ordered.iter().find(|d| !d.is_main),
                DisplaySelector::Index(n) => ordered.get(*n),
                DisplaySelector::Id(id) => ordered.iter().find(|d| d.id == *id),
                DisplaySelector::Uuid(u) => ordered
                    .iter()
                    .find(|d| d.uuid.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(u))),
            };
            if let Some(d) = hit {
                return Some(*d);
            }
        }
        None
    }
}

/// Stable display order: main first, then the rest left-to-right,
/// top-to-bottom. `index:N` and `secondary` are defined against this order.
pub fn ordered_displays(displays: &[DisplayInfo]) -> Vec<&DisplayInfo> {
    let mut out: Vec<&DisplayInfo> = displays.iter().collect();
    out.sort_by(|a, b| {
        b.is_main
            .cmp(&a.is_main)
            .then(a.bounds.x.total_cmp(&b.bounds.x))
            .then(a.bounds.y.total_cmp(&b.bounds.y))
            .then(a.id.cmp(&b.id))
    });
    out
}

/// The display that holds most of `window`, if any.
pub fn display_containing<'a>(
    window: &Rect,
    displays: &'a [DisplayInfo],
) -> Option<&'a DisplayInfo> {
    displays
        .iter()
        .map(|d| (d, d.bounds.intersection_area(window)))
        .filter(|(_, area)| *area > 0.0)
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(d, _)| d)
}

/// Frame for `window` moved from `source` onto `target`: keep its offset from
/// the source display's usable origin, shrink it to fit the target's usable
/// region, then clamp it inside that region.
pub fn place_rect(window: &Rect, source: Option<&DisplayInfo>, target: &DisplayInfo) -> Rect {
    let area = target.usable;
    let (dx, dy) = match source {
        Some(s) => (window.x - s.usable.x, window.y - s.usable.y),
        None => (0.0, 0.0),
    };
    let width = window.width.min(area.width);
    let height = window.height.min(area.height);
    let x = (area.x + dx).clamp(area.x, area.x + area.width - width);
    let y = (area.y + dy).clamp(area.y, area.y + area.height - height);
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rect(x: f64, y: f64, width: f64, height: f64) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn display(id: u32, bounds: Rect, is_main: bool) -> DisplayInfo {
        DisplayInfo {
            id,
            uuid: Some(format!("UUID-{id}")),
            bounds,
            usable: bounds,
            is_main,
        }
    }

    fn three_displays() -> Vec<DisplayInfo> {
        vec![
            display(7, rect(1512.0, 0.0, 2560.0, 1440.0), false),
            display(1, rect(0.0, 0.0, 1512.0, 982.0), true),
            display(9, rect(-1920.0, 0.0, 1920.0, 1080.0), false),
        ]
    }

    fn policy(v: Value) -> DisplayPolicy {
        DisplayPolicy::from_json(&v).unwrap()
    }

    #[test]
    fn default_policy_prefers_secondary_and_skips_single_display() {
        let displays = three_displays();
        assert_eq!(DisplayPolicy::default().resolve(&displays).unwrap().id, 9);
        let single = vec![display(1, rect(0.0, 0.0, 1512.0, 982.0), true)];
        assert!(DisplayPolicy::default().resolve(&single).is_none());
    }

    #[test]
    fn ordering_is_main_then_left_to_right() {
        let displays = three_displays();
        let ids: Vec<u32> = ordered_displays(&displays).iter().map(|d| d.id).collect();
        assert_eq!(ids, vec![1, 9, 7]);
        assert_eq!(policy(json!("index:2")).resolve(&displays).unwrap().id, 7);
        assert_eq!(policy(json!("primary")).resolve(&displays).unwrap().id, 1);
    }

    #[test]
    fn preference_list_falls_through_unmatched_selectors() {
        let displays = three_displays();
        let p = policy(json!([
            "uuid:missing",
            "id:404",
            "uuid:uuid-7",
            "secondary"
        ]));
        assert_eq!(p.resolve(&displays).unwrap().id, 7);
        let p = policy(json!(["uuid:missing", "secondary"]));
        assert_eq!(p.resolve(&displays).unwrap().id, 9);
    }

    #[test]
    fn none_stops_the_list() {
        let displays = three_displays();
        assert!(policy(json!(["none", "secondary"]))
            .resolve(&displays)
            .is_none());
        assert!(policy(json!("off")).resolve(&displays).is_none());
        assert!(policy(Value::Null).resolve(&displays).is_none());
        assert!(policy(json!([])).resolve(&displays).is_none());
    }

    #[test]
    fn parse_rejects_garbage_and_round_trips() {
        assert!(DisplayPolicy::from_json(&json!("left")).is_err());
        assert!(DisplayPolicy::from_json(&json!("index:x")).is_err());
        assert!(DisplayPolicy::from_json(&json!(3)).is_err());
        assert!(DisplayPolicy::from_json(&json!([1])).is_err());
        let p = policy(json!(["UUID:ABC", " Secondary ", "id:3", "index:0", "off"]));
        assert_eq!(
            p.to_json(),
            json!(["uuid:abc", "secondary", "id:3", "index:0", "none"])
        );
        assert_eq!(DisplayPolicy::from_json(&p.to_json()).unwrap(), p);
    }

    #[test]
    fn place_rect_keeps_offset_and_fits_target() {
        let displays = three_displays();
        let main = &displays[1];
        let left = &displays[2];
        let win = rect(100.0, 80.0, 800.0, 600.0);
        assert_eq!(display_containing(&win, &displays).unwrap().id, 1);
        assert_eq!(
            place_rect(&win, Some(main), left),
            rect(-1820.0, 80.0, 800.0, 600.0)
        );

        // Too large and too far right for the 1080p target: shrink, then clamp.
        let big = rect(1400.0, 200.0, 2400.0, 1300.0);
        let right = &displays[0];
        assert_eq!(display_containing(&big, &displays).unwrap().id, 7);
        assert_eq!(
            place_rect(&big, Some(right), left),
            rect(-1920.0, 0.0, 1920.0, 1080.0)
        );

        // Unknown source: anchor at the target's usable origin.
        let off = rect(99999.0, 99999.0, 400.0, 300.0);
        assert!(display_containing(&off, &displays).is_none());
        assert_eq!(
            place_rect(&off, None, right),
            rect(1512.0, 0.0, 400.0, 300.0)
        );
    }

    #[test]
    fn place_rect_respects_usable_region() {
        let mut target = display(2, rect(1512.0, 0.0, 1920.0, 1080.0), false);
        target.usable = rect(1512.0, 25.0, 1920.0, 1055.0);
        let main = display(1, rect(0.0, 0.0, 1512.0, 982.0), true);
        let win = rect(0.0, 0.0, 500.0, 2000.0);
        assert_eq!(
            place_rect(&win, Some(&main), &target),
            rect(1512.0, 25.0, 500.0, 1055.0)
        );
    }
}
