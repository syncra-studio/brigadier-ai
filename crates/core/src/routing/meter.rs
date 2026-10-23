//! Turns a CLI session's running token totals into what each turn used.

use std::sync::{Mutex, PoisonError};

use brigadier_providers::{CallCache, TokenUsage};

use super::store::{BIG_MISS, CacheTrace, LastSession, WriteCause};

/// Both CLIs report a session's totals so far (`ProviderEvent::Usage`); the difference from
/// the previous report is what the turn in between used. A total below the last one means the
/// CLI started counting afresh (a new process for the same session), so it counts whole.
#[derive(Debug, Default)]
pub struct TokenMeter {
    last: Mutex<Baseline>,
    /// When the running turn started, or last reported: what the next report's use took.
    mark: Mutex<Option<i64>>,
    /// A turn started and hasn't reported its use yet.
    opened: Mutex<bool>,
    /// When the session's Codex child threads were last looked for.
    children: Mutex<Option<i64>>,
    /// The extra account the session runs on (its use counts against that account's quota);
    /// absent: the user's own login.
    account: Option<String>,
    /// Why the session started, until its first call is reported (BUILD-PLAN B0).
    start: Mutex<Option<SessionStart>>,
    /// The session's tool set, as a key to tell a later session's change by.
    tools_key: String,
    /// The CLI's version, once the session said.
    cli_version: Mutex<Option<String>>,
    /// The prompt of the session's latest call in its main chain: what a fork of it should
    /// read back.
    last_prompt: Mutex<Option<i64>>,
}

/// Why a session started, and what its first call is compared with (BUILD-PLAN B0).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionStart {
    pub cause: WriteCause,
    /// The prompt the first call should read back (the session it carries on, or the thread
    /// a fork came from); unknown until looked up.
    pub origin_prompt: Option<i64>,
}

impl SessionStart {
    pub fn new(cause: WriteCause) -> Self {
        Self {
            cause,
            origin_prompt: None,
        }
    }
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
            opened: Mutex::new(false),
            children: Mutex::new(None),
            account: None,
            start: Mutex::new(Some(SessionStart::new(if continues {
                WriteCause::Resume
            } else {
                WriteCause::FreshStart
            }))),
            tools_key: String::new(),
            cli_version: Mutex::new(None),
            last_prompt: Mutex::new(None),
        }
    }

    /// The same meter, for a session started for `cause`.
    pub fn starting(self, cause: WriteCause) -> Self {
        self.starting_from(cause, None)
    }

    /// The same, with the prompt its first call should read back when it is known here (a
    /// fork of a live thread).
    pub fn starting_from(self, cause: WriteCause, origin_prompt: Option<i64>) -> Self {
        *self.start.lock().unwrap_or_else(PoisonError::into_inner) = Some(SessionStart {
            cause,
            origin_prompt,
        });
        self
    }

    /// The same meter, for a session with the tool set `key` stands for.
    pub fn with_tools(mut self, key: String) -> Self {
        self.tools_key = key;
        self
    }

    /// The session said which CLI version runs it.
    pub fn session_started(&self, cli_version: Option<&str>) {
        if let Some(version) = cli_version {
            *self
                .cli_version
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(version.to_owned());
        }
    }

    /// Why the session started, while its first call isn't reported yet.
    pub fn pending_start(&self) -> Option<SessionStart> {
        self.start
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// The session's CLI version and tool set together: a later session of the same owner
    /// that differs was an upgrade or a tool change.
    pub fn session_key(&self) -> String {
        let version = self
            .cli_version
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_default();
        format!("{version}|{}", self.tools_key)
    }

    /// The prompt of the session's latest main-chain call.
    pub fn last_prompt(&self) -> Option<i64> {
        *self
            .last_prompt
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Where the reported `calls`' cache writes came from (`used` is what they used): each
    /// call against the prompt before it, a session's first against `last` (its owner's last
    /// session, or the session it was forked from) when its cause carries on one (research
    /// P0.1). The session's cause goes on its first report with calls, refined by `last`: a
    /// resume on another CLI version or tool set is an upgrade or a tool change.
    pub fn trace(
        &self,
        used: &TokenUsage,
        calls: &[CallCache],
        last: Option<&LastSession>,
    ) -> CacheTrace {
        let mut trace = CacheTrace {
            write_1h: used.cache_write_1h_tokens,
            write_5m: used.cache_write_5m_tokens,
            ..CacheTrace::default()
        };
        if calls.is_empty() {
            return trace;
        }
        let start = self
            .start
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        let key = self.session_key();
        if let Some(start) = &start {
            trace.cause = Some(refined(start.cause, last, &key));
            trace.session_key = Some(key);
        }
        let origin = start.as_ref().and_then(|start| {
            start.cause.continues().then(|| {
                start
                    .origin_prompt
                    .or_else(|| last.and_then(|last| last.last_prompt))
            })?
        });
        let mut first = start.is_some();
        for call in calls {
            let opening = first && !call.subagent && call.previous_prompt.is_none();
            let previous = if call.after_compaction {
                None
            } else if opening {
                origin
            } else {
                call.previous_prompt
            };
            match previous {
                Some(previous) => {
                    let missed = (previous - call.cache_read).max(0);
                    trace.missed += missed;
                    if missed > BIG_MISS {
                        trace.missed_calls += 1;
                    }
                    if opening {
                        trace.cause_missed = missed;
                    }
                }
                None => trace.unestimated_calls += 1,
            }
            if opening {
                trace.cause_write = call.cache_write;
                first = false;
            }
            if !call.subagent {
                trace.last_prompt = Some(call.prompt);
            }
        }
        if let Some(prompt) = trace.last_prompt {
            *self
                .last_prompt
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(prompt);
        }
        trace
    }

    /// The same meter, for a session on `account`.
    pub fn on_account(mut self, account: Option<String>) -> Self {
        self.account = account;
        self
    }

    /// The extra account its session runs on.
    pub fn account(&self) -> Option<&str> {
        self.account.as_deref()
    }

    /// A turn started at `at_ms`: its first report's use took from here.
    pub fn turn_started(&self, at_ms: i64) {
        *self.mark.lock().unwrap_or_else(PoisonError::into_inner) = Some(at_ms);
        *self.opened.lock().unwrap_or_else(PoisonError::into_inner) = true;
    }

    /// Whether the use reported now is its turn's first; later reports of the turn aren't.
    pub fn opens_turn(&self) -> bool {
        std::mem::take(&mut *self.opened.lock().unwrap_or_else(PoisonError::into_inner))
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
                model_calls: (total.model_calls - previous.model_calls).max(0),
                cache_write_1h_tokens: (total.cache_write_1h_tokens
                    - previous.cache_write_1h_tokens)
                    .max(0),
                cache_write_5m_tokens: (total.cache_write_5m_tokens
                    - previous.cache_write_5m_tokens)
                    .max(0),
            },
            Baseline::Fresh | Baseline::Seen(_) => total.clone(),
        };
        (sum(&delta) > 0).then_some(delta)
    }
}

