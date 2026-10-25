//! The thread's own tool calls as the transcript items a worker's fold into (BUILD-PLAN B1b).
//!
//! Each call keeps its row on the board ([`OrchestratorStepKind::Tool`]): what item the call
//! is ([`ThreadItemKind`]), a file change's files with their lines in and out, how much a
//! command has printed while it streams and which turn it ran in. The call itself is read
//! from the CLI's own session file ([`crate::thread_view`]), never from a copy (#540).
//!
//! Native CLI output arrives at completion for Claude and streams for Codex. Brigadier's
//! own command tools feed the same live buffers from their child pipes, for both providers.
//! The row hears how much came at most once a [`OUTPUT_STEP_EVERY`].

use std::collections::HashMap;
use std::time::{Duration, Instant};

use brigadier_providers::{FileChange, ItemStatus, diff_counts};

use super::conversation::ConvLive;
use super::{OUTPUT_MAX_BYTES, SessionManager};
use crate::work::{FileStat, OrchestratorStepKind, OutputSource, ThreadItemKind};

/// How often a streaming command's row hears how much it printed.
pub(super) const OUTPUT_STEP_EVERY: Duration = Duration::from_secs(1);
/// What a Codex command's event keeps of its output (the parser's clip): a command that
/// printed more has its whole output stored.
const EVENT_OUTPUT: usize = 8 * 1024;

/// What a conversation holds of its thread's running calls.
#[derive(Default)]
pub(crate) struct ThreadItems {
    /// When the running turn started: it keys its calls' rows.
    turn: Option<i64>,
    /// Running commands' streamed output, by item.
    live: HashMap<String, LiveOutput>,
}

#[derive(Default)]
struct LiveOutput {
    output: Vec<u8>,
    /// Bytes streamed past [`OUTPUT_MAX_BYTES`]: counted, not kept.
    dropped: u64,
    /// The row last filed for the command, filed again with the bytes so far.
    row: Option<OrchestratorStepKind>,
    /// When the row last heard, and the bytes it heard of.
    told: Option<(Instant, u64)>,
}

impl LiveOutput {
    fn bytes(&self) -> u64 {
        self.output.len() as u64 + self.dropped
    }
}

impl ThreadItems {
    pub(super) fn turn(&self) -> Option<i64> {
        self.turn
    }

    pub(super) fn turn_started(&mut self, at_ms: i64) {
        self.turn = Some(at_ms);
    }

    /// The CLI ended: its commands' output streams no more.
    pub(super) fn cli_ended(&mut self) {
        self.live.clear();
    }
}

/// What a tool call `name` (with or without its MCP server's prefix) given `input` is as a
/// transcript item.
pub(crate) fn tool_item(name: &str, input: Option<&str>) -> ThreadItemKind {
    let bare = crate::digest::bare_tool_name(name);
    if crate::digest::is_shell_tool(bare) || crate::digest::own_run(bare) {
        return ThreadItemKind::Command;
    }
    match bare {
        "WebSearch" | "WebFetch" | "web_search" => ThreadItemKind::Web,
        "view_image" => ThreadItemKind::Image,
        "Read" if image_path(bare, input).is_some() => ThreadItemKind::Image,
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" | "apply_patch" => ThreadItemKind::Files,
        _ => ThreadItemKind::Tool,
    }
}

/// The image an image view looked at: Codex's `view_image` gives the path itself, Claude's
/// `Read` of an image file names it as `file_path`.
pub(crate) fn image_path(name: &str, input: Option<&str>) -> Option<String> {
    let input = input?;
    match crate::digest::bare_tool_name(name) {
        "view_image" => Some(json_field(input, "path").unwrap_or_else(|| input.to_owned())),
        "Read" => json_field(input, "file_path").filter(|path| is_image_file(path)),
        _ => None,
    }
}

