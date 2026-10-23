//! The quota monitor's view of each provider: every usage window with a rolling estimate of
//! where it ends up, and how hot the provider runs.
//!
//! The daemon's quota monitor builds these from the CLIs' own reports (Claude's `get_usage`
//! and `rate_limit_event`, Codex's `account/rateLimits/*`) and its sample history.

use brigadier_providers::{LimitHit, ProviderKind, QuotaWindow};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How close a window (or a provider, by its hottest window) is to running out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub enum Heat {
    /// Projected well under the limit at reset.
    Cool,
    /// Projected at 70% or more at reset: routing starts to prefer other providers.
    Warm,
    /// Projected at 90% or more at reset, or 85% used: new work shifts to other providers.
    Hot,
    /// Refusing work (a limit was reached, or 97% used): not routed to until it resets.
    Limited,
}

/// Where a window is heading at its current rate of use.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct Forecast {
    /// Percentage points used per hour lately (the last hour for short windows, the last day
    /// for weekly ones).
    pub rate_per_hour: f64,
    /// Share used by the reset at that rate (may exceed 100).
    pub projected_at_reset: f64,
    /// When it runs out at that rate, if before the reset.
    pub runs_out_at_ms: Option<i64>,
    /// Samples the rate is based on.
    pub samples: u32,
    /// The samples skip an hour or more (Brigadier wasn't running): what was used in that
    /// stretch counts as spread evenly over it, so the rate is a rougher estimate.
    #[serde(default)]
    pub gap: bool,
}

/// One usage window with its estimate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub window: QuotaWindow,
    /// Absent until there are enough samples.
    pub forecast: Option<Forecast>,
    pub heat: Heat,
}

/// A provider's quota as routing sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuota {
    pub provider: ProviderKind,
    pub windows: Vec<WindowState>,
    /// Set while the whole provider refuses work, until it resets (or, for a spend control,
    /// until a fresh read shows it clear).
    pub limit: Option<LimitHit>,
    /// The hottest of its provider-wide windows (windows scoped to one model count only for
    /// that model).
    pub heat: Heat,
    pub observed_at_ms: Option<i64>,
}

/// A provider as the app sees it now: logged in or not, and its quota.
#[derive(Debug, Clone)]
pub struct ProviderState {
    pub provider: ProviderKind,
    /// Installed and logged in.
    pub logged_in: bool,
    /// The quota monitor's view; `None` before the first read.
    pub quota: Option<ProviderQuota>,
}

/// One sample of a window's use, for the Usage page's charts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct QuotaSample {
    pub at_ms: i64,
    pub used_percent: f64,
}
