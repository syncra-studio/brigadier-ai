//! Reading the restrictions Brigadier enforces from the user's words (PLAN.md §10.4).
//!
//! Only the deadline, which phases run and the worker cap are executable; quality settings
//! (effort, Fable, hand-off size) are listed as ignored, and every other word is Rules. The
//! parser is deterministic, takes the current time and time zone as inputs, and reads only
//! the user's own prose: code, block quotes and quoted text are skipped, so a deadline inside
//! an example never becomes one.

use jiff::civil::{Date, DateTime, Time};
use jiff::tz::{AmbiguousOffset, Offset, TimeZone};
use jiff::{Timestamp, ToSpan};

use crate::overnight::{
    Deadline, DirectiveKind, DirectiveProblem, DirectiveSpan, Directives, PhaseRange, ResolvedTime,
    StopAfter,
};

/// "By morning".
const MORNING: (i8, i8) = (7, 0);
/// The shortest run worth starting: preparation plus a clean ending.
pub const MIN_RUN_MINUTES: i64 = 15;
/// The longest duration or cap read as meant (a typo beyond it is a problem, not a run).
const MAX_HOURS: i64 = 72;
const MAX_WORKERS: u32 = 32;

/// What the user's words say, before it is checked against the plan.
#[derive(Debug, Clone, Default)]
pub struct Parsed {
    pub directives: Directives,
    pub problems: Vec<DirectiveProblem>,
    /// Kinds the words set, and whether they said "instead".
    pub set: Vec<(DirectiveKind, bool)>,
    /// "Stop after this phase": bound to the running phase when applied.
    pub stop_after_this: bool,
}

impl Parsed {
    fn says(&self, kind: DirectiveKind) -> Option<bool> {
        self.set
            .iter()
            .find(|(set, _)| *set == kind)
            .map(|(_, instead)| *instead)
    }
}

/// The current time and time zone, injected so a check can fix them.
#[derive(Debug, Clone)]
pub struct Clock {
    pub now: Timestamp,
    pub tz: TimeZone,
}

impl Clock {
    pub fn system() -> Self {
        Self {
            now: Timestamp::now(),
            tz: TimeZone::system(),
        }
    }
}

/// The intent is read only from the user's prose, never examples/code/quoted sources.
pub(crate) fn unattended(text: &str) -> bool {
    let prose = String::from_utf8_lossy(&mask(text)).to_lowercase();
    if prose.split_whitespace().next() == Some("/overnight") {
        return true;
    }
    // Words without the punctuation around them ("tonight," "tonight:").
    let words: Vec<_> = prose
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !c.is_alphanumeric()))
        .collect();
    let night = words
        .iter()
        .any(|word| matches!(*word, "tonight" | "overnight" | "unattended"));
    let deadline = || {
        parse(text, &Clock::system())
            .set
            .iter()
            .any(|(kind, _)| *kind == DirectiveKind::Deadline)
    };
    (night
        && (words.iter().any(|word| {
            matches!(
                *word,
                "work" | "run" | "implement" | "build" | "finish" | "continue"
            )
        }) || deadline()))
        || prose.contains("by morning")
        || (words
            .iter()
            .any(|word| matches!(*word, "work" | "run" | "implement" | "build" | "finish"))
            && deadline())
}

pub(crate) fn continuation(text: &str) -> bool {
    let prose = String::from_utf8_lossy(&mask(text)).trim().to_lowercase();
    prose == "continue" || prose.starts_with("continue ")
}

/// Reads the restrictions in `text`.
pub fn parse(text: &str, clock: &Clock) -> Parsed {
    let prose = mask(text);
    let tokens = tokenize(&prose);
    let mut found = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match read_at(&tokens, i, clock) {
            Some(hit) => {
                i = hit.end;
                found.push(hit);
            }
            None => i += 1,
        }
    }
    combine(text, &tokens, found)
}

