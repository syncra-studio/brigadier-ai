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

/// `text` without the byte range `range`.
fn without(text: &str, range: &std::ops::Range<usize>) -> String {
    format!("{}{}", &text[..range.start], &text[range.end..])
}

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

/// Words that, anywhere in a message, stop what goes on whatever it is about: "please wait",
/// "hold on", "don't do it", "stop for now".
const STOPS: &[&str] = &[
    "wait",
    "stop",
    "hold",
    "hang",
    "cancel",
    "abort",
    "pause",
    "halt",
    "dont",
    "never",
    "nevermind",
    "later",
];

/// Words that, opening or ending a clause, answer no: "No.", "actually no", "nope, leave it".
const NOS: &[&str] = &["no", "nope", "nah"];

/// Pairs that put it off wherever they stand: "not yet", "not now".
const NOT_YET: &[[&str; 2]] = &[["not", "yet"], ["not", "now"], ["changed", "my"]];

/// What in a message the user wrote after they consented takes the consent back or puts it
/// off: a stop word anywhere ("wait", "hold on", "don't"), "not yet", a clause that opens or
/// ends with a "no", or, in a message that speaks of the merge, a hold or a condition. `None`
/// when nothing does: "tell me when it's done" is about something else. In doubt it takes
/// it back: asking again is the safe failure.
pub(super) fn takes_back(message: &str) -> Option<String> {
    let tokens = clause_words(&normalize(message));
    let edge = |at: usize| {
        at == 0
            || tokens[at - 1].1 != tokens[at].1
            || tokens.get(at + 1).is_none_or(|next| next.1 != tokens[at].1)
    };
    if let Some((_, (word, _))) = tokens.iter().enumerate().find(|(at, (word, _))| {
        STOPS.contains(&word.as_str()) || (NOS.contains(&word.as_str()) && edge(*at))
    }) {
        return Some(format!("says \"{word}\""));
    }
    let words: Vec<String> = tokens.into_iter().map(|(word, _)| word).collect();
    if let Some(pair) = words.windows(2).find(|pair| {
        NOT_YET
            .iter()
            .any(|stop| pair[0] == stop[0] && pair[1] == stop[1])
    }) {
        return Some(format!("says \"{} {}\"", pair[0], pair[1]));
    }
    if !says_merge(&words) {
        return None;
    }
    if let Some(word) = words.iter().find(|word| holds(word)) {
        return Some(format!("says \"{word}\""));
    }
    words
        .iter()
        .find(|word| CONDITIONS.contains(&word.as_str()))
        .map(|word| format!("puts a condition on it (\"{word}\")"))
}

/// Whether `quoted`, the words the thread passed, give consent in `latest`, the user's latest
/// message; `before` is the thread's reply right before it. The error says why not; a
/// consent says what it waits for: nothing, or the running work ending ("merge when done").
pub(super) fn check(
    quoted: &str,
    latest: &str,
    before: Option<&str>,
    branch: &str,
    base: &str,
) -> Result<MergeCondition, String> {
    let quoted = normalize(quoted);
    let quoted = quoted.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
    let message = normalize(latest);
    // "merge when done" is a plain yes that waits for the work: checked without its clause.
    let (quoted, message, condition) = match finish_clause(&message) {
        Some(clause) => {
            let quoted = match finish_clause(quoted) {
                Some(own) => without(quoted, &own),
                None => quoted.to_owned(),
            };
            (
                quoted,
                without(&message, &clause),
                MergeCondition::WorkersFinish,
            )
        }
        None => (quoted.to_owned(), message, MergeCondition::Now),
    };
    plain_check(&quoted, &message, before, branch, base).map(|()| condition)
}

/// [`check`] of a normalized message with no "when done" clause.
fn plain_check(
    quoted: &str,
    message: &str,
    before: Option<&str>,
    branch: &str,
    base: &str,
) -> Result<(), String> {
    let quoted = quoted.trim();
    let quoted_words = words(quoted);
    if quoted_words.is_empty() {
        return Err(
            "pass the user's own words that ask for the merge, quoted from their latest message"
                .into(),
        );
    }
    let message = message.to_owned();
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

    /// SESSION-FIXES-PLAN.md Q3: the running work ending is the one condition a consent may
    /// carry; any other still refuses.
    #[test]
    fn merge_when_done_waits_for_the_workers_and_other_conditions_refuse() {
        let condition = |quoted: &str, latest: &str| {
            check(quoted, latest, None, "brigadier/s1/session", "main")
        };
        for (quoted, latest) in [
            ("merge when done", "merge when done"),
            ("merge once they finish", "Merge once they finish."),
            (
                "merge it after the workers land",
                "merge it after the workers land",
            ),
            ("merge when it's all in", "merge when it's all in"),
            ("merge it", "Looks good, merge it when they're done"),
            (
                "merge into main once everything is complete",
                "merge into main once everything is complete",
            ),
            (
                "merge it when it has landed",
                "merge it when it has landed, thanks",
            ),
        ] {
            assert_eq!(
                condition(quoted, latest),
                Ok(MergeCondition::WorkersFinish),
                "{latest}"
            );
        }
        assert_eq!(condition("merge it", "merge it"), Ok(MergeCondition::Now));
        for latest in [
            "merge once the tests pass",
            "merge after you fix the docs",
            "merge it if the review is clean",
            "merge when done if the review is clean",
            "don't merge when done",
            "merge when done? not sure",
            "merge when the docs are in",
            "explain the merge when done",
        ] {
            assert!(condition(latest, latest).is_err(), "{latest}");
        }
    }

    #[test]
    fn a_later_message_takes_consent_back_when_it_stops_or_holds_the_merge() {
        for message in [
            "wait!",
            "No.",
            "ok, wait",
            "Stop, I changed my mind",
            "don't merge",
            "Hold off on the merge",
            "only merge once the review passes",
            "merge it after the docs are fixed",
            "let's not merge it yet",
            "please wait",
            "not yet",
            "Not now, the demo is on",
            "don't do it",
            "hold on",
            "Hold off for a bit",
            "stop for now",
            "actually no",
            "no",
            "Nope, leave it",
            "cancel that",
            "hang on a second",
            "never mind",
            "I changed my mind",
            "do it later",
            "Don't forget the changelog",
        ] {
            assert!(takes_back(message).is_some(), "{message}");
        }
        for message in [
            "Resolve the conflict.",
            "resolve it and tell me when it's done",
            "after that, also fix the typo in the README",
            "If the tests are slow, run only the core crate",
            "There's no rush, check it after lunch",
            "That's not what I meant by the README",
            "ok, go ahead and resolve it",
            "thanks, keep me posted",
        ] {
            assert_eq!(takes_back(message), None, "{message}");
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
