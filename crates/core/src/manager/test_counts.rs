//! A test run's counts, read from its output when it ends ([`super::preview`]): the summary
//! lines of the common runners (cargo test, vitest, jest, pytest, go test -v, swift test and
//! xcodebuild's XCTest and Swift Testing, node --test, mocha). None when no summary is found.

use crate::work::TestCounts;

/// The counts `log` reports, from the first runner whose summary it holds.
pub fn read(log: &str) -> Option<TestCounts> {
    let plain = strip_ansi(log);
    let lines: Vec<&str> = plain.lines().map(str::trim).collect();
    [cargo, jest, vitest, pytest, swift, node, mocha, go]
        .iter()
        .find_map(|runner| runner(&lines))
}

/// Without terminal colour codes (`ESC [ … m`).
fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(char) = chars.next() {
        if char == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if code.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(char);
        }
    }
    out
}

/// A line's words, split at spaces and punctuation.
fn words(line: &str) -> Vec<&str> {
    line.split(|char: char| !(char.is_alphanumeric() || char == '.' || char == '_'))
        .map(|word| word.trim_end_matches('.'))
        .filter(|word| !word.is_empty())
        .collect()
}

/// Adds "15 passed", "1 failed", "2 skipped" (and their synonyms) on `line` to `counts`;
/// whether any was there.
fn add_counted(line: &str, counts: &mut TestCounts) -> bool {
    let words = words(line);
    let mut found = false;
    for pair in words.windows(2) {
        let Ok(count) = pair[0].parse::<u32>() else {
            continue;
        };
        let slot = match pair[1].to_ascii_lowercase().as_str() {
            "passed" | "passing" => &mut counts.passed,
            "failed" | "failing" | "error" | "errors" => &mut counts.failed,
            "skipped" | "ignored" | "pending" => &mut counts.skipped,
            _ => continue,
        };
        *slot += count;
        found = true;
    }
    found
}

/// The last line `pick` takes, its counts.
fn last_summary(lines: &[&str], pick: impl Fn(&str) -> bool) -> Option<TestCounts> {
    lines
        .iter()
        .rev()
        .filter(|line| pick(line))
        .find_map(|line| {
            let mut counts = TestCounts::default();
            add_counted(line, &mut counts).then_some(counts)
        })
}

/// `test result: ok. 15 passed; 0 failed; 1 ignored; …`, one per test binary: summed.
fn cargo(lines: &[&str]) -> Option<TestCounts> {
    let mut counts = TestCounts::default();
    let mut found = false;
    for line in lines.iter().filter(|line| line.starts_with("test result:")) {
        found |= add_counted(line, &mut counts);
    }
    found.then_some(counts)
}

/// `Tests:       1 failed, 2 skipped, 15 passed, 18 total`.
fn jest(lines: &[&str]) -> Option<TestCounts> {
    last_summary(lines, |line| line.starts_with("Tests:"))
}

/// `Tests  15 passed | 1 failed | 2 skipped (18)` (not `Test Files`).
fn vitest(lines: &[&str]) -> Option<TestCounts> {
    last_summary(lines, |line| line.starts_with("Tests "))
}

/// `==== 15 passed, 1 failed, 2 skipped in 0.12s ====`, or `15 passed in 0.12s` with -q.
fn pytest(lines: &[&str]) -> Option<TestCounts> {
    last_summary(lines, |line| {
        let line = line.trim_matches(|char| char == '=' || char == ' ');
        line.starts_with(|char: char| char.is_ascii_digit())
            && line.rsplit_once(" in ").is_some_and(|(_, time)| {
                time.split_whitespace().next().is_some_and(|time| {
                    time.ends_with('s') && time.starts_with(|c: char| c.is_ascii_digit())
                })
            })
    })
}

/// swift test and xcodebuild: XCTest's and Swift Testing's counts, together.
fn swift(lines: &[&str]) -> Option<TestCounts> {
    match (xctest(lines), swift_testing(lines)) {
        (Some(one), Some(other)) => Some(TestCounts {
            passed: one.passed + other.passed,
            failed: one.failed + other.failed,
            skipped: one.skipped + other.skipped,
        }),
        (one, other) => one.or(other),
    }
}

