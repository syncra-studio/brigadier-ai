use std::path::{Path, PathBuf};

use super::*;

struct Record(PathBuf);

impl Record {
    fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("brigadier-ledger-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn ledger(&self) -> String {
        std::fs::read_to_string(self.0.join(LEDGER_FILE)).unwrap()
    }

    fn write(&self, file: &str, text: &str) {
        std::fs::write(self.0.join(file), text).unwrap();
    }

    fn add(&self, text: &str) -> String {
        add_entry(&self.0, text, stamp()).unwrap()
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn stamp() -> Stamp {
    Stamp {
        at: "10-23 11:40".into(),
        generation: 2,
    }
}

#[test]
fn ids_count_up_and_survive_a_move_to_the_archive() {
    let record = Record::new();
    assert_eq!(
        record.add("Rule from the user: \"tabs never\" (message 1)"),
        "L1\n"
    );
    assert_eq!(
        record.add("- Decision: keep spaces\n(user decided)"),
        "L2\n"
    );
    assert_eq!(record.add("Exact identifier: abc123"), "L3\n");
    assert_eq!(
        record.ledger(),
        "- L1 · 10-23 11:40 · gen 2 · Rule from the user: \"tabs never\" (message 1)\n\
         - L2 · 10-23 11:40 · gen 2 · Decision: keep spaces (user decided)\n\
         - L3 · 10-23 11:40 · gen 2 · Exact identifier: abc123\n"
    );
    // The thread moves finished entries to the archive by hand: the ids go on from there.
    let ledger = record.ledger();
    let (moved, kept) = ledger.split_at(ledger.find("- L3").unwrap());
    record.write(LEDGER_ARCHIVE_FILE, moved);
    record.write(LEDGER_FILE, kept);
    assert_eq!(record.add("Open: tell the user"), "L4\n");
    // Even when the ledger is emptied whole, an id is never reused.
    let all = format!("{moved}{}", record.ledger());
    record.write(LEDGER_ARCHIVE_FILE, &all);
    record.write(LEDGER_FILE, "");
    assert_eq!(record.add("Ruled out: tabs"), "L5\n");
    assert_eq!(
        record.ledger(),
        "- L5 · 10-23 11:40 · gen 2 · Ruled out: tabs\n"
    );
}

#[test]
fn close_appends_and_rewrites_nothing() {
    let record = Record::new();
    record.add("Open: the 02:45 question waits on the user (message 4)");
    let before = record.ledger();
    assert_eq!(
        close_entry(
            record.path(),
            "L1",
            "asked again at 11:10; answered \"yes\"",
            stamp()
        )
        .unwrap(),
        "L2\n"
    );
    assert_eq!(
        record.ledger(),
        format!(
            "{before}- L2 · 10-23 11:40 · gen 2 · Closed L1: asked again at 11:10; answered \
             \"yes\"\n"
        )
    );
    // An id the ledger doesn't have, or no id at all, is refused and changes nothing.
    let now = record.ledger();
    for (id, how) in [("L9", "done"), ("tabs", "done"), ("L1", "  ")] {
        assert!(
            close_entry(record.path(), id, how, stamp()).is_err(),
            "{id}"
        );
    }
    assert_eq!(record.ledger(), now);
}

#[test]
fn show_says_the_ledger_is_empty_instead_of_printing_nothing() {
    let record = Record::new();
    assert_eq!(
        show_entries(record.path(), false, false).unwrap(),
        "No entries yet.\n"
    );
}

#[test]
fn show_open_leaves_out_closed_items_and_rules_leaves_out_superseded_ones() {
    let record = Record::new();
    record.write(
        LEDGER_ARCHIVE_FILE,
        "- L1 · 10-22 09:00 · gen 1 · Rule from the user: \"no Fable\" (message 2)\n",
    );
    record.add("Open: promise to rerun the battery");
    record.add("Open: question on the PR words waits on the user");
    record.add("Rule from the user: \"tests on Linux\" (message 5)");
    record.add("Rule from the user: \"tests on Linux and macOS\" (message 9); supersedes L4");
    record.add("Ruled out: a second ledger file (one file is enough)");
    close_entry(record.path(), "L2", "reran it", stamp()).unwrap();
    let whole = record.ledger();
    assert_eq!(show_entries(record.path(), false, false).unwrap(), whole);
    assert_eq!(
        show_entries(record.path(), true, false).unwrap(),
        "Open items (1):\n\
         - L3 · 10-23 11:40 · gen 2 · Open: question on the PR words waits on the user\n"
    );
    assert_eq!(
        show_entries(record.path(), false, true).unwrap(),
        "Rules and decisions (2):\n\
         - L1 · 10-22 09:00 · gen 1 · Rule from the user: \"no Fable\" (message 2)\n\
         - L5 · 10-23 11:40 · gen 2 · Rule from the user: \"tests on Linux and macOS\" \
         (message 9); supersedes L4\n"
    );
    let both = show_entries(record.path(), true, true).unwrap();
    assert!(both.starts_with("Open items (1):\n"));
    assert!(both.contains("\n\nRules and decisions (2):\n"));
}

const FIXTURE_LEDGER: &str = "# Ledger\n\
\n\
- L1 · 10-23 08:00 · gen 1 · Rule from the user: \"Sources = what this thread looked at from \
outside the repo\" (message 3)\n\
- L2 · 10-23 08:05 · gen 1 · Rule from the user: \"tests on Linux\" (message 4)\n\
- L3 · 10-23 08:10 · gen 1 · Decision: PR words \"Open, Merged, Closed\"; the user answered \
\"A: green, purple, grey\" (message 6)\n  reason: matches GitHub\n\
- L4 · 10-23 08:20 · gen 1 · Open: ask the user about the terms page\n\
- L5 · 10-23 09:00 · gen 2 · Rule from the user: \"tests on Linux and macOS\"; supersedes L2\n\
- L6 · 10-23 09:30 · gen 2 · Closed L4: asked; the user said later\n\
- Decision: no new setting (I decided; written by hand)\n\
- L7 · 10-23 09:40 · gen 2 · Learned from reading: crates/x.rs:12 builds the map\n";

const FIXTURE_PLAN: &str = "# Plan\n\
\n\
1. Sources list what the thread read from outside the repo (L1).\n\
2. CI runs the tests on Linux only (L2).\n\
3. Ask about the terms page first (L4), then build it.\n\
4. A cache of 40 entries (proposed); see also L99 and CL1 and L1a.\n";

#[test]
fn check_lists_exactly_the_uncited_the_ended_and_the_entries_with_no_id() {
    let record = Record::new();
    record.write(LEDGER_FILE, FIXTURE_LEDGER);
    let check = check_file(record.path(), "plan.md", FIXTURE_PLAN).unwrap();
    assert_eq!(
        check,
        LedgerCheck {
            file: "plan.md".into(),
            uncited: vec![
                LedgerEntry {
                    id: "L3".into(),
                    entry: "- L3 · 10-23 08:10 · gen 1 · Decision: PR words \"Open, Merged, \
                            Closed\"; the user answered \"A: green, purple, grey\" (message 6)\n  \
                            reason: matches GitHub"
                        .into(),
                },
                LedgerEntry {
                    id: "L5".into(),
                    entry: "- L5 · 10-23 09:00 · gen 2 · Rule from the user: \"tests on Linux \
                            and macOS\"; supersedes L2"
                        .into(),
                },
            ],
            ended: vec![
                LedgerEnded {
                    id: "L2".into(),
                    how: "superseded".into(),
                    by: Some("L5".into()),
                },
                LedgerEnded {
                    id: "L4".into(),
                    how: "closed".into(),
                    by: Some("L6".into()),
                },
            ],
            unknown: vec!["L99".into()],
            no_id: vec![LedgerNoId {
                file: "ledger.md".into(),
                line: 10,
                entry: "- Decision: no new setting (I decided; written by hand)".into(),
            }],
        }
    );
    let text = render(&check);
    assert!(text.starts_with("plan.md against the ledger:\n"));
    for section in [
        "\nRules and decisions it never cites (2):\n- L3 ·",
        "\nIds it cites that are closed or superseded (2):\n- L2 (superseded by L5)\n- L4 \
         (closed by L6)\n",
        "\nIds it cites that the ledger doesn't have (1):\n- L99\n",
        "\nLedger entries with no id (1):\n- ledger.md:10: - Decision: no new setting",
    ] {
        assert!(text.contains(section), "{section}\n{text}");
    }
    // A file that cites every rule and decision, and a ledger all with ids: nothing to list.
    record.write(
        LEDGER_FILE,
        &FIXTURE_LEDGER.replace("- Decision: no new", "- L8 · Decision: no new"),
    );
    let clean = check_file(record.path(), "brief.md", "Per L1, L3, L5 and L8.").unwrap();
    assert!(clean.uncited.is_empty() && clean.ended.is_empty() && clean.no_id.is_empty());
    assert!(render(&clean).contains("Nothing to list"));
}

#[test]
fn kinds_are_read_from_whole_words() {
    let kinds: Vec<Kind> = parse_entries(
        "- Ruled out: tabs\n- Opened PR #4\n- Open question: which nozzle?\n- Rules from the \
         user: \"a\", \"b\"\n- **Decision**: keep\n- I decided: spaces\n- L3 · Closed L1, L2: \
         done\n",
        LedgerFile::Ledger,
    )
    .into_iter()
    .map(|entry| entry.kind)
    .collect();
    assert_eq!(
        kinds,
        [
            Kind::Other,
            Kind::Other,
            Kind::Open,
            Kind::Rule,
            Kind::Decision,
            Kind::Decision,
            Kind::Close(vec![1, 2]),
        ]
    );
    assert_eq!(cited("L1, (L22) xL3 L4b L5."), [1, 22, 5]);
    // A letter that lowercases longer before "supersedes" still finds the id after it.
    assert_eq!(supersedes("Rule: İİİİ; Supersedes L1"), [1]);
}
