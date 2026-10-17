//! Quota estimates: where each usage window is heading, how hot a provider runs, and what that
//! costs a model's routing score. Pure: the daemon's quota monitor keeps the samples and calls
//! these with the time.
//!
//! - **Rate:** the use over the time it took, over the last hour for windows of 5 hours or less,
//!   the last day for longer ones (a window of unknown length counts as short), each interval
//!   weighted by how recent it is (time constant: a third of the look-back). Weighing use and
//!   time alike keeps one short busy interval from setting the pace (BUILD-PLAN A10).
//!   Samples are taken as a step function (the monitor stores a value when it changes), so the
//!   value at the start of the look-back is the last sample before it. A drop of more than a
//!   point is a reset and that interval is skipped; smaller drops count as no use, and only a
//!   rise past the highest value since the last reset counts as use (Claude's two reports can
//!   round the same use a point apart, so the reading can flicker 14, 15, 14, 15). When the
//!   samples start inside the look-back with no reset among them, the time before the first
//!   counts at the window's own pace so far (its use over the time since it began).
//! - **Forecast:** `projected_at_reset = used + rate × time left`, and "runs out at" when that
//!   passes 100. Absent until the samples span 10 minutes (short windows) or an hour. Samples
//!   more than an hour apart (Brigadier wasn't running) mark it as estimated across a gap.
//! - **Heat:** Limited when a limit names the window or it is 97% used; Hot when projected at 90%
//!   or 85% used; Warm when projected at 70%; else Cool. A provider's heat is its hottest
//!   provider-wide window (Limited while a limit is set); a window scoped to one model counts only
//!   for that model ([`window_applies`]).
//! - **Penalty:** 0 below a projected 70%, rising smoothly (smoothstep) to [`MAX_QUOTA_PENALTY`]
//!   at 100%. A confirmed limit is never a penalty: routing excludes the model outright.

use brigadier_providers::{LimitHit, LimitKind, QuotaSnapshot, QuotaWindow};

use crate::quota::{Forecast, Heat, ProviderQuota, QuotaSample, WindowState};
use crate::registry::{MergedModel, ModelStatus, Registry};
use crate::table;

/// Windows up to this long use the short look-back.
pub const SHORT_WINDOW_MINUTES: i64 = 300;
const SHORT_LOOKBACK_MS: i64 = 60 * 60 * 1000;
const LONG_LOOKBACK_MS: i64 = 24 * 60 * 60 * 1000;
const SHORT_MIN_SPAN_MS: i64 = 10 * 60 * 1000;
const LONG_MIN_SPAN_MS: i64 = 60 * 60 * 1000;
/// A fall of more than this many points between samples is a reset, not use.
const RESET_DROP: f64 = 1.0;
/// Samples further apart than this mean nobody was watching: the monitor stores one at least
/// every half hour while Brigadier runs.
const GAP_MS: i64 = 60 * 60 * 1000;
const HOUR_MS: f64 = 3_600_000.0;

/// Used share at which a window counts as used up.
pub const LIMITED_USED: f64 = 97.0;
pub const HOT_USED: f64 = 85.0;
pub const HOT_PROJECTED: f64 = 90.0;
pub const WARM_PROJECTED: f64 = 70.0;
/// The largest quota penalty, at a projected 100% or more.
pub const MAX_QUOTA_PENALTY: f64 = 6.0;

/// One window's state from its samples (any order; the monitor's history for this window) and
/// its current reading, taken at `observed_at_ms`. `limited`: a provider limit names it.
pub fn window_state(
    window: &QuotaWindow,
    samples: &[QuotaSample],
    observed_at_ms: i64,
    limited: bool,
    now_ms: i64,
) -> WindowState {
    let mut forecast = forecast(window, samples, observed_at_ms, now_ms);
    let projected = forecast.map_or(window.used_percent, |f| f.projected_at_reset);
    let heat = heat(window.used_percent, projected, limited);
    // A window at its limit has run out already: no "runs out at" estimate.
    if heat == Heat::Limited
        && let Some(forecast) = &mut forecast
    {
        forecast.runs_out_at_ms = None;
    }
    WindowState {
        window: window.clone(),
        forecast,
        heat,
    }
}

