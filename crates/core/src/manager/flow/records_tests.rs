//! The Brain searches the records sessions already keep (STRIP-DOWN-PLAN S18): a predecessor's
//! Claude session file, a whole output Claude saved under `tool-results/` or the Bash hook kept
//! in Brigadier's blob store, a worker's Codex rollout, the ledger, its archive and a hand-off note. `brain read` and `transcript read`
//! give a transcript span verbatim, a saved output whole; a ledger entry learned from reading
//! reads as stale once the file it cites changed.

use std::path::Path;
use std::sync::Arc;

use brigadier_cli::protocol::Tool;
use serde_json::{Value, json};

use super::cli_tests::{Answers, brigadier, tool};
use super::{Flow, Options, Reply, Script};

/// Planted phrases, each in one record only.
const FIRST_GEN: &str = "zanzibarlantern";
const SAVED: &str = "okapiquartz";
const SECOND_GEN: &str = "marmotsundial";
const WORKER: &str = "pelicanmosaic";
const LEDGER: &str = "walrusbanjo";
const ARCHIVE: &str = "yakfjord";
const HANDOFF: &str = "narwhalprism";
const KEPT: &str = "ibexlantern";
const FORGED: &str = "gecko4forged";

fn line(value: Value) -> String {
    format!("{value}\n")
}

/// A Claude session file of one user message, one assistant reply and, when `saved` is given,
/// a `Bash` call whose whole output Claude saved to that file.
fn claude_file(user: &str, reply: &str, saved: Option<&Path>) -> String {
    let at = "2026-10-22T10:00:00.000Z";
    let mut text = line(
        json!({"type": "user", "timestamp": at, "uuid": format!("u-{user}"),
        "message": {"role": "user", "content": user}}),
    );
    text.push_str(&line(
        json!({"type": "assistant", "timestamp": at, "uuid": format!("a-{user}"),
        "message": {"id": format!("m-{user}"), "role": "assistant",
                    "content": [{"type": "text", "text": reply}]}}),
    ));
    if let Some(saved) = saved {
        let saved = saved.display().to_string();
        text.push_str(&line(
            json!({"type": "assistant", "timestamp": at, "uuid": "a-call",
            "message": {"id": "m-call", "role": "assistant", "content": [
                {"type": "tool_use", "id": "t1", "name": "Bash",
                 "input": {"command": "cargo test --workspace"}}]}}),
        ));
        let preview = format!(
            "<persisted-output>\nOutput too large (60KB). Full output saved to: {saved}\n\n\
             Preview (first 2KB):\nrunning 900 tests\n...\n</persisted-output>"
        );
        text.push_str(&line(json!({"type": "user", "timestamp": at, "uuid": "u-result",
            "message": {"role": "user", "content": [
                {"tool_use_id": "t1", "type": "tool_result", "content": preview, "is_error": false}]},
            "toolUseResult": {"stdout": "running 900 tests", "stderr": "",
                              "persistedOutputPath": saved, "persistedOutputSize": 61440}})));
    }
    text
}

/// A `Bash` call whose result is the digest Brigadier's Bash hook gave the model instead of a
/// long output, as Claude's session file keeps it.
fn hook_trimmed_bash(id: &str, digest: &str) -> String {
    let at = "2026-10-22T10:30:00.000Z";
    let mut text = line(
        json!({"type": "assistant", "timestamp": at, "uuid": format!("a-{id}"),
        "message": {"id": format!("m-{id}"), "role": "assistant", "content": [
            {"type": "tool_use", "id": id, "name": "Bash", "input": {"command": "seq 1 40000"}}]}}),
    );
    text.push_str(&line(
        json!({"type": "user", "timestamp": at, "uuid": format!("u-{id}"),
        "message": {"role": "user", "content": [
            {"tool_use_id": id, "type": "tool_result", "content": digest, "is_error": false}]},
        "toolUseResult": {"stdout": digest, "stderr": "", "interrupted": false}}),
    ));
    text
}