/// The directives after the user's new words during a proposal or a run. A kind the new words
/// set replaces the current one when they say "instead", when none was set, or when the new
/// one is stricter (an earlier deadline, a lower cap); otherwise the current one stays and the
/// conflict is a problem. `running` is the phase running now, for "stop after this phase".
pub fn steer(
    current: &Directives,
    words: &str,
    clock: &Clock,
    running: Option<&str>,
) -> (Directives, Vec<DirectiveProblem>) {
    let parsed = parse(words, clock);
    let mut next = current.clone();
    let mut problems = parsed.problems.clone();
    let new = &parsed.directives;
    if let Some(instead) = parsed.says(DirectiveKind::Deadline) {
        let replace = instead
            || current.deadline == Deadline::UntilDone
            || earlier(&new.deadline, &current.deadline, clock).unwrap_or(false);
        if replace {
            next.deadline = new.deadline.clone();
        } else if new.deadline != current.deadline {
            let said = new
                .spans
                .iter()
                .rev()
                .find(|span| span.kind == DirectiveKind::Deadline)
                .map_or_else(|| describe(&new.deadline), |span| span.text.clone());
            problems.push(conflict(
                DirectiveKind::Deadline,
                &format!(
                    "The report is already due {}. To change that, say \"{said} instead\".",
                    match &current.deadline {
                        Deadline::At { time } => format!("at {} ({})", time.local_time, time.day),
                        other => describe(other),
                    }
                ),
            ));
        }
    }
    if let Some(instead) = parsed.says(DirectiveKind::StopAfter) {
        let wanted = match (parsed.stop_after_this, running) {
            (true, Some(phase)) => Some(StopAfter::Current {
                phase_id: phase.to_owned(),
            }),
            (true, None) => {
                problems.push(conflict(
                    DirectiveKind::StopAfter,
                    "No phase is running yet. Say \"stop after phase N\" instead.",
                ));
                None
            }
            (false, _) => new.stop_after.clone(),
        };
        if let Some(wanted) = wanted {
            if instead || current.stop_after.is_none() || current.stop_after == Some(wanted.clone())
            {
                next.stop_after = Some(wanted);
            } else {
                problems.push(conflict(
                    DirectiveKind::StopAfter,
                    "A stop is already set. To change it, add \"instead\".",
                ));
            }
        }
    }
    if let Some(instead) = parsed.says(DirectiveKind::Only) {
        if instead || current.only.is_none() || current.only == new.only {
            next.only = new.only;
        } else {
            problems.push(conflict(
                DirectiveKind::Only,
                "Phases are already limited. To change which run, add \"instead\".",
            ));
        }
    }
    for number in &new.skip {
        if !next.skip.contains(number) {
            next.skip.push(*number);
        }
    }
    if let Some(instead) = parsed.says(DirectiveKind::MaxWorkers) {
        let lower = match (new.max_workers, current.max_workers) {
            (Some(new), Some(old)) => new <= old,
            (Some(_), None) => true,
            _ => false,
        };
        if instead || lower {
            next.max_workers = new.max_workers;
        } else {
            problems.push(conflict(
                DirectiveKind::MaxWorkers,
                &format!(
                    "The run already uses at most {} workers. To allow more, say \"max {} workers instead\".",
                    current.max_workers.unwrap_or_default(),
                    new.max_workers.unwrap_or_default()
                ),
            ));
        }
    }
    for line in &new.ignored {
        if !next.ignored.contains(line) {
            next.ignored.push(line.clone());
        }
    }
    next.spans.extend(new.spans.iter().cloned());
    (next, problems)
}

/// What a phase of the plan needs to be checked against.
#[derive(Debug, Clone)]
pub struct PhaseInfo {
    pub number: u32,
    pub depends_on: Vec<u32>,
}

