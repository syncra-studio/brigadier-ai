//! `brigadier why <file>:<line>` (STRIP-DOWN-PLAN S39, D51): which session turn wrote a line,
//! and what was asked and answered in it. Takes, doesn't rebuild: the turn snapshot refs and
//! the request → turn records S11 keeps, the CLI's own session files (G11,
//! `record/lineage.json`) and the ledger. Nothing is recorded for it, nothing is pushed into a
//! thread, and nothing leaves the machine.

use std::path::{Path, PathBuf};

use brigadier_cli::protocol::{
    CliReply, WhyBy, WhyCommit, WhyLedgerEntry, WhyMove, WhyOutput, WhySession, WhyTurn, WhyWorker,
};
use brigadier_git::Oid;
use brigadier_git::why::{BlameCommit, Origin, TurnOrigin};
use brigadier_providers::session_file::FileEntry;
use brigadier_providers::{ProviderEvent, Role as CliRole};

use super::cli::{agent, short};
use super::cli_ledger::{Entry, Ledger, stamped_at};
use super::records::{READ_MAX, transcript, transcript_stamp};
use super::{SessionManager, blocking, git_error};
use crate::model::{Conversation, ConversationId, Setup};
use crate::work::Task;
use crate::{Error, Result};

/// The most of a reply `why` prints; the read command it names gives the rest.
const REPLY_MAX: usize = 1200;

/// How long before its snapshot a turn's user message may be stamped (the CLI's clock and
/// Brigadier's): only for a turn its CLI's files no longer tie to its request.
const MESSAGE_SLACK_MS: i64 = 5_000;

/// A turn of a session that worked in the checkout, as S11's records keep it.
struct Known {
    conversation: ConversationId,
    number: u64,
    request: String,
    /// When its start was captured.
    at_ms: i64,
}

/// A worker of one of those sessions, with what its branch says.
struct Worker {
    conversation: ConversationId,
    task: Task,
}

/// What a turn's (or a worker's) CLI files say: the message, the reply, and where they are.
#[derive(Default)]
struct Spoken {
    message: Option<String>,
    reply: Option<String>,
    cut: bool,
    /// The transcript positions from the message to the reply (`brigadier brain read`'s).
    span: Option<(usize, usize)>,
    /// The ledger ids its `brigadier ledger add` and `close` commands printed.
    ledger: Vec<u32>,
    gone: Option<String>,
}

