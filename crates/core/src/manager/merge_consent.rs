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
}