/// Problems with `directives` for a plan with `phases` (`None`: a bare goal whose phases are
/// still to be written, so phase restrictions wait for them). `verified`: phases an earlier
/// segment verified, which a selected phase may build on without running them again.
/// `starting`: the run hasn't started, so its deadline must leave time for a whole run;
/// a started run only refuses a deadline that has passed.
pub fn check(
    directives: &Directives,
    phases: Option<&[PhaseInfo]>,
    verified: &[u32],
    clock: &Clock,
    starting: bool,
) -> Vec<DirectiveProblem> {
    let mut problems = Vec::new();
    match &directives.deadline {
        Deadline::At { time } => {
            let left = (time.at_ms - clock.now.as_millisecond()) / 60_000;
            if left < 0 {
                problems.push(conflict(
                    DirectiveKind::Deadline,
                    &format!(
                        "{} {} has already passed. Say a later time.",
                        time.day, time.local_time
                    ),
                ));
            } else if starting && left < MIN_RUN_MINUTES {
                problems.push(conflict(
                    DirectiveKind::Deadline,
                    &format!(
                        "{} leaves only {left} minutes, too little to work and write the report. Say a later time.",
                        time.local_time
                    ),
                ));
            }
        }
        Deadline::For { minutes } if starting && i64::from(*minutes) < MIN_RUN_MINUTES => {
            problems.push(conflict(
                DirectiveKind::Deadline,
                &format!(
                    "{minutes} minutes is too little to work and write the report. Say a longer time."
                ),
            ));
        }
        _ => {}
    }
    if directives.max_workers == Some(0) {
        problems.push(conflict(
            DirectiveKind::MaxWorkers,
            "\"max 0 workers\" would never start anything. Say 1 or more.",
        ));
    }
    let Some(phases) = phases else {
        return problems;
    };
    let known = |number: u32| phases.iter().any(|phase| phase.number == number);
    let numbers = || {
        phases
            .iter()
            .map(|phase| phase.number.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let selected = |number: u32| {
        directives
            .only
            .is_none_or(|range| (range.from..=range.to).contains(&number))
            && !directives.skip.contains(&number)
    };
    if let Some(range) = directives.only {
        if range.from > range.to {
            problems.push(conflict(
                DirectiveKind::Only,
                &format!(
                    "\"phases {}–{}\" runs backwards. Say \"only phases {}–{}\".",
                    range.from, range.to, range.to, range.from
                ),
            ));
        } else if !phases
            .iter()
            .any(|phase| (range.from..=range.to).contains(&phase.number))
        {
            problems.push(conflict(
                DirectiveKind::Only,
                &format!(
                    "The plan has no phases {}–{}. Its phases are {}.",
                    range.from,
                    range.to,
                    numbers()
                ),
            ));
        }
    }
    for number in &directives.skip {
        if !known(*number) {
            problems.push(conflict(
                DirectiveKind::Skip,
                &format!(
                    "The plan has no phase {number} to skip. Its phases are {}.",
                    numbers()
                ),
            ));
        }
    }
    if let Some(StopAfter::Phase { number }) = &directives.stop_after {
        if !known(*number) {
            problems.push(conflict(
                DirectiveKind::StopAfter,
                &format!(
                    "The plan has no phase {number} to stop after. Its phases are {}.",
                    numbers()
                ),
            ));
        } else if !selected(*number) {
            problems.push(conflict(
                DirectiveKind::StopAfter,
                &format!(
                    "Phase {number} won't run, so the run can't stop after it. Say which phase to stop after."
                ),
            ));
        }
    }
    for phase in phases.iter().filter(|phase| selected(phase.number)) {
        for needed in &phase.depends_on {
            if !selected(*needed) && known(*needed) && !verified.contains(needed) {
                problems.push(conflict(
                    DirectiveKind::Skip,
                    &format!(
                        "Phase {} builds on phase {needed}, which wouldn't run. Include phase {needed}, or leave out phase {} too.",
                        phase.number, phase.number
                    ),
                ));
            }
        }
    }
    let range_reported = problems
        .iter()
        .any(|problem| problem.kind == DirectiveKind::Only);
    if !range_reported && !phases.is_empty() && !phases.iter().any(|phase| selected(phase.number)) {
        problems.push(conflict(
            DirectiveKind::Only,
            "These restrictions leave no phase to run.",
        ));
    }
    problems
}

/// Turns a duration deadline into a time, at Start.
pub fn resolve_duration(minutes: u32, clock: &Clock) -> Option<ResolvedTime> {
    let at = clock.now.checked_add(i64::from(minutes).minutes()).ok()?;
    Some(resolved(at, &clock.tz))
}

/// The deadline in a few words, for messages.
pub fn describe(deadline: &Deadline) -> String {
    match deadline {
        Deadline::UntilDone => "until done".into(),
        Deadline::At { time } => format!("until {} ({})", time.local_time, time.day),
        Deadline::For { minutes } if minutes % 60 == 0 => match minutes / 60 {
            1 => "for 1 hour".into(),
            hours => format!("for {hours} hours"),
        },
        Deadline::For { minutes } => format!("for {minutes} minutes"),
    }
}

// ----- reading ------------------------------------------------------------------------------

/// `text` with everything that isn't the user's own prose blanked out, byte for byte (so
/// offsets still point into `text`): fenced and inline code, block-quote lines and quoted
/// text.
fn mask(text: &str) -> Vec<u8> {
    let mut bytes = text.as_bytes().to_vec();
    let blank = |bytes: &mut [u8], start: usize, end: usize| {
        let end = end.min(bytes.len());
        for byte in &mut bytes[start..end] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    };
    // Fenced code and block quotes, line by line.
    let mut fence: Option<&str> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let start = offset;
        offset += line.len();
        let marker = ["```", "~~~"]
            .into_iter()
            .find(|marker| trimmed.starts_with(marker));
        match (fence, marker) {
            (None, Some(marker)) => {
                fence = Some(marker);
                blank(&mut bytes, start, offset);
            }
            (Some(open), Some(marker)) if open == marker => {
                fence = None;
                blank(&mut bytes, start, offset);
            }
            (Some(_), _) => blank(&mut bytes, start, offset),
            (None, None) if trimmed.starts_with('>') => blank(&mut bytes, start, offset),
            (None, None) => {}
        }
    }
    // Inline code and quoted text: from an opening mark to its closing one on the same line.
    let pairs: [(&str, &str); 4] = [("`", "`"), ("\"", "\""), ("“", "”"), ("„", "“")];
    let mut i = 0;
    while i < bytes.len() {
        let rest = &bytes[i..];
        let Some((open, close)) = pairs
            .iter()
            .find(|(open, _)| rest.starts_with(open.as_bytes()))
        else {
            i += 1;
            continue;
        };
        let body = i + open.len();
        let line_end = bytes[body..]
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(bytes.len(), |at| body + at);
        match bytes[body..line_end]
            .windows(close.len())
            .position(|window| window == close.as_bytes())
        {
            Some(at) => {
                let end = body + at + close.len();
                blank(&mut bytes, i, end);
                i = end;
            }
            None => i = body,
        }
    }
    bytes
}

#[derive(Debug, Clone)]
struct Token {
    /// Lowercase.
    text: String,
    start: usize,
    end: usize,
}

/// Words, numbers and times (`07:30`, `2026-10-03`, `1-3`, `80%`, `+03:00`), and dashes and
/// commas as tokens of their own. Sentence ends become `.` tokens, which no pattern crosses.
fn tokenize(prose: &[u8]) -> Vec<Token> {
    let text = String::from_utf8_lossy(prose);
    let mut tokens = Vec::new();
    let mut chars = text.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if c.is_alphanumeric() || c == '+' {
            let mut end = start + c.len_utf8();
            while let Some(&(at, next)) = chars.peek() {
                let inner = matches!(next, ':' | '.' | '-' | '\'' | '’');
                let keep = next.is_alphanumeric()
                    || next == '%'
                    || (inner
                        && text[at + next.len_utf8()..]
                            .chars()
                            .next()
                            .is_some_and(char::is_alphanumeric));
                if !keep {
                    break;
                }
                end = at + next.len_utf8();
                chars.next();
            }
            tokens.push(Token {
                text: text[start..end].to_lowercase(),
                start,
                end,
            });
        } else if matches!(c, '–' | '—' | '-' | ',') {
            tokens.push(Token {
                text: if c == ',' { ",".into() } else { "-".into() },
                start,
                end: start + c.len_utf8(),
            });
        } else if matches!(c, '.' | '!' | '?' | ';' | '\n') {
            tokens.push(Token {
                text: ".".into(),
                start,
                end: start + c.len_utf8(),
            });
        }
    }
    tokens
}

