//! The Brain as a pull-only search over the records sessions already keep (STRIP-DOWN-PLAN
//! S18, G14, Q5). Brigadier writes none of them: each session's (and each worker's) CLI
//! session files as `record/lineage.json` lists them, with the whole outputs Claude saved under
//! `tool-results/` (or the Bash hook kept in Brigadier's blob store); its ledger, `ledger-archive.md` and hand-off notes. They are indexed when a
//! session searches, each file again from where it changed.
//!
//! - `brigadier brain search <query> [--session <id>]`: every session of the project, or one.
//! - `brigadier brain read <session> <from> <to>`: a span of a session's transcript, verbatim.
//!   Positions number its CLI items across all its generations, from 0.
//! - `brigadier transcript search|read`: the same, within the calling session (a thread's
//!   workers included).
//!
//! A ledger entry "Learned from reading: <path>:<line> …" is a pointer, not a copy: the file it
//! cites is hashed when the entry is first indexed, and a hit reads as stale once it changed.

mod items;

use std::path::{Path, PathBuf};
use std::time::Instant;

use brigadier_brain::{Brain, Cited, RecordEntry, RecordFile, RecordHit, RecordSource};
use brigadier_providers::session_file::{self, FileEntry};
use brigadier_providers::{ProviderEvent, Role as CliRole};
use serde::Deserialize;

use super::session_dir::{LEDGER_FILE, read_lineage};
use super::{SessionManager, blocking};
use crate::model::{ConversationId, ProjectId};
use crate::{Error, Result};

/// The archive beside the ledger, where finished entries move.
pub(crate) const LEDGER_ARCHIVE_FILE: &str = "ledger-archive.md";
/// The folder of hand-off notes in `record/`.
pub(crate) const HANDOFF_DIR: &str = "handoff";
/// Hits a search shows by default, and at most.
const HITS: u32 = 8;
const HITS_MAX: u32 = 30;
/// Most transcript items one read returns.
pub(crate) const READ_MAX: i64 = 50;
/// What marks a ledger entry that points at a file.
const LEARNED: &str = "Learned from reading";

/// `brigadier brain search` and `transcript search`.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecordSearch {
    pub query: String,
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
}

/// `brigadier brain read` and `transcript read`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecordRead {
    pub session: String,
    #[serde(default)]
    pub from: Option<i64>,
    #[serde(default)]
    pub to: Option<i64>,
    #[serde(default)]
    pub item: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub tool: Option<String>,
    #[serde(default)]
    pub offset: Option<usize>,
    #[serde(default)]
    pub limit: Option<usize>,
}

/// One session's records: a session's folder, or a worker's in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RecordSession {
    pub conversation: ConversationId,
    /// How hits name it: the folder's short id; a worker's `<short>/<n>-<task8>`.
    pub key: String,
    /// Its `record/`.
    pub record: PathBuf,
    /// Where a relative path a ledger entry cites is looked up, in order.
    pub roots: Vec<PathBuf>,
    /// Brigadier's blob store, where the Bash hook keeps a whole output it trimmed.
    pub blobs: PathBuf,
}

