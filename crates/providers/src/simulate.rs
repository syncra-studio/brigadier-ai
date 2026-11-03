//! Simulated CLI output, in each CLI's real wire format, for checking how the adapters
//! detect and classify events that are hard to trigger on demand.
//!
//! The lines are fed through a fresh [`crate::Replayer`] in an isolated diagnostic session;
//! they never reach a live session's transcript, quota or turn state.

use crate::model::ProviderKind;
use crate::now_ms;

/// A turn that ends because the subscription's usage window ran out; none for a CLI that isn't
/// registered.
pub fn usage_limit(provider: ProviderKind) -> Vec<String> {
    crate::adapters::adapter(provider)
        .map_or_else(Vec::new, |adapter| (adapter.usage_limit)(now_ms() / 1_000))
        .into_iter()
        .map(|line| line.to_string())
        .collect()
}