#[derive(Debug, Clone)]
enum Value {
    Deadline(Deadline),
    StopPhase(u32),
    StopThis,
    Only(PhaseRange),
    Skip(Vec<u32>),
    Max(u32),
    Ignored(String),
    /// A deadline that can't be used as said (a time that doesn't exist that day, or happens
    /// twice): the problem, in words.
    BadDeadline(String),
}

#[derive(Debug, Clone)]
struct Hit {
    kind: DirectiveKind,
    value: Value,
    /// Token range `[start, end)`.
    start: usize,
    end: usize,
    instead: bool,
}

fn word(tokens: &[Token], i: usize) -> &str {
    tokens.get(i).map_or("", |token| token.text.as_str())
}

/// Whether the word at `i` is ruled out by the words before it: "never Fable", "no Fable",
/// "don't use Fable", "without Fable".
fn negated(tokens: &[Token], i: usize) -> bool {
    const NOT: &[&str] = &[
        "never", "no", "not", "without", "don't", "don’t", "dont", "avoid", "except",
    ];
    let before = |back: usize| i.checked_sub(back).map_or("", |at| word(tokens, at));
    NOT.contains(&before(1))
        || (matches!(before(1), "use" | "using" | "pick" | "run" | "any")
            && NOT.contains(&before(2)))
}

