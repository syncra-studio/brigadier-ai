//! The user's consent to the session's merge, as plumbing (#240): the thread judges whether
//! the user consents, reading their words like any other message (a question about the merge,
//! or a "not" elsewhere, is no "no"), and whether it waits for the running work (#243:
//! finish_session's `when`). Here only: whether words the thread quotes are in a message.

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

/// Whether `quoted` is in `message`, word for word, and not empty.
pub(super) fn quotes(quoted: &str, message: &str) -> bool {
    let (quoted, message) = (words(&normalize(quoted)), words(&normalize(message)));
    !quoted.is_empty()
        && format!(" {} ", message.join(" ")).contains(&format!(" {} ", quoted.join(" ")))
}

/// Whether the user's `text` explicitly says to merge without the checks, or despite them
/// (the merge rule's waiver): "merge anyway", "merge it without the checks", "skip the tests
/// and merge". A plain "merge it" never does, nor words that say not to: a "don't", "not",
/// "never" or "no" before them ("don't skip the tests and merge", "never merge without the
/// checks") or "without skipping" them. Each clause (between `.`, `,`, `;`, `!`, `?` or a new
/// line) is read on its own.
pub(super) fn skips_checks(text: &str) -> bool {
    normalize(text)
        .split(['.', ',', ';', '!', '?', '\n'])
        .any(|clause| clause_skips_checks(&words(clause)))
}

fn clause_skips_checks(words: &[String]) -> bool {
    let at = |list: &[&str]| words.iter().position(|word| list.contains(&word.as_str()));
    let has = |list: &[&str]| at(list).is_some();
    const SKIP: &[&str] = &["skip", "skipping", "ignore", "ignoring"];
    let merge = has(&["merge", "merging", "merged", "land", "ship"]);
    let anyway = at(&["anyway", "anyways", "regardless"]);
    let checks = has(&[
        "check", "checks", "test", "tests", "ci", "failing", "failure", "failures",
    ]);
    let skip = at(&[
        "without", "despite", "skip", "skipping", "ignore", "ignoring",
    ])
    .filter(|_| checks);
    let Some(waiver) = [anyway, skip].into_iter().flatten().min() else {
        return false;
    };
    // "without skipping the tests" keeps them.
    let without_skipping = words
        .windows(2)
        .any(|pair| pair[0] == "without" && SKIP.contains(&pair[1].as_str()));
    let negated = words[..waiver]
        .iter()
        .any(|word| NOT.contains(&word.as_str()));
    merge && !negated && !without_skipping
}

/// Words that say not to; `words` folds "n't" to "nt" ("dont", "cant").
const NOT: &[&str] = &[
    "not", "never", "no", "nor", "dont", "doesnt", "didnt", "cant", "cannot", "wont", "wouldnt",
    "shouldnt", "mustnt", "isnt", "arent", "wasnt", "werent", "neednt",
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Only explicit words skip the checks; consent alone never does.
    #[test]
    fn only_explicit_words_skip_the_checks() {
        for text in [
            "Merge anyway",
            "merge it anyway, the failing test is flaky",
            "Skip the checks and merge.",
            "merge without the tests",
            "Merge it despite the failing checks",
            "Merge \u{201c}anyway\u{201d}",
            "Merge it anyway, don't wait for CI",
            "merge it anyway even though the tests didn't pass",
            "The failures are not caused by this change. Merge anyway",
        ] {
            assert!(skips_checks(text), "{text}");
        }
        for text in [
            "merge it",
            "yes, merge into main",
            "run the checks, then merge",
            "skip the docs and merge",
            "anyway, how are the tests?",
            "Don't skip the tests and merge",
            "Merge without skipping tests",
            "never merge without the checks",
            "don't merge anyway",
            "Don't skip the tests; merge after they pass",
            "I can't merge without the tests",
        ] {
            assert!(!skips_checks(text), "{text}");
        }
    }

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
}
