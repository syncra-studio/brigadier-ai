use std::path::Path;

use brigadier_providers::session_file::FileEntry;
use brigadier_providers::{ProviderEvent, ProviderKind, Role};

use super::*;

fn message(id: &str, role: Role, text: &str) -> FileEntry {
    FileEntry {
        event: ProviderEvent::Message {
            item_id: id.into(),
            role,
            text: text.into(),
        },
        at_ms: None,
        updated_at_ms: None,
        parent: None,
        steer: false,
        images: Vec::new(),
        phase: None,
    }
}

/// A transcript of `exchanges` user messages, each answered, the first one `first`.
fn transcript(exchanges: usize, answer: &str) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    for i in 0..exchanges {
        let text = if i == 0 {
            "FIRST: the standing rule".to_owned()
        } else {
            format!("user {i}")
        };
        entries.push(message(&format!("u{i}"), Role::User, &text));
        entries.push(message(
            &format!("a{i}"),
            Role::Assistant,
            &format!("answer {i} {answer}"),
        ));
    }
    entries
}

fn boot<'a>(entries: &'a [FileEntry], note: Option<&'a str>, death: Option<&'a str>) -> String {
    bootstrap(&Boot {
        n: 2,
        key: "abcd1234",
        folder: Path::new("/s/abcd1234"),
        ledger_path: Path::new("/s/abcd1234/record/ledger.md"),
        ledger: Some("- Rule from the user: keep it short"),
        lineage: &[],
        note_path: Path::new("/s/abcd1234/record/handoff/2.md"),
        note,
        start_path: Path::new("/s/abcd1234/record/handoff/2-start.md"),
        death,
        entries,
        blobs: Path::new("/nowhere"),
    })
}

/// The `#from–#to` spans the hand-off says are left out, from their read commands.
fn spans(text: &str) -> Vec<(usize, usize)> {
    text.lines()
        .filter_map(|line| line.strip_prefix("- #"))
        .filter_map(|line| {
            let command = line.split('`').nth(1)?;
            let words: Vec<&str> = command.split_whitespace().collect();
            assert_eq!(&words[..4], ["brigadier", "brain", "read", "abcd1234"]);
            Some((words[4].parse().ok()?, words[5].parse().ok()?))
        })
        .collect()
}

/// The positions the hand-off shows, by their `[#n · role]` headers.
fn shown(text: &str) -> Vec<usize> {
    text.lines()
        .filter_map(|line| line.strip_prefix("[#"))
        .filter_map(|line| line.split(' ').next()?.parse().ok())
        .collect()
}

#[test]
fn the_handoff_comes_in_its_order() {
    let entries = transcript(400, &"x".repeat(400));
    let text = boot(&entries, Some("doing now: the parser"), None);
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing"))
    };
    assert!(
        text.starts_with("[Brigadier hand-off 2 · abcd1234]\nYou continue an earlier session.")
    );
    assert!(at("## Map") < at("## Ledger"));
    assert!(at("## Ledger") < at("## Hand-off note"));
    assert!(at("## Hand-off note") < at("## The session's first message"));
    assert!(at("## The session's first message") < at("## The latest exchange"));
    assert!(at("## The latest exchange") < at("## Earlier, newest first"));
    assert!(at("## Earlier, newest first") < at("## Left out, each a read away"));
    assert!(text.ends_with(HANDOFF_END));
    // The first message is always there, and the latest exchange whole.
    assert!(text.contains("[#0 · user]\nFIRST: the standing rule"));
    assert!(text.contains("[#798 · user]\nuser 399\n\n[#799 · assistant]\nanswer 399"));
    // The tail runs newest first, right before the latest exchange.
    let tail = &text[at("## Earlier, newest first")..at("## Left out")];
    let positions = shown(tail);
    assert_eq!(positions[..3], [797, 796, 795]);
    assert!(positions.windows(2).all(|pair| pair[0] == pair[1] + 1));
    // ≈15k tokens of it, no more.
    assert!(tail.len() <= 60_000 + 1_000, "{}", tail.len());
    assert!(!text.contains("The previous session ended without a hand-off note"));
}

#[test]
fn every_item_left_out_is_named_by_a_read_that_returns_all_of_it() {
    let entries = transcript(400, &"x".repeat(400));
    let text = boot(&entries, None, None);
    let mut covered: Vec<usize> = shown(&text);
    for (from, to) in spans(&text) {
        assert!(from <= to && to - from < READ_MAX as usize, "{from}–{to}");
        covered.extend(from..=to);
    }
    covered.sort_unstable();
    covered.dedup();
    assert_eq!(covered, (0..entries.len()).collect::<Vec<_>>());
}