/// The restriction starting at token `i`, if one does.
fn read_at(tokens: &[Token], i: usize, clock: &Clock) -> Option<Hit> {
    let hit = |kind, value, end: usize| {
        let instead = (end..(end + 3).min(tokens.len()))
            .take_while(|at| word(tokens, *at) != ".")
            .any(|at| word(tokens, at) == "instead");
        Some(Hit {
            kind,
            value,
            start: i,
            end,
            instead,
        })
    };
    match word(tokens, i) {
        "until" | "till" | "til" | "by" => {
            let (value, end) = read_deadline(tokens, i + 1, clock)?;
            hit(DirectiveKind::Deadline, value, end)
        }
        "for" => {
            let (minutes, end) = read_duration(tokens, i + 1)?;
            hit(
                DirectiveKind::Deadline,
                Value::Deadline(Deadline::For { minutes }),
                end,
            )
        }
        "stop" if word(tokens, i + 1) == "after" => {
            let at = i + 2;
            match (word(tokens, at), word(tokens, at + 1)) {
                ("this" | "the", "phase") => hit(DirectiveKind::StopAfter, Value::StopThis, at + 2),
                ("the", "current") if word(tokens, at + 2) == "phase" => {
                    hit(DirectiveKind::StopAfter, Value::StopThis, at + 3)
                }
                ("phase", number) => {
                    let number = number.parse().ok()?;
                    hit(DirectiveKind::StopAfter, Value::StopPhase(number), at + 2)
                }
                _ => None,
            }
        }
        "only" | "just" if matches!(word(tokens, i + 1), "phase" | "phases") => {
            let (range, end) = read_range(tokens, i + 2)?;
            hit(DirectiveKind::Only, Value::Only(range), end)
        }
        "phase" | "phases" => {
            // "phases 2–4 only".
            let (range, end) = read_range(tokens, i + 1)?;
            (word(tokens, end) == "only").then_some(())?;
            hit(DirectiveKind::Only, Value::Only(range), end + 1)
        }
        "skip" if matches!(word(tokens, i + 1), "phase" | "phases") => {
            let (numbers, end) = read_list(tokens, i + 2)?;
            hit(DirectiveKind::Skip, Value::Skip(numbers), end)
        }
        "max" | "maximum" => {
            let at = if word(tokens, i + 1) == "of" {
                i + 2
            } else {
                i + 1
            };
            let (count, end) = read_workers(tokens, at)?;
            hit(DirectiveKind::MaxWorkers, Value::Max(count), end)
        }
        "at" if word(tokens, i + 1) == "most" => {
            let (count, end) = read_workers(tokens, i + 2)?;
            hit(DirectiveKind::MaxWorkers, Value::Max(count), end)
        }
        "up" if word(tokens, i + 1) == "to" => {
            let (count, end) = read_workers(tokens, i + 2)?;
            hit(DirectiveKind::MaxWorkers, Value::Max(count), end)
        }
        "no" if word(tokens, i + 1) == "more" && word(tokens, i + 2) == "than" => {
            let (count, end) = read_workers(tokens, i + 3)?;
            hit(DirectiveKind::MaxWorkers, Value::Max(count), end)
        }
        "effort" => {
            let at = if matches!(word(tokens, i + 1), "at" | "to") {
                i + 2
            } else {
                i + 1
            };
            let level = word(tokens, at);
            effort_level(level)?;
            hit(
                DirectiveKind::Ignored,
                Value::Ignored(format!("effort {level}")),
                at + 1,
            )
        }
        level if effort_level(level).is_some() && word(tokens, i + 1) == "effort" => hit(
            DirectiveKind::Ignored,
            Value::Ignored(format!("effort {level}")),
            i + 2,
        ),
        // "Never Fable" is a rule Brigadier already keeps: it stays in the Rules, not
        // "Ignored".
        "fable" if negated(tokens, i) => None,
        "fable" => hit(
            DirectiveKind::Ignored,
            Value::Ignored("Fable".into()),
            i + 1,
        ),
        "ultracode" => hit(
            DirectiveKind::Ignored,
            Value::Ignored("ultracode".into()),
            i + 1,
        ),
        "handoff" | "hand-off" => read_handoff(tokens, i + 1).and_then(|(percent, end)| {
            hit(
                DirectiveKind::Ignored,
                Value::Ignored(format!("hand-off at {percent}")),
                end,
            )
        }),
        "hand" if word(tokens, i + 1) == "off" => {
            read_handoff(tokens, i + 2).and_then(|(percent, end)| {
                hit(
                    DirectiveKind::Ignored,
                    Value::Ignored(format!("hand-off at {percent}")),
                    end,
                )
            })
        }
        _ => {
            // "3 workers max".
            let count: u32 = word(tokens, i).parse().ok()?;
            matches!(word(tokens, i + 1), "worker" | "workers").then_some(())?;
            matches!(word(tokens, i + 2), "max" | "maximum" | "at").then_some(())?;
            let end = if word(tokens, i + 2) == "at" {
                (word(tokens, i + 3) == "most").then_some(i + 4)?
            } else {
                i + 3
            };
            hit(DirectiveKind::MaxWorkers, Value::Max(count), end)
        }
    }
}

fn effort_level(level: &str) -> Option<()> {
    matches!(
        level,
        "low" | "medium" | "high" | "xhigh" | "x-high" | "max" | "maximum" | "extra-high"
    )
    .then_some(())
}

fn read_handoff(tokens: &[Token], at: usize) -> Option<(String, usize)> {
    let at = if word(tokens, at) == "at" { at + 1 } else { at };
    let percent = word(tokens, at);
    percent.ends_with('%').then(|| (percent.to_owned(), at + 1))
}

fn read_workers(tokens: &[Token], at: usize) -> Option<(u32, usize)> {
    let count: u32 = word(tokens, at).parse().ok()?;
    matches!(word(tokens, at + 1), "worker" | "workers").then_some((count, at + 2))
}

/// `2`, `2-4`, `2 – 4`, `2 to 4`, `2 through 4`.
fn read_range(tokens: &[Token], at: usize) -> Option<(PhaseRange, usize)> {
    let first = word(tokens, at);
    if let Some((from, to)) = first.split_once('-')
        && let (Ok(from), Ok(to)) = (from.parse(), to.parse())
    {
        return Some((PhaseRange { from, to }, at + 1));
    }
    let from: u32 = first.parse().ok()?;
    if matches!(word(tokens, at + 1), "-" | "to" | "through" | "thru")
        && let Ok(to) = word(tokens, at + 2).parse()
    {
        return Some((PhaseRange { from, to }, at + 3));
    }
    Some((PhaseRange { from, to: from }, at + 1))
}

/// `2`, `2, 4 and 5`, `2-4` (each of them).
fn read_list(tokens: &[Token], mut at: usize) -> Option<(Vec<u32>, usize)> {
    let mut numbers = Vec::new();
    loop {
        let (range, end) = read_range(tokens, at)?;
        if range.from <= range.to && range.to - range.from <= 100 {
            numbers.extend(range.from..=range.to);
        } else {
            numbers.push(range.from);
        }
        at = end;
        if matches!(word(tokens, at), "," | "and") && word(tokens, at + 1).parse::<u32>().is_ok() {
            at += 1;
        } else if word(tokens, at) == "," && word(tokens, at + 1) == "and" {
            at += 2;
        } else {
            return Some((numbers, at));
        }
    }
}

