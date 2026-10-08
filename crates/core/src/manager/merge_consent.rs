//! Whether the user's latest message asks for the session's merge, checked in code before
//! `finish_session` merges: the thread passes the user's words, quoted from that message.
//!
//! Consent is plain and unconditional, and anything in doubt refuses (the thread can just ask
//! again): the words are in the latest message; that message asks no question (bar "can you
//! merge it?"), sets no condition ("if", "once", "after", …) and says no "no", "wait" or
//! "don't"; and either the words ask for the merge, as a request ("merge it", "please merge",
//! "go ahead and merge", "… then merge it into main"), or the whole message is a plain yes to a
//! reply that offered this session's merge, as a question naming its branch or base ("Merge
//! `brigadier/s1/session` into `main`?") with no hold or alternative in it. No model judges it.

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

/// What may come before a requested "merge" in its clause: "please merge", "ok, go ahead and
/// merge", "can you merge it". Anything else ("explain the merge", "how do I merge it") isn't a
/// request.
const REQUEST_LEADS: &[&str] = &[
    "please", "pls", "ok", "okay", "yes", "yeah", "yep", "sure", "great", "good", "fine",
    "perfect", "alright", "thanks", "now", "just", "so", "go", "ahead", "you", "can", "could",
    "would", "will", "lets", "let's", "also", "cool", "nice",
];

/// Words that start a request of their own within a clause: "… and merge it", "… then merge".
const REQUEST_BOUNDS: &[&str] = &["and", "then"];

/// What a requested "merge" may take: "merge it", "merge this", "merge everything".
const MERGE_OBJECTS: &[&str] = &["it", "this", "that", "them", "everything", "all"];

/// What "merge the …" may name: "merge the session", "merge my branch".
const MERGE_TARGETS: &[&str] = &[
    "session", "branch", "work", "change", "changes", "commit", "commits", "pr",
];

/// What may follow a requested "merge" directly: "merge into main", "merge now".
const MERGE_AFTER: &[&str] = &["into", "to", "now", "please", "right"];

/// Words a merge proposal may not hold: one that offers to wait, or an alternative, offers
/// something else than the merge ("Should I wait to merge into main?", "… or keep it?").
const PROPOSAL_HOLDS: &[&str] = &[
    "postpone", "defer", "delay", "or", "instead", "rather", "first", "yet", "skip", "keep",
    "leave", "until", "till", "before", "after", "if", "once", "when", "unless",
];

