//! Turns a CLI session's running token totals into what each turn used.

use std::sync::{Mutex, PoisonError};

use brigadier_providers::TokenUsage;

/// Both CLIs report a session's totals so far (`ProviderEvent::Usage`); the difference from
/// the previous report is what the turn in between used. A total below the last one means the
/// CLI started counting afresh (a new process for the same session), so it counts whole.
#[derive(Debug, Default)]
pub struct TokenMeter {
    last: Mutex<Baseline>,
    /// When the running turn started, or last reported: what the next report's use took.
    mark: Mutex<Option<i64>>,
    /// When the session's Codex child threads were last looked for.
    children: Mutex<Option<i64>>,
}

#[derive(Debug, Default)]
enum Baseline {
    /// Nothing reported yet: the first report is all this session used.
    #[default]
    Fresh,
    /// Nothing reported yet, and the first report includes turns counted before: only its
    /// latest request (when the CLI says what that used) is new.
    Continued,
    Seen(TokenUsage),
}

impl TokenMeter {
    /// A meter for a CLI session. `continues` says the CLI resumes a session whose earlier
    /// turns it already counted (a Codex thread's totals span its whole life): its first
    /// report only sets the baseline.
    pub fn new(continues: bool) -> Self {
        Self {
            last: Mutex::new(if continues {
                Baseline::Continued
            } else {
                Baseline::Fresh
            }),
            mark: Mutex::new(None),
            children: Mutex::new(None),
        }
    }

    /// A turn started at `at_ms`: its first report's use took from here.
    pub fn turn_started(&self, at_ms: i64) {
        *self.mark.lock().unwrap_or_else(PoisonError::into_inner) = Some(at_ms);
    }

    /// When the session's child threads were last looked for (never: `None`); now is
    /// remembered as the last time.
    pub fn children_looked(&self, now_ms: i64) -> Option<i64> {
        self.children
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(now_ms)
    }

    /// How long the use reported at `at_ms` took, since the turn started or last reported;
    /// unknown when no turn start was seen.
    pub fn took(&self, at_ms: i64) -> Option<i64> {
        let mut mark = self.mark.lock().unwrap_or_else(PoisonError::into_inner);
        let since = mark.replace(at_ms)?;
        Some((at_ms - since).max(0))
    }

    /// What was used since the last report, if anything. `latest` is what the report's latest
    /// request used, when the CLI says.
    pub fn delta(&self, total: &TokenUsage, latest: Option<&TokenUsage>) -> Option<TokenUsage> {
        let mut last = self.last.lock().unwrap_or_else(PoisonError::into_inner);
        let previous = std::mem::replace(&mut *last, Baseline::Seen(total.clone()));
        let delta = match previous {
            Baseline::Continued => TokenUsage {
                cost_usd: None,
                ..latest?.clone()
            },
            Baseline::Seen(previous) if sum(total) >= sum(&previous) => TokenUsage {
                // Claude's cost is the session's so far too.
                cost_usd: match (total.cost_usd, previous.cost_usd) {
                    (Some(now), Some(before)) if now >= before => Some(now - before),
                    (Some(now), None) => Some(now),
                    _ => None,
                },
                input_tokens: (total.input_tokens - previous.input_tokens).max(0),
                cached_input_tokens: (total.cached_input_tokens - previous.cached_input_tokens)
                    .max(0),
                cache_write_tokens: (total.cache_write_tokens - previous.cache_write_tokens).max(0),
                output_tokens: (total.output_tokens - previous.output_tokens).max(0),
                reasoning_tokens: (total.reasoning_tokens - previous.reasoning_tokens).max(0),
            },
            Baseline::Fresh | Baseline::Seen(_) => total.clone(),
        };
        (sum(&delta) > 0).then_some(delta)
    }
}

fn sum(usage: &TokenUsage) -> i64 {
    usage.input_tokens + usage.cached_input_tokens + usage.cache_write_tokens + usage.output_tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: i64, cost: Option<f64>) -> TokenUsage {
        TokenUsage {
            input_tokens: input,
            cost_usd: cost,
            ..TokenUsage::default()
        }
    }

    #[test]
    fn a_turns_cost_is_what_the_sessions_cost_grew_by() {
        let meter = TokenMeter::new(false);
        let first = meter.delta(&usage(10, Some(0.5)), None).unwrap();
        assert_eq!(first.cost_usd, Some(0.5));
        let second = meter.delta(&usage(25, Some(0.75)), None).unwrap();
        assert_eq!(second.input_tokens, 15);
        assert_eq!(second.cost_usd, Some(0.25));
        // A CLI that doesn't say what it cost.
        let codex = TokenMeter::new(false);
        codex.delta(&usage(10, None), None);
        assert_eq!(codex.delta(&usage(20, None), None).unwrap().cost_usd, None);
        // A resumed session's first report: its cost so far isn't this turn's.
        let resumed = TokenMeter::new(true);
        let latest = usage(4, Some(0.1));
        assert_eq!(
            resumed
                .delta(&usage(40, Some(2.0)), Some(&latest))
                .unwrap()
                .cost_usd,
            None
        );
    }
}
