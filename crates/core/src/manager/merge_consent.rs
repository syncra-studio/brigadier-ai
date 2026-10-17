//! The user's consent to the session's merge, as plumbing (#240): the thread judges whether
//! the user consents, reading their words like any other message (a question about the merge,
//! or a "not" elsewhere, is no "no"). Here only: whether words the thread quotes are in a
//! message, and whether that message asks to wait for the running work ("merge when done").

/// The one condition a merge may wait on (SESSION-FIXES-PLAN.md Q3): the running work
/// ending. "merge when done", "merge once they finish", "merge it after the workers land",
/// "merge when it's all in": a condition word, an optional subject from these, an optional
/// verb, and a word for ending, closing its clause.
pub(crate) use crate::work::MergeCondition;

const FINISH_LEADS: &[&str] = &["when", "once", "after"];
const FINISH_SUBJECTS: &[&[&str]] = &[
    &["it"],
    &["it's"],
    &["they"],
    &["they're"],
    &["theyre"],
    &["the", "workers"],
    &["workers"],
    &["the", "work"],
    &["everything"],
    &["everything's"],
    &["all"],
];
const FINISH_VERBS: &[&str] = &["is", "are", "have", "has", "all", "s"];
const FINISH_WORDS: &[&str] = &[
    "done",
    "finish",
    "finished",
    "finishes",
    "land",
    "landed",
    "lands",
    "complete",
    "completed",
    "completes",
    "in",
];

/// The byte range in `message` (normalized) of a "when the work is done" clause, if it has
/// one: from its condition word to its last word.
fn finish_clause(message: &str) -> Option<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    let mut start = None;
    for (at, c) in message.char_indices() {
        let in_word = c.is_alphanumeric() || c == '\'';
        match (in_word, start) {
            (true, None) => start = Some(at),
            (false, Some(from)) => {
                spans.push((from, at));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(from) = start {
        spans.push((from, message.len()));
    }
    let word = |i: usize| -> Option<String> {
        spans
            .get(i)
            .map(|&(from, to)| message[from..to].trim_matches('\'').replace("n't", "nt"))
    };
    // A clause ends at punctuation, the message's end, or an "and"/"then".
    let closes = |i: usize| -> bool {
        let Some(&(_, end)) = spans.get(i) else {
            return true;
        };
        match spans.get(i + 1) {
            None => true,
            Some(&(next, _)) => {
                message[end..next].contains(|c: char| ".,;:!?()\n\u{2014}\u{2013}\"".contains(c))
                    || matches!(word(i + 1).as_deref(), Some("and" | "then"))
            }
        }
    };
    (0..spans.len()).find_map(|lead| {
        if !word(lead).is_some_and(|w| FINISH_LEADS.contains(&w.as_str())) {
            return None;
        }
        let mut at = lead + 1;
        if let Some(subject) = FINISH_SUBJECTS.iter().find(|subject| {
            subject
                .iter()
                .enumerate()
                .all(|(k, w)| word(at + k).as_deref() == Some(w))
        }) {
            at += subject.len();
        }
        while word(at).is_some_and(|w| FINISH_VERBS.contains(&w.as_str()))
            && word(at + 1).is_some_and(|w| {
                FINISH_WORDS.contains(&w.as_str()) || FINISH_VERBS.contains(&w.as_str())
            })
        {
            at += 1;
        }
        let finish = word(at)?;
        // "in" ends the work only as "all in".
        let ends = FINISH_WORDS.contains(&finish.as_str())
            && (finish != "in" || word(at - 1).as_deref() == Some("all"));
        (ends && closes(at)).then(|| spans[lead].0..spans[at].1)
    })
}

/// Lower case, curly quotes made straight, backticks gone, runs of space made one.
fn normalize(text: &str) -> String {
    text.to_lowercase()
        .replace(['\u{2019}', '\u{2018}'], "'")
        .replace(['\u{201c}', '\u{201d}', '`'], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The words, with "don't"-like contractions folded to their stem ("dont", "cant").
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|word| word.trim_matches('\''))
        .filter(|word| !word.is_empty())
        .map(|word| word.replace("n't", "nt"))
        .collect()
}

/// What a consent given in `message` waits for: nothing, or the running work ending ("merge
/// when done").
pub(super) fn condition(message: &str) -> MergeCondition {
    if finish_clause(&normalize(message)).is_some() {
        MergeCondition::WorkersFinish
    } else {
        MergeCondition::Now
    }
}

/// Whether `quoted` is in `message`, word for word, and not empty.
pub(super) fn quotes(quoted: &str, message: &str) -> bool {
    let (quoted, message) = (words(&normalize(quoted)), words(&normalize(message)));
    !quoted.is_empty()
        && format!(" {} ", message.join(" ")).contains(&format!(" {} ", quoted.join(" ")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #240: the thread judges consent; no message is refused for a word in it. Words quoted
    /// from a message that asks for the merge with a "not" or "don't" elsewhere are found.
    #[test]
    fn quoted_words_are_found_whatever_else_the_message_says() {
        let asked = "Rebase on GitHub's main, run only the tests for the files you changed (the failures are not caused by this change. Don't rerun them), then merge into main and push";
        assert!(quotes("then merge into main and push", asked));
        assert!(quotes("Merge it", "Looks good. Merge it."));
        assert!(quotes("merge it", "Can you merge it?"));
        assert!(quotes(
            "let's merge into main",
            "Let\u{2019}s merge into `main`"
        ));
        assert!(!quotes("merge it", "Thanks, looks good"));
        assert!(!quotes("merge it now", "merge it"));
        assert!(!quotes("", "merge it"));
        assert!(!quotes("merge", "the merger looks fine"));
    }

    /// SESSION-FIXES-PLAN.md Q3: a consent that waits for the running work says so.
    #[test]
    fn merge_when_done_waits_for_the_work() {
        for message in [
            "merge when done",
            "Merge once they finish.",
            "merge it after the workers land",
            "merge when it's all in",
            "Looks good, merge it when they're done",
            "merge into main once everything is complete",
            "merge it when it has landed, thanks",
        ] {
            assert_eq!(
                condition(message),
                MergeCondition::WorkersFinish,
                "{message}"
            );
        }
        for message in ["merge it", "let's merge into main", "merge it after lunch"] {
            assert_eq!(condition(message), MergeCondition::Now, "{message}");
        }
    }
}