impl SessionManager {
    /// The sessions of project `project` that keep records, each worker's apart. Tab chats keep
    /// theirs to themselves.
    pub(crate) fn record_sessions(&self, project: &ProjectId) -> Vec<RecordSession> {
        let repos: Vec<PathBuf> = self
            .core
            .project(project)
            .map(|project| {
                project
                    .repos
                    .into_iter()
                    .map(|repo| PathBuf::from(repo.path))
                    .collect()
            })
            .unwrap_or_default();
        let blobs = self.core.store().blobs().root().to_path_buf();
        let mut sessions = Vec::new();
        for conversation in self.core.catalog().conversations {
            if conversation.project_id.as_ref() != Some(project) || conversation.tab_chat.is_some()
            {
                continue;
            }
            let dir = self.session_dir(&conversation.id);
            let mut roots: Vec<PathBuf> = self
                .recorded_workspace(&conversation.id)
                .map(|workspace| workspace.path)
                .into_iter()
                .collect();
            roots.extend(repos.iter().cloned());
            let key = conversation.id.short().to_owned();
            sessions.push(RecordSession {
                conversation: conversation.id.clone(),
                key: key.clone(),
                record: dir.record(),
                roots: roots.clone(),
                blobs: blobs.clone(),
            });
            let Ok(workers) = std::fs::read_dir(dir.root().join("workers")) else {
                continue;
            };
            let mut workers: Vec<PathBuf> = workers
                .flatten()
                .map(|entry| entry.path())
                .filter(|path| path.join("record").is_dir())
                .collect();
            workers.sort();
            for worker in workers {
                let Some(name) = worker.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                sessions.push(RecordSession {
                    conversation: conversation.id.clone(),
                    key: format!("{key}/{name}"),
                    record: worker.join("record"),
                    roots: std::iter::once(worker.join("work"))
                        .chain(roots.iter().cloned())
                        .collect(),
                    blobs: blobs.clone(),
                });
            }
        }
        sessions
    }

    /// The project and its records index for a call from `conversation`.
    async fn records_brain(&self, conversation: &ConversationId) -> Result<(ProjectId, Brain)> {
        let project = self.project_of(conversation).ok_or_else(|| {
            Error::Invalid("this conversation has no project, so no records to search".into())
        })?;
        let brain = self.project_brain(&project).await?.brain.clone();
        Ok((project, brain))
    }

    /// `brigadier brain search` (`own` unset) and `transcript search` (`own`: the caller's key).
    pub(crate) async fn records_search(
        &self,
        conversation: &ConversationId,
        own: Option<String>,
        args: RecordSearch,
    ) -> Result<String> {
        validate_role(args.role.as_deref())?;
        let started = Instant::now();
        let (project, brain) = self.records_brain(conversation).await?;
        let sessions = self.record_sessions(&project);
        let scope = match (&own, args.session.as_deref()) {
            (Some(own), None) => Some(own.clone()),
            (own, Some(asked)) => {
                let key = session_key(&sessions, asked)?;
                if let Some(own) = own
                    && !within(&key, own)
                {
                    return Err(Error::Invalid(format!(
                        "{asked} is not this session's; `brigadier brain search --session {asked}` searches it"
                    )));
                }
                Some(key)
            }
            (None, None) => None,
        };
        let synced: Vec<RecordSession> = sessions
            .into_iter()
            .filter(|session| {
                scope
                    .as_deref()
                    .is_none_or(|scope| within(&session.key, scope))
            })
            .collect();
        let limit = args.limit.unwrap_or(HITS).clamp(1, HITS_MAX);
        let query = args.query;
        let hits = blocking(move || {
            for session in &synced {
                sync_session(&brain, session)?;
            }
            let mut hits = brain
                .search_records_filtered(
                    &query,
                    scope.as_deref(),
                    limit,
                    args.role.as_deref(),
                    args.tool.as_deref(),
                )
                .map_err(super::brains::brain_error)?;
            for hit in &mut hits {
                mark_stale(hit);
                if let Some(session) = synced.iter().find(|session| session.key == hit.session) {
                    mark_ended(hit, session);
                }
            }
            Ok(hits)
        })
        .await?;
        self.brains
            .note_query(started.elapsed().as_secs_f64() * 1000.0);
        let command = if own.is_some() { "transcript" } else { "brain" };
        Ok(hits_text(&hits, command))
    }

    /// `brigadier brain read` (`own` unset) and `transcript read` (`own`: the caller's key).
    pub(crate) async fn records_read(
        &self,
        conversation: &ConversationId,
        own: Option<String>,
        args: RecordRead,
    ) -> Result<String> {
        let (project, _) = self.records_brain(conversation).await?;
        let sessions = self.record_sessions(&project);
        let key = session_key(&sessions, &args.session)?;
        if let Some(own) = &own
            && !within(&key, own)
        {
            return Err(Error::Invalid(format!(
                "{} is not this session's; `brigadier brain read` reads it",
                args.session
            )));
        }
        let session = sessions
            .into_iter()
            .find(|session| session.key == key)
            .ok_or_else(|| Error::Invalid(format!("no session {}", args.session)))?;
        blocking(move || read_items(&session, &args)).await
    }
}

