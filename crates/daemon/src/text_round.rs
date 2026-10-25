//! The check behind `brigadierd hook stop` (harness D43, D45): does a thread's reply end with
//! a round of questions for the user written as text, where its question tool (or `brigadier
//! ask`) belongs? A cheap look at the lines, no model: a round is two or more question items,
//! or one ❓ / `Q1` item with a ➡️ recommendation, followed by at most a short closing. One
//! ordinary closing question, or questions in the middle of a reply that goes on, is no round.

/// The most lines, and characters, after a round's last item that still leave it the end of
/// the reply ("A quick "yes to all" works, or answer just the ones you'd change.").
const TRAILER_LINES: usize = 3;
const TRAILER_CHARS: usize = 400;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Line {
    Blank,
    /// `---` between items.
    Rule,
    /// A ❓ line, or a `Q1` line ([`Line::Item`] when it has a "?" or a ➡️ follows it).
    Marked {
        question: bool,
        q_number: bool,
    },
    /// A list item with a "?" in it.
    Listed,
    /// A ➡️ or "Recommended:" line.
    Answer,
    /// Anything else; `nested`: indented, so part of the item above it.
    Text {
        nested: bool,
    },
}

/// Whether `reply` ends with a round of questions for the user written as text.
pub fn is_text_round(reply: &str) -> bool {
    let lines = classify(reply);
    // A `Q1` line is an item when it asks something or a recommendation follows it ("Q1 revenue
    // grew" in a report is not).
    let next_item = |from: usize| {
        (from..lines.len())
            .find(|&at| matches!(lines[at], Line::Marked { .. } | Line::Listed))
            .unwrap_or(lines.len())
    };
    let answered = |at: usize| lines[at + 1..next_item(at + 1)].contains(&Line::Answer);
    let items: Vec<usize> = (0..lines.len())
        .filter(|&at| match lines[at] {
            Line::Marked { question, q_number } => !q_number || question || answered(at),
            Line::Listed => true,
            _ => false,
        })
        .collect();
    let Some(&last) = items.last() else {
        return false;
    };
    let shaped = items.len() >= 2 || (matches!(lines[last], Line::Marked { .. }) && answered(last));
    if !shaped {
        return false;
    }
    // Past the last item's own lines: its recommendation's paragraph, or else its body (the
    // lines right under it and anything indented).
    let mut at = last + 1;
    if let Some(answer) = (at..lines.len())
        .rev()
        .find(|&at| lines[at] == Line::Answer)
    {
        at = answer + 1;
        while at < lines.len() && matches!(lines[at], Line::Text { .. }) {
            at += 1;
        }
    } else {
        while at < lines.len() && matches!(lines[at], Line::Text { .. }) {
            at += 1;
        }
    }
    while at < lines.len() && matches!(lines[at], Line::Text { nested: true } | Line::Blank) {
        at += 1;
    }
    let trailer: Vec<&str> = reply
        .lines()
        .zip(&lines)
        .skip(at)
        .filter(|(_, kind)| !matches!(kind, Line::Blank | Line::Rule))
        .map(|(text, _)| text.trim())
        .collect();
    trailer.len() <= TRAILER_LINES
        && trailer.iter().map(|text| text.len()).sum::<usize>() <= TRAILER_CHARS
}

/// Each line of `reply`, a code block's lines as plain text.
fn classify(reply: &str) -> Vec<Line> {
    let mut fenced = false;
    reply
        .lines()
        .map(|line| {
            let trimmed = line.trim_start();
            let nested = trimmed.len() < line.len();
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
                fenced = !fenced;
                return Line::Text { nested };
            }
            if fenced {
                return Line::Text { nested };
            }
            if trimmed.is_empty() {
                return Line::Blank;
            }
            if trimmed.len() >= 3 && trimmed.chars().all(|c| matches!(c, '-' | '*' | '_' | ' ')) {
                return Line::Rule;
            }
            let (listed, rest) = list_item(trimmed);
            let core = rest.trim_start_matches(['#', '*', '_', ' ']);
            let question = asks(rest);
            if core.starts_with('❓') {
                return Line::Marked {
                    question,
                    q_number: false,
                };
            }
            if q_number(core) {
                return Line::Marked {
                    question,
                    q_number: true,
                };
            }
            if core.starts_with('➡') || core.to_lowercase().starts_with("recommend") {
                return Line::Answer;
            }
            if listed && question {
                return Line::Listed;
            }
            Line::Text { nested }
        })
        .collect()
}

