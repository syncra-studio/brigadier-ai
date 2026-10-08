//! Whether the user's latest message asks for the session's merge, checked in code before
//! `finish_session` merges: the thread passes the user's words, quoted from that message.
//!
//! Consent is plain and unconditional, and anything in doubt refuses (the thread can just ask
//! again): the words are in the latest message; that message asks no question (bar "can you
//! merge it?"), sets no condition ("if", "once", "after", …) and says no "no", "wait" or
//! "don't"; and either the words ask for the merge ("merge it") or the whole message is a plain
//! yes to a reply that proposed this session's merge, as a question naming its branch or base
//! ("Merge `brigadier/s1/session` into `main`?"). No model judges it.

/// Words that take a yes back, or put it off: anywhere in the message, they refuse.
const HOLDS: &[&str] = &[
    "no", "not", "nope", "nah", "never", "wait", "hold", "stop", "cancel", "later", "abort",
    "hang", "pause", "cannot", "dont", "doesnt", "didnt", "isnt", "arent", "wasnt", "werent",
    "wont", "wouldnt", "shouldnt", "cant", "couldnt", "havent", "hasnt", "hadnt", "aint", "mustnt",
    "neednt",
];

/// Words that make the merge depend on something: anywhere in the message, they refuse.
const CONDITIONS: &[&str] = &[
    "if", "once", "when", "whenever", "after", "unless", "until", "till", "provided", "assuming",
    "before", "soon",
];

/// What a plain yes is made of: a message of these words only agrees to a proposed merge.
const AGREEMENT: &[&str] = &[
    "yes",
    "yeah",
    "yep",
    "yup",
    "ya",
    "y",
    "sure",
    "ok",
    "okay",
    "k",
    "alright",
    "go",
    "ahead",
    "do",
    "it",
    "for",
    "please",
    "pls",
    "sounds",
    "good",
    "great",
    "fine",
    "perfect",
    "lgtm",
    "approved",
    "approve",
    "ship",
    "thanks",
    "thank",
    "you",
    "absolutely",
    "definitely",
    "of",
    "course",
    "lets",
    "let's",
    "right",
    "away",
    "now",
    "that",
    "this",
];

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

fn says_merge(words: &[String]) -> bool {
    words.iter().any(|word| word.starts_with("merg"))
}

/// A hold word, or a negated one ("don't", "isn't", "shouldn't").
fn holds(word: &str) -> bool {
    HOLDS.contains(&word)
}

/// "can you merge it?" and the like: a request phrased as a question, with one question mark
/// at its end.
fn polite_request(message: &str, words: &[String]) -> bool {
    let Some(body) = message.trim_end().strip_suffix('?') else {
        return false;
    };
    !body.contains('?')
        && words.len() >= 3
        && ["can", "could", "would", "will"].contains(&words[0].as_str())
        && words[1] == "you"
        && says_merge(words)
}

/// Whether `reply` proposed merging the session: a question that says merge and names the
/// session branch or its base.
pub(super) fn proposes(reply: &str, branch: &str, base: &str) -> bool {
    let reply = normalize(reply);
    let (branch, base) = (normalize(branch), normalize(base));
    let mut questions: Vec<&str> = reply.split('?').collect();
    // The text after the last question mark asks nothing.
    questions.pop();
    questions.into_iter().any(|before| {
        let sentence = before.rsplit(['.', '!', '\n']).next().unwrap_or(before);
        says_merge(&words(sentence))
            && ((!base.is_empty() && sentence.contains(&base))
                || (!branch.is_empty() && sentence.contains(&branch)))
    })
}

