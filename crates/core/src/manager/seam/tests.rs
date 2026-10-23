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
    boot_tail(
        entries,
        note,
        death,
        TailBudget {
            reason: Reason::Size,
            hot_tokens: None,
            target_window: None,
        },
    )
}

fn boot_tail(
    entries: &[FileEntry],
    note: Option<&str>,
    death: Option<&str>,
    tail: TailBudget,
) -> String {
    bootstrap(&Boot {
        n: 2,
        tail,
        key: "abcd1234",
        folder: Path::new("/s/abcd1234"),
        ledger_path: Path::new("/s/abcd1234/record/ledger.md"),
        ledger: Some("- Rule from the user: keep it short"),
        archive_path: Path::new("/s/abcd1234/record/ledger-archive.md"),
        archive: None,
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
    assert!(text.starts_with(
        "[Brigadier hand-off 2 · abcd1234]\nYou continue an earlier session. The open items, \
         rules and decisions below are verbatim and authoritative; so are the note and last \
         exchanges.\nFirst add one ledger entry: \"Took over gen 3 · open <ids> · applies next: \
         <id> (how), …\". If a question waits on the user, raise the oldest one in this reply.\n"
    ));
    assert!(at("## Map") < at("## Open items, verbatim and whole"));
    assert!(at("## Open items") < at("## Rules and decisions, verbatim and whole"));
    assert!(at("## Rules and decisions") < at("## Hand-off note"));
    assert!(at("## Hand-off note") < at("## The user's messages, oldest first"));
    assert!(at("## The user's messages") < at("## The latest exchange, after #798 (above)"));
    assert!(at("## The latest exchange") < at("## Earlier, newest first"));
    assert!(at("## Earlier, newest first") < at("## Left out, each a read away"));
    assert!(text.ends_with(HANDOFF_END));
    assert!(text.contains(
        "## Rules and decisions, verbatim and whole\n- Rule from the user: keep it short\n"
    ));
    // Every message of the user's, oldest first, the first and the latest among them.
    let said = &text[at("## The user's messages")..at("## The latest exchange")];
    assert_eq!(shown(said), (0..400).map(|i| i * 2).collect::<Vec<_>>());
    assert!(said.contains("[#0 · user]\nFIRST: the standing rule"));
    assert!(!said.contains("Left out here"));
    // The latest exchange follows its message.
    assert!(text.contains("(above)\n[#799 · assistant]\nanswer 399"));
    // The tail runs newest first, right before the latest exchange.
    let tail = &text[at("## Earlier, newest first")..at("## Left out")];
    let positions = shown(tail);
    assert_eq!(positions[..3], [797, 795, 793]);
    assert!(positions.windows(2).all(|pair| pair[0] == pair[1] + 2));
    // ≈15k tokens of it, no more.
    assert!(tail.len() <= 60_000 + 1_000, "{}", tail.len());
    assert!(!text.contains("The previous session ended without a hand-off note"));
}

/// A ledger far over its budget: every open item, rule and decision still travels whole, and
/// the `sed` the hand-off names for the rest prints every entry it left out.
#[test]
fn a_ledger_over_budget_carries_every_open_item_rule_and_decision_whole() {
    let dir = std::env::temp_dir().join(format!("brigadier-seam-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let ledger_path = dir.join("ledger.md");
    let archive = "- L1 · 10-22 09:00 · gen 1 · Rule from the user: \"no Fable\" (message 1)\n\
                   - L2 · 10-22 09:05 · gen 1 · Open: the terms page waits on the user\n";
    let mut ledger = String::from("# Ledger\n\n");
    let mut pinned = vec![
        "- L1 · 10-22 09:00 · gen 1 · Rule from the user: \"no Fable\" (message 1)".to_owned(),
        "- L2 · 10-22 09:05 · gen 1 · Open: the terms page waits on the user".to_owned(),
    ];
    let mut closed = Vec::new();
    let mut id = 3;
    for i in 0..3_000 {
        let line = match i % 100 {
            0 => format!(
                "- L{id} · 10-23 10:00 · gen 2 · Rule from the user: \"rule {i}\" (message {i})"
            ),
            1 => format!(
                "- L{id} · 10-23 10:00 · gen 2 · Decision: option {i}; the user answered \
                 \"B: the long one\"\n  reason: {}",
                "because ".repeat(200)
            ),
            2 => format!("- L{id} · 10-23 10:00 · gen 2 · Open: promise {i}"),
            // Every promise but the last is kept and closed.
            3 if i < 2_900 => {
                closed.push(format!(
                    "- L{} · 10-23 10:00 · gen 2 · Open: promise {}",
                    id - 1,
                    i - 1
                ));
                format!("- L{id} · 10-23 10:00 · gen 2 · Closed L{}: kept", id - 1)
            }
            _ => format!(
                "- L{id} · 10-23 10:00 · gen 2 · Learned from reading: crates/x.rs:{i} builds \
                 part {i} of the map"
            ),
        };
        if matches!(i % 100, 0 | 1) {
            pinned.push(line.clone());
        }
        ledger.push_str(&line);
        ledger.push('\n');
        id += 1;
    }
    // One rule far longer than any budget is still never cut.
    let long = format!(
        "- L{id} · 10-23 12:00 · gen 2 · Rule from the user: \"{}\"",
        "z".repeat(50_000)
    );
    ledger.push_str(&long);
    ledger.push('\n');
    pinned.push(long);
    assert!(ledger.len() > 300_000);
    std::fs::write(&ledger_path, &ledger).unwrap();

    let entries = transcript(3, "short");
    let text = bootstrap(&Boot {
        n: 2,
        tail: TailBudget {
            reason: Reason::Size,
            hot_tokens: None,
            target_window: None,
        },
        key: "abcd1234",
        folder: &dir,
        ledger_path: &ledger_path,
        ledger: Some(&ledger),
        archive_path: &dir.join("ledger-archive.md"),
        archive: Some(archive),
        lineage: &[],
        note_path: &dir.join("handoff/2.md"),
        note: Some("doing now: the map"),
        start_path: &dir.join("handoff/2-start.md"),
        death: None,
        entries: &entries,
        blobs: Path::new("/nowhere"),
    });
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing"))
    };
    let kept = &text[..at("## The rest of the ledger, newest first")];
    for entry in &pinned {
        assert!(kept.contains(&format!("{entry}\n")), "{}", &entry[..60]);
    }
    for entry in &closed {
        assert!(
            !kept.contains(&format!("{entry}\n")),
            "a closed item: {entry}"
        );
    }
    // Promise 2 was closed; the archive's open item and the last promise were not.
    let open = &text[at("## Open items")..at("## Rules and decisions")];
    assert!(open.contains("- L2 · 10-22 09:05 · gen 1 · Open: the terms page waits on the user\n"));
    assert!(!open.contains("Open: promise 2\n"));
    assert!(open.contains("Open: promise 2902\n"));
    assert!(text.contains(&format!(
        "- Ledger archive: {}",
        dir.join("ledger-archive.md").display()
    )));

    // The rest: newest first, within ≈8k tokens, then the read for what it left out.
    let rest = &text[at("## The rest of the ledger")..at("## Hand-off note")];
    assert!(rest.len() <= 32_000 + 500, "{}", rest.len());
    assert!(rest.contains("crates/x.rs:2999 builds"));
    let read = rest
        .lines()
        .find(|line| line.contains("older entries left out"))
        .expect("the read for the rest");
    let command = read.split('`').nth(1).unwrap();
    let words: Vec<String> = command
        .split(' ')
        .map(|word| word.trim_matches('\'').to_owned())
        .collect();
    assert_eq!(words[..2], ["sed", "-n"]);
    let printed = std::process::Command::new(&words[0])
        .args(&words[1..])
        .output()
        .unwrap();
    assert!(printed.status.success());
    let printed = String::from_utf8(printed.stdout).unwrap();
    for line in ledger
        .lines()
        .filter(|line| line.contains("Learned from reading"))
    {
        assert!(
            rest.contains(line) || printed.contains(line),
            "neither carried nor read: {line}"
        );
    }
    assert!(!printed.contains("crates/x.rs:2999 builds"));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The user's own messages: all of them while they fit in ≈8k tokens, the first and the latest
/// always, the rest named with their read.
#[test]
fn the_users_messages_come_within_their_budget_the_first_and_latest_always() {
    let mut entries = transcript(300, "short");
    for entry in &mut entries {
        if let ProviderEvent::Message {
            role: Role::User,
            text,
            ..
        } = &mut entry.event
        {
            text.push_str(&format!(" {}", "w".repeat(400)));
        }
    }
    // A mid-turn message is the user's too; a successor's bare start is not.
    entries[591].steer = true;
    if let ProviderEvent::Message { role, .. } = &mut entries[591].event {
        *role = Role::User;
    }
    entries.push(message(
        "c",
        Role::User,
        &format!(
            "{}\nmap{HANDOFF_END}\n\n{CARRY_ON}",
            handoff_line(1, "abcd1234")
        ),
    ));
    let text = boot(&entries, None, None);
    let said = &text[text.find("## The user's messages").unwrap()
        ..text.find("## The latest exchange").unwrap()];
    let positions = shown(said);
    assert_eq!(positions[0], 0);
    assert_eq!(*positions.last().unwrap(), 598);
    assert!(positions.contains(&591), "the mid-turn message");
    assert!(said.len() > 32_000 + 1_000);
    assert!(said.len() <= 80_000 + 1_000, "{}", said.len());
    assert!(!said.contains(CARRY_ON));
    let left = said
        .lines()
        .find_map(|line| line.strip_prefix("Left out here, for length: "))
        .expect("the rest are named");
    let (count, range) = left.split_once(" older messages, #").unwrap();
    let (from, to) = range.split_once("; ").unwrap().0.split_once("–#").unwrap();
    let (from, to): (usize, usize) = (from.parse().unwrap(), to.parse().unwrap());
    assert_eq!(from, 2);
    let named: Vec<usize> = (from..=to).step_by(2).filter(|n| *n != 591).collect();
    assert_eq!(named.len(), count.parse::<usize>().unwrap());
    for n in &named {
        assert!(!positions.contains(n));
    }
    assert_eq!(named.len() + positions.len(), 301);
    // Each is in the tail, or in a span the hand-off's reads cover.
    let elsewhere = shown(&text[text.find("## The latest exchange").unwrap()..]);
    for n in named {
        assert!(
            elsewhere.contains(&n)
                || spans(&text)
                    .iter()
                    .any(|(from, to)| (*from..=*to).contains(&n)),
            "#{n}"
        );
    }
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
        hot_tokens: None,
        predecessor: Some("p".into()),
        nudged_at_ms: Some(1),
        closed_at_ms: 2,
        death: death.map(str::to_owned),
        stopped: false,
        shown: Vec::new(),
        moved_to: None,
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
        first_context: None,
    };
    write_seam(
        record,
        &SeamFile {
            generation: 3,
            phase: Phase::Live,
            seams: vec![seam(1, None), seam(2, Some(TOO_LONG_TEST))],
            moving: None,
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

/// A successor given again only messages the thread shows already (a usage limit's move to
/// another agent, S38) draws no bubble of the user's twice (#562); one with a new message does.
#[test]
fn a_successor_given_only_shown_messages_draws_no_bubble_again() {
    let dir = std::env::temp_dir().join(format!("brigadier-seam-{}", uuid::Uuid::new_v4()));
    let record = dir.as_path();
    std::fs::create_dir_all(record).unwrap();
    let seam = |n: u32, messages: &[&str], shown: &[&str]| Seam {
        n,
        end: SeamEnd {
            hot_tokens: None,
            predecessor: Some("p".into()),
            nudged_at_ms: None,
            closed_at_ms: 2,
            death: Some("usage limit on Claude Code · continued on Codex".into()),
            stopped: false,
            shown: shown.iter().map(|id| (*id).to_owned()).collect(),
            moved_to: None,
        },
        successor: format!("s{n}"),
        provider: ProviderKind::Codex,
        started_at_ms: 3,
        note: None,
        start: start_path(record, n).display().to_string(),
        messages: messages.iter().map(|id| (*id).to_owned()).collect(),
        first_context: None,
    };
    write_seam(
        record,
        &SeamFile {
            generation: 3,
            phase: Phase::Live,
            seams: vec![seam(1, &["m1"], &["m1"]), seam(2, &["m2", "m3"], &["m2"])],
            moving: None,
        },
    )
    .unwrap();
    let mut entries = vec![
        message("u0", Role::User, "Do the thing."),
        message(
            "u1",
            Role::User,
            &format!(
                "{}\nmap{HANDOFF_END}\n\nDo the thing.",
                handoff_line(1, "k")
            ),
        ),
        message(
            "u2",
            Role::User,
            &format!("{}\nmap{HANDOFF_END}\n\nMore.", handoff_line(2, "k")),
        ),
    ];
    let marks = marks(record, &mut entries);
    assert_eq!(marks[0].at.as_deref(), Some("u1"));
    assert!(marks[0].carried);
    assert_eq!(marks[1].at.as_deref(), Some("u2"));
    assert!(!marks[1].carried);
    std::fs::remove_dir_all(&dir).unwrap();
}

const TOO_LONG_TEST: &str = "its context was too long for the model";

#[test]
fn the_threshold_leaves_room_in_a_small_window() {
    let config = SeamConfig {
        tokens: 200_000,
        hold: HOLD,
    };
    assert_eq!(config.threshold(Some(1_000_000), None), 200_000);
    assert_eq!(config.threshold(Some(200_000), None), 160_000);
    assert_eq!(config.threshold(None, None), 200_000);
    // A generation that started near it grows a quarter of it first, within the window.
    assert_eq!(config.threshold(Some(1_000_000), Some(40_000)), 200_000);
    assert_eq!(config.threshold(Some(1_000_000), Some(190_000)), 240_000);
    assert_eq!(config.threshold(Some(250_000), Some(190_000)), 212_500);
}

#[test]
fn the_seam_stays_below_codex_compaction_even_with_a_large_bootstrap() {
    for maximum in [128_000, 272_000, 872_000, 1_000_000] {
        let compact = maximum / 10 * 9;
        for window in [maximum, maximum / 100 * 95] {
            for tokens in [60_000, 200_000, 2_000_000] {
                let config = SeamConfig { tokens, hold: HOLD };
                for base in [None, Some(maximum / 5 * 4), Some(maximum)] {
                    assert!(config.threshold(Some(window), base) < compact);
                }
            }
        }
    }
}

#[test]
fn the_nudge_is_never_quoted_nor_left_out() {
    let mut entries = transcript(3, "done");
    entries.push(message("n", Role::User, &nudge(Path::new("/s/n.md"))));
    entries.push(message("h", Role::Assistant, "Handed off."));
    let text = boot(&entries, Some("doing now: nothing"), None);
    assert!(!text.contains(NUDGE_MARK), "{text}");
    assert!(text.contains("[#4 · user]\nuser 2"));
    assert!(text.contains("[#7 · assistant]\nHanded off."));
    assert!(
        !shown(&text).contains(&6) && spans(&text).is_empty(),
        "{text}"
    );
}

#[test]
fn d49_tail_budget_uses_reason_hot_context_and_target_cap() {
    for reason in [Reason::Size, Reason::Request, Reason::Quota] {
        let budget = |hot_tokens, target_window| TailBudget {
            reason,
            hot_tokens,
            target_window,
        };
        assert_eq!(budget(Some(90_000), None).bytes(10_000), TAIL_BYTES);
        assert_eq!(budget(None, Some(200_000)).bytes(10_000), TAIL_BYTES);
        assert_eq!(budget(Some(90_000), Some(0)).bytes(10_000), TAIL_BYTES);
        if reason == Reason::Size {
            assert_eq!(
                budget(Some(90_000), Some(200_000)).bytes(10_000),
                TAIL_BYTES
            );
        } else {
            assert_eq!(budget(Some(90_000), Some(200_000)).bytes(10_000), 350_000);
            assert_eq!(budget(Some(90_000), Some(100_000)).bytes(10_000), 190_000);
            assert_eq!(budget(Some(90_000), Some(100_000)).bytes(250_000), 0);
        }
    }
}

#[test]
fn expanded_tail_keeps_the_sections_first_and_whole_and_omissions_readable() {
    let entries = transcript(80, &"x".repeat(4_000));
    let size = boot(&entries, Some("whole note"), None);
    for reason in [Reason::Request, Reason::Quota] {
        let text = boot_tail(
            &entries,
            Some("whole note"),
            None,
            TailBudget {
                reason,
                hot_tokens: Some(90_000),
                target_window: Some(200_000),
            },
        );
        let earlier = "## Earlier, newest first";
        assert_eq!(size.split(earlier).next(), text.split(earlier).next());
        assert!(shown(&text).len() > shown(&size).len());
        for at in 0..entries.len() {
            assert!(
                shown(&text).contains(&at)
                    || spans(&text).iter().any(|(a, b)| (*a..=*b).contains(&at))
            );
        }
        assert!(text.len() <= 400_000 + 2_000);
    }
}

#[test]
fn existing_seam_records_choose_request_quota_or_size_and_missing_usage_is_safe() {
    let old = serde_json::json!({"predecessor": "p", "closedAtMs": 1});
    let end: SeamEnd = serde_json::from_value(old.clone()).unwrap();
    assert_eq!(end.reason(), Reason::Size);
    assert_eq!(end.hot_tokens, None);
    let mut request = old.clone();
    request["movedTo"] = serde_json::json!("codex");
    request["hotTokens"] = serde_json::json!(90_000);
    let end: SeamEnd = serde_json::from_value(request).unwrap();
    assert_eq!(end.reason(), Reason::Request);
    assert_eq!(end.hot_tokens, Some(90_000));
    let mut quota = old;
    quota["death"] = serde_json::json!("usage limit on Claude Code · continued on Codex");
    let end: SeamEnd = serde_json::from_value(quota).unwrap();
    assert_eq!(end.reason(), Reason::Quota);
}

#[test]
fn an_expanded_tail_cuts_an_earlier_item_too_big_for_it_and_keeps_it() {
    let mut entries = transcript(4, "short");
    // Before the latest exchange: an answer larger than the whole expanded tail.
    let at = entries.len() - 3;
    entries[at] = message("big", Role::Assistant, &"y".repeat(500_000));
    let text = boot_tail(
        &entries,
        None,
        None,
        TailBudget {
            reason: Reason::Request,
            hot_tokens: Some(90_000),
            target_window: Some(200_000),
        },
    );
    assert!(shown(&text).contains(&at), "the big item, cut");
    assert!(text.contains("[cut at "), "its cut line");
    assert!(text.len() <= 360_000 + 2_000, "{}", text.len());
}
