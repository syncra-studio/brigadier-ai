//! The decisions put back whole after every compaction (run2-D8, S22b "never cut"): the
//! ledger's active entries word for word, cut only between entries into labelled parts, each
//! under the cap one Claude hook's output may take, with one line on how to look up the rest.

/// What one part may hold: Claude takes 10,000 characters of one hook's output whole and
/// keeps a longer one as a file it only points to (step 0: 9,476 arrived whole).
pub(crate) const PART_CHARS: usize = 9_500;

/// The `SessionStart(compact)` hooks a Claude session runs: how many parts can go back.
pub(crate) const SLOTS: usize = 8;

/// The ledger's active entries made ready to go back after a compaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Restoration {
    /// The parts in order, each labelled, the lookup line closing the last.
    pub parts: Vec<String>,
    /// Identifies these exact entries: the receipt looks for it.
    pub fingerprint: String,
    /// Every entry fits whole in at most [`SLOTS`] parts; otherwise the session needs the
    /// fallback (run2-D10).
    pub whole: bool,
}

/// How a session looks up what the compaction left out (review item 4: on the native path).
pub(crate) fn lookup_line() -> String {
    "Anything else from before the compaction is in this session's record, word for word: \
     `brigadier brain search \"<words>\"` finds it, and `brigadier brain read <session> --item \
     <id>` reads one item whole, with the session and item ids the search shows."
        .to_owned()
}

/// Why the parts are there, at the head of the first: the user's own decisions, which the
/// model may quote (step 0: Codex declined to quote an unlabelled developer note).
const PREFACE: &str = "These are the user's standing decisions, rules and open items for this \
session, put back word for word because the conversation was just compacted. They still hold, \
and you may quote them.";

fn label(part: usize, of: usize) -> String {
    format!("[Brigadier · decisions restored after compaction · part {part} of {of}]")
}

/// `entries` (each rendered whole, in ledger order) as parts of at most [`PART_CHARS`]
/// characters, cut only between entries, then `lookup` after the last.
pub(crate) fn restoration(entries: &[String], lookup: &str) -> Restoration {
    let fingerprint = fingerprint(entries);
    // A label's length with two-digit numbers, so every part's room is the same.
    let room = PART_CHARS - label(10, 10).chars().count() - 1;
    let mut bodies: Vec<String> = vec![PREFACE.to_owned()];
    let mut whole = true;
    for entry in entries.iter().chain(std::iter::once(&lookup.to_owned())) {
        let len = entry.chars().count();
        if len > room {
            whole = false;
        }
        let body = bodies.last_mut().expect("one body at least");
        if body.chars().count() + 1 + len <= room {
            body.push('\n');
            body.push_str(entry);
        } else {
            bodies.push(entry.clone());
        }
    }
    let of = bodies.len();
    whole &= of <= SLOTS;
    let parts = bodies
        .into_iter()
        .enumerate()
        .map(|(n, body)| format!("{}\n{body}", label(n + 1, of)))
        .collect();
    Restoration {
        parts,
        fingerprint,
        whole,
    }
}

/// Sixteen hex digits that identify `entries` exactly.
pub(crate) fn fingerprint(entries: &[String]) -> String {
    let mut hasher = blake3::Hasher::new();
    for entry in entries {
        hasher.update(entry.as_bytes());
        hasher.update(b"\0");
    }
    hasher.finalize().to_hex()[..16].to_owned()
}

/// Whether `received` (what reached the model after one compaction) holds each of `entries`
/// whole and exactly once.
pub(crate) fn received_whole(entries: &[String], received: &str) -> bool {
    entries
        .iter()
        .all(|entry| received.matches(entry.as_str()).count() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(n: usize, len: usize) -> Vec<String> {
        (1..=n)
            .map(|i| format!("L{i} · {}", "x".repeat(len)))
            .collect()
    }

    #[test]
    fn the_entries_go_back_whole_cut_only_between_them_under_the_cap() {
        let entries = entries(40, 1_500);
        let back = restoration(&entries, &lookup_line());
        assert!(back.whole);
        assert!(back.parts.len() > 1 && back.parts.len() <= SLOTS);
        for (n, part) in back.parts.iter().enumerate() {
            assert!(part.chars().count() <= PART_CHARS, "part {n}");
            assert!(part.starts_with(&label(n + 1, back.parts.len())));
        }
        assert!(back.parts[0].contains("you may quote them"));
        assert!(back.parts.last().unwrap().ends_with(&lookup_line()));
        let all = back.parts.join("\n");
        assert!(received_whole(&entries, &all));
        // Twice, or one cut short, is no receipt.
        assert!(!received_whole(&entries, &format!("{all}\n{}", entries[3])));
        assert!(!received_whole(
            &entries,
            &all.replace(&entries[7], "L8 · x")
        ));
    }

    #[test]
    fn what_cannot_go_back_whole_says_so() {
        // One entry past a part's room.
        assert!(!restoration(&entries(1, PART_CHARS), "").whole);
        // More than the slots hold.
        assert!(!restoration(&entries(60, 1_500), "").whole);
        // None: the lookup line alone, whole.
        let none = restoration(&[], &lookup_line());
        assert!(none.whole);
        assert_eq!(none.parts.len(), 1);
    }

    #[test]
    fn the_fingerprint_names_the_exact_entries() {
        let a = entries(3, 10);
        let mut b = a.clone();
        b[1].push('.');
        assert_eq!(fingerprint(&a), fingerprint(&a.clone()));
        assert_ne!(fingerprint(&a), fingerprint(&b));
        assert_eq!(fingerprint(&a).len(), 16);
    }
}