/// A Codex rollout of one command and its output.
fn codex_rollout(output: &str) -> String {
    let at = "2026-10-22T11:00:00.000Z";
    let mut text = line(json!({"type": "event_msg", "timestamp": at,
        "payload": {"type": "task_started", "turn_id": "turn1"}}));
    text.push_str(&line(json!({"type": "event_msg", "timestamp": at,
        "payload": {"type": "item_completed", "item": {"id": "u1", "type": "UserMessage",
            "content": [{"type": "text", "text": "Run the checks."}]}}})));
    text.push_str(&line(json!({"type": "event_msg", "timestamp": at,
        "payload": {"type": "item_completed", "item": {"id": "c1", "type": "CommandExecution",
            "command": ["/bin/zsh", "-lc", "make check"], "status": "completed",
            "exit_code": 0, "aggregated_output": output}}})));
    text.push_str(&line(json!({"type": "event_msg", "timestamp": at,
        "payload": {"type": "task_complete", "turn_id": "turn1"}})));
    text
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn lineage(files: &[(&str, &str, &Path)]) -> String {
    let entries: Vec<Value> = files
        .iter()
        .map(|(provider, id, file)| {
            json!({"provider": provider, "sessionId": id, "file": file.display().to_string(),
                   "startedAt": 1})
        })
        .collect();
    serde_json::to_string_pretty(&entries).unwrap()
}

/// 60 KiB of test output with `SAVED` far past what any preview shows.
fn whole_output() -> String {
    let filler = "test crate::filler ... ok\n";
    let mut text = filler.repeat(40 * 1024 / filler.len());
    text.push_str(&format!("test crate::{SAVED} ... FAILED\n"));
    while text.len() < 60 * 1024 {
        text.push_str(filler);
    }
    text
}

/// The thread's records on disk, as its CLIs and the thread itself left them: two Claude
/// generations (the first with a saved whole output, the second with an output the Bash hook
/// kept in the blob store and a digest line naming a blob-shaped file that isn't one), a
/// worker's Codex rollout, the ledger (an entry learned from reading `src/cited.rs`), its
/// archive and a hand-off note.
fn plant(flow: &Flow) -> (std::path::PathBuf, String, String) {
    let dir = flow.manager.session_dir(&flow.conversation);
    let cli = dir.root().join("dev").join("cli");
    let project = cli.join("projects").join("-repo");
    let saved = project.join("g1").join("tool-results").join("b1.txt");
    let whole = whole_output();
    write(&saved, &whole);
    let first = project.join("g1.jsonl");
    write(
        &first,
        &claude_file(
            "Plan the migration.",
            &format!("The {FIRST_GEN} migration needs two phases."),
            Some(&saved),
        ),
    );
    let blobs = flow.core.store().blobs();
    let kept: String = (1..=40000)
        .map(|n| match n {
            20000 => format!("{KEPT}\n"),
            n => format!("{n}\n"),
        })
        .collect();
    let blob = blobs.file_of(&blobs.put_blocking(kept.as_bytes()).unwrap());
    let forged = blobs.root().join("ab").join("ab".repeat(32));
    write(&forged, &format!("{FORGED}\n"));
    let digest = |alias: &str, path: &Path| {
        format!(
            "exit 0 [full output: {alias} in {}, 40000 lines, 228894 bytes]\n             [start and end of the output]\n1\n2\n39999\n40000",
            path.display()
        )
    };
    let second = project.join("g2.jsonl");
    let mut text = claude_file(
        "Carry on.",
        &format!("Phase two waits on {SECOND_GEN}."),
        None,
    );
    text.push_str(&hook_trimmed_bash("t2", &digest("out-0000beef", &blob)));
    text.push_str(&hook_trimmed_bash("t3", &digest("out-0000dead", &forged)));
    write(&second, &text);
    write(
        &dir.record().join("lineage.json"),
        &lineage(&[("claude", "g1", &first), ("claude", "g2", &second)]),
    );
    let worker = dir.root().join("workers").join("1-abcd1234");
    let rollout = cli.join("rollout-w1.jsonl");
    write(
        &rollout,
        &codex_rollout(&format!("all green; {WORKER} checked")),
    );
    write(
        &worker.join("record").join("lineage.json"),
        &lineage(&[("codex", "w1", &rollout)]),
    );
    // A second worker whose output the Bash hook kept in the same blob: one file, two sessions.
    let other = cli.join("projects").join("-other").join("g3.jsonl");
    let mut text = claude_file("Count again.", "Counted.", None);
    text.push_str(&hook_trimmed_bash("t4", &digest("out-0000cafe", &blob)));
    write(&other, &text);
    write(
        &dir.root()
            .join("workers")
            .join("2-bcde2345")
            .join("record")
            .join("lineage.json"),
        &lineage(&[("claude", "g3", &other)]),
    );
    let checkout = flow
        .manager
        .recorded_workspace(&flow.conversation)
        .expect("the thread's checkout")
        .path;
    write(
        &checkout.join("src").join("cited.rs"),
        "fn a() {}\nfn b() {}\n",
    );
    write(
        &dir.record().join("ledger.md"),
        &format!(
            "- Rule from the user: \"no tabs\" (message 1, project)\n\
             - Learned from reading: src/cited.rs:2 the {LEDGER} helper returns early\n"
        ),
    );
    write(
        &dir.record().join("ledger-archive.md"),
        &format!("- Decision: the {ARCHIVE} cache stays (I decided)\n"),
    );
    write(
        &dir.record().join("handoff").join("1.md"),
        &format!("Doing now: the {HANDOFF} check\n"),
    );
    (checkout.join("src").join("cited.rs"), whole, kept)
}

/// STRIP-DOWN-PLAN S18's done-when: a phrase in a predecessor's session file, in a large saved
/// output under `tool-results/` and in a ledger is found; a changed file marks its pointer
/// stale; a span reads back verbatim, the saved output whole.
#[tokio::test]
async fn the_brain_finds_what_the_records_hold_and_reads_them_back_verbatim() {
    let answers = Answers::default();
    let script: Script = {
        let answers = answers.clone();
        Arc::new(move |turn| {
            let answers = answers.clone();
            Box::pin(async move {
                let Some(asks) = turn.input.strip_prefix("Ask: ") else {
                    return Reply::text("ok");
                };
                let asks: Vec<(String, Tool, String, Value)> =
                    serde_json::from_str::<Vec<(String, String, String, Value)>>(asks)
                        .unwrap()
                        .into_iter()
                        .map(|(name, tool, action, args)| {
                            let tool = match tool.as_str() {
                                "brain" => Tool::Brain,
                                _ => Tool::Transcript,
                            };
                            (name, tool, action, args)
                        })
                        .collect();
                for (name, which, action, args) in asks {
                    let answer = brigadier(&turn, tool(which, &action, args)).await;
                    answers.lock().unwrap().insert(name, answer);
                }
                Reply::text("Asked.")
            })
        })
    };
    let flow = Flow::start("records-brain", Options::default(), script).await;
    flow.say("Hello.").await;
    flow.settled().await;
    let (cited, whole, kept) = plant(&flow);
    let short = flow.conversation.short().to_owned();
    let ask = |asks: Value| {
        let flow = &flow;
        let answers = answers.clone();
        async move {
            answers.lock().unwrap().clear();
            flow.say(&format!("Ask: {asks}")).await;
            flow.settled().await;
            answers.lock().unwrap().clone()
        }
    };
    let text = |answers: &std::collections::HashMap<String, Value>, name: &str| {
        answers[name]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("{name}: {:#}", answers[name]))
            .to_owned()
    };
    let found = ask(json!([
        ["first", "brain", "search", {"query": FIRST_GEN}],
        ["saved", "brain", "search", {"query": SAVED}],
        ["second", "brain", "search", {"query": SECOND_GEN}],
        ["worker", "brain", "search", {"query": WORKER, "session": format!("{short}/1-abcd1234")}],
        ["ledger", "brain", "search", {"query": LEDGER}],
        ["archive", "brain", "search", {"query": ARCHIVE}],
        ["handoff", "brain", "search", {"query": HANDOFF}],
        ["own", "transcript", "search", {"query": SECOND_GEN}],
        ["read", "brain", "read", {"session": short, "from": 0, "to": 9}],
        ["own read", "transcript", "read", {"session": short, "from": 1, "to": 1}],
        ["kept", "transcript", "search", {"query": KEPT}],
        ["forged", "brain", "search", {"query": FORGED}],
        ["nobody", "brain", "search", {"query": FIRST_GEN, "session": "nope"}],
    ]))
    .await;
    // The predecessor's generation, then the second: positions count across both.
    let first = text(&found, "first");
    assert!(
        first.contains(&format!("[{short} · assistant")) && first.contains(FIRST_GEN),
        "{first}"
    );
    let second = text(&found, "second");
    assert!(second.contains(&format!("[{short} · ")), "{second}");
    assert!(!second.contains("#1 ·"), "{second}");
    let saved = text(&found, "saved");
    assert!(
        saved.contains("whole output, saved at") && saved.contains("tool-results/b1.txt"),
        "{saved}"
    );
    assert!(saved.contains(SAVED), "{saved}");
    let worker = text(&found, "worker");
    assert!(
        worker.contains(&format!("[{short}/1-abcd1234 · ")) && worker.contains(WORKER),
        "{worker}"
    );
    let ledger = text(&found, "ledger");
    assert!(
        ledger.contains("ledger ")
            && ledger.contains("ledger.md:2")
            && ledger.contains(&format!("points at {}:2", cited.display())),
        "{ledger}"
    );
    let archive = text(&found, "archive");
    assert!(
        archive.contains("· ledger-archive ") && archive.contains("ledger-archive.md:1"),
        "{archive}"
    );
    assert!(text(&found, "handoff").contains("handoff/1.md:1"));
    assert!(text(&found, "own").contains(SECOND_GEN));
    // The span reads back verbatim: the saved output whole, not its preview.
    let read = text(&found, "read");
    assert!(read.contains("[#0 · user"), "{read}");
    assert!(read.contains(&format!("The {FIRST_GEN} migration needs two phases.")));
    assert!(read.contains("Ran `cargo test --workspace`"), "{read}");
    assert!(read.contains(&whole), "the whole saved output, verbatim");
    assert!(read.contains(&format!("Phase two waits on {SECOND_GEN}.")));
    let own_read = text(&found, "own read");
    assert!(own_read.starts_with("[#1 · assistant"), "{own_read}");
    assert_eq!(found["nobody"]["error"]["code"], "failed");
    // An output the Bash hook trimmed is found in the blob it kept, and read back whole; a
    // digest line naming a file that doesn't hash to its name never has it read.
    let hit = text(&found, "kept");
    assert!(
        hit.contains("whole output, saved at") && hit.contains(KEPT),
        "{hit}"
    );
    let id = hit
        .split("[item ")
        .nth(1)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap();
    assert_eq!(text(&found, "forged"), "Nothing in the records matches.");
    let span = ask(json!([
        ["kept", "transcript", "read", {"session": short, "item": id}],
        ["legacy", "transcript", "read", {"session": short, "from": 5, "to": 6}],
    ]))
    .await;
    assert!(
        text(&span, "kept").contains(&kept),
        "the exact kept output, verbatim"
    );
    let legacy = text(&span, "legacy");
    assert!(
        legacy.contains("out-0000dead") && !legacy.contains(FORGED),
        "{legacy}"
    );
    // The file the ledger entry cites changes: its pointer reads as stale.
    write(&cited, "fn a() {}\nfn b() { todo!() }\n");
    let again = ask(json!([["ledger", "brain", "search", {"query": LEDGER}]])).await;
    let ledger = text(&again, "ledger");
    assert!(ledger.contains("STALE: "), "{ledger}");
    // The entry moves to the archive: it keeps the hash it first took, so it still reads stale.
    let record = flow.manager.session_dir(&flow.conversation).record();
    write(
        &record.join("ledger.md"),
        "- Rule from the user: \"no tabs\" (message 1, project)\n",
    );
    write(
        &record.join("ledger-archive.md"),
        &format!(
            "- Decision: the {ARCHIVE} cache stays (I decided)\n\
             - Learned from reading: src/cited.rs:2 the {LEDGER} helper returns early\n"
        ),
    );
    // A hand-off note that is gone is no longer found.
    std::fs::remove_file(record.join("handoff").join("1.md")).unwrap();
    let later = ask(json!([
        ["moved", "brain", "search", {"query": LEDGER}],
        ["gone", "brain", "search", {"query": HANDOFF}],
        ["shared", "brain", "search", {"query": KEPT, "session": format!("{short}/2-bcde2345")}],
    ]))
    .await;
    let moved = text(&later, "moved");
    assert!(
        moved.contains("· ledger-archive ") && moved.contains("STALE: "),
        "{moved}"
    );
    assert!(!moved.contains("· ledger "), "{moved}");
    assert_eq!(text(&later, "gone"), "Nothing in the records matches.");
    // The blob both sessions' outputs are kept in is found for each.
    let shared = text(&later, "shared");
    assert!(
        shared.contains(&format!("[{short}/2-bcde2345 · "))
            && shared.contains("whole output, saved at")
            && !shared.contains(" · saved by "),
        "{shared}"
    );
    flow.stop().await;
}