fn sum(usage: &TokenUsage) -> i64 {
    usage.input_tokens + usage.cached_input_tokens + usage.cache_write_tokens + usage.output_tokens
}

/// A key for a session's tool set: its built-in tools and its MCP servers by name. Brigadier's
/// own server's tools change with Brigadier's version, which a CLI version change or a new
/// session already covers.
pub fn tools_key(spec: &brigadier_providers::SessionSpec) -> String {
    let servers: Vec<&str> = spec
        .mcp_servers
        .iter()
        .map(|server| server.name.as_str())
        .collect();
    format!("{:?} {}", spec.tools, servers.join(","))
}

/// A plain resume's cause, told apart by the owner's last session (`last`): another CLI
/// version is an upgrade, another tool set a tool change. A cause Brigadier set on purpose
/// (a reroute, a fork…) stands.
fn refined(cause: WriteCause, last: Option<&LastSession>, key: &str) -> WriteCause {
    if cause != WriteCause::Resume {
        return cause;
    }
    let Some((version, tools)) = last
        .and_then(|last| last.session_key.as_deref())
        .and_then(|key| key.split_once('|'))
    else {
        return cause;
    };
    let (now_version, now_tools) = key.split_once('|').unwrap_or((key, ""));
    if !version.is_empty() && !now_version.is_empty() && version != now_version {
        WriteCause::CliUpgrade
    } else if tools != now_tools {
        WriteCause::ToolSetChange
    } else {
        cause
    }
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

    /// BUILD-PLAN A0: a turn's model calls are what the session's count grew by, and a turn
    /// started reports as its turn's first use once.
    #[test]
    fn a_turns_calls_are_what_the_count_grew_by_and_its_first_use_opens_it() {
        let meter = TokenMeter::new(false);
        let calls = |input: i64, calls: i64| TokenUsage {
            model_calls: calls,
            ..usage(input, None)
        };
        assert_eq!(meter.delta(&calls(10, 2), None).unwrap().model_calls, 2);
        assert_eq!(meter.delta(&calls(30, 5), None).unwrap().model_calls, 3);
        meter.turn_started(0);
        assert!(meter.opens_turn());
        assert!(!meter.opens_turn());
    }

    fn call(prompt: i64, read: i64, previous: Option<i64>) -> CallCache {
        CallCache {
            prompt,
            cache_read: read,
            cache_write: prompt - read,
            previous_prompt: previous,
            after_compaction: false,
            subagent: false,
        }
    }

    /// BUILD-PLAN B0: a session's cause goes on its first report with calls only; a fresh
    /// start's first call has nothing to compare with; a resume on another CLI version or tool
    /// set is told as such; a compaction's next call is reported apart.
    #[test]
    fn a_sessions_cause_and_its_calls_misses_are_traced_once() {
        let used = TokenUsage {
            cache_write_tokens: 1_000,
            cache_write_1h_tokens: 600,
            cache_write_5m_tokens: 400,
            ..TokenUsage::default()
        };
        let fresh = TokenMeter::new(false);
        // No calls yet: the cause waits for the first.
        assert_eq!(fresh.trace(&used, &[], None).cause, None);
        let first = fresh.trace(&used, &[call(1_000, 0, None)], None);
        assert_eq!(
            (first.cause, first.unestimated_calls, first.missed),
            (Some(WriteCause::FreshStart), 1, 0)
        );
        assert_eq!((first.write_1h, first.write_5m), (600, 400));
        assert_eq!(fresh.last_prompt(), Some(1_000));
        let next = fresh.trace(&used, &[call(1_500, 1_000, Some(1_000))], None);
        assert_eq!((next.cause, next.missed, next.session_key), (None, 0, None));
        // Resumed on a newer CLI: the first call against the last session's prompt.
        let last = LastSession {
            last_prompt: Some(50_000),
            session_key: Some("2.1.290|Thread brigadier".into()),
        };
        let upgraded = TokenMeter::new(false)
            .starting(WriteCause::Resume)
            .with_tools("Thread brigadier".into());
        upgraded.session_started(Some("2.1.300"));
        let trace = upgraded.trace(&used, &[call(51_000, 1_000, None)], Some(&last));
        assert_eq!(
            (trace.cause, trace.cause_missed, trace.missed_calls),
            (Some(WriteCause::CliUpgrade), 49_000, 1)
        );
        assert_eq!(
            trace.session_key.as_deref(),
            Some("2.1.300|Thread brigadier")
        );
        // The same CLI with another tool set.
        let tools = TokenMeter::new(false)
            .starting(WriteCause::Resume)
            .with_tools("Thread brigadier,computer".into());
        tools.session_started(Some("2.1.290"));
        assert_eq!(
            tools
                .trace(&used, &[call(51_000, 0, None)], Some(&last))
                .cause,
            Some(WriteCause::ToolSetChange)
        );
        // A cause set on purpose stands, and a fork's live origin wins over the stored one.
        let fork = TokenMeter::new(false).starting_from(WriteCause::Handoff, Some(60_000));
        let trace = fork.trace(&used, &[call(61_000, 60_000, None)], Some(&last));
        assert_eq!(
            (trace.cause, trace.cause_missed),
            (Some(WriteCause::Handoff), 0)
        );
        // After a compaction: reported apart, not estimated.
        let compacted = TokenMeter::new(false).starting(WriteCause::Resume);
        let trace = compacted.trace(
            &used,
            &[CallCache {
                after_compaction: true,
                ..call(3_000, 0, Some(90_000))
            }],
            Some(&last),
        );
        assert_eq!((trace.missed, trace.unestimated_calls), (0, 1));
    }

    /// The lifetime split is a delta like the rest.
    #[test]
    fn the_lifetime_split_is_what_the_session_grew_by() {
        let meter = TokenMeter::new(false);
        let total = |hour: i64, five: i64| TokenUsage {
            cache_write_tokens: hour + five,
            cache_write_1h_tokens: hour,
            cache_write_5m_tokens: five,
            ..TokenUsage::default()
        };
        meter.delta(&total(100, 10), None);
        let used = meter.delta(&total(250, 15), None).unwrap();
        assert_eq!(
            (used.cache_write_1h_tokens, used.cache_write_5m_tokens),
            (150, 5)
        );
    }
}