/// What may come before "merge" in a question that offers it: "Merge …?", "Shall I merge …?",
/// "Do you want me to merge …?", "Ready to merge …?".
const PROPOSAL_LEADS: &[&str] = &[
    "shall", "should", "i", "can", "may", "want", "me", "to", "do", "you", "would", "like",
    "ready", "ok", "okay", "now", "so", "then", "go", "ahead", "and",
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

/// The words with the clause each is in: punctuation ends a clause.
fn clause_words(text: &str) -> Vec<(String, usize)> {
    text.split_inclusive(|c: char| ".,;:!?()\n\u{2014}\u{2013}\"".contains(c))
        .enumerate()
        .flat_map(|(clause, piece)| words(piece).into_iter().map(move |word| (word, clause)))
        .collect()
}

fn says_merge(words: &[String]) -> bool {
    words.iter().any(|word| word == "merge")
}

/// Whether `inner` is in `outer`, word for word.
fn contains_words(outer: &[String], inner: &[String]) -> bool {
    !inner.is_empty() && outer.windows(inner.len()).any(|window| window == inner)
}

/// Whether the "merge" at `at` is asked for: only request words before it in its clause (back
/// to an "and" or "then"), and after it nothing, an object ("it", "the branch") or "into".
fn requested_at(tokens: &[(String, usize)], at: usize) -> bool {
    let clause = tokens[at].1;
    let mut before = at;
    let led = loop {
        if before == 0 || tokens[before - 1].1 != clause {
            break true;
        }
        let word = tokens[before - 1].0.as_str();
        if REQUEST_BOUNDS.contains(&word) {
            break true;
        }
        if !REQUEST_LEADS.contains(&word) {
            break false;
        }
        before -= 1;
    };
    let word = |at: usize| {
        tokens
            .get(at)
            .filter(|(_, of)| *of == clause)
            .map(|(word, _)| word.as_str())
    };
    let followed = match word(at + 1) {
        None => true,
        Some(next) if MERGE_OBJECTS.contains(&next) || MERGE_AFTER.contains(&next) => true,
        Some("the" | "my" | "our" | "your") => {
            word(at + 2).is_some_and(|target| MERGE_TARGETS.contains(&target))
        }
        Some(_) => false,
    };
    led && followed
}

/// Whether the quoted words, where they stand in `message`, ask for the merge.
fn requests_merge(message: &str, quoted: &[String]) -> bool {
    let tokens = clause_words(message);
    let words: Vec<String> = tokens.iter().map(|(word, _)| word.clone()).collect();
    (0..words.len().saturating_sub(quoted.len() - 1))
        .filter(|&start| words[start..start + quoted.len()] == *quoted)
        .any(|start| {
            (start..start + quoted.len())
                .any(|at| words[at] == "merge" && requested_at(&tokens, at))
        })
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

/// Whether `reply` offered the session's merge: a question that opens with the offer ("Merge
/// …?", "Shall I merge …?"), names the session branch or its base, and holds no hold, negation
/// or alternative.
pub(super) fn proposes(reply: &str, branch: &str, base: &str) -> bool {
    let reply = normalize(reply);
    let (branch, base) = (words(&normalize(branch)), words(&normalize(base)));
    let mut questions: Vec<&str> = reply.split('?').collect();
    // The text after the last question mark asks nothing.
    questions.pop();
    questions.into_iter().any(|before| {
        let sentence = before
            .rsplit(['.', '!', ';', ':', '\n'])
            .next()
            .unwrap_or(before);
        let words = words(sentence);
        let Some(at) = words.iter().position(|word| word == "merge") else {
            return false;
        };
        words[..at]
            .iter()
            .all(|word| PROPOSAL_LEADS.contains(&word.as_str()))
            && !words
                .iter()
                .any(|word| holds(word) || PROPOSAL_HOLDS.contains(&word.as_str()))
            && (contains_words(&words, &base) || contains_words(&words, &branch))
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
    if requests_merge(&message, &quoted_words) {
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
    fn only_a_request_for_the_merge_counts_not_the_word() {
        for (quoted, latest) in [
            ("merge", "Explain the merge strategy"),
            ("merge strategy", "Explain the merge strategy"),
            ("merged it", "I merged it"),
            ("merger", "the merger looks fine"),
            ("merge it", "how do I merge it"),
            ("merge", "the merge conflict is gone"),
            ("merge", "merge conflicts are annoying"),
        ] {
            assert!(!ok(quoted, latest, None), "{latest}");
        }
        for (quoted, latest) in [
            ("merge this", "merge this"),
            ("merge the session", "Merge the session."),
            ("merge the branch into main", "merge the branch into main"),
            ("merge into main", "Merge into main"),
            ("go ahead and merge", "go ahead and merge"),
            ("please merge", "please merge"),
            ("can you merge it", "can you merge it?"),
            ("could you merge it", "Could you merge it please"),
            ("then merge it", "commit it then merge it"),
            (
                "then merge it into main",
                "Create THIRD.md and commit it yourself, then merge it into main.",
            ),
            ("merge", "Add notes and merge."),
        ] {
            assert!(ok(quoted, latest, None), "{latest}");
        }
    }

    #[test]
    fn a_proposal_to_wait_or_to_choose_is_no_offer_to_merge() {
        for reply in [
            "Should I wait to merge into main?",
            "Merge into `main`, or keep it on its branch?",
            "Shall I merge it into main later?",
            "Should I not merge into main?",
            "Merge into main after the tests?",
            "Do you want me to hold off and merge into main tomorrow instead?",
        ] {
            assert!(!ok("yes", "yes", Some(reply)), "{reply}");
        }
        for reply in [
            "Do you want me to merge it into `main`?",
            "Ready to merge into main?",
            "All landed. Shall I merge `brigadier/s1/session` into `main`?",
        ] {
            assert!(ok("yes", "yes", Some(reply)), "{reply}");
        }
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