/// Whether session `key` is `scope` or one of its workers.
fn within(key: &str, scope: &str) -> bool {
    key == scope
        || key
            .strip_prefix(scope)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The session `asked` names: its key, its conversation's full or short id.
fn session_key(sessions: &[RecordSession], asked: &str) -> Result<String> {
    let asked = asked.trim();
    sessions
        .iter()
        .find(|session| session.key == asked || session.conversation.0 == asked)
        .map(|session| session.key.clone())
        .ok_or_else(|| Error::Invalid(format!("no session {asked} in this project")))
}

/// Indexes what changed of `session`'s records.
pub(crate) fn sync_session(brain: &Brain, session: &RecordSession) -> Result<()> {
    let error = super::brains::brain_error;
    let file = |source: RecordSource, path: &Path, stamp: String| RecordFile {
        conversation_id: session.conversation.0.clone(),
        session: session.key.clone(),
        source,
        path: path.display().to_string(),
        stamp,
    };
    let lineage = session.record.join("lineage.json");
    if let Some((base_stamp, _)) = transcript_stamp(&session.record) {
        let held = brain.record_paths(&session.key).map_err(error)?;
        let output_stamp = |paths: Vec<String>| {
            let paths: std::collections::BTreeSet<_> = paths.into_iter().collect();
            let mut stamp = base_stamp.clone();
            for path in paths {
                stamp.push_str(&format!(
                    ";{}={}",
                    path,
                    file_stamp(Path::new(&path)).unwrap_or_default()
                ));
            }
            stamp
        };
        let stamp = output_stamp(
            held.iter()
                .filter(|(_, source)| *source == RecordSource::ToolOutput)
                .map(|(path, _)| path.split(SAVED_BY).next().unwrap_or(path).to_owned())
                .collect(),
        );
        let path = lineage.display().to_string();
        if brain.record_stamp(&path).map_err(error)?.as_deref() != Some(stamp.as_str()) {
            let entries = items::read(session);
            // Saved output files can appear or grow after the transcript reference was written.
            let stamp = output_stamp(
                entries
                    .iter()
                    .filter_map(|item| item.saved.clone())
                    .collect(),
            );
            let live_saved: std::collections::HashSet<_> = entries
                .iter()
                .filter_map(|item| {
                    Some(saved_key(
                        item.saved.as_deref()?,
                        &format!("{}:{}", session.key, item.id),
                    ))
                })
                .collect();
            for (path, source) in held {
                if source == RecordSource::ToolOutput && !live_saved.contains(&path) {
                    brain.forget_record_file(&path).map_err(error)?;
                }
            }
            let mut indexed = Vec::new();
            for (position, item) in entries.into_iter().enumerate() {
                let entry = RecordEntry {
                    item_id: Some(item.id.clone()),
                    tool: item.tool,
                    position: position as i64,
                    role: item.role,
                    at_ms: item.at_ms,
                    text: item.text,
                    cited: None,
                };
                if let Some(saved) = item.saved {
                    let key = saved_key(&saved, &format!("{}:{}", session.key, item.id));
                    brain
                        .sync_records(
                            file(RecordSource::ToolOutput, Path::new(&key), stamp.clone()),
                            vec![entry],
                        )
                        .map_err(error)?;
                } else {
                    indexed.push(entry);
                }
            }
            brain
                .sync_records(file(RecordSource::Transcript, &lineage, stamp), indexed)
                .map_err(error)?;
        }
    }
    // The archive before the ledger: an entry that moved there takes the hash its ledger row
    // took of the file it cites before that row goes.
    let mut notes: Vec<(RecordSource, PathBuf)> = vec![
        (
            RecordSource::LedgerArchive,
            session.record.join(LEDGER_ARCHIVE_FILE),
        ),
        (RecordSource::Ledger, session.record.join(LEDGER_FILE)),
    ];
    if let Ok(dir) = std::fs::read_dir(session.record.join(HANDOFF_DIR)) {
        let mut found: Vec<PathBuf> = dir
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
            .collect();
        found.sort();
        notes.extend(found.into_iter().map(|path| (RecordSource::Handoff, path)));
    }
    for (source, path) in notes {
        let Some(stamp) = file_stamp(&path) else {
            continue;
        };
        let key = path.display().to_string();
        if brain.record_stamp(&key).map_err(error)?.as_deref() == Some(stamp.as_str()) {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        brain
            .sync_records(
                file(source, &path, stamp),
                note_entries(&text, &session.roots),
            )
            .map_err(error)?;
    }
    // A ledger, archive or hand-off note that is gone (deleted, renamed) leaves nothing behind.
    for (path, source) in brain.record_paths(&session.key).map_err(error)? {
        let note = matches!(
            source,
            RecordSource::Ledger | RecordSource::LedgerArchive | RecordSource::Handoff
        );
        if note && !Path::new(&path).is_file() {
            brain.forget_record_file(&path).map_err(error)?;
        }
    }
    Ok(())
}

/// Where a session's saved whole output is indexed: the file and the session, since a blob the
/// Bash hook kept is one file for every session whose output it holds.
fn saved_key(saved: &str, session: &str) -> String {
    format!("{saved}{SAVED_BY}{session}")
}

/// Joins a saved output's file and session in its record key ([`saved_key`]).
const SAVED_BY: &str = " · saved by ";

/// A file's size and change time, as a stamp; none when it is missing.
fn file_stamp(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let at = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some(format!("{}:{at}", meta.len()))
}

/// The stamp of a session's transcript (its lineage and each CLI file in it), and those files
/// with their CLI; none without a lineage.
pub(crate) fn transcript_stamp(
    record: &Path,
) -> Option<(String, Vec<(brigadier_providers::ProviderKind, PathBuf)>)> {
    let mut stamp = file_stamp(&record.join("lineage.json"))?;
    stamp.push_str(";exact-items-v2");
    let mut files = Vec::new();
    for session in read_lineage(record) {
        let Some(file) = session.file else {
            continue;
        };
        let path = session_file::locate(session.provider, Path::new(&file));
        for (_, source) in items::sources(session.provider, &path) {
            stamp.push_str(&format!(
                ";{}={}",
                source.display(),
                file_stamp(&source).unwrap_or_default()
            ));
        }
        files.push((session.provider, path));
    }
    Some((stamp, files))
}

/// A session's transcript: its CLI files' items, oldest generation first.
pub(crate) fn transcript(files: &[(brigadier_providers::ProviderKind, PathBuf)]) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    for (provider, path) in files {
        let more = session_file::read(*provider, path).unwrap_or_default();
        entries = session_file::merge(entries, more);
    }
    entries
}

/// What a transcript item says, as search and read show it: its role, its text, and the file
/// its whole output was saved to, when it was (Claude's `tool-results/`, or the blob the Bash
/// hook kept it in when it trimmed it).
pub(crate) fn entry_text(
    entry: &FileEntry,
    blobs: &Path,
) -> Option<(&'static str, String, Option<String>)> {
    let saved = |kept: &Option<Box<brigadier_providers::KeptOutput>>| {
        kept.as_ref()
            .and_then(|kept| kept.file.as_ref())
            .map(|file| file.path.clone())
            .filter(|path| brigadier_providers::is_tool_results_file(path))
    };
    Some(match &entry.event {
        ProviderEvent::Message { role, text, .. } => (
            match role {
                CliRole::User => "user",
                CliRole::Assistant => "assistant",
            },
            text.clone(),
            None,
        ),
        ProviderEvent::Command {
            command,
            exit_code,
            output,
            kept,
            ..
        } => {
            let mut text = format!("Ran `{}`", first_line(command));
            if let Some(code) = exit_code {
                text.push_str(&format!(" (exit {code})"));
            }
            if command.trim().contains('\n') {
                text.push('\n');
                text.push_str(command.trim());
            }
            if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
                text.push('\n');
                text.push_str(output);
            }
            let saved = saved(kept).or_else(|| {
                output
                    .as_deref()
                    .and_then(|output| kept_blob(output, blobs))
            });
            ("command", text, saved)
        }
        ProviderEvent::ToolCall {
            name,
            input,
            output,
            kept,
            ..
        } => {
            let mut text = format!("Called {name}");
            if let Some(input) = input
                .as_deref()
                .filter(|input| !matches!(input.trim(), "" | "{}"))
            {
                text.push('\n');
                text.push_str(input.trim());
            }
            if let Some(output) = output.as_deref().filter(|output| !output.trim().is_empty()) {
                text.push('\n');
                text.push_str(output);
            }
            ("tool", text, saved(kept))
        }
        ProviderEvent::FileChanges { changes, .. } => {
            let paths: Vec<&str> = changes.iter().map(|change| change.path.as_str()).collect();
            ("edit", format!("Changed {}", paths.join(", ")), None)
        }
        ProviderEvent::Looked {
            reads, searches, ..
        } => {
            let mut parts: Vec<String> = reads
                .iter()
                .map(|read| format!("Read {}", read.path))
                .collect();
            parts.extend(searches.iter().filter_map(|search| {
                search
                    .pattern
                    .as_ref()
                    .map(|pattern| format!("Searched `{pattern}`"))
            }));
            ("read", parts.join("\n"), None)
        }
        ProviderEvent::Image { path, prompt, .. } => (
            "image",
            [prompt.as_deref(), path.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join("\n"),
            None,
        ),
        _ => return None,
    })
    .filter(|(_, text, _)| !text.trim().is_empty())
}

/// The blob a Bash hook digest (`exit 0 [full output: out-… in <blob>, N lines, M bytes]`, its
/// first line) names, when it is shaped as a file of Brigadier's blob store: `<blobs>/<ab>/<hash>`.
fn kept_blob(output: &str, blobs: &Path) -> Option<String> {
    let first = output.lines().next()?;
    let alias = crate::digest::digest_alias(first)?;
    let rest = &first[first.find(alias)? + alias.len()..];
    let rest = rest.strip_prefix(" in ")?;
    let (path, _) = rest.rsplit_once(" lines, ")?;
    let (path, _) = path.rsplit_once(", ")?;
    let name = Path::new(path).file_name()?.to_str()?;
    let shaped = name.len() == 64
        && name
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'));
    (shaped && Path::new(path) == blobs.join(&name[..2]).join(name)).then(|| path.to_owned())
}

/// The whole of a saved output. A blob is read only when its content still hashes to its name,
/// so a digest line a command printed never has another file read as one.
fn saved_text(path: &str, blobs: &Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let path = Path::new(path);
    if path.starts_with(blobs)
        && path.file_name()?.to_str()? != blake3::hash(&bytes).to_hex().as_str()
    {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn first_line(text: &str) -> &str {
    text.trim().lines().next().unwrap_or_default()
}

/// A ledger's, archive's or hand-off note's lines, numbered from 1; an entry learned from
/// reading points at the file it cites, hashed now.
fn note_entries(text: &str, roots: &[PathBuf]) -> Vec<RecordEntry> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| !line.trim().is_empty())
        .map(|(n, line)| RecordEntry {
            item_id: None,
            tool: None,
            position: n as i64 + 1,
            role: "ledger".into(),
            at_ms: None,
            text: line.to_owned(),
            cited: cited(line).map(|(path, line)| {
                let path = resolve(&path, roots);
                Cited {
                    hash: hash_of(&path),
                    path: path.display().to_string(),
                    line,
                }
            }),
        })
        .collect()
}

/// The file and line an entry learned from reading cites (`path:line`, `path:from-to`, or a
/// bare path).
pub(crate) fn cited(line: &str) -> Option<(String, Option<u32>)> {
    let rest = &line[line.find(LEARNED)? + LEARNED.len()..];
    rest.split(|c: char| c.is_whitespace() || matches!(c, '`' | '(' | ')' | ',' | '"' | '\''))
        .map(|word| word.trim_matches(|c: char| matches!(c, ':' | ';' | '.' | '*')))
        .filter(|word| !word.is_empty())
        .find_map(|word| {
            let (path, line) = match word.rsplit_once(':') {
                Some((path, line)) if line.starts_with(|c: char| c.is_ascii_digit()) => {
                    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
                    (path, digits.parse().ok())
                }
                _ => (word, None),
            };
            let pathy = path.contains('/')
                || path.rsplit_once('.').is_some_and(|(stem, ext)| {
                    !stem.is_empty()
                        && (1..=6).contains(&ext.len())
                        && ext.chars().all(|c| c.is_ascii_alphanumeric())
                });
            pathy.then(|| (path.to_owned(), line))
        })
}

/// `path` as a file: absolute as it is, else in the first of `roots` that has it (the first
/// root when none does).
fn resolve(path: &str, roots: &[PathBuf]) -> PathBuf {
    let given = Path::new(path);
    if given.is_absolute() {
        return given.to_owned();
    }
    roots
        .iter()
        .map(|root| root.join(given))
        .find(|candidate| candidate.is_file())
        .or_else(|| roots.first().map(|root| root.join(given)))
        .unwrap_or_else(|| given.to_owned())
}

fn hash_of(path: &Path) -> Option<String> {
    std::fs::read(path)
        .ok()
        .map(|bytes| blake3::hash(&bytes).to_hex().to_string())
}

/// Whether the file a hit cites changed since its entry was indexed: then the hash it carries
/// is dropped, which marks it stale.
fn mark_stale(hit: &mut RecordHit) {
    if let Some(cited) = &mut hit.cited
        && cited.hash.is_some()
        && hash_of(Path::new(&cited.path)) != cited.hash
    {
        cited.hash = None;
    }
}

/// Only explicit ledger IDs carry a reliable reversal relation. Do not guess whether a
/// later decision contradicts arbitrary user words or tool evidence.
fn mark_ended(hit: &mut RecordHit, session: &RecordSession) {
    use super::cli_ledger::{Ended, Ledger};
    if !matches!(
        hit.source,
        RecordSource::Ledger | RecordSource::LedgerArchive
    ) {
        return;
    }
    let Ok(ledger) = Ledger::read(&session.record) else {
        return;
    };
    let entry = ledger.entries.iter().find(|entry| {
        session.record.join(entry.file.name()) == Path::new(&hit.path)
            && (entry.line..=entry.last_line).contains(&(hit.position as usize))
    });
    let Some(ended) = entry
        .and_then(|entry| entry.id)
        .and_then(|id| ledger.ended(id))
    else {
        return;
    };
    let (label, by) = match ended {
        Ended::Closed { by } => ("CLOSED", by),
        Ended::Superseded { by } => ("SUPERSEDED", by),
    };
    hit.ended = Some(format!(
        "{label}{}",
        by.map(|id| format!(" by L{id}")).unwrap_or_default()
    ));
}

/// Search hits as the command prints them.
fn hits_text(hits: &[RecordHit], command: &str) -> String {
    if hits.is_empty() {
        return "Nothing in the records matches.".into();
    }
    let mut text = String::new();
    let mut transcript = None;
    for hit in hits {
        let date = hit
            .at_ms
            .map(|at| format!(" · {}", super::prompts::date_of(at)))
            .unwrap_or_default();
        match hit.source {
            RecordSource::Transcript => {
                if hit.item_id.is_none() {
                    transcript.get_or_insert((hit.session.clone(), hit.position));
                }
                text.push_str(&format!(
                    "[{} · {}{}{}]\n",
                    hit.session,
                    hit.item_id
                        .as_ref()
                        .map(|_| String::new())
                        .unwrap_or_else(|| format!("#{} · ", hit.position)),
                    hit.role,
                    date
                ));
                let snippet = hit.snippet.trim();
                if !hit.lead.is_empty() && !snippet.contains(hit.lead.trim()) {
                    text.push_str(hit.lead.trim());
                    text.push_str(if snippet.starts_with('…') {
                        "\n"
                    } else {
                        "\n…"
                    });
                }
            }
            RecordSource::ToolOutput => {
                if hit.item_id.is_none() {
                    transcript.get_or_insert((hit.session.clone(), hit.position));
                }
                text.push_str(&format!(
                    "[{} · {}whole output, saved at {}{date}]\n",
                    hit.session,
                    hit.item_id
                        .as_ref()
                        .map(|_| String::new())
                        .unwrap_or_else(|| format!("#{} · ", hit.position)),
                    hit.path.split(SAVED_BY).next().unwrap_or(&hit.path)
                ));
            }
            RecordSource::Ledger | RecordSource::LedgerArchive | RecordSource::Handoff => {
                text.push_str(&format!(
                    "[{} · {} {}:{}",
                    hit.session,
                    hit.source.as_str(),
                    hit.path,
                    hit.position
                ));
                if let Some(ended) = &hit.ended {
                    text.push_str(&format!(" · {ended}"));
                }
                if let Some(cited) = &hit.cited {
                    let at = cited
                        .line
                        .map(|line| format!(":{line}"))
                        .unwrap_or_default();
                    match cited.hash {
                        Some(_) => text.push_str(&format!(" · points at {}{at}", cited.path)),
                        None => text.push_str(&format!(
                            " · STALE: {}{at} changed since, or can't be read; read it again",
                            cited.path
                        )),
                    }
                }
                text.push_str("]\n");
            }
        }
        if let Some(id) = &hit.item_id {
            text.push_str(&format!(
                "[item {id} · origin {}{} · bytes omitted: {} before, {} after]\n",
                hit.role,
                hit.tool
                    .as_ref()
                    .map(|tool| format!(" · tool {tool}"))
                    .unwrap_or_default(),
                hit.bytes_before,
                hit.bytes_after
            ));
            text.push_str(&format!(
                "`brigadier {command} read {} --item {id}`\n",
                hit.session
            ));
        }
        text.push_str(&hit.snippet);
        text.push_str("\n\n");
    }
    if let Some((session, position)) = transcript {
        text.push_str(&format!(
            "`brigadier {command} read {session} {} {}` gives a transcript span verbatim.",
            (position - 2).max(0),
            position + 2
        ));
    }
    text
}

/// Items `from` to `to` of `session`'s transcript, verbatim: a saved whole output in full.
pub(crate) fn read_span(session: &RecordSession, from: i64, to: i64) -> Result<String> {
    if from < 0 || to < from {
        return Err(Error::Invalid(format!(
            "{from}–{to} is not a span: give <from> <to>, from 0"
        )));
    }
    let to = to.min(from + READ_MAX - 1);
    let files = transcript_stamp(&session.record)
        .map(|(_, files)| files)
        .unwrap_or_default();
    let entries = transcript(&files);
    if entries.is_empty() {
        return Err(Error::Invalid(format!(
            "session {} has no transcript yet",
            session.key
        )));
    }
    let last = entries.len() as i64 - 1;
    if from > last {
        return Err(Error::Invalid(format!(
            "session {}'s transcript ends at #{last}",
            session.key
        )));
    }
    let mut text = String::new();
    for position in from..=to.min(last) {
        let entry = &entries[position as usize];
        let Some((role, body, saved)) = entry_text(entry, &session.blobs) else {
            continue;
        };
        let date = entry
            .at_ms
            .map(|at| format!(" · {}", super::prompts::date_of(at)))
            .unwrap_or_default();
        text.push_str(&format!("[#{position} · {role}{date}]\n{body}\n"));
        if let Some(saved) = saved
            && let Some(whole) = saved_text(&saved, &session.blobs)
        {
            text.push_str(&format!("[the whole output, saved at {saved}]\n{whole}\n"));
        }
        text.push('\n');
    }
    if to < last {
        text.push_str(&format!("[the transcript goes on to #{last}]"));
    }
    Ok(text.trim_end().to_owned())
}

fn validate_role(role: Option<&str>) -> Result<()> {
    if role.is_some_and(|role| !matches!(role, "user" | "assistant" | "tool")) {
        return Err(Error::Invalid(
            "--role must be user, assistant or tool".into(),
        ));
    }
    Ok(())
}

/// An exact read never trims source bytes. Limits and offsets are UTF-8 bytes; boundaries
/// inside a codepoint advance to the next boundary so successive pages do not repeat bytes.
fn read_items(session: &RecordSession, args: &RecordRead) -> Result<String> {
    validate_role(args.role.as_deref())?;
    if args.item.is_none()
        && args.role.is_none()
        && args.tool.is_none()
        && args.offset.is_none()
        && args.limit.is_none()
    {
        return read_span(
            session,
            args.from.ok_or_else(|| {
                Error::Invalid("read takes <session> <from> <to>, or --item <id>".into())
            })?,
            args.to.ok_or_else(|| {
                Error::Invalid("read takes <session> <from> <to>, or --item <id>".into())
            })?,
        );
    }
    if args.item.is_some() && (args.from.is_some() || args.to.is_some()) {
        return Err(Error::Invalid(
            "--item cannot be combined with <from> <to>".into(),
        ));
    }
    if args.limit == Some(0) {
        return Err(Error::Invalid("--limit must be above 0".into()));
    }
    if args.from.is_some() != args.to.is_some()
        || args
            .from
            .is_some_and(|from| from < 0 || args.to.is_some_and(|to| to < from))
    {
        return Err(Error::Invalid(
            "give a valid <from> <to> span, from 0".into(),
        ));
    }
    let items = items::read(session);
    let selected: Vec<_> = items
        .iter()
        .filter(|item| {
            args.item.as_ref().is_none_or(|id| item.id == *id)
                && args.role.as_ref().is_none_or(|role| item.role == *role)
                && args
                    .tool
                    .as_ref()
                    .is_none_or(|tool| item.tool.as_ref() == Some(tool))
                && args.from.is_none_or(|from| item.position >= from)
                && args.to.is_none_or(|to| item.position <= to)
        })
        .collect();
    if selected.is_empty() {
        return Err(Error::Invalid("no matching item in this session".into()));
    }
    let count = selected.len();
    let mut text = String::new();
    for item in selected.into_iter().take(READ_MAX as usize) {
        let offset = args.offset.unwrap_or(0).min(item.text.len());
        let mut start = offset;
        while !item.text.is_char_boundary(start) {
            start += 1;
        }
        let mut end = args.limit.map_or(item.text.len(), |limit| {
            offset.saturating_add(limit).min(item.text.len())
        });
        while !item.text.is_char_boundary(end) {
            end += 1;
        }
        text.push_str(&format!(
            "[item {} · origin {} · {}{} · bytes omitted: {} before, {} after]\n",
            item.id,
            item.role,
            item.kind,
            item.tool
                .as_ref()
                .map(|tool| format!(" · tool {tool}"))
                .unwrap_or_default(),
            start,
            item.text.len() - end
        ));
        text.push_str(&item.text[start..end]);
        text.push('\n');
    }
    if count > READ_MAX as usize {
        text.push_str(&format!(
            "[{} matching items omitted; narrow the filters or use --item]\n",
            count - READ_MAX as usize
        ));
    }
    Ok(text)
}

#[cfg(test)]
mod tests;
