//! The digest a long command output reaches the thread's model as (THREAD-PLAN.md Q4).
//!
//! The full output is stored first (an `out-<id>` artifact the model pages through with
//! `read_artifact`); the model gets at most [`DIGEST_MAX`] bytes, built in this order and cut at
//! line boundaries:
//!
//! 1. a header (at most [`HEADER_MAX`] bytes): the exit status and where the full output is;
//! 2. the lines that look like trouble (`error`, `warning`, `panic` in any case, `FAIL` in
//!    capitals), at most [`MATCHES_MAX`] bytes, then how many more there are;
//! 3. the first and last lines, taken alternately until the budget is used, with how many lines
//!    between them were left out.
//!
//! A line too long for its room is cut at a character boundary and ends with `…`.

/// Most bytes a digest takes.
pub const DIGEST_MAX: usize = 4096;
/// Outputs up to this size reach the model as they are; longer ones as a digest.
pub const TRIM_ABOVE: usize = 8192;
/// Most bytes the header takes.
const HEADER_MAX: usize = 200;
/// Most bytes the matching lines take, their label included.
const MATCHES_MAX: usize = 2048;
/// Most bytes one matching line, or one head or tail line, shows (unless it has the room to
/// itself: the last line to show gets what is left).
const LINE_MAX: usize = 512;
/// Room kept for the omitted-lines marker while the head and tail are filled.
const OMITTED_ROOM: usize = 48;
/// A line cut for room ends with this.
const CUT: &str = "…";

const MATCHES_LABEL: &str = "[lines with error, warning, FAIL or panic]";
const OUTPUT_LABEL: &str = "[start and end of the output]";

/// The digest of `output`, a command's full output stored as `artifact` (`out-<id>`), which
/// ended with `status` ("exit 0", "exit 101", "No matches found", "timed out after 600 s").
pub fn digest(status: &str, output: &[u8], artifact: &str) -> String {
    let text = String::from_utf8_lossy(output);
    let lines = split_lines(&text);
    let mut out = header(status, artifact, lines.len(), output.len());

    // The lines that look like trouble, in order, as many as fit.
    let matching: Vec<&str> = lines.iter().copied().filter(|line| matches(line)).collect();
    if !matching.is_empty() {
        let mut section = String::from(MATCHES_LABEL);
        let mut shown = 0;
        for line in &matching {
            let line = clip(line, LINE_MAX);
            if section.len() + 1 + line.len() > MATCHES_MAX {
                break;
            }
            section.push('\n');
            section.push_str(&line);
            shown += 1;
        }
        if shown > 0 {
            out.push('\n');
            out.push_str(&section);
        }
        let more = matching.len() - shown;
        if more > 0 {
            out.push_str(&format!(
                "\n[+{more} more matching lines in the full output]"
            ));
        }
    }

    // The first and last lines, alternately.
    let budget = DIGEST_MAX.saturating_sub(out.len() + 1 + OUTPUT_LABEL.len() + OMITTED_ROOM);
    let mut used = 0;
    let (mut head, mut tail) = (Vec::new(), Vec::new());
    let (mut next_head, mut next_tail) = (0usize, lines.len());
    let mut from_head = true;
    while next_head < next_tail {
        let left = budget.saturating_sub(used);
        if left <= CUT.len() + 1 {
            break;
        }
        let index = if from_head { next_head } else { next_tail - 1 };
        // The last line to show has what is left; others half of it at most.
        let room = if next_tail - next_head == 1 {
            left - 1
        } else {
            LINE_MAX.max(left / 2).min(left - 1)
        };
        let line = clip(lines[index], room);
        used += line.len() + 1;
        if from_head {
            head.push(line);
            next_head += 1;
        } else {
            tail.push(line);
            next_tail -= 1;
        }
        from_head = !from_head;
    }
    if !head.is_empty() || !tail.is_empty() {
        out.push('\n');
        out.push_str(OUTPUT_LABEL);
        for line in &head {
            out.push('\n');
            out.push_str(line);
        }
        let omitted = next_tail - next_head;
        if omitted > 0 {
            out.push_str(&format!("\n[… {omitted} lines omitted …]"));
        }
        for line in tail.iter().rev() {
            out.push('\n');
            out.push_str(line);
        }
    }
    // Never more than the budget, whatever the arithmetic above missed.
    if out.len() > DIGEST_MAX {
        let end = floor_char_boundary(&out, DIGEST_MAX);
        out.truncate(end);
    }
    out
}