impl SessionManager {
    /// `brigadier why`: the turn whose own diff wrote line `line` of the file at `path` as it
    /// is now, among the turns of the sessions that worked in that checkout.
    pub(super) async fn cli_why(&self, path: &str, line: u32) -> Result<CliReply> {
        let asked = PathBuf::from(path);
        let git = self.git.clone();
        let (root, relative) = blocking(move || {
            let file = std::fs::canonicalize(&asked)
                .map_err(|err| Error::Invalid(format!("{}: {err}", asked.display())))?;
            let root = git.checkout_of(&file).map_err(git_error)?.ok_or_else(|| {
                Error::Invalid(format!("{} is in no git checkout", asked.display()))
            })?;
            let relative = file
                .strip_prefix(&root)
                .map_err(|_| {
                    Error::Invalid(format!("{} is outside its checkout", asked.display()))
                })?
                .components()
                .map(|part| part.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            Ok((root, relative))
        })
        .await?;
        // The sessions that worked in this checkout: a seam's successor is the same session.
        let sessions: Vec<(Conversation, Option<String>)> = self
            .core
            .catalog()
            .conversations
            .into_iter()
            .filter_map(|conversation| {
                let (checkout, branch) = self.changes_checkout(&conversation.id)?;
                (std::fs::canonicalize(checkout).ok()? == root).then_some((conversation, branch))
            })
            .collect();
        let mut known = Vec::new();
        let mut workers = Vec::new();
        for (session, branch) in &sessions {
            let prefix = format!("snapshot:{}:", session.id);
            for head in self.core.store().stream_heads(prefix.clone()).await? {
                let Some(request) = head.stream.strip_prefix(&prefix) else {
                    continue;
                };
                if let Some(turn) = self.snapshot_turn_record(&session.id, request).await? {
                    known.push(Known {
                        conversation: session.id.clone(),
                        number: turn.number,
                        request: request.to_owned(),
                        at_ms: head.at_ms,
                    });
                }
            }
            // A worker on the session's own branch (`--local`) has no branch of its own to
            // tell its commits by.
            workers.extend(
                self.core
                    .tasks(&session.id)
                    .await?
                    .into_iter()
                    .filter(|task| {
                        let own = task.workspace.as_ref().and_then(|w| w.branch.as_ref());
                        own.is_some() && own != branch.as_ref()
                    })
                    .map(|task| Worker {
                        conversation: session.id.clone(),
                        task,
                    }),
            );
        }
        known.sort_by(|a, b| b.at_ms.cmp(&a.at_ms).then(b.number.cmp(&a.number)));
        let turns: Vec<brigadier_git::why::WhyTurn> = known
            .iter()
            .map(|turn| brigadier_git::why::WhyTurn {
                session: turn.conversation.0.clone(),
                turn: turn.number,
            })
            .collect();
        let branches: Vec<(String, Option<String>)> = workers
            .iter()
            .filter_map(|worker| {
                let workspace = worker.task.workspace.as_ref()?;
                Some((workspace.branch.clone()?, workspace.base.clone()))
            })
            .collect();
        let git = self.git.clone();
        let file = relative.clone();
        let (why, landed) = blocking(move || {
            let repo = git.open(&root).map_err(git_error)?;
            let why = repo
                .why_line(&file, line as usize, &turns)
                .map_err(git_error)?;
            let commit = match &why.origin {
                Origin::Turn(origin) => origin
                    .commit
                    .as_ref()
                    .map(|commit| (&commit.commit.id, commit.branch.as_deref())),
                Origin::Outside(commit) => commit.as_ref().map(|commit| (&commit.id, None)),
            };
            let landed = match commit {
                Some((id, branch)) => worker_branch(&repo, id, branch, &branches)?,
                None => None,
            };
            Ok((why, landed))
        })
        .await?;
        let worker = landed.and_then(|branch| {
            workers.iter().find(|worker| {
                worker
                    .task
                    .workspace
                    .as_ref()
                    .and_then(|w| w.branch.as_ref())
                    == Some(&branch)
            })
        });
        let mut output = WhyOutput {
            path: relative,
            line,
            text: why.text,
            by: WhyBy::Uncommitted,
            session: None,
            turn: None,
            worker: worker.map(|worker| self.why_worker(&worker.conversation, &worker.task)),
            commit: None,
            moved_by: Vec::new(),
        };
        let session_of = |id: &ConversationId| {
            sessions
                .iter()
                .find(|(session, _)| session.id == *id)
                .map(|(session, _)| why_session(session))
        };
        match why.origin {
            Origin::Turn(origin) => {
                let known = known
                    .iter()
                    .find(|turn| {
                        turn.conversation.0 == origin.turn.session
                            && turn.number == origin.turn.turn
                    })
                    .ok_or_else(|| Error::Invalid("the turn the walk found is unknown".into()))?;
                output.by = if worker.is_some() {
                    WhyBy::Worker
                } else {
                    WhyBy::Turn
                };
                output.session = session_of(&known.conversation);
                output.commit = origin.commit.as_ref().map(|commit| WhyCommit {
                    merge: commit.merge.as_ref().map(|merge| merge.0.clone()),
                    branch: commit.branch.clone(),
                    ..why_commit(&commit.commit)
                });
                output.moved_by = origin
                    .moved_by
                    .iter()
                    .map(|turn| WhyMove {
                        session: short(&turn.session),
                        turn: u32::try_from(turn.turn).unwrap_or(u32::MAX),
                    })
                    .collect();
                output.turn = Some(self.why_turn(known, &origin, worker.is_none()));
            }
            Origin::Outside(commit) => {
                if let Some(worker) = worker {
                    output.by = WhyBy::Worker;
                    output.session = session_of(&worker.conversation);
                } else if commit.is_some() {
                    output.by = WhyBy::Commit;
                }
                output.commit = commit.as_ref().map(why_commit);
            }
        }
        Ok(CliReply::ok(&output))
    }

    /// The turn as its session's CLI files and ledger keep it. A turn that only landed a
    /// worker's work (`spoken` false) names its number and time alone.
    fn why_turn(&self, known: &Known, origin: &TurnOrigin, spoken: bool) -> WhyTurn {
        let id = &known.conversation;
        let thread = self.thread_view(id).ok();
        let turn = thread.as_ref().and_then(|thread| {
            thread
                .turns
                .iter()
                .find(|turn| turn.requests.contains(&known.request))
        });
        let started_at_ms = turn
            .and_then(|turn| turn.started_at_ms)
            .or(Some(known.at_ms));
        let ended_at_ms = turn.and_then(|turn| turn.ended_at_ms);
        let record = self.session_dir(id).record();
        let said = if spoken {
            said(
                &record,
                Some(Which::Turn {
                    user_message: turn.map(|turn| turn.user_message.as_str()),
                    after_ms: known.at_ms,
                }),
            )
        } else {
            Spoken::default()
        };
        let ledger = match (spoken, &said.gone) {
            (false, _) => Vec::new(),
            (true, None) => ledger_entries(&record, |entry| {
                entry.id.is_some_and(|id| said.ledger.contains(&id))
            }),
            // Without the CLI's file, by the entries' stamps.
            (true, Some(_)) => ledger_between(&record, started_at_ms, ended_at_ms),
        };
        WhyTurn {
            number: u32::try_from(known.number).unwrap_or(u32::MAX),
            started_at_ms,
            ended_at_ms,
            path: origin.path.clone(),
            line: u32::try_from(origin.line).unwrap_or(u32::MAX),
            message: said.message,
            reply: said.reply,
            reply_cut: said.cut,
            read: said
                .span
                .map(|(from, to)| format!("brigadier brain read {} {from} {to}", id.short())),
            ledger,
            gone: said.gone,
        }
    }

    /// A worker as its own CLI files keep it: the task it was given and its last reply.
    fn why_worker(&self, conversation: &ConversationId, task: &Task) -> WhyWorker {
        let dir = self.session_dir(conversation).worker(task);
        let said = said(&dir.join("record"), None);
        let key = format!(
            "{}/{}",
            conversation.short(),
            dir.file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
        );
        WhyWorker {
            id: short(&task.id.0),
            number: task.number,
            title: task.title.clone(),
            agent: agent(task.route.choice.provider),
            branch: task.workspace.as_ref().and_then(|w| w.branch.clone()),
            task: said.message,
            reply: said.reply,
            reply_cut: said.cut,
            read: said
                .span
                .map(|(from, to)| format!("brigadier brain read {key} {from} {to}")),
            gone: said.gone,
        }
    }
}

fn why_session(session: &Conversation) -> WhySession {
    let provider = match &session.setup {
        Some(Setup::Session { orchestrator, .. }) => orchestrator.provider,
        Some(Setup::Chat { model, .. }) => model.provider,
        None => brigadier_providers::ProviderKind::Claude,
    };
    WhySession {
        id: short(&session.id.0),
        title: session.title.clone(),
        agent: agent(provider),
    }
}

fn why_commit(commit: &BlameCommit) -> WhyCommit {
    WhyCommit {
        id: commit.id.0.clone(),
        author: commit.author.clone(),
        at_ms: commit.at_ms,
        summary: commit.summary.clone(),
        merge: None,
        branch: None,
    }
}

/// The worker branch (of `workers`: each branch and the commit it started from) that commit
/// `id` was made on: the branch a merge brought it from, else a branch that still holds it
/// and didn't start after it.
fn worker_branch(
    repo: &brigadier_git::Repo,
    id: &Oid,
    merged_from: Option<&str>,
    workers: &[(String, Option<String>)],
) -> Result<Option<String>> {
    if let Some(branch) = merged_from
        && workers.iter().any(|(own, _)| own == branch)
    {
        return Ok(Some(branch.to_owned()));
    }
    for (branch, base) in workers {
        let Some(tip) = repo.branch_tip(branch).map_err(git_error)? else {
            continue;
        };
        if !repo.ancestor(id, &tip).map_err(git_error)? {
            continue;
        }
        if let Some(base) = base
            && repo.ancestor(id, &Oid(base.clone())).unwrap_or(false)
        {
            continue;
        }
        return Ok(Some(branch.clone()));
    }
    Ok(None)
}

/// Which items of a transcript [`said`] reads.
#[derive(Clone, Copy)]
enum Which<'a> {
    /// The turn its user message (by item id) starts; else the first user message stamped
    /// at or after `after_ms`.
    Turn {
        user_message: Option<&'a str>,
        after_ms: i64,
    },
}