/// XCTest (swift test, xcodebuild): `Executed 15 tests, with 2 tests skipped and 1 failure
/// (0 unexpected) in 0.5 (0.6) seconds`. The last one is the whole run's.
fn xctest(lines: &[&str]) -> Option<TestCounts> {
    let line = lines
        .iter()
        .rev()
        .find(|line| line.starts_with("Executed ") && line.contains(" test"))?;
    let words = words(line);
    // `swift test` runs XCTest too, with none when a package has only Swift Testing's.
    let executed: u32 = words.get(1)?.parse().ok().filter(|count| *count > 0)?;
    let (mut failed, mut skipped) = (0, 0);
    for (at, word) in words.iter().enumerate() {
        let Ok(count) = word.parse::<u32>() else {
            continue;
        };
        match (words.get(at + 1).copied(), words.get(at + 2).copied()) {
            (Some("failure" | "failures"), _) => failed = count,
            (Some("test" | "tests"), Some("skipped")) => skipped = count,
            _ => {}
        }
    }
    Some(TestCounts {
        passed: executed.saturating_sub(failed + skipped),
        failed,
        skipped,
    })
}

/// Swift Testing: `✔ Test run with 15 tests in 3 suites passed after 0.01 seconds.`; failed,
/// a `✘ Test "…" failed after …` line per failing test; skipped, a `➜ Test … skipped` line.
fn swift_testing(lines: &[&str]) -> Option<TestCounts> {
    let line = lines
        .iter()
        .rev()
        .find(|line| line.contains("Test run with "))?;
    let words = words(line.split_once("Test run with ")?.1);
    let total: u32 = words.first()?.parse().ok()?;
    let failed = lines
        .iter()
        .filter(|line| {
            line.starts_with('✘')
                && line.contains(" failed after ")
                && !line.contains("Test run with ")
                && !line.contains("Suite ")
        })
        .count() as u32;
    let skipped = lines
        .iter()
        .filter(|line| line.starts_with('➜') && line.contains(" skipped"))
        .count() as u32;
    Some(TestCounts {
        passed: total.saturating_sub(failed + skipped),
        failed,
        skipped,
    })
}

/// node --test: `ℹ pass 15`, `ℹ fail 1`, `ℹ skipped 2` (spec reporter), or `# pass 15` (TAP).
fn node(lines: &[&str]) -> Option<TestCounts> {
    let mut counts = TestCounts::default();
    let mut found = false;
    for line in lines {
        let Some(rest) = line.strip_prefix("ℹ ").or_else(|| line.strip_prefix("# ")) else {
            continue;
        };
        let mut parts = rest.split_whitespace();
        let (Some(word), Some(count), None) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let Ok(count) = count.parse::<u32>() else {
            continue;
        };
        let slot = match word {
            "pass" => &mut counts.passed,
            "fail" => &mut counts.failed,
            "skipped" | "skip" => &mut counts.skipped,
            _ => continue,
        };
        *slot = count;
        found = true;
    }
    found.then_some(counts)
}

/// mocha: `15 passing (412ms)`, `1 failing`, `2 pending`.
fn mocha(lines: &[&str]) -> Option<TestCounts> {
    let mut counts = TestCounts::default();
    let mut found = false;
    for line in lines {
        let mut parts = line.split_whitespace();
        let (Some(count), Some("passing" | "failing" | "pending")) = (parts.next(), parts.next())
        else {
            continue;
        };
        if count.parse::<u32>().is_ok() {
            found |= add_counted(line, &mut counts);
        }
    }
    found.then_some(counts)
}

/// go test -v: a `--- PASS:`, `--- FAIL:` or `--- SKIP:` line per test (and subtest).
fn go(lines: &[&str]) -> Option<TestCounts> {
    let mut counts = TestCounts::default();
    let mut found = false;
    for line in lines {
        let slot = if line.starts_with("--- PASS: ") {
            &mut counts.passed
        } else if line.starts_with("--- FAIL: ") {
            &mut counts.failed
        } else if line.starts_with("--- SKIP: ") {
            &mut counts.skipped
        } else {
            continue;
        };
        *slot += 1;
        found = true;
    }
    found.then_some(counts)
}