/// Whether `quoted`, the words the thread passed, give consent in `latest`, the user's latest
/// message; `before` is the thread's reply right before it. The error says why not.
pub(super) fn check(
    quoted: &str,
    latest: &str,
    before: Option<&str>,
    branch: &str,
    base: &str,
) -> Result<(), String> {
    let quoted = normalize(quoted);
    let quoted = quoted.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    let quoted_words = words(quoted);
    if quoted_words.is_empty() {
        return Err(
            "pass the user's own words that ask for the merge, quoted from their latest message"
                .into(),
        );
    }
    let message = normalize(latest);
    let message_words = words(&message);
    // Word for word, whatever the punctuation between.
    if !format!(" {} ", message_words.join(" ")).contains(&format!(" {} ", quoted_words.join(" ")))
    {
        return Err(format!(
            "\"{quoted}\" is not in the user's latest message; quote their own words from it"
        ));
    }
    if message.contains('?') && !polite_request(&message, &message_words) {
        return Err(
            "the user's latest message asks a question; answer it, and merge only on a plain yes"
                .into(),
        );
    }
    if let Some(word) = message_words.iter().find(|word| holds(word)) {
        return Err(format!(
            "the user's latest message says \"{word}\"; it doesn't plainly ask for the merge"
        ));
    }
    if let Some(word) = message_words
        .iter()
        .find(|word| CONDITIONS.contains(&word.as_str()))
    {
        return Err(format!(
            "the user's latest message puts a condition on it (\"{word}\"); merge only on a plain yes once it is met"
        ));
    }
    if says_merge(&quoted_words) {
        return Ok(());
    }
    let agrees = |words: &[String]| words.iter().all(|word| AGREEMENT.contains(&word.as_str()));
    if !(agrees(&quoted_words) && agrees(&message_words)) {
        return Err("the user's latest message doesn't ask for the merge".into());
    }
    match before {
        Some(reply) if proposes(reply, branch, base) => Ok(()),
        _ => Err(format!(
            "a plain yes agrees only to a merge your reply right before it proposed, as a question that names `{base}`; it didn't"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROPOSAL: &str =
        "The change landed and the review is clean. Merge `brigadier/s1/session` into `main`?";

    fn ok(quoted: &str, latest: &str, before: Option<&str>) -> bool {
        check(quoted, latest, before, "brigadier/s1/session", "main").is_ok()
    }

    #[test]
    fn a_direct_ask_merges_without_a_proposal() {
        assert!(ok("merge it", "merge it", None));
        assert!(ok("Merge it", "Looks good. Merge it.", None));
        assert!(ok("please merge", "please merge", None));
        assert!(ok("merge it", "Can you merge it?", None));
        assert!(ok(
            "go ahead and merge",
            "Go ahead and merge — thanks!",
            None
        ));
        assert!(ok(
            "merge it",
            "The current comment is fine, merge it",
            None
        ));
    }

    #[test]
    fn a_plain_yes_merges_only_after_a_proposal() {
        assert!(ok("yes", "yes", Some(PROPOSAL)));
        assert!(ok("yes, merge it", "yes, merge it", Some(PROPOSAL)));
        assert!(ok("ok go ahead", "OK, go ahead!", Some(PROPOSAL)));
        assert!(!ok("yes", "yes", None));
        assert!(!ok("yes", "yes", Some("The change landed. Anything else?")));
        // Saying merge isn't proposing it.
        assert!(!ok(
            "yes",
            "yes",
            Some("I won't merge yet; the tests run. OK?")
        ));
        assert!(!ok(
            "yes",
            "yes",
            Some("Merged nothing so far. Should I start on the docs?")
        ));
    }

    #[test]
    fn a_no_a_hold_or_a_condition_refuses() {
        for latest in [
            "no",
            "No, don't merge it",
            "don't merge it yet",
            "do not merge",
            "wait",
            "not yet",
            "hold on, merge it later",
            "merge it if the review passes",
            "merge it once the tests pass",
            "merge it after you fix the docs",
            "merge it as soon as CI is green",
            "ok but wait for the review",
            "I wouldn't merge it",
        ] {
            let quoted = if latest.contains("merge") {
                "merge"
            } else {
                latest
            };
            assert!(!ok(quoted, latest, Some(PROPOSAL)), "{latest}");
        }
    }

    #[test]
    fn a_question_refuses() {
        assert!(!ok("merge it", "should I merge it?", Some(PROPOSAL)));
        assert!(!ok("yes", "yes? what did the review say?", Some(PROPOSAL)));
        assert!(!ok("merge", "merge? hmm", Some(PROPOSAL)));
    }

    #[test]
    fn the_words_must_be_the_users_own() {
        assert!(!ok("merge it", "Thanks, looks good", Some(PROPOSAL)));
        assert!(!ok("", "merge it", None));
        assert!(!ok("\"\"", "merge it", None));
        // A yes inside a longer message is not a plain yes.
        assert!(!ok("yes", "yes, and also rename the flag", Some(PROPOSAL)));
    }

    #[test]
    fn a_proposal_names_the_branch_or_its_base() {
        assert!(proposes("Shall I merge it into main?", "b", "main"));
        assert!(proposes(
            "Done. Merge `brigadier/s1/session`?",
            "brigadier/s1/session",
            "main"
        ));
        assert!(!proposes(
            "Shall I merge it?",
            "brigadier/s1/session",
            "main"
        ));
        assert!(!proposes("Merge into main.", "b", "main"));
    }
}