/// What a session's (or a worker's, `which` none: its whole transcript) CLI files say: the
/// user's message and the last reply after it, before the next message.
fn said(record: &Path, which: Option<Which<'_>>) -> Spoken {
    let files = transcript_stamp(record)
        .map(|(_, files)| files)
        .unwrap_or_default();
    if !files.iter().any(|(_, path)| path.exists()) {
        let gone = match files.last() {
            Some((_, path)) => format!(
                "Its CLI session file is gone ({}), so what was said can't be read.",
                path.display()
            ),
            None => "It has no CLI session file, so what was said can't be read.".to_owned(),
        };
        return Spoken {
            gone: Some(gone),
            ..Spoken::default()
        };
    }
    // The positions `brigadier brain read` numbers: every file of the lineage, in order.
    let entries = transcript(&files);
    let own = |entry: &FileEntry| entry.parent.is_none();
    let from_user = |entry: &FileEntry| {
        own(entry)
            && !entry.steer
            && matches!(
                &entry.event,
                ProviderEvent::Message {
                    role: CliRole::User,
                    ..
                }
            )
    };
    let start = match which {
        None => entries.iter().position(from_user),
        Some(Which::Turn {
            user_message,
            after_ms,
        }) => user_message
            .and_then(|id| {
                entries.iter().position(|entry| {
                    matches!(&entry.event, ProviderEvent::Message { item_id, role: CliRole::User, .. } if item_id == id)
                })
            })
            .or_else(|| {
                entries.iter().position(|entry| {
                    from_user(entry)
                        && entry
                            .at_ms
                            .is_some_and(|at| at >= after_ms - MESSAGE_SLACK_MS)
                })
            }),
    };
    let Some(start) = start else {
        return Spoken {
            gone: Some("Its message is no longer in its CLI session file.".to_owned()),
            ..Spoken::default()
        };
    };
    let end = match which {
        None => entries.len(),
        Some(_) => entries[start + 1..]
            .iter()
            .position(from_user)
            .map_or(entries.len(), |at| start + 1 + at),
    };
    let text = |at: usize| match &entries[at].event {
        ProviderEvent::Message { text, .. } => Some(text.clone()),
        _ => None,
    };
    let reply_at = (start + 1..end).rev().find(|at| {
        own(&entries[*at])
            && matches!(
                entries[*at].event,
                ProviderEvent::Message {
                    role: CliRole::Assistant,
                    ..
                }
            )
    });
    let (reply, cut) = match reply_at.and_then(text) {
        Some(reply) => {
            let (reply, cut) = trimmed(&reply);
            (Some(reply), cut)
        }
        None => (None, false),
    };
    let ledger = entries[start..end]
        .iter()
        .filter_map(|entry| match &entry.event {
            ProviderEvent::Command {
                command,
                output: Some(output),
                ..
            } if command.contains("ledger add") || command.contains("ledger close") => {
                Some(ledger_ids(output))
            }
            _ => None,
        })
        .flatten()
        .collect();
    let last = reply_at.unwrap_or(start);
    let span = if (last - start) as i64 >= READ_MAX {
        (last, last)
    } else {
        (start, last)
    };
    Spoken {
        message: text(start),
        reply,
        cut,
        span: Some(span),
        ledger,
        gone: None,
    }
}