/// A provider's quota as routing sees it: each window's state, and the provider's limit unless
/// it has expired (a usage window's limit lifts at its reset; a spend control or credits only by
/// a fresh read, which replaces the snapshot). `history` pairs a window id with its samples.
pub fn provider_quota(
    snapshot: &QuotaSnapshot,
    history: &[(&str, &[QuotaSample])],
    now_ms: i64,
) -> ProviderQuota {
    let limit = active_limit(snapshot.limit.as_ref(), now_ms).cloned();
    let windows: Vec<WindowState> = snapshot
        .windows
        .iter()
        .map(|window| {
            let samples = history
                .iter()
                .find(|(id, _)| *id == window.id)
                .map_or(&[][..], |(_, samples)| *samples);
            let limited = limit
                .as_ref()
                .is_some_and(|limit| limit.window.as_deref() == Some(window.id.as_str()));
            window_state(window, samples, snapshot.observed_at_ms, limited, now_ms)
        })
        .collect();
    let heat = if limit.is_some() {
        Heat::Limited
    } else {
        windows
            .iter()
            .filter(|state| state.window.model.is_none())
            .map(|state| state.heat)
            .max()
            .unwrap_or(Heat::Cool)
    };
    ProviderQuota {
        provider: snapshot.provider,
        windows,
        limit,
        heat,
        observed_at_ms: Some(snapshot.observed_at_ms),
    }
}

/// A limit still in force at `now_ms`.
pub fn active_limit(limit: Option<&LimitHit>, now_ms: i64) -> Option<&LimitHit> {
    limit.filter(|limit| {
        !(limit.kind == LimitKind::UsageWindow
            && limit.resets_at_ms.is_some_and(|reset| reset <= now_ms))
    })
}

/// How hot a window runs, from its used and projected shares.
pub fn heat(used: f64, projected: f64, limited: bool) -> Heat {
    if limited || used >= LIMITED_USED {
        Heat::Limited
    } else if projected >= HOT_PROJECTED || used >= HOT_USED {
        Heat::Hot
    } else if projected >= WARM_PROJECTED {
        Heat::Warm
    } else {
        Heat::Cool
    }
}

/// The score penalty for a window projected at `projected` percent at its reset.
pub fn quota_penalty(projected: f64) -> f64 {
    if projected < WARM_PROJECTED {
        return 0.0;
    }
    let t = ((projected - WARM_PROJECTED) / (100.0 - WARM_PROJECTED)).min(1.0);
    MAX_QUOTA_PENALTY * t * t * (3.0 - 2.0 * t)
}

/// Whether a window limits this model: provider-wide windows limit every model; a scoped one
/// names its model by id, by the concrete id an alias resolves to (the registry entry's ids), or
/// by family word (Claude's `opus`, Codex's `gpt-5.6-luna`).
pub fn window_applies(window: &QuotaWindow, model: &MergedModel, registry: &Registry) -> bool {
    let Some(scope) = window.model.as_deref().map(str::trim) else {
        return true;
    };
    if scope.is_empty()
        || scope.eq_ignore_ascii_case(&model.id)
        || model
            .resolved
            .as_deref()
            .is_some_and(|resolved| resolved.eq_ignore_ascii_case(scope))
    {
        return true;
    }
    let entry = model
        .registry_key
        .as_deref()
        .and_then(|key| registry.entry(key));
    // The entry's ids name this model only when it is the entry's own (curated); an inherited
    // newer version is not the model a window for the entry's id limits.
    let own_entry = entry.filter(|_| model.status == ModelStatus::Curated);
    own_entry.is_some_and(|entry| {
        entry
            .matches
            .ids
            .iter()
            .any(|id| id.eq_ignore_ascii_case(scope))
    }) || model
        .family
        .as_deref()
        .is_some_and(|family| family.eq_ignore_ascii_case(scope))
        || (table::words(scope).count() == 1 && table::has_word(&model.id, scope))
}