/// `3 hours`, `90 minutes`, `3h`, `1.5 hours`, `an hour`, `2h30`.
fn read_duration(tokens: &[Token], at: usize) -> Option<(u32, usize)> {
    let first = word(tokens, at);
    let unit = |unit: &str| -> Option<f64> {
        match unit {
            "hour" | "hours" | "hr" | "hrs" | "h" => Some(60.0),
            "minute" | "minutes" | "min" | "mins" | "m" => Some(1.0),
            _ => None,
        }
    };
    let (minutes, end) =
        if matches!(first, "an" | "a" | "one") && unit(word(tokens, at + 1)) == Some(60.0) {
            (60.0, at + 2)
        } else if let Ok(amount) = first.parse::<f64>() {
            (amount * unit(word(tokens, at + 1))?, at + 2)
        } else {
            // `3h`, `90m`, `2h30`.
            let digits = first.find(|c: char| !c.is_ascii_digit() && c != '.')?;
            let (amount, rest) = first.split_at(digits);
            let amount: f64 = amount.parse().ok()?;
            match rest.split_once('h') {
                Some(("", "")) => (amount * 60.0, at + 1),
                Some(("", extra)) => (amount * 60.0 + extra.parse::<f64>().ok()?, at + 1),
                _ => (amount * unit(rest)?, at + 1),
            }
        };
    (1.0..=(MAX_HOURS * 60) as f64)
        .contains(&minutes)
        .then_some((minutes.round() as u32, end))
}

/// After "until"/"by": `done`, `morning`, a time, `tomorrow 07:30`, `2026-10-03 07:30`.
fn read_deadline(tokens: &[Token], at: usize, clock: &Clock) -> Option<(Value, usize)> {
    let today = clock.now.to_zoned(clock.tz.clone()).date();
    match word(tokens, at) {
        "done" | "finished" | "complete" | "completed" => {
            return Some((Value::Deadline(Deadline::UntilDone), at + 1));
        }
        "it" | "it's" | "it’s" | "its" | "everything" | "all" => {
            let mut next = at + 1;
            if matches!(word(tokens, next), "is" | "s" | "'s") {
                next += 1;
            }
            matches!(word(tokens, next), "done" | "finished" | "complete").then_some(())?;
            return Some((Value::Deadline(Deadline::UntilDone), next + 1));
        }
        "morning" => return Some((next_occurrence(MORNING, clock), at + 1)),
        "the" if word(tokens, at + 1) == "morning" => {
            return Some((next_occurrence(MORNING, clock), at + 2));
        }
        _ => {}
    }
    let (date, at) = match word(tokens, at) {
        "today" | "tonight" => (Some(today), at + 1),
        "tomorrow" => (today.tomorrow().ok(), at + 1),
        other => match other.parse::<Date>() {
            Ok(date) if other.len() == 10 => (Some(date), at + 1),
            _ => (None, at),
        },
    };
    let at = if date.is_some() && word(tokens, at) == "at" {
        at + 1
    } else {
        at
    };
    if date.is_some() && word(tokens, at) == "morning" {
        let date = date?;
        return Some((at_date(date, MORNING, None, clock), at + 1));
    }
    let ((hour, minute), end) = read_time(tokens, at)?;
    let (offset, end) = match read_offset(word(tokens, end)) {
        Some(offset) => (Some(offset), end + 1),
        // `-05:00` is a dash token and a time token.
        None if word(tokens, end) == "-" => {
            match read_offset(&format!("-{}", word(tokens, end + 1))) {
                Some(offset) => (Some(offset), end + 2),
                None => (None, end),
            }
        }
        None => (None, end),
    };
    let value = match date {
        Some(date) => at_date(date, (hour, minute), offset, clock),
        None => match offset {
            Some(offset) => at_date(
                next_date((hour, minute), clock),
                (hour, minute),
                Some(offset),
                clock,
            ),
            None => next_occurrence((hour, minute), clock),
        },
    };
    Some((value, end))
}

/// `07:30`, `7:30`, `7am`, `7:30 pm`, `7 am`, `19h`? (no: an hour alone needs am/pm or a
/// minute, so "by 7 tasks" isn't a time).
fn read_time(tokens: &[Token], at: usize) -> Option<((i8, i8), usize)> {
    let first = word(tokens, at);
    let (clock_part, suffix) = match first.find(['a', 'p']) {
        Some(split) => (&first[..split], Some(&first[split..])),
        None => (first, None),
    };
    let (mut hour, minute, had_minute): (i8, i8, bool) = match clock_part.split_once(':') {
        Some((hour, minute)) if minute.len() == 2 => {
            (hour.parse().ok()?, minute.parse().ok()?, true)
        }
        Some(_) => return None,
        None => (clock_part.parse().ok()?, 0, false),
    };
    let (suffix, end) = match suffix {
        Some(suffix) => (Some(suffix), at + 1),
        None => match word(tokens, at + 1) {
            next @ ("am" | "pm" | "a.m" | "p.m") => (Some(next), at + 2),
            _ => (None, at + 1),
        },
    };
    match suffix.map(|suffix| suffix.trim_end_matches('.')) {
        Some("am" | "a.m") if (1..=12).contains(&hour) => {
            if hour == 12 {
                hour = 0;
            }
        }
        Some("pm" | "p.m") if (1..=12).contains(&hour) => {
            if hour != 12 {
                hour += 12;
            }
        }
        Some(_) => return None,
        None if !had_minute => return None,
        None => {}
    }
    ((0..24).contains(&hour) && (0..60).contains(&minute)).then_some(((hour, minute), end))
}