#[test]
fn a_death_says_to_check_git_first() {
    let entries = transcript(3, "short");
    let text = boot(
        &entries,
        None,
        Some("its context was too long for the model"),
    );
    assert!(text.contains(
        "The previous session ended without a hand-off note (its context was too long for the \
         model): check `git status` and `git log` first."
    ));
    assert!(text.contains("- Hand-off note: none (its context was too long for the model)"));
    assert!(!text.contains("## Hand-off note"));
    // Short: nothing is left out.
    assert!(!text.contains("## Left out"));
}

#[test]
fn an_earlier_handoff_is_not_carried_again() {
    let mut entries = transcript(3, "short");
    let ProviderEvent::Message { text, .. } = &mut entries[2].event else {
        unreachable!()
    };
    *text = format!(
        "{}\nold map{HANDOFF_END}\n\nuser 1",
        handoff_line(1, "abcd1234")
    );
    let text = boot(&entries, None, None);
    assert!(!text.contains("old map"));
    assert!(text.contains("[#2 · user]\nuser 1"));
}

#[test]
fn a_long_item_is_cut_with_its_read() {
    let mut entries = transcript(3, "short");
    let ProviderEvent::Message { text, .. } = &mut entries[3].event else {
        unreachable!()
    };
    *text = "y".repeat(20_000);
    let text = boot(&entries, None, None);
    assert!(text.contains(
        "[cut at 6000 of 20000 bytes; `brigadier brain read abcd1234 3 3` gives it whole]"
    ));
}

#[test]
fn marks_strip_the_handoff_and_place_the_dividers() {
    let dir = std::env::temp_dir().join(format!("brigadier-seam-{}", uuid::Uuid::new_v4()));
    let record = dir.as_path();
    std::fs::create_dir_all(record.join("handoff")).unwrap();
    std::fs::write(note_path(record, 1), "note").unwrap();
    let end = |death: Option<&str>| SeamEnd {
        predecessor: Some("p".into()),
        nudged_at_ms: Some(1),
        closed_at_ms: 2,
        death: death.map(str::to_owned),
    };
    let seam = |n: u32, death: Option<&str>| Seam {
        n,
        end: end(death),
        successor: format!("s{n}"),
        provider: ProviderKind::Claude,
        started_at_ms: 3,
        note: (n == 1).then(|| note_path(record, 1).display().to_string()),
        start: start_path(record, n).display().to_string(),
        messages: Vec::new(),
    };
    write_seam(
        record,
        &SeamFile {
            generation: 3,
            phase: Phase::Live,
            seams: vec![seam(1, None), seam(2, Some(TOO_LONG_TEST))],
        },
    )
    .unwrap();
    let mut entries = vec![
        message("u0", Role::User, "hello"),
        message("n1", Role::User, &nudge(&note_path(record, 1))),
        message(
            "u1",
            Role::User,
            &format!("{}\nmap{HANDOFF_END}\n\nnext", handoff_line(1, "k")),
        ),
        message(
            "u2",
            Role::User,
            &format!("{}\nmap{HANDOFF_END}\n\nagain", handoff_line(2, "k")),
        ),
    ];
    let marks = marks(record, &mut entries);
    assert_eq!(marks.len(), 2);
    assert_eq!(marks[0].at.as_deref(), Some("u1"));
    assert_eq!(marks[0].nudge.as_deref(), Some("n1"));
    assert!(marks[0].note.is_some() && marks[0].death.is_none());
    assert_eq!(marks[1].at.as_deref(), Some("u2"));
    assert_eq!(marks[1].nudge, None);
    assert_eq!(marks[1].death.as_deref(), Some(TOO_LONG_TEST));
    let ProviderEvent::Message { text, .. } = &entries[2].event else {
        unreachable!()
    };
    assert_eq!(text, "next");
    std::fs::remove_dir_all(&dir).unwrap();
}

const TOO_LONG_TEST: &str = "its context was too long for the model";

#[test]
fn the_threshold_leaves_room_in_a_small_window() {
    let config = SeamConfig {
        on: true,
        tokens: 200_000,
        hold: HOLD,
        carry_on: false,
    };
    assert_eq!(config.threshold(Some(1_000_000)), 200_000);
    assert_eq!(config.threshold(Some(200_000)), 160_000);
    assert_eq!(config.threshold(None), 200_000);
}