/// Whether `line` starts as a list item (`- `, `* `, `• `, `1. `, `1) `), and what follows it.
fn list_item(line: &str) -> (bool, &str) {
    for bullet in ["- ", "* ", "+ ", "• "] {
        if let Some(rest) = line.strip_prefix(bullet) {
            return (true, rest);
        }
    }
    let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    if digits > 0
        && let Some(rest) = line[digits..]
            .strip_prefix(". ")
            .or_else(|| line[digits..].strip_prefix(") "))
    {
        return (true, rest);
    }
    (false, line)
}

/// `Q1`, `Q12:`, `Q3 -`: a numbered question.
fn q_number(core: &str) -> bool {
    let Some(rest) = core.strip_prefix('Q') else {
        return false;
    };
    let digits = rest.len() - rest.trim_start_matches(|c: char| c.is_ascii_digit()).len();
    digits > 0 && !rest[digits..].starts_with(|c: char| c.is_alphanumeric())
}

/// Whether `text` asks something: a "?" outside inline code.
fn asks(text: &str) -> bool {
    text.split('`')
        .step_by(2)
        .any(|outside| outside.contains(['?', '？']))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real round written as text (session 698142a4): twelve ❓ items with ➡️
    /// recommendations, one with no "?" at all, between `---` rules.
    const SESSION_ROUND: &str = include_str!("text_round_698142a4.md");

    #[test]
    fn a_real_round_written_as_text_is_one() {
        assert!(is_text_round(SESSION_ROUND));
    }

    /// The grilling skill's own format (~/.claude/skills/grill-me/SKILL.md, which took grilling's, D48).
    #[test]
    fn the_grilling_format_is_one() {
        let round = "Round 1:\n\n\
            ❓ **Q1** - **Storage**: Where do drafts live? Options: the database or files.\n\n\
            ➡️ The database.\n\n---\n\n\
            ❓ **Q2** - **Sharing**: Can others see a draft before it's sent.\n\n\
            ➡️ No, only the author.\n";
        assert!(is_text_round(round));
        // One item with its recommendation is a round too.
        let single = "❓ **Q1** - **Storage**: Where do drafts live?\n\n➡️ The database.\n";
        assert!(is_text_round(single));
    }

    #[test]
    fn numbered_and_bulleted_questions_are_one() {
        let numbered = "Before I start, a few things:\n\n\
            1. Should the export include archived items?\n\
            2. CSV or JSON?\n\
            3. Do you want a progress bar?\n\n\
            Answer whichever you like.";
        assert!(is_text_round(numbered));
        let bulleted = "Some open points:\n\
            - Which branch should this land on?\n  I'd say main.\n\
            - Is the old endpoint still used anywhere?\n";
        assert!(is_text_round(bulleted));
        let q_lines =
            "Q1: Keep the old flag? Recommended: no.\nQ2: Rename the table? Recommended: yes.\n";
        assert!(is_text_round(q_lines));
    }

    #[test]
    fn one_closing_question_is_none() {
        assert!(!is_text_round(
            "Done: the export now skips archived items and the tests pass.\n\nWant me to push it?"
        ));
        assert!(!is_text_round(
            "Fixed the flaky test.\n\n- Should I also look at the other one?"
        ));
        assert!(!is_text_round("Q1 revenue grew 4%.\nQ2 revenue grew 6%.\n"));
    }

    #[test]
    fn questions_mid_reply_or_in_code_are_none() {
        let mid = "I looked at these:\n\
            1. Does the cache expire?\n\
            2. Is the lock released on error?\n\n\
            Both turned out fine: the cache expires after an hour, and the lock is released in\n\
            the drop guard. I changed the retry to back off and added a test for it.\n\
            The suite passes locally.\n\
            Next I'll run the integration tests.\n\
            Then I'll write the summary.\n";
        assert!(!is_text_round(mid));
        let code = "Here's the prompt I used:\n\n```\n1. What is your name?\n2. What is your quest?\n```\n";
        assert!(!is_text_round(code));
        let report = "Built the toggle.\n\n- Settings: a new switch\n- Tests: 4 new, all pass\n";
        assert!(!is_text_round(report));
        assert!(!is_text_round(""));
    }
}