/// What a web search looked for or a page read opened: Claude's `WebSearch` query and
/// `WebFetch` URL, Codex's `web_search` query (given as it is) or the page it opened.
pub(crate) fn web_target(name: &str, input: Option<&str>) -> Option<String> {
    let input = input?;
    match crate::digest::bare_tool_name(name) {
        "WebSearch" => json_field(input, "query"),
        "WebFetch" => json_field(input, "url"),
        "web_search" => Some(
            json_field(input, "query")
                .or_else(|| json_field(input, "url"))
                .unwrap_or_else(|| input.to_owned()),
        ),
        _ => None,
    }
}

/// A Chat's web search or page read, as a row for its answer: a Codex page open is a page
/// read, named by its URL as its call's row is; its find in a page has no row of its own.
pub(super) fn web_step(tool: &str, input: Option<&str>) -> Option<OrchestratorStepKind> {
    let target = web_target(tool, input)?;
    let action = input.and_then(|input| json_field(input, "action"));
    Some(match (tool, action.as_deref()) {
        ("WebFetch", _) | ("web_search", Some("openPage")) => {
            OrchestratorStepKind::ReadPage { url: target }
        }
        (_, Some(_)) => return None,
        _ => OrchestratorStepKind::SearchedWeb { query: target },
    })
}

fn json_field(input: &str, key: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(input).ok()?;
    value.get(key)?.as_str().map(str::to_owned)
}

fn is_image_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tiff", ".heic",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

/// A file change's files with their lines in and out: as its CLI counted them, else from
/// its diff as kept, else none. A moved file is named where it went, with where it was.
pub(super) fn file_stats(changes: &[FileChange]) -> Vec<FileStat> {
    changes
        .iter()
        .map(|change| {
            let (insertions, deletions) = match (change.added, change.removed) {
                (Some(added), Some(removed)) => (added, removed),
                _ => change
                    .diff
                    .as_deref()
                    .map(|diff| diff_counts(change.kind, diff))
                    .unwrap_or_default(),
            };
            FileStat {
                path: change
                    .moved_to
                    .clone()
                    .unwrap_or_else(|| change.path.clone()),
                insertions,
                deletions,
                binary: false,
                change: Some(change.kind),
                from: change.moved_to.is_some().then(|| change.path.clone()),
            }
        })
        .collect()
}

/// A tool call's one-line detail on its row, from its arguments: a find in a web page by what it
/// looked for and where ("'lts' in https://nodejs.org/…"), else the first of its query, pattern,
/// path, URL, title, task, name (a preview's) or command (Bash's, run's own words).
pub(super) fn step_detail(args: &serde_json::Value) -> Option<String> {
    let text = |key: &str| args.get(key).and_then(serde_json::Value::as_str);
    let find = (text("action") == Some("findInPage"))
        .then(|| Some(format!("'{}' in {}", text("pattern")?, text("url")?)))
        .flatten();
    find.or_else(|| {
        [
            "query",
            "pattern",
            "file_path",
            "path",
            "url",
            "title",
            "task",
            "name",
            "command",
        ]
        .into_iter()
        .find_map(|key| text(key).map(str::to_owned))
    })
    .map(|text| text.chars().take(240).collect())
}

/// The `out-…` alias a digest's header names: an output the model got as a digest has its
/// whole self stored under it.
pub(super) fn digest_alias(output: &str) -> Option<String> {
    let first = output.lines().next()?;
    crate::digest::digest_alias(first).map(str::to_owned)
}

impl SessionManager {
    pub(super) async fn tool_row(&self, conv: &ConvLive, row: OrchestratorStepKind) {
        if matches!(&row, OrchestratorStepKind::Tool { name, .. } if crate::digest::own_run(name)) {
            self.command_row(conv, row).await;
        } else {
            self.orchestrator_step(&conv.id, row).await;
        }
    }