/// The ledger ids (`L<n>`) a command printed.
fn ledger_ids(output: &str) -> Vec<u32> {
    output
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter_map(|word| word.strip_prefix('L')?.parse().ok())
        .collect()
}

/// `text`'s start, at most [`REPLY_MAX`] bytes, ending at a line's end where one is near.
fn trimmed(text: &str) -> (String, bool) {
    let text = text.trim();
    if text.len() <= REPLY_MAX {
        return (text.to_owned(), false);
    }
    let mut end = REPLY_MAX;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let head = &text[..end];
    let head = match head.rfind('\n') {
        Some(at) if at > REPLY_MAX / 2 => &head[..at],
        _ => head,
    };
    (format!("{}…", head.trim_end()), true)
}

/// The ledger entries (its archive's too) stamped from the turn's start to its end, by the
/// minute and in local time as `ledger add` stamps them; to now while it runs. Only when the
/// CLI's file is gone: a turn that began in the minute the one before it ended shares it.
fn ledger_between(record: &Path, from: Option<i64>, to: Option<i64>) -> Vec<WhyLedgerEntry> {
    let Some(from) = from.and_then(minute) else {
        return Vec::new();
    };
    let Some(to) = minute(to.unwrap_or_else(crate::now_ms)) else {
        return Vec::new();
    };
    ledger_entries(record, |entry| {
        stamped_at(entry).is_some_and(|at| from.as_str() <= at && at <= to.as_str())
    })
}