/// Whether a line looks like trouble: `error`, `warning` or `panic` (`panicked`) in any case,
/// or `FAIL` (`FAILED`, `FAILURE`) in capitals; a lower-case "failed" is too common in passing
/// logs ("0 failed").
fn matches(line: &str) -> bool {
    if line.contains("FAIL") {
        return true;
    }
    let lower = line.to_ascii_lowercase();
    ["error", "warning", "panic"]
        .iter()
        .any(|word| lower.contains(word))
}

/// The header: the status and where the full output is, at most [`HEADER_MAX`] bytes (a long
/// status is cut).
fn header(status: &str, artifact: &str, lines: usize, bytes: usize) -> String {
    let location = format!("[full output: read_artifact {artifact}, {lines} lines, {bytes} bytes]");
    let status = status.trim();
    let status = status.lines().next().unwrap_or_default();
    let room = HEADER_MAX.saturating_sub(location.len() + 1);
    let header = if status.is_empty() || room <= CUT.len() {
        location
    } else {
        format!("{} {location}", clip(status, room))
    };
    clip(&header, HEADER_MAX).into_owned()
}

/// The output's lines, without their line ends; a final line end starts no empty line.
fn split_lines(text: &str) -> Vec<&str> {
    let text = text.strip_suffix('\n').unwrap_or(text);
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .collect()
}

/// `line` in at most `max` bytes: cut at a character boundary, ending with [`CUT`].
fn clip(line: &str, max: usize) -> std::borrow::Cow<'_, str> {
    if line.len() <= max {
        return std::borrow::Cow::Borrowed(line);
    }
    let end = floor_char_boundary(line, max.saturating_sub(CUT.len()));
    std::borrow::Cow::Owned(format!("{}{CUT}", &line[..end]))
}

/// The largest character boundary of `text` at or below `index`.
fn floor_char_boundary(text: &str, index: usize) -> usize {
    let mut end = index.min(text.len());
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    end
}

#[cfg(test)]
mod tests {
    use super::*;