    /// Files a command row of the thread's: while it runs, its streamed output
    /// is kept for it; once it ends, an output longer than its event kept is stored whole and
    /// the row names where.
    pub(super) async fn command_row(&self, conv: &ConvLive, mut row: OrchestratorStepKind) {
        let OrchestratorStepKind::Tool {
            item_id,
            status,
            exit,
            output_bytes,
            full_output,
            ..
        } = &mut row
        else {
            return;
        };
        if *status == ItemStatus::InProgress {
            let mut items = conv.items();
            let live = items.live.entry(item_id.clone()).or_default();
            if live.bytes() > 0 {
                *output_bytes = Some(live.bytes());
            }
            live.row = Some(row.clone());
        } else {
            let ended = conv.items().live.remove(item_id.as_str());
            let Some(live) = ended else {
                self.orchestrator_step(&conv.id, row).await;
                return;
            };
            if live.bytes() > 0 {
                *output_bytes = Some(live.bytes());
            }
            if full_output.is_none() && live.bytes() > EVENT_OUTPUT as u64 {
                let ended = exit.map_or_else(
                    || format!("{status:?}").to_lowercase(),
                    |code| format!("exit {code}"),
                );
                match self
                    .store_output_as(
                        &conv.id,
                        OutputSource::Shell,
                        &ended,
                        live.output,
                        false,
                        false,
                        false,
                    )
                    .await
                {
                    Ok((stored, _)) => *full_output = Some(stored.alias),
                    Err(err) => {
                        crate::logging::private_trace!(conv.private_logs, warn, "could not store a command's output"; conversation = %conv.id, error = %err);
                    }
                }
            }
        }
        self.orchestrator_step(&conv.id, row).await;
    }

    /// Output a running command of the thread's printed (Codex): kept for it, and its row hears
    /// how much at most once a [`OUTPUT_STEP_EVERY`].
    pub(super) async fn command_output(&self, conv: &ConvLive, item_id: &str, text: &str) {
        self.command_output_bytes(conv, item_id, text.as_bytes())
            .await;
    }