/// Where a window is heading, if its samples say enough.
fn forecast(
    window: &QuotaWindow,
    samples: &[QuotaSample],
    observed_at_ms: i64,
    now_ms: i64,
) -> Option<Forecast> {
    let short = window
        .window_minutes
        .is_none_or(|minutes| minutes <= SHORT_WINDOW_MINUTES);
    let (lookback, min_span) = if short {
        (SHORT_LOOKBACK_MS, SHORT_MIN_SPAN_MS)
    } else {
        (LONG_LOOKBACK_MS, LONG_MIN_SPAN_MS)
    };
    let cutoff = now_ms - lookback;
    let mut sorted: Vec<QuotaSample> = samples
        .iter()
        .copied()
        .filter(|sample| sample.at_ms <= now_ms && sample.used_percent.is_finite())
        .collect();
    sorted.sort_by_key(|sample| sample.at_ms);

    let mut points: Vec<QuotaSample> = Vec::new();
    let anchor = sorted.iter().rev().find(|sample| sample.at_ms <= cutoff);
    if let Some(anchor) = anchor {
        points.push(QuotaSample {
            at_ms: cutoff,
            used_percent: anchor.used_percent,
        });
    }
    points.extend(sorted.iter().filter(|sample| sample.at_ms > cutoff));
    if observed_at_ms > cutoff && observed_at_ms <= now_ms {
        match points.last() {
            Some(last) if last.at_ms > observed_at_ms => {}
            Some(last) if last.at_ms == observed_at_ms => {
                points.pop();
                points.push(QuotaSample {
                    at_ms: observed_at_ms,
                    used_percent: window.used_percent,
                });
            }
            _ => points.push(QuotaSample {
                at_ms: observed_at_ms,
                used_percent: window.used_percent,
            }),
        }
    }
    let (first, last) = (points.first()?, points.last()?);
    if points.len() < 2 || last.at_ms - first.at_ms < min_span {
        return None;
    }

    let tau = lookback as f64 / 3.0;
    // How much an interval ending `to` and lasting `dt` counts: recent ones more.
    let weight = |to: i64, dt: f64| (-((now_ms - to) as f64 + dt / 2.0) / tau).exp();
    let (mut use_sum, mut time_sum) = (0.0, 0.0);
    let mut reset = false;
    // The highest reading since the last reset: a flicker back up to it is not new use.
    let mut peak = first.used_percent;
    for pair in points.windows(2) {
        let dt = (pair[1].at_ms - pair[0].at_ms) as f64;
        if pair[1].used_percent - pair[0].used_percent < -RESET_DROP {
            peak = pair[1].used_percent;
            reset = true;
            continue;
        }
        if dt <= 0.0 {
            continue;
        }
        let used = (pair[1].used_percent - peak).max(0.0);
        peak = peak.max(pair[1].used_percent);
        let w = weight(pair[1].at_ms, dt);
        use_sum += w * used;
        time_sum += w * dt / HOUR_MS;
    }
    let used = window.used_percent;
    let hours_left = window
        .resets_at_ms
        .map(|reset| ((reset - now_ms).max(0)) as f64 / HOUR_MS);
    // The look-back's time before the first sample, at the window's own pace so far.
    let began_hours_ago = window
        .window_minutes
        .zip(hours_left)
        .map(|(minutes, left)| minutes as f64 / 60.0 - left)
        .filter(|hours| *hours > 0.0);
    if let Some(began) = began_hours_ago
        && !reset
    {
        let gap = ((first.at_ms - cutoff) as f64 / HOUR_MS).min(began);
        if gap > 0.0 {
            let w = weight(first.at_ms, gap * HOUR_MS);
            use_sum += w * gap * (used / began);
            time_sum += w * gap;
        }
    }
    if time_sum <= 0.0 {
        return None;
    }
    let rate = use_sum / time_sum;
    let projected = used + rate * hours_left.unwrap_or(0.0);
    let runs_out_at_ms = (rate > 0.0 && projected > 100.0).then(|| {
        let hours = ((100.0 - used) / rate).max(0.0);
        now_ms + (hours * HOUR_MS) as i64
    });
    Some(Forecast {
        rate_per_hour: rate,
        projected_at_reset: projected,
        runs_out_at_ms,
        samples: u32::try_from(points.len()).unwrap_or(u32::MAX),
        // From the readings' own times: the anchor's, not the look-back's start.
        gap: anchor
            .map(|anchor| anchor.at_ms)
            .into_iter()
            .chain(
                points
                    .iter()
                    .skip(usize::from(anchor.is_some()))
                    .map(|p| p.at_ms),
            )
            .collect::<Vec<_>>()
            .windows(2)
            .any(|pair| pair[1] - pair[0] > GAP_MS),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weekly(used_percent: f64) -> QuotaWindow {
        QuotaWindow {
            id: "seven_day".into(),
            label: "Weekly".into(),
            used_percent,
            resets_at_ms: Some(1_791_370_800_000),
            window_minutes: Some(10_080),
            bucket: None,
            model: None,
        }
    }

    fn samples(points: &[(i64, f64)]) -> Vec<QuotaSample> {
        points
            .iter()
            .map(|&(at_ms, used_percent)| QuotaSample {
                at_ms,
                used_percent,
            })
            .collect()
    }

    /// A real weekly window from 30 September: reset to 14% in the evening, then read back and
    /// forth between 14 and 15 as Claude's two reports rounded it differently. Only one point
    /// was used in that half hour, with six and a half days to go: not hot.
    #[test]
    fn a_reading_flickering_by_a_point_is_not_use() {
        let history = samples(&[
            (1_790_718_686_931, 47.0),
            (1_790_720_487_202, 47.0),
            (1_790_755_665_080, 52.0),
            (1_790_793_828_688, 14.0),
            (1_790_800_053_934, 14.0),
            (1_790_800_787_975, 13.0),
            (1_790_800_893_187, 14.000_000_000_000_002),
            (1_790_802_154_694, 15.0),
            (1_790_802_218_185, 14.000_000_000_000_002),
            (1_790_802_456_611, 15.0),
            (1_790_802_532_777, 14.000_000_000_000_002),
            (1_790_802_631_427, 15.0),
        ]);
        let now = 1_790_802_631_427;
        let state = window_state(&weekly(15.0), &history, now, false, now);
        let forecast = state.forecast.expect("a day of samples gives a forecast");
        assert!(
            forecast.projected_at_reset < HOT_PROJECTED,
            "projected {}",
            forecast.projected_at_reset
        );
        assert!(state.heat < Heat::Hot, "heat {:?}", state.heat);
    }

    /// Steady use still counts in full, and a reset still starts the count again.
    #[test]
    fn a_steady_rise_counts_across_a_reset() {
        let hour = 3_600_000;
        let start = 1_790_700_000_000;
        let history = samples(&[
            (start, 80.0),
            (start + hour, 2.0),
            (start + 2 * hour, 4.0),
            (start + 3 * hour, 6.0),
        ]);
        let now = start + 3 * hour;
        let state = window_state(&weekly(6.0), &history, now, false, now);
        let forecast = state
            .forecast
            .expect("three hours of samples give a forecast");
        assert!(
            (forecast.rate_per_hour - 2.0).abs() < 1e-9,
            "rate {}",
            forecast.rate_per_hour
        );
    }

    /// One busy moment is not the week's pace (BUILD-PLAN A10): a point used in two minutes,
    /// with nothing used in the hour since and five days to the weekly reset, once read as a
    /// window "projected at" thousands of percent and sent work to the other vendor.
    #[test]
    fn one_busy_moment_does_not_project_the_week() {
        let hour = 3_600_000;
        let now = weekly(21.0).resets_at_ms.unwrap() - 5 * 24 * hour;
        let history = samples(&[(now - hour, 20.0), (now - hour + 120_000, 21.0)]);
        let state = window_state(&weekly(21.0), &history, now, false, now);
        let forecast = state.forecast.expect("an hour of samples gives a forecast");
        assert!(
            forecast.projected_at_reset < HOT_PROJECTED,
            "projected {}",
            forecast.projected_at_reset
        );
        // A busy hour after a quiet day, the same.
        let history = samples(&[
            (now - 30 * hour, 17.0),
            (now - 45 * 60_000, 18.0),
            (now - 30 * 60_000, 19.0),
            (now - 15 * 60_000, 20.0),
            (now, 21.0),
        ]);
        let state = window_state(&weekly(21.0), &history, now, false, now);
        let forecast = state.forecast.expect("a day of samples gives a forecast");
        assert!(
            forecast.projected_at_reset < HOT_PROJECTED,
            "projected {}",
            forecast.projected_at_reset
        );
        // A whole day at two points an hour is the pace, and runs hot.
        let history: Vec<QuotaSample> = (0..=48)
            .map(|step| QuotaSample {
                at_ms: now - 24 * hour + step * hour / 2,
                used_percent: 20.0 + step as f64,
            })
            .collect();
        let state = window_state(&weekly(68.0), &history, now, false, now);
        let forecast = state.forecast.expect("a day of samples gives a forecast");
        assert!(
            (forecast.rate_per_hour - 2.0).abs() < 1e-6,
            "rate {}",
            forecast.rate_per_hour
        );
        // Readings every half hour leave no gap.
        assert!(!forecast.gap);
    }

    /// A short window read three hours apart (Brigadier closed in between) is marked as an
    /// estimate across a gap, though its look-back is only the last hour.
    #[test]
    fn a_short_window_across_a_restart_has_a_gap() {
        let hour = 3_600_000;
        let now = 10 * hour;
        let window = QuotaWindow {
            id: "five_hour".into(),
            label: "5-hour".into(),
            used_percent: 70.0,
            resets_at_ms: Some(now + 2 * hour),
            window_minutes: Some(300),
            bucket: None,
            model: None,
        };
        let history = samples(&[(now - 3 * hour, 10.0), (now - 20 * 60_000, 70.0)]);
        let state = window_state(&window, &history, now, false, now);
        assert!(state.forecast.expect("a forecast").gap);
        assert_eq!(state.heat, Heat::Hot);
    }
}