#[cfg(test)]
mod tests {
    use super::read;
    use crate::work::TestCounts;

    const fn counts(passed: u32, failed: u32, skipped: u32) -> Option<TestCounts> {
        Some(TestCounts {
            passed,
            failed,
            skipped,
        })
    }

    /// Each common runner's summary gives its counts; output without one gives none.
    #[test]
    fn each_runner_s_summary_is_read() {
        let cases = [
            (
                "cargo test",
                "running 12 tests\n....\ntest result: ok. 12 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out; finished in 0.01s\n\n   Doc-tests todo\n\nrunning 3 tests\ntest result: FAILED. 2 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out\n",
                counts(14, 1, 1),
            ),
            (
                "vitest",
                " \u{1b}[32m✓\u{1b}[39m src/todo.test.ts (18 tests) 4ms\n\n\u{1b}[2m Test Files \u{1b}[22m \u{1b}[1m\u{1b}[32m2 passed\u{1b}[39m\u{1b}[22m\u{1b}[90m (2)\u{1b}[39m\n\u{1b}[2m      Tests \u{1b}[22m \u{1b}[1m\u{1b}[32m15 passed\u{1b}[39m\u{1b}[22m | 1 failed | 2 skipped\u{1b}[90m (18)\u{1b}[39m\n",
                counts(15, 1, 2),
            ),
            (
                "jest",
                "Test Suites: 1 failed, 3 passed, 4 total\nTests:       1 failed, 2 skipped, 15 passed, 18 total\nSnapshots:   0 total\n",
                counts(15, 1, 2),
            ),
            (
                "pytest",
                "tests/test_todo.py ..F.s\n=========== 1 failed, 15 passed, 2 skipped, 1 error in 0.12s ===========\n",
                counts(15, 2, 2),
            ),
            (
                "pytest -q",
                "...............\n15 passed in 0.08s\n",
                counts(15, 0, 0),
            ),
            (
                "swift test (XCTest)",
                "Test Suite 'TodoTests' passed at 2026-10-20.\n\t Executed 3 tests, with 0 failures (0 unexpected) in 0.002 (0.003) seconds\nTest Suite 'All tests' failed at 2026-10-20.\n\t Executed 18 tests, with 2 tests skipped and 1 failure (0 unexpected) in 0.5 (0.6) seconds\n",
                counts(15, 1, 2),
            ),
            (
                "swift test (Swift Testing)",
                "Executed 0 tests, with 0 failures (0 unexpected) in 0.000 (0.000) seconds\n◇ Test run started.\n➜ Test slow() skipped.\n✔ Test adds() passed after 0.001 seconds.\n✘ Test dueDate() failed after 0.002 seconds with 1 issue.\n✘ Test run with 17 tests in 2 suites failed after 0.004 seconds with 1 issue.\n",
                counts(15, 1, 1),
            ),
            (
                "xcodebuild",
                "Test Case '-[TodoTests testAdd]' passed (0.001 seconds).\nExecuted 15 tests, with 0 failures (0 unexpected) in 0.020 (0.030) seconds\n** TEST SUCCEEDED **\n",
                counts(15, 0, 0),
            ),
            (
                "node --test",
                "✔ adds a todo (0.5ms)\nℹ tests 18\nℹ suites 0\nℹ pass 15\nℹ fail 1\nℹ cancelled 0\nℹ skipped 2\nℹ todo 0\nℹ duration_ms 41\n",
                counts(15, 1, 2),
            ),
            (
                "mocha",
                "  ✓ marks a past due date as overdue\n\n  15 passing (412ms)\n  2 pending\n  1 failing\n",
                counts(15, 1, 2),
            ),
            (
                "go test -v",
                "=== RUN   TestAdd\n--- PASS: TestAdd (0.00s)\n=== RUN   TestDue\n    --- PASS: TestDue/today (0.00s)\n--- FAIL: TestDue (0.00s)\n--- SKIP: TestSlow (0.00s)\nFAIL\n",
                counts(2, 1, 1),
            ),
            ("no summary", "ok  \texample.com/todo\t0.012s\n", None),
            ("nothing", "", None),
        ];
        for (runner, log, want) in cases {
            assert_eq!(read(log), want, "{runner}");
        }
    }
}