/// `+03:00`, `-05:00`, `+0300`.
fn read_offset(text: &str) -> Option<Offset> {
    let sign = match text.chars().next()? {
        '+' => 1,
        '-' => -1,
        _ => return None,
    };
    let digits: String = text[1..].chars().filter(char::is_ascii_digit).collect();
    if digits.len() != 4 {
        return None;
    }
    let hours: i32 = digits[..2].parse().ok()?;
    let minutes: i32 = digits[2..].parse().ok()?;
    Offset::from_seconds(sign * (hours * 3600 + minutes * 60)).ok()
}

/// The date the next occurrence of `time` falls on: today, or tomorrow once it has passed.
fn next_date(time: (i8, i8), clock: &Clock) -> Date {
    let now = clock.now.to_zoned(clock.tz.clone());
    let today = now.date();
    let civil = Time::new(time.0, time.1, 0, 0).unwrap_or(Time::midnight());
    if civil > now.time() {
        today
    } else {
        today.tomorrow().unwrap_or(today)
    }
}

fn next_occurrence(time: (i8, i8), clock: &Clock) -> Value {
    at_date(next_date(time, clock), time, None, clock)
}

/// `time` on `date` in the clock's time zone, or at `offset` when the user gave one (to pick
/// one of the two times a clock change repeats).
fn at_date(date: Date, time: (i8, i8), offset: Option<Offset>, clock: &Clock) -> Value {
    let Ok(civil) = Time::new(time.0, time.1, 0, 0) else {
        return Value::BadDeadline("That isn't a time of day.".into());
    };
    let datetime = DateTime::from_parts(date, civil);
    let ambiguous = clock.tz.to_ambiguous_timestamp(datetime);
    let local = format!("{:02}:{:02}", time.0, time.1);
    let instant = match (ambiguous.offset(), offset) {
        (AmbiguousOffset::Unambiguous { offset: actual }, given) => {
            if given.is_some_and(|given| given != actual) {
                return Value::BadDeadline(format!(
                    "{local} on {} is at {} here, not {}. Say the time without an offset.",
                    day_words(date),
                    offset_text(actual),
                    offset_text(given.unwrap_or(actual))
                ));
            }
            actual.to_timestamp(datetime)
        }
        (AmbiguousOffset::Gap { .. }, _) => {
            return Value::BadDeadline(format!(
                "{local} doesn't exist on {}: the clocks skip it. Say another time.",
                day_words(date)
            ));
        }
        (AmbiguousOffset::Fold { before, after }, None) => {
            return Value::BadDeadline(format!(
                "{local} happens twice on {} as the clocks go back. Say \"{local} {}\" for the first or \"{local} {}\" for the second.",
                day_words(date),
                offset_text(before),
                offset_text(after)
            ));
        }
        (AmbiguousOffset::Fold { before, after }, Some(given)) => {
            if given != before && given != after {
                return Value::BadDeadline(format!(
                    "{local} on {} is at {} or {}. Say one of those.",
                    day_words(date),
                    offset_text(before),
                    offset_text(after)
                ));
            }
            given.to_timestamp(datetime)
        }
    };
    match instant {
        Ok(instant) => Value::Deadline(Deadline::At {
            time: resolved(instant, &clock.tz),
        }),
        Err(_) => Value::BadDeadline("That time is out of range.".into()),
    }
}

fn resolved(instant: Timestamp, tz: &TimeZone) -> ResolvedTime {
    let zoned = instant.to_zoned(tz.clone());
    ResolvedTime {
        at_ms: instant.as_millisecond(),
        local_date: zoned.date().to_string(),
        local_time: format!("{:02}:{:02}", zoned.hour(), zoned.minute()),
        day: day_words(zoned.date()),
        offset: offset_text(zoned.offset()),
        time_zone: tz.iana_name().unwrap_or("local").to_owned(),
    }
}

