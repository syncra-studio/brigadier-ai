use super::*;
use brigadier_brain::{Embedder, Scope};

struct Fixture {
    session: RecordSession,
    root: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "20261024-1334-long-session-memory-w14-{}",
            uuid::Uuid::now_v7()
        ));
        let record = root.join("record");
        std::fs::create_dir_all(&record).unwrap();
        let saved = root.join("cli/projects/-repo/claude/tool-results/output.txt");
        std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
        std::fs::write(
            &saved,
            format!("{}failedneedle: café\n\n", "filler\n".repeat(1000)),
        )
        .unwrap();
        let claude = root.join("claude.jsonl");
        std::fs::write(
            &claude,
            include_str!("fixtures/claude.jsonl")
                .replace("SAVED_PATH", &saved.to_string_lossy().replace('\\', "\\\\")),
        )
        .unwrap();
        let codex = root.join("codex.jsonl");
        std::fs::write(&codex, include_str!("fixtures/codex.jsonl")).unwrap();
        let generations = serde_json::json!([
            {"provider":"claude","sessionId":"claude-one","file":claude,"startedAt":1},
            {"provider":"codex","sessionId":"codex-two","file":codex,"startedAt":2}
        ]);
        std::fs::write(record.join("lineage.json"), generations.to_string()).unwrap();
        Self {
            session: RecordSession {
                conversation: ConversationId("fixture-session".into()),
                key: "session".into(),
                record,
                roots: vec![],
                blobs: root.join("blobs"),
            },
            root,
        }
    }
    fn brain(&self) -> Brain {
        Brain::open(
            &self.root.join("brain.sqlite"),
            Scope::Project,
            Embedder::new(self.root.join("models")),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for _ in 0..100 {
            if std::fs::remove_dir_all(&self.root).is_ok() || !self.root.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
fn args(id: &str) -> RecordRead {
    RecordRead {
        session: "session".into(),
        item: Some(id.into()),
        ..Default::default()
    }
}

#[test]
fn exact_items_cover_both_clis_failed_outputs_filters_and_saved_files() {
    let fixture = Fixture::new();
    let brain = fixture.brain();
    sync_session(&brain, &fixture.session).unwrap();
    let hits = brain
        .search_records_filtered("failedneedle", Some("session"), 30, Some("tool"), None)
        .unwrap();
    assert_eq!(hits.len(), 2, "{hits:?}");
    assert_eq!(
        hits[0].tool.as_deref(),
        Some("exec_command"),
        "newest first"
    );
    assert_eq!(hits[1].tool.as_deref(), Some("Bash"));
    assert!(hits[1].bytes_before > 6000);
    let rendered = hits_text(&hits, "brain");
    assert!(rendered.contains("origin tool"));
    assert!(rendered.contains("bytes omitted:"));
    for hit in &hits {
        let id = hit.item_id.as_ref().unwrap();
        assert!(rendered.contains(&format!("read session --item {id}")));
        let output = read_items(&fixture.session, &args(id)).unwrap();
        let body = output.split_once('\n').unwrap().1;
        if hit.tool.as_deref() == Some("exec_command") {
            assert_eq!(body, "Process exited with code 2\nfailedneedle: café\n\n\n");
        } else {
            assert_eq!(
                body,
                format!("{}failedneedle: café\n\n\n", "filler\n".repeat(1000))
            );
        }
        let mut page = args(id);
        page.offset = Some(2);
        page.limit = Some(5);
        let output = read_items(&fixture.session, &page).unwrap();
        assert!(output.contains("2 before,"), "{output}");
        assert_eq!(
            output.split_once('\n').unwrap().1,
            format!("{}\n", &body[2..7])
        );
    }
    assert!(
        brain
            .search_records_filtered("failedneedle", None, 30, Some("user"), None)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        brain
            .search_records_filtered("failedneedle", None, 30, None, Some("Bash"))
            .unwrap()
            .len(),
        1
    );
    let filtered = read_items(
        &fixture.session,
        &RecordRead {
            role: Some("user".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(filtered.matches("origin user").count(), 2);
    assert!(!filtered.contains("failedneedle"));
    let all = items::read(&fixture.session);
    assert_eq!(all.iter().filter(|i| i.kind == "call").count(), 3);
    assert_eq!(all.iter().filter(|i| i.kind == "output").count(), 3);
    assert_eq!(
        all.iter()
            .map(|i| &i.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        all.len()
    );
    assert!(read_items(&fixture.session, &args("unknown")).is_err());
    let mut wrong = args(hits[0].item_id.as_ref().unwrap());
    wrong.tool = Some("Bash".into());
    assert!(read_items(&fixture.session, &wrong).is_err());
}

#[test]
fn ids_survive_compaction_restart_lineage_growth_and_archive_moves() {
    let fixture = Fixture::new();
    let original = items::read(&fixture.session);
    let codex = fixture.root.join("codex.jsonl");
    use std::io::Write;
    writeln!(std::fs::OpenOptions::new().append(true).open(&codex).unwrap(), "{}", serde_json::json!({
        "type":"response_item", "payload":{"type":"message", "role":"user", "content":[{"type":"input_text","text":"new steering"}]}
    })).unwrap();
    let later = items::read(&fixture.session);
    assert_eq!(
        original.iter().map(|i| &i.id).collect::<Vec<_>>(),
        later[..original.len()]
            .iter()
            .map(|i| &i.id)
            .collect::<Vec<_>>()
    );
    let archive = fixture.root.join("archived_sessions/codex.jsonl");
    std::fs::create_dir_all(archive.parent().unwrap()).unwrap();
    std::fs::rename(&codex, &archive).unwrap();
    let mut lineage: serde_json::Value = serde_json::from_slice(
        &std::fs::read(fixture.session.record.join("lineage.json")).unwrap(),
    )
    .unwrap();
    lineage[1]["file"] = serde_json::json!(archive);
    let third = fixture.root.join("third.jsonl");
    std::fs::write(&third, include_str!("fixtures/codex.jsonl")).unwrap();
    lineage.as_array_mut().unwrap().push(serde_json::json!({"provider":"codex","sessionId":"third-session","file":third,"startedAt":3}));
    std::fs::write(
        fixture.session.record.join("lineage.json"),
        lineage.to_string(),
    )
    .unwrap();
    let after = items::read(&fixture.session);
    assert_eq!(
        later.iter().map(|i| &i.id).collect::<Vec<_>>(),
        after[..later.len()]
            .iter()
            .map(|i| &i.id)
            .collect::<Vec<_>>()
    );
    let brain = fixture.brain();
    sync_session(&brain, &fixture.session).unwrap();
    drop(brain);
    let brain = fixture.brain();
    sync_session(&brain, &fixture.session).unwrap();
    let hits = brain.search_records("failedneedle", None, 30).unwrap();
    assert_eq!(hits.len(), 3);
    for item in &original {
        let output = read_items(&fixture.session, &args(&item.id)).unwrap();
        assert_eq!(
            output.split_once('\n').unwrap().1,
            format!("{}\n", item.text)
        );
    }
}

#[test]
fn utf8_offsets_and_empty_results_report_exact_omissions() {
    let fixture = Fixture::new();
    let user = items::read(&fixture.session)
        .into_iter()
        .find(|i| i.role == "user")
        .unwrap();
    let accent = user.text.find('é').unwrap();
    let mut page = args(&user.id);
    page.offset = Some(accent + 1);
    page.limit = Some(3);
    let output = read_items(&fixture.session, &page).unwrap();
    assert!(output.contains(&format!("{} before,", accent + 2)));
    assert_eq!(output.split_once('\n').unwrap().1, " s\n");
    page.offset = Some(usize::MAX);
    assert!(
        read_items(&fixture.session, &page)
            .unwrap()
            .ends_with("0 after]\n\n")
    );
    page.limit = Some(0);
    assert!(read_items(&fixture.session, &page).is_err());
}

#[test]
fn explicit_ledger_closures_and_replacements_mark_the_matching_entry() {
    let fixture = Fixture::new();
    std::fs::write(
        fixture.session.record.join("ledger-archive.md"),
        "- L1 · 10-22 10:00 · gen 1 · Open: olderneedle task\n",
    )
    .unwrap();
    std::fs::write(fixture.session.record.join("ledger.md"), "- L2 · 10-22 10:01 · gen 1 · Rule from user: replacementneedle use tabs\n- L3 · 10-22 10:02 · gen 1 · Closed L1: finished\n- L4 · 10-22 10:03 · gen 1 · Rule from user: use spaces (supersedes L2)\n").unwrap();
    let brain = fixture.brain();
    sync_session(&brain, &fixture.session).unwrap();
    for (query, expected) in [
        ("olderneedle", "CLOSED by L3"),
        ("replacementneedle", "SUPERSEDED by L4"),
    ] {
        let mut hits = brain.search_records(query, None, 10).unwrap();
        assert_eq!(hits.len(), 1);
        mark_ended(&mut hits[0], &fixture.session);
        assert_eq!(hits[0].ended.as_deref(), Some(expected));
        assert!(hits_text(&hits, "brain").contains(expected));
    }
}

#[test]
fn saved_output_changes_are_findable_without_a_transcript_change() {
    let fixture = Fixture::new();
    let brain = fixture.brain();
    sync_session(&brain, &fixture.session).unwrap();
    let old = brain
        .search_records_filtered("failedneedle", None, 10, None, Some("Bash"))
        .unwrap();
    let saved = fixture
        .root
        .join("cli/projects/-repo/claude/tool-results/output.txt");
    std::fs::write(&saved, "newoutputneedle\n").unwrap();
    sync_session(&brain, &fixture.session).unwrap();
    let new = brain.search_records("newoutputneedle", None, 10).unwrap();
    assert_eq!(new.len(), 1);
    assert_eq!(new[0].item_id, old[0].item_id);
    assert!(
        brain
            .search_records_filtered("failedneedle", None, 10, None, Some("Bash"))
            .unwrap()
            .is_empty()
    );
    std::fs::remove_file(&saved).unwrap();
    sync_session(&brain, &fixture.session).unwrap();
    assert!(
        brain
            .search_records("newoutputneedle", None, 10)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn generated_claude_summary_metadata_and_sidechain_echoes_are_not_user_words() {
    let fixture = Fixture::new();
    let file = fixture.root.join("claude.jsonl");
    let mut text = std::fs::read_to_string(&file).unwrap();
    for flag in ["isCompactSummary", "isMeta", "isSidechain"] {
        let mut row = serde_json::json!({"type":"user", "uuid":flag,
            "message":{"role":"user", "content":"generatedsummaryneedle"}});
        row[flag] = serde_json::json!(true);
        text.push_str(&format!("{row}\n"));
    }
    std::fs::write(file, text).unwrap();
    let brain = fixture.brain();
    sync_session(&brain, &fixture.session).unwrap();
    assert!(
        brain
            .search_records("generatedsummaryneedle", None, 10)
            .unwrap()
            .is_empty()
    );
    let users = items::read(&fixture.session)
        .into_iter()
        .filter(|item| item.role == "user")
        .collect::<Vec<_>>();
    assert_eq!(users.len(), 2);
}

#[test]
fn registered_claude_subagents_keep_separate_ids_and_refresh_without_parent_changes() {
    let fixture = Fixture::new();
    use std::io::Write;
    writeln!(std::fs::OpenOptions::new().append(true).open(fixture.root.join("claude.jsonl")).unwrap(), "{}",
        serde_json::json!({"type":"assistant", "uuid":"parent-agent", "message":{"id":"parent-agent-message","role":"assistant",
            "content":[{"type":"tool_use","id":"agent-call","name":"Agent","input":{"prompt":"check"}}]}})).unwrap();
    let parent_before = items::read(&fixture.session);
    let brain = fixture.brain();
    sync_session(&brain, &fixture.session).unwrap();
    let children = fixture.root.join("claude/subagents");
    std::fs::create_dir_all(&children).unwrap();
    std::fs::write(
        children.join("agent-child.meta.json"),
        r#"{"toolUseId":"agent-call"}"#,
    )
    .unwrap();
    let child = children.join("agent-child.jsonl");
    // Reuse a parent call ID to prove the child namespace keeps both original outputs.
    let rows = [
        serde_json::json!({"type":"user","uuid":"brief", "isSidechain":true,
            "message":{"role":"user","content":"childbriefneedle"}}),
        serde_json::json!({"type":"assistant","uuid":"child-call","isSidechain":true,
            "message":{"id":"child-message","role":"assistant","content":[{"type":"tool_use","id":"same-call","name":"Bash","input":{"command":"false"}}]}}),
        serde_json::json!({"type":"user","uuid":"child-result","isSidechain":true,
            "message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"same-call","content":"childoutputneedle","is_error":true}]}}),
    ];
    std::fs::write(
        &child,
        rows.iter().map(|v| format!("{v}\n")).collect::<String>(),
    )
    .unwrap();
    let (_, files) = transcript_stamp(&fixture.session.record).unwrap();
    assert!(
        transcript(&files)
            .iter()
            .filter_map(|entry| entry_text(entry, &fixture.session.blobs))
            .any(|(_, body, _)| body.contains("childoutputneedle")),
        "the existing display reader also sees this child output"
    );
    sync_session(&brain, &fixture.session).unwrap();
    let hits = brain
        .search_records_filtered("childoutputneedle", None, 10, Some("tool"), Some("Bash"))
        .unwrap();
    assert_eq!(hits.len(), 1);
    let id = hits[0].item_id.as_ref().unwrap();
    assert!(
        read_items(&fixture.session, &args(id))
            .unwrap()
            .ends_with("childoutputneedle\n")
    );
    assert!(
        brain
            .search_records("childbriefneedle", None, 10)
            .unwrap()
            .is_empty()
    );
    let after = items::read(&fixture.session);
    assert_eq!(
        after
            .iter()
            .map(|i| &i.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        after.len()
    );
    for old in parent_before {
        assert!(
            after
                .iter()
                .any(|new| new.id == old.id && new.text == old.text)
        );
    }
    writeln!(
        std::fs::OpenOptions::new()
            .append(true)
            .open(child)
            .unwrap(),
        "{}",
        serde_json::json!({
            "type":"assistant","uuid":"later-child","isSidechain":true,
            "message":{"role":"assistant","content":[{"type":"text","text":"childlaterneedle"}]}
        })
    )
    .unwrap();
    sync_session(&brain, &fixture.session).unwrap();
    assert_eq!(
        brain
            .search_records("childlaterneedle", None, 10)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        brain.search_records("childoutputneedle", None, 10).unwrap()[0]
            .item_id
            .as_deref(),
        Some(id.as_str())
    );
}

#[test]
fn a_codex_restart_can_append_response_items_to_legacy_records_without_losing_old_ids() {
    let fixture = Fixture::new();
    let codex = fixture.root.join("codex.jsonl");
    std::fs::write(&codex, format!("{}\n", serde_json::json!({"type":"event_msg", "payload":{"type":"item_completed",
        "item":{"type":"UserMessage", "id":"older-user", "content":[{"type":"text","text":"legacyrestartneedle"}]}}}))).unwrap();
    let old = items::read(&fixture.session)
        .into_iter()
        .find(|item| item.text == "legacyrestartneedle")
        .unwrap();
    let brain = fixture.brain();
    sync_session(&brain, &fixture.session).unwrap();
    use std::io::Write;
    writeln!(std::fs::OpenOptions::new().append(true).open(codex).unwrap(), "{}", serde_json::json!({
        "type":"response_item", "payload":{"type":"message", "role":"user", "content":[{"type":"input_text","text":"canonicalrestartneedle"}]}
    })).unwrap();
    sync_session(&brain, &fixture.session).unwrap();
    assert_eq!(
        brain
            .search_records("legacyrestartneedle", None, 10)
            .unwrap()[0]
            .item_id
            .as_ref(),
        Some(&old.id)
    );
    assert_eq!(
        brain
            .search_records("canonicalrestartneedle", None, 10)
            .unwrap()
            .len(),
        1
    );
    assert!(
        read_items(&fixture.session, &args(&old.id))
            .unwrap()
            .ends_with("legacyrestartneedle\n")
    );
}

#[test]
fn filtered_paging_keeps_the_original_assistant_span_position() {
    let fixture = Fixture::new();
    let output = read_items(
        &fixture.session,
        &RecordRead {
            from: Some(1),
            to: Some(1),
            role: Some("assistant".into()),
            limit: Some(8),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(output.contains("origin assistant"));
    assert!(output.ends_with("Checking\n"), "{output}");
}