    fn passing_log(lines: usize) -> String {
        (0..lines)
            .map(|n| format!("test tests::case_{n:05} ... ok"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }

    #[test]
    fn a_passing_log_shows_its_header_head_and_tail_within_the_budget() {
        let log = passing_log(1700);
        assert!(log.len() > 50_000);
        let digest = digest("exit 0", log.as_bytes(), "out-1a2b3c4d");
        assert!(digest.len() <= DIGEST_MAX, "{}", digest.len());
        assert!(
            digest.len() > DIGEST_MAX - 200,
            "the budget is used: {}",
            digest.len()
        );
        let first = digest.lines().next().unwrap();
        assert_eq!(
            first,
            format!(
                "exit 0 [full output: read_artifact out-1a2b3c4d, 1700 lines, {} bytes]",
                log.len()
            )
        );
        assert!(!digest.contains(MATCHES_LABEL));
        assert!(digest.contains("case_00000 ... ok\n"));
        assert!(digest.ends_with("case_01699 ... ok"));
        // Head and tail alternate: as many of each, give or take one.
        let shown = digest
            .lines()
            .filter(|line| line.starts_with("test "))
            .count();
        let omitted: usize = digest
            .lines()
            .find_map(|line| line.strip_prefix("[… ")?.strip_suffix(" lines omitted …]"))
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(shown + omitted, 1700);
        let head = digest
            .lines()
            .take_while(|line| !line.starts_with("[…"))
            .filter(|line| line.starts_with("test "))
            .count();
        assert!(head.abs_diff(shown - head) <= 1, "{head} of {shown}");
    }

    #[test]
    fn matching_lines_come_first_up_to_their_budget_with_the_rest_counted() {
        let mut lines: Vec<String> = (0..2000).map(|n| format!("compiling crate_{n}")).collect();
        for n in (100..2000).step_by(20) {
            lines[n] = format!("error[E0308]: mismatched types in crate_{n}");
        }
        lines[1500] = "thread 'main' panicked at src/lib.rs:4:5".into();
        lines[1999] = "test result: FAILED. 3 passed; 1 failed".into();
        lines[50] = "Warning: deprecated flag".into();
        let output = lines.join("\n");
        let digest = digest("exit 101", output.as_bytes(), "out-ffff0000");
        assert!(digest.len() <= DIGEST_MAX);
        let mut parts = digest.split('\n');
        assert!(
            parts
                .next()
                .unwrap()
                .starts_with("exit 101 [full output: read_artifact out-ffff0000, 2000 lines,")
        );
        assert_eq!(parts.next(), Some(MATCHES_LABEL));
        // In output order: the warning, then the errors.
        assert_eq!(parts.next(), Some("Warning: deprecated flag"));
        assert_eq!(
            parts.next(),
            Some("error[E0308]: mismatched types in crate_100")
        );
        let section: String = digest
            .split_once('\n')
            .unwrap()
            .1
            .split("\n[+")
            .next()
            .unwrap()
            .to_owned();
        assert!(section.len() <= MATCHES_MAX, "{}", section.len());
        let shown = section.lines().count() - 1;
        // 95 errors (one of them replaced by the panic), the warning and the FAILED line.
        let total = 95 + 1 + 1;
        let more: usize = digest
            .lines()
            .find_map(|line| {
                line.strip_prefix("[+")?
                    .strip_suffix(" more matching lines in the full output]")
            })
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(shown + more, total);
        // The head and tail follow, the last line of the output last.
        let rest = digest.split_once(OUTPUT_LABEL).unwrap().1;
        assert!(rest.starts_with("\ncompiling crate_0\n"));
        assert!(digest.ends_with("test result: FAILED. 3 passed; 1 failed"));
    }

    #[test]
    fn case_rules_match_trouble_but_not_passing_counts() {
        assert!(matches("ERROR: boom"));
        assert!(matches("src/a.rs: warning: unused"));
        assert!(matches("thread 'x' panicked at"));
        assert!(matches("--- FAIL: TestThing"));
        assert!(matches("FAILED tests/test_a.py::test_b"));
        assert!(!matches("test result: ok. 41 passed; 0 failed"));
        assert!(!matches("Finished `test` profile"));
    }

    #[test]
    fn a_huge_single_line_is_cut_safely_at_a_character_boundary() {
        // Multi-byte characters everywhere, so any cut lands next to one.
        let line = "é€😀".repeat(200_000);
        for output in [
            line.clone(),
            format!("error: {line}"),
            format!("{line}\n{line}\n"),
        ] {
            let digest = digest("exit 1", output.as_bytes(), "out-00000001");
            assert!(digest.len() <= DIGEST_MAX, "{}", digest.len());
            assert!(digest.contains(CUT));
            // It is valid UTF-8 by construction (a String); the cut kept whole characters.
            assert!(!digest.contains('\u{fffd}'));
        }
        // A one-line output gets most of the budget, not one line's share.
        let digest = digest("exit 0", line.as_bytes(), "out-00000001");
        assert!(digest.len() > DIGEST_MAX - 200, "{}", digest.len());
    }

    #[test]
    fn invalid_utf8_and_crlf_are_shown_as_text() {
        let mut output = b"first\r\n".to_vec();
        output.extend_from_slice(&[0xff, 0xfe, b'\n']);
        output.extend_from_slice(b"last\r\n");
        let digest = digest("exit 0", &output, "out-00000002");
        assert!(digest.contains("3 lines, 16 bytes]"));
        assert!(digest.contains("\nfirst\n"));
        assert!(digest.ends_with("\nlast"));
    }

    #[test]
    fn a_long_status_keeps_the_header_within_its_budget() {
        let status = "x".repeat(1000);
        let digest = digest(&status, b"one\ntwo\n", "out-00000003");
        let header = digest.lines().next().unwrap();
        assert!(header.len() <= HEADER_MAX, "{}", header.len());
        assert!(header.ends_with("[full output: read_artifact out-00000003, 2 lines, 8 bytes]"));
        // A short output shows every line, with nothing omitted.
        assert!(digest.ends_with("\none\ntwo"));
        assert!(!digest.contains("omitted"));
    }

    #[test]
    fn many_long_matching_lines_stay_within_both_budgets() {
        let output = (0..5000)
            .map(|n| format!("error: {}{n}", "y".repeat(3000)))
            .collect::<Vec<_>>()
            .join("\n");
        let digest = digest("exit 2", output.as_bytes(), "out-00000004");
        assert!(digest.len() <= DIGEST_MAX);
        assert!(digest.contains("more matching lines in the full output]"));
        assert!(digest.contains("lines omitted …]"));
    }
}