fn offset_text(offset: Offset) -> String {
    let seconds = offset.seconds();
    let sign = if seconds < 0 { '-' } else { '+' };
    let seconds = seconds.abs();
    format!("{sign}{:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

/// `Sat 3 Oct`.
fn day_words(date: Date) -> String {
    const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let weekday = DAYS[usize::from(date.weekday().to_monday_zero_offset().unsigned_abs())];
    let month = MONTHS[usize::from(date.month().unsigned_abs()) - 1];
    format!("{weekday} {} {month}", date.day())
}

/// The directives `found` set, with conflicts between them as problems.
fn combine(text: &str, tokens: &[Token], found: Vec<Hit>) -> Parsed {
    let mut parsed = Parsed::default();
    let span = |hit: &Hit| {
        let start = tokens[hit.start].start;
        let end = tokens[hit.end.saturating_sub(1).max(hit.start)].end;
        DirectiveSpan {
            kind: hit.kind,
            text: text.get(start..end).unwrap_or_default().to_owned(),
            start: start as u32,
            end: end as u32,
        }
    };
    // For each kind that takes one value: the one said "instead", else the only one said.
    let mut chosen: Vec<(DirectiveKind, &Hit)> = Vec::new();
    for hit in &found {
        parsed.directives.spans.push(span(hit));
        if let Value::BadDeadline(message) = &hit.value {
            // The current deadline stays until the user says a usable one.
            parsed
                .problems
                .push(conflict(DirectiveKind::Deadline, message));
            continue;
        }
        match hit.kind {
            DirectiveKind::Skip | DirectiveKind::Ignored => {}
            kind => match chosen.iter_mut().find(|(known, _)| *known == kind) {
                None => chosen.push((kind, hit)),
                Some((_, known)) => {
                    if hit.instead {
                        *known = hit;
                    } else if !known.instead && !same(&known.value, &hit.value) {
                        parsed.problems.push(conflict(
                            kind,
                            &format!(
                                "\"{}\" and \"{}\" disagree. Keep one, or add \"instead\" to the one you mean.",
                                span(known).text,
                                span(hit).text
                            ),
                        ));
                    }
                }
            },
        }
        mark(&mut parsed.set, hit.kind, hit.instead);
        match &hit.value {
            Value::Skip(numbers) => {
                for number in numbers {
                    if !parsed.directives.skip.contains(number) {
                        parsed.directives.skip.push(*number);
                    }
                }
            }
            Value::Ignored(what) => {
                let line = format!("Ignored: {what} (Brigadier picks this itself)");
                if !parsed.directives.ignored.contains(&line) {
                    parsed.directives.ignored.push(line);
                }
            }
            _ => {}
        }
    }
    for (_, hit) in chosen {
        match &hit.value {
            Value::Deadline(deadline) => parsed.directives.deadline = deadline.clone(),
            Value::StopPhase(number) => {
                parsed.directives.stop_after = Some(StopAfter::Phase { number: *number });
                parsed.stop_after_this = false;
            }
            Value::StopThis => parsed.stop_after_this = true,
            Value::Only(range) => parsed.directives.only = Some(*range),
            Value::Max(count) => {
                if *count > MAX_WORKERS {
                    parsed.problems.push(conflict(
                        DirectiveKind::MaxWorkers,
                        &format!("{count} workers at once is more than Brigadier runs. Say {MAX_WORKERS} or fewer."),
                    ));
                }
                parsed.directives.max_workers = Some(*count);
            }
            Value::Skip(_) | Value::Ignored(_) | Value::BadDeadline(_) => {}
        }
    }
    parsed
}

fn mark(set: &mut Vec<(DirectiveKind, bool)>, kind: DirectiveKind, instead: bool) {
    match set.iter_mut().find(|(known, _)| *known == kind) {
        Some((_, known)) => *known |= instead,
        None => set.push((kind, instead)),
    }
}

fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Deadline(a), Value::Deadline(b)) => a == b,
        (Value::StopPhase(a), Value::StopPhase(b)) => a == b,
        (Value::StopThis, Value::StopThis) => true,
        (Value::Only(a), Value::Only(b)) => a == b,
        (Value::Max(a), Value::Max(b)) => a == b,
        _ => false,
    }
}

/// Whether deadline `a` comes before `b` (`None` when that can't be told without Start).
fn earlier(a: &Deadline, b: &Deadline, clock: &Clock) -> Option<bool> {
    let at = |deadline: &Deadline| match deadline {
        Deadline::UntilDone => Some(i64::MAX),
        Deadline::At { time } => Some(time.at_ms),
        Deadline::For { minutes } => {
            Some(clock.now.as_millisecond() + i64::from(*minutes) * 60_000)
        }
    };
    Some(at(a)? < at(b)?)
}

fn conflict(kind: DirectiveKind, message: &str) -> DirectiveProblem {
    DirectiveProblem {
        kind,
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rule_against_fable_is_kept_not_ignored() {
        let clock = Clock::system();
        for words in [
            "Never Fable.",
            "no fable, effort at most high",
            "Don't use Fable for anything.",
            "Work without Fable.",
        ] {
            let parsed = parse(words, &clock);
            assert!(
                !parsed
                    .directives
                    .ignored
                    .iter()
                    .any(|line| line.contains("Fable")),
                "{words}: {:?}",
                parsed.directives.ignored
            );
        }
        let parsed = parse("Use Fable for the hard parts.", &clock);
        assert_eq!(
            parsed.directives.ignored,
            vec!["Ignored: Fable (Brigadier picks this itself)".to_owned()]
        );
    }
}
