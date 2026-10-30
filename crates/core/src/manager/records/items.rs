//! Exact items from the CLI's durable records, before the display reader merges tool calls
//! and results. IDs namespace native IDs by CLI session; records without IDs use content and
//! occurrence, never a position in the merged lineage. Compaction markers are not items. Each
//! CLI's adapter reads its own format ([`brigadier_providers::adapters::Adapter::items`]).
use brigadier_providers::KindFacts as _;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use brigadier_providers::adapters::adapter;
use brigadier_providers::session_file::FileEntry;
use brigadier_providers::{ProviderKind, session_file};
use serde_json::Value;

use super::{RecordSession, entry_text, read_lineage, saved_text};

#[derive(Debug, Clone)]
pub(super) struct Item {
    pub id: String,
    pub role: String,
    pub tool: Option<String>,
    pub kind: &'static str,
    pub text: String,
    pub saved: Option<String>,
    pub at_ms: Option<i64>,
    pub position: i64,
}

/// Reads originals on every exact read; an index is never the authority for the bytes.
pub(super) fn read(session: &RecordSession) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let mut known = HashMap::new();
    let mut position = 0i64;
    let describe =
        |entry: &FileEntry| entry_text(entry, &session.blobs).map(|(_, body, saved)| (body, saved));
    for generation in read_lineage(&session.record) {
        let Some(file) = generation.file else {
            continue;
        };
        let Some(cli) = adapter(generation.provider) else {
            continue;
        };
        let path = session_file::locate(generation.provider, Path::new(&file));
        let lines: Vec<(Option<String>, Value)> = sources(generation.provider, &path)
            .into_iter()
            .flat_map(|(scope, path)| {
                std::fs::read_to_string(path)
                    .unwrap_or_default()
                    .lines()
                    .filter_map(|line| serde_json::from_str(line).ok())
                    .map(|value| (scope.clone(), value))
                    .collect::<Vec<_>>()
            })
            .collect();
        let display = session_file::read(generation.provider, &path).unwrap_or_default();
        for native in (cli.items)(&lines, &display, &describe) {
            let key = format!(
                "{}\0{}\0{}\0{}",
                // Ids kept name the CLI by its vendor word ("Claude"), as they always did.
                generation.provider.vendor(),
                generation.session_id,
                native.id,
                native.kind
            );
            let id = format!("i-{}", &blake3::hash(key.as_bytes()).to_hex()[..32]);
            // An output Brigadier's output hook trimmed names the blob it kept the whole in.
            let saved = native.saved.or_else(|| {
                (cli.output_hook && native.kind == "output")
                    .then(|| super::kept_blob(&native.text, &session.blobs))
                    .flatten()
            });
            let item = Item {
                id: id.clone(),
                role: native.role.into(),
                tool: native.tool,
                kind: native.kind,
                text: native.text,
                saved,
                at_ms: native.at_ms,
                position: native.at.map_or(position, |n| position + n as i64),
            };
            if let Some(&index) = known.get(&id) {
                items[index] = item;
            } else {
                known.insert(id, items.len());
                items.push(item);
            }
        }
        position += display.len() as i64;
    }
    for item in &mut items {
        if let Some(saved) = &item.saved
            && let Some(whole) = saved_text(saved, &session.blobs)
        {
            item.text = whole;
        }
    }
    items
}

/// The files `provider`'s exact items at `path` are read from, each with its subagent scope.
pub(super) fn sources(provider: ProviderKind, path: &Path) -> Vec<(Option<String>, PathBuf)> {
    adapter(provider).map_or_else(
        || vec![(None, path.to_owned())],
        |cli| (cli.item_sources)(path),
    )
}
