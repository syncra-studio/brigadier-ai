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
        said: None,
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

/// A move to another CLI's hand-off, the one that carries the conversation (D49).
fn boot<'a>(entries: &'a [FileEntry], note: Option<&'a str>, death: Option<&'a str>) -> String {
    boot_tail(
        entries,
        note,
        death,
        TailBudget {
            reason: Reason::Request,
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
fn the_note_instructions_name_skills_and_keep_secrets_out_of_the_note() {
    let text = nudge(Path::new("/s/abcd1234/record/handoff/2.md"));
    assert!(text.contains(
        "Add a \"Skills to use next\" section when any apply, naming the skills by the names \
         available to you."
    ));
    assert!(text.contains(
        "Keep API keys, passwords, tokens and personal data out of the note; say where they \
         live instead."
    ));
}

#[test]
fn the_successor_gets_the_notes_skills_section_and_the_users_words_whole() {
    let note = "Doing now: the parser.\n\n## Skills to use next\nUse code-review next.\n\n\
                Secrets live in /s/abcd1234/.env.";
    let user = "Use API_TOKEN=example-placeholder for this test.";
    let entries = vec![message("u1", Role::User, user)];
    let text = boot(&entries, Some(note), None);
    assert!(text.contains(&format!("## Hand-off note\n{note}\n")));
    assert!(text.contains(&format!("[#0 · user]\n{user}")));
}

/// A queued message's frame is Brigadier's word to the session that had it: its successor gets
/// the user's words alone, never an old "judge it" to act on (run2-D40).
#[test]
fn the_successor_gets_queued_messages_without_their_frames() {
    let deciding = crate::manager::conversation::queue_judging::deciding_frame("feedbeef");
    let own_turn = crate::manager::conversation::queue_judging::own_turn_frame("cafebabe");
    let withdrawn = crate::manager::conversation::queue_judging::withdrawn_frame("deadbeef");
    let mut steer = message("s1", Role::User, &format!("{deciding}Also the docs."));
    steer.steer = true;
    let entries = vec![
        message("u1", Role::User, "Fix the parser"),
        steer,
        message("a1", Role::Assistant, "On it"),
        message(
            "u2",
            Role::User,
            &format!("{own_turn}Then the tests.\n\n{withdrawn}"),
        ),
    ];
    let text = boot(&entries, None, None);
    assert!(!text.contains("judge it"), "{text}");
    assert!(!text.contains("[queued message"), "{text}");
    assert!(text.contains("[#1 · user]\nAlso the docs."), "{text}");
    assert!(text.contains("[#3 · user]\nThen the tests."), "{text}");
}

/// An earlier hand-off quoted inside this one never ends it early: the one end mark is the last
/// line, so a successor's message after it is the user's alone.
#[test]
fn a_quoted_end_mark_does_not_end_the_handoff() {
    let entries = vec![
        message("u1", Role::User, "Fix the parser"),
        message(
            "a1",
            Role::Assistant,
            &format!("The last note said {HANDOFF_END} there."),
        ),
    ];
    let text = boot(&entries, Some(&format!("quoting {HANDOFF_END}")), None);
    assert_eq!(text.matches(HANDOFF_END).count(), 1, "{text}");
    assert!(text.ends_with(HANDOFF_END));
    let first = format!("{text}\n\nMy words, [End of the hand-off] included");
    assert_eq!(
        without_handoff(&first),
        "My words, [End of the hand-off] included"
    );
}

/// A hand-off on the same CLI (a compaction count, a failed or looping compaction) is small:
/// the successor lines, the `brigadier context` pointer and the note, nothing of the
/// conversation or the ledger (D13, D14, D16).
#[test]
fn a_same_cli_handoff_is_the_note_and_the_pointer() {
    let entries = transcript(40, "x");
    let size = TailBudget {
        reason: Reason::Size,
        hot_tokens: Some(150_000),
        target_window: Some(1_000_000),
    };
    let text = boot_tail(&entries, Some("doing now: the parser"), None, size);
    assert!(text.starts_with("[Brigadier hand-off 2 · abcd1234]\nResume the user's work"));
    assert!(text.contains("You continue an earlier session.\n"));
    assert!(text.contains(CONTEXT_POINTER));
    assert!(text.contains("Took over gen 3 · open <ids>"));
    assert!(text.contains("## Hand-off note\ndoing now: the parser\n"));
    assert!(text.ends_with(HANDOFF_END));
    for absent in [
        "## Map",
        "FIRST: the standing rule",
        "answer 39",
        "brigadier brain read",
    ] {
        assert!(!text.contains(absent), "{absent}: {text}");
    }
    assert!(text.len() < 1_500, "{}", text.len());
    // Without a note, why, and where to look first.
    let text = boot_tail(&entries, None, Some("the CLI exited"), size);
    assert!(text.contains("ended without a hand-off note (the CLI exited): check `git status`"));
    assert!(!text.contains("## Hand-off note"));
}

#[test]
fn the_handoff_comes_in_its_order() {
    let entries = transcript(400, &"x".repeat(400));
    let text = boot(&entries, Some("doing now: the parser"), None);
    let at = |needle: &str| {
        text.find(needle)
            .unwrap_or_else(|| panic!("{needle} missing"))
    };
    assert!(text.starts_with("[Brigadier hand-off 2 · abcd1234]\nResume the user's work"));
    assert!(text.contains("without announcing this internal continuation"));
    assert!(text.contains("Took over gen 3 · open <ids>"));
    assert!(text.contains(CONTEXT_POINTER));
    assert!(text.contains("on another CLI, which has none of its history"));
    assert!(at("## Map") < at("## Hand-off note"));
    // No ledger travels; it is a `brigadier context` away (D13).
    assert!(!text.contains("verbatim and whole"), "{}", &text[..2_000]);
    assert!(!text.contains("ledger.md"));
    assert!(at("## Hand-off note") < at("## The user's messages, oldest first"));
    assert!(at("## The user's messages") < at("## The latest exchange, after #798 (above)"));
    assert!(at("## The latest exchange") < at("## Earlier, newest first"));
    assert!(at("## Earlier, newest first") < at("## Left out, each a read away"));
    assert!(text.ends_with(HANDOFF_END));
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
            &format!("\n\n{}\nmap{HANDOFF_END}\n\nnext", handoff_line(1, "k")),
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
        long: None,
        tokens_set: false,
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
                let config = SeamConfig {
                    tokens,
                    hold: HOLD,
                    long: None,
                    tokens_set: false,
                };
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
            // The cap above a large hot context and window; under it, the smaller of the hot
            // context and half the window, each as four bytes a token.
            assert_eq!(
                budget(Some(90_000), Some(200_000)).bytes(10_000),
                EXPANDED_TAIL_BYTES - 10_000
            );
            assert_eq!(budget(Some(20_000), Some(200_000)).bytes(10_000), 70_000);
            assert_eq!(budget(Some(90_000), Some(30_000)).bytes(10_000), 50_000);
            assert_eq!(budget(Some(90_000), Some(100_000)).bytes(250_000), 0);
        }
    }
}

#[test]
fn a_claude_startup_window_expands_only_request_and_quota_bootstraps() {
    let entries = transcript(80, &"x".repeat(4_000));
    let size = boot(&entries, Some("whole note"), None);
    for reason in [Reason::Size, Reason::Request, Reason::Quota] {
        for target_window in [Some(100_000), None] {
            let text = boot_tail(
                &entries,
                Some("whole note"),
                None,
                TailBudget {
                    reason,
                    hot_tokens: Some(90_000),
                    target_window,
                },
            );
            if reason == Reason::Size {
                // On the same CLI it carries none of the conversation (D13).
                assert!(!text.contains("## Map") && shown(&text).is_empty());
            } else if target_window.is_none() {
                assert_eq!(text, size);
            } else {
                assert!(shown(&text).len() > shown(&size).len());
                assert!(text.len() <= 200_000 + 2_000);
                let earlier = "## Earlier, newest first";
                assert_eq!(size.split(earlier).next(), text.split(earlier).next());
            }
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
        assert!(text.len() <= EXPANDED_TAIL_BYTES + 8_000, "{}", text.len());
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