/// The session's ledger entries (its archive's too) that `keep` keeps, verbatim.
fn ledger_entries(record: &Path, keep: impl Fn(&Entry) -> bool) -> Vec<WhyLedgerEntry> {
    let Ok(ledger) = Ledger::read(record) else {
        return Vec::new();
    };
    ledger
        .entries
        .iter()
        .filter(|entry| keep(entry))
        .map(|entry| WhyLedgerEntry {
            id: entry.id.map(|id| format!("L{id}")),
            text: entry.text.clone(),
        })
        .collect()
}

/// An instant as a ledger stamp's `MM-DD HH:MM`, local time.
fn minute(at_ms: i64) -> Option<String> {
    let at = jiff::Timestamp::from_millisecond(at_ms).ok()?;
    Some(
        at.to_zoned(jiff::tz::TimeZone::system())
            .strftime("%m-%d %H:%M")
            .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ledger_command_prints_its_ids() {
        assert_eq!(ledger_ids("L12\n"), [12]);
        assert_eq!(
            ledger_ids("{\"action\": \"add\", \"text\": \"L3\\n\"}"),
            [3]
        );
        assert!(ledger_ids("Log saved").is_empty());
    }

    #[test]
    fn a_long_reply_is_cut_at_a_line_end() {
        let (short_reply, cut) = trimmed("  Done.\n");
        assert_eq!((short_reply.as_str(), cut), ("Done.", false));
        let long = format!("{}\n{}", "a".repeat(REPLY_MAX - 100), "b".repeat(400));
        let (head, cut) = trimmed(&long);
        assert!(cut);
        assert_eq!(head, format!("{}…", "a".repeat(REPLY_MAX - 100)));
        let (head, _) = trimmed(&"é".repeat(REPLY_MAX));
        assert!(head.ends_with('…') && head.len() <= REPLY_MAX + '…'.len_utf8());
    }
}