    async fn command_output_bytes(&self, conv: &ConvLive, item_id: &str, text: &[u8]) {
        let row = {
            let mut items = conv.items();
            let live = items.live.entry(item_id.to_owned()).or_default();
            let room = OUTPUT_MAX_BYTES.saturating_sub(live.output.len());
            let kept = text.len().min(room);
            live.output.extend_from_slice(&text[..kept]);
            live.dropped += (text.len() - kept) as u64;
            let bytes = live.bytes();
            let due = live
                .told
                .is_none_or(|(at, told)| told != bytes && at.elapsed() >= OUTPUT_STEP_EVERY);
            match live.row.as_mut() {
                Some(OrchestratorStepKind::Tool { output_bytes, .. }) if due => {
                    *output_bytes = Some(bytes);
                    live.told = Some((Instant::now(), bytes));
                    live.row.clone()
                }
                _ => None,
            }
        };
        if let Some(row) = row {
            self.orchestrator_step(&conv.id, row).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use brigadier_providers::FileChangeKind;

    /// An image view is an image item with the path it looked at, from either CLI: Codex's
    /// `view_image` gives the path as it is, Claude's `Read` of an image file names it. A
    /// `Read` of a text file stays a tool call.
    #[test]
    fn an_image_view_is_an_image_item_with_its_path() {
        assert_eq!(
            tool_item("view_image", Some("/w/shot.png")),
            ThreadItemKind::Image
        );
        assert_eq!(
            image_path("view_image", Some("/w/shot.png")).as_deref(),
            Some("/w/shot.png")
        );
        let read = r#"{"file_path":"/w/ui/Screen Shot.PNG"}"#;
        assert_eq!(tool_item("Read", Some(read)), ThreadItemKind::Image);
        assert_eq!(
            image_path("Read", Some(read)).as_deref(),
            Some("/w/ui/Screen Shot.PNG")
        );
        let text = r#"{"file_path":"/w/src/main.rs"}"#;
        assert_eq!(tool_item("Read", Some(text)), ThreadItemKind::Tool);
        assert_eq!(image_path("Read", Some(text)), None);
        assert_eq!(tool_item("Read", None), ThreadItemKind::Tool);
    }

    /// A row's detail: a Codex find in a page says what and where, an opened page its URL, a
    /// search its query, any other call its first known argument.
    #[test]
    fn a_rows_detail_says_what_a_web_action_looked_for() {
        let detail = |args: serde_json::Value| step_detail(&args);
        assert_eq!(
            detail(serde_json::json!({"action": "findInPage", "url": "https://nodejs.org/en", "pattern": "LTS"}))
                .as_deref(),
            Some("'LTS' in https://nodejs.org/en")
        );
        assert_eq!(
            detail(serde_json::json!({"action": "openPage", "url": "https://nodejs.org/en"}))
                .as_deref(),
            Some("https://nodejs.org/en")
        );
        assert_eq!(
            detail(serde_json::json!({"query": "node lts site:nodejs.org"})).as_deref(),
            Some("node lts site:nodejs.org")
        );
        assert_eq!(
            detail(serde_json::json!({"command": "cargo test"})).as_deref(),
            Some("cargo test")
        );
        assert_eq!(detail(serde_json::json!({"other": 1})), None);
    }

    /// A web search is a web item with its query, a page read with its URL: Claude's tools
    /// give them as JSON fields, Codex's `web_search` the query as it is.
    #[test]
    fn a_web_search_is_a_web_item_with_its_query_or_url() {
        let search = r#"{"query":"tokio select biased","allowed_domains":["docs.rs"]}"#;
        assert_eq!(tool_item("WebSearch", Some(search)), ThreadItemKind::Web);
        assert_eq!(
            web_target("WebSearch", Some(search)).as_deref(),
            Some("tokio select biased")
        );
        let fetch = r#"{"url":"https://docs.rs/tokio","prompt":"What does biased do?"}"#;
        assert_eq!(tool_item("WebFetch", Some(fetch)), ThreadItemKind::Web);
        assert_eq!(
            web_target("WebFetch", Some(fetch)).as_deref(),
            Some("https://docs.rs/tokio")
        );
        assert_eq!(
            tool_item("web_search", Some("rust 2027 edition")),
            ThreadItemKind::Web
        );
        assert_eq!(
            web_target("web_search", Some("rust 2027 edition")).as_deref(),
            Some("rust 2027 edition")
        );
        assert_eq!(web_target("Read", Some(fetch)), None);
    }

    /// A Chat's web rows: a page read by its URL, from Claude's `WebFetch` and a Codex page open
    /// alike (not a search for the open's JSON), a search by its query; a find in a page has
    /// none (its call's row says it).
    #[test]
    fn a_codex_page_open_is_a_page_read_not_a_search_for_its_json() {
        let url = "https://blog.rust-lang.org/";
        let fetch = serde_json::json!({ "url": url, "prompt": "What's new?" }).to_string();
        let open = serde_json::json!({ "action": "openPage", "url": url }).to_string();
        let find = serde_json::json!({ "action": "findInPage", "url": url, "pattern": "1.95" })
            .to_string();
        let search = serde_json::json!({ "query": "rust 1.95" }).to_string();
        let read = |tool: &str, input: &str| match web_step(tool, Some(input)) {
            Some(OrchestratorStepKind::ReadPage { url }) => Some(format!("read {url}")),
            Some(OrchestratorStepKind::SearchedWeb { query }) => Some(format!("search {query}")),
            _ => None,
        };
        assert_eq!(read("WebFetch", &fetch), Some(format!("read {url}")));
        assert_eq!(read("web_search", &open), Some(format!("read {url}")));
        assert_eq!(read("web_search", &find), None);
        assert_eq!(
            read("web_search", &search).as_deref(),
            Some("search rust 1.95")
        );
        assert_eq!(
            read("WebSearch", &search).as_deref(),
            Some("search rust 1.95")
        );
        // Its call's row names the same page, so the thread shows one row for it.
        assert_eq!(
            step_detail(&serde_json::from_str(&open).unwrap()).as_deref(),
            Some(url)
        );
    }

    /// Commands (with or without the MCP prefix), edits and other tools.
    #[test]
    fn other_calls_are_commands_file_changes_or_tools() {
        assert_eq!(
            tool_item("mcp__brigadier__run", None),
            ThreadItemKind::Command
        );
        assert_eq!(
            tool_item("brigadier/run_unsandboxed", None),
            ThreadItemKind::Command
        );
        assert_eq!(tool_item("Edit", None), ThreadItemKind::Files);
        assert_eq!(
            tool_item("mcp__brigadier__delegate_task", None),
            ThreadItemKind::Tool
        );
        assert_eq!(tool_item("Grep", None), ThreadItemKind::Tool);
    }

    /// A change's files keep their CLI's counts; a change from before they were counted is
    /// counted from its diff; one with neither has 0 and 0.
    #[test]
    fn a_file_change_lists_each_files_lines_in_and_out() {
        let change = |path: &str, diff: Option<&str>, counted: Option<(u32, u32)>| FileChange {
            original: None,
            path: path.into(),
            moved_to: None,
            kind: FileChangeKind::Update,
            diff: diff.map(str::to_owned),
            added: counted.map(|(added, _)| added),
            removed: counted.map(|(_, removed)| removed),
        };
        let stats = file_stats(&[
            change("a.rs", Some("-x\n+y\n"), Some((7, 2))),
            change("b.rs", Some("-x\n+y\n+z\n"), None),
            change("c.rs", None, None),
        ]);
        let counts: Vec<(&str, u32, u32)> = stats
            .iter()
            .map(|stat| (stat.path.as_str(), stat.insertions, stat.deletions))
            .collect();
        assert_eq!(counts, [("a.rs", 7, 2), ("b.rs", 2, 1), ("c.rs", 0, 0)]);
        assert!(stats.iter().all(|stat| stat.from.is_none()));
    }

    /// A Codex rename is named where the file went, with where it was: its row reads Renamed,
    /// not Edited.
    #[test]
    fn a_moved_file_is_named_where_it_went_with_where_it_was() {
        let stats = file_stats(&[FileChange {
            original: None,
            path: "/w/a.rs".into(),
            moved_to: Some("/w/b.rs".into()),
            kind: FileChangeKind::Update,
            diff: Some(String::new()),
            added: Some(0),
            removed: Some(0),
        }]);
        assert_eq!(stats[0].path, "/w/b.rs");
        assert_eq!(stats[0].from.as_deref(), Some("/w/a.rs"));
        assert_eq!((stats[0].insertions, stats[0].deletions), (0, 0));
    }

    /// Only a digest's header names a stored output, as it does now or did before S16b; an
    /// output that only mentions an alias doesn't.
    #[test]
    fn a_digest_names_where_its_whole_output_is() {
        let digest = "exit 2 [full output: out-1a2b3c4d in /data/blobs/ab/abcd, 3000 lines, 26890 bytes]\nline 0";
        assert_eq!(digest_alias(digest).as_deref(), Some("out-1a2b3c4d"));
        let old =
            "exit 2 [full output: read_artifact out-1a2b3c4d, 3000 lines, 26890 bytes]\nline 0";
        assert_eq!(digest_alias(old).as_deref(), Some("out-1a2b3c4d"));
        let json = r#"[{"type":"text","text":"exit 0 [full output: read_artifact out-0000ffff, 9 lines, 9000 bytes]\nok"}]"#;
        assert_eq!(digest_alias(json).as_deref(), Some("out-0000ffff"));
        assert_eq!(digest_alias("ok\nsee read_artifact out-1a2b3c4d"), None);
        assert_eq!(digest_alias("ok\nsee out-1a2b3c4d"), None);
    }
}
