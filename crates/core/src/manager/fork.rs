//! Forking a conversation from one of its answers ("Fork chat from here").
//!
//! The fork is a new conversation holding the branch shown up to that answer: its messages
//! and everything their requests set in motion (workers with their transcripts, cards, worker
//! steps, ratings), so its blocks, diff cards and worker threads read as in the source. It
//! shares nothing that can still change: its workers get new ids and copies of their
//! transcripts, they keep no branch or worktree of the source, and its model starts a CLI
//! session of its own, seeded with the transcript.
//!
//! A session's fork works from the commit that was current at that answer: the last commit
//! its copied requests landed, or where the session stood before its first landing when they
//! landed none. It gets a new branch there, in the user's checkout ("in this workspace") or
//! in a worktree of its own.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use brigadier_git::Oid;
use brigadier_store::NewEvent;

use super::SessionManager;
use super::{blocking, git_error};
use crate::board::Board;
use crate::model::{
    Conversation, ConversationId, DomainEvent, Environment, ForkOrigin, ForkPlace, Lifecycle,
    MessageRole, Setup, streams,
};
use crate::sessions::{Origin, ROOT, branch_of, decode};
use crate::work::{RequestState, TaskId};
use crate::{Error, Result};

/// Events of a stream read at once while copying it.
const COPY_PAGE: u32 = 500;

impl SessionManager {
    /// Starts a new conversation from the answer `message_id` of `source`: the thread up to
    /// it, and for a session, a branch at the commit current then, in `place`.
    pub async fn fork(
        &self,
        source: ConversationId,
        message_id: String,
        place: ForkPlace,
    ) -> Result<Conversation> {
        self.admit()?;
        let conversation = self.core.conversation(&source)?;
        if conversation.lifecycle == Lifecycle::Archived {
            return Err(Error::Invalid(
                "this conversation is archived; restore it first".into(),
            ));
        }
        let messages = self.core.all_messages(&source).await?;
        let point = messages
            .iter()
            .find(|message| message.id == message_id)
            .ok_or_else(|| Error::NotFound(format!("message {message_id}")))?;
        if point.role != MessageRole::Assistant {
            return Err(Error::Invalid("a chat is forked from an answer".into()));
        }
        // A fork takes everything its requests did, so it starts at a request's last answer
        // (as the thread's "Fork chat from here" does), never at one of its interim replies.
        let later = messages.iter().any(|message| {
            message.role == MessageRole::Assistant
                && message.id != message_id
                && message.request_id.is_some()
                && message.request_id == point.request_id
                && branch_of(&messages, &message.id)
                    .iter()
                    .any(|before| before.id == message_id)
        });
        if later {
            return Err(Error::Invalid(
                "fork from the request's last answer, not from an earlier reply".into(),
            ));
        }
        let path = branch_of(&messages, &message_id);
        let requests: HashSet<String> = path
            .iter()
            .filter_map(|message| message.request_id.clone())
            .collect();
        let board = self.core.board(&source).await?;
        if requests.iter().any(|id| {
            board.requests.get(id).is_some_and(|request| {
                matches!(request.state, RequestState::Working | RequestState::Waiting)
            })
        }) {
            return Err(Error::Invalid(
                "wait until this answer is finished before forking it".into(),
            ));
        }
        let copied: Vec<TaskId> = board
            .tasks
            .values()
            .filter(|task| {
                task.request_id
                    .as_ref()
                    .is_some_and(|request| requests.contains(request))
            })
            .map(|task| task.id.clone())
            .collect();
        if copied.iter().any(|id| !board.tasks[id].state.is_final()) {
            return Err(Error::Invalid(
                "wait for its workers to finish before forking it".into(),
            ));
        }

        // A fork is a new conversation: its model must be available, as when starting one.
        if let Some(setup) = &conversation.setup {
            self.check_choice(setup.choice())?;
        }

        let id = ConversationId::generate();
        let setup = match conversation.setup.clone() {
            Some(Setup::Session {
                repo,
                environment,
                permission,
                orchestrator,
                workers_see_uncommitted,
                plan_mode,
            }) => {
                let start = self
                    .fork_commit(&repo, &environment, &board, &copied)
                    .await?;
                let prefix = self.branch_prefix(&conversation.id).await;
                let environment = self
                    .fork_environment(&repo, &environment, place, &start, &id, &prefix)
                    .await?;
                Some(Setup::Session {
                    repo,
                    environment,
                    permission,
                    orchestrator,
                    workers_see_uncommitted,
                    plan_mode,
                })
            }
            other => other,
        };
        let title = self.fork_title(&conversation.title);
        let fork = self
            .core
            .create_conversation(
                id.clone(),
                conversation.kind,
                conversation.project_id.clone(),
                Some(title),
                setup,
                Origin {
                    forked_from: Some(ForkOrigin {
                        conversation_id: source.clone(),
                        message_id: message_id.clone(),
                    }),
                    side_of: None,
                    tab_chat: None,
                },
            )
            .await?;

        // Every id that names the source's things is swapped for the fork's own.
        let mut ids: Vec<(String, String)> = vec![(source.0.clone(), id.0.clone())];
        ids.extend(
            copied
                .iter()
                .map(|task| (task.0.clone(), TaskId::generate().0)),
        );
        let swap = |text: String| {
            ids.iter()
                .fold(text, |text, (from, to)| text.replace(from, to))
        };

        for (from, to) in ids.iter().skip(1) {
            self.copy_stream(
                streams::task(&TaskId(from.clone())),
                streams::task(&TaskId(to.clone())),
                &swap,
            )
            .await?;
        }
        let parents: HashMap<&str, &str> = path
            .iter()
            .enumerate()
            .map(|(at, message)| {
                let parent = at.checked_sub(1).map_or(ROOT, |before| &path[before].id);
                (message.id.as_str(), parent)
            })
            .collect();
        let tasks: HashSet<String> = copied.iter().map(|task| format!("task:{task}")).collect();
        let events = self
            .fork_events(&source, &parents, &requests, &tasks)
            .await?;
        let events = events
            .into_iter()
            .map(|event| {
                let text =
                    serde_json::to_string(&event).map_err(|err| Error::Invalid(err.to_string()))?;
                serde_json::from_str(&swap(text)).map_err(|err| Error::Invalid(err.to_string()))
            })
            .collect::<Result<Vec<DomainEvent>>>()?;
        self.core.record_conversation(&id, events).await?;
        Ok(fork)
    }

    /// The source's conversation events the fork keeps, oldest first: the branch's messages
    /// (each under the one before it), and what their requests did.
    async fn fork_events(
        &self,
        source: &ConversationId,
        parents: &HashMap<&str, &str>,
        requests: &HashSet<String>,
        tasks: &HashSet<String>,
    ) -> Result<Vec<DomainEvent>> {
        let of =
            |request: &Option<String>| request.as_ref().is_some_and(|id| requests.contains(id));
        let carried = |task: &TaskId| tasks.contains(&format!("task:{task}"));
        let mut kept = Vec::new();
        let mut after = 0;
        loop {
            let page = self
                .core
                .store()
                .read_stream_since(streams::conversation(source), after, COPY_PAGE)
                .await?;
            let Some(last) = page.last() else {
                break;
            };
            after = last.seq;
            for stored in &page {
                let event = match decode(stored)? {
                    DomainEvent::MessageAppended { mut message } => {
                        let Some(parent) = parents.get(message.id.as_str()) else {
                            continue;
                        };
                        message.parent_id = Some((*parent).to_owned());
                        DomainEvent::MessageAppended { message }
                    }
                    DomainEvent::RequestUpdated { request } if requests.contains(&request.id) => {
                        DomainEvent::RequestUpdated { request }
                    }
                    // Only the tasks the fork carries: one a later request took over (it
                    // re-accepted it) was filed under a copied request earlier on.
                    DomainEvent::TaskUpdated { mut task } if carried(&task.id) => {
                        // The source's branches and worktrees stay the source's.
                        task.workspace = None;
                        task.kept = None;
                        DomainEvent::TaskUpdated { task }
                    }
                    DomainEvent::ApprovalUpdated { approval } if of(&approval.request_id) => {
                        DomainEvent::ApprovalUpdated { approval }
                    }
                    DomainEvent::QuestionUpdated { question } if of(&question.request_id) => {
                        DomainEvent::QuestionUpdated { question }
                    }
                    DomainEvent::WorkerStepped { step }
                        if of(&step.request_id) && carried(&step.task_id) =>
                    {
                        DomainEvent::WorkerStepped { step }
                    }
                    DomainEvent::OrchestratorStepped { step } if of(&step.request_id) => {
                        DomainEvent::OrchestratorStepped { step }
                    }
                    // What was decided for the copied requests; what waited for the user
                    // stays the source's, whose work it holds up.
                    DomainEvent::DecidedForYou { decision } if of(&decision.request_id) => {
                        DomainEvent::DecidedForYou { decision }
                    }
                    DomainEvent::MessageRated { subject, rating }
                        if parents.contains_key(subject.as_str()) || tasks.contains(&subject) =>
                    {
                        DomainEvent::MessageRated { subject, rating }
                    }
                    DomainEvent::RatingCleared { subject }
                        if parents.contains_key(subject.as_str()) || tasks.contains(&subject) =>
                    {
                        DomainEvent::RatingCleared { subject }
                    }
                    _ => continue,
                };
                kept.push(event);
            }
            if page.len() < COPY_PAGE as usize {
                break;
            }
        }
        Ok(kept)
    }

    /// Copies every event of `from` into `to`, with the fork's ids.
    async fn copy_stream(
        &self,
        from: String,
        to: String,
        swap: &impl Fn(String) -> String,
    ) -> Result<()> {
        let store = self.core.store();
        let mut after = 0;
        loop {
            let page = store
                .read_stream_since(from.clone(), after, COPY_PAGE)
                .await?;
            let Some(last) = page.last() else {
                return Ok(());
            };
            after = last.seq;
            let events = page
                .iter()
                .map(|stored| {
                    let payload = serde_json::value::RawValue::from_string(swap(
                        stored.payload.get().to_owned(),
                    ))
                    .map_err(|err| Error::Invalid(err.to_string()))?;
                    Ok(NewEvent {
                        stream: to.clone(),
                        kind: stored.kind.clone(),
                        at_ms: stored.at_ms,
                        payload,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            store.append(events).await?;
            if page.len() < COPY_PAGE as usize {
                return Ok(());
            }
        }
    }

    /// The commit a session stood at when the fork's last request was answered: the newest
    /// commit its copied requests landed; with none, where the session was before its first
    /// landing; with no landing at all, where it is now.
    async fn fork_commit(
        &self,
        repo: &str,
        environment: &Environment,
        board: &Board,
        copied: &[TaskId],
    ) -> Result<String> {
        let landed = |tasks: &mut dyn Iterator<Item = &crate::work::Task>| -> Vec<Oid> {
            tasks
                .filter_map(|task| task.landed.clone().map(Oid))
                .collect()
        };
        let in_range = landed(&mut copied.iter().map(|id| &board.tasks[id]));
        let all = landed(&mut board.tasks.values());
        let (git, repo, environment) = (self.git.clone(), repo.to_owned(), environment.clone());
        blocking(move || {
            let repo = git.open(Path::new(&repo)).map_err(git_error)?;
            // Landings go one after another onto one branch: the newest descends from the rest.
            let newest = |commits: &[Oid], newer: bool| -> Result<Option<Oid>> {
                let mut best: Option<Oid> = None;
                for commit in commits {
                    best = Some(match best {
                        None => commit.clone(),
                        Some(best) => {
                            let base = repo.merge_base(&best, commit).map_err(git_error)?;
                            if (base == best) == newer {
                                commit.clone()
                            } else {
                                best
                            }
                        }
                    });
                }
                Ok(best)
            };
            if let Some(commit) = newest(&in_range, true)? {
                return Ok(commit.0);
            }
            if let Some(first) = newest(&all, false)? {
                return Ok(repo
                    .resolve(&format!("{}^1", first.0))
                    .map_err(git_error)?
                    .0);
            }
            let now = match &environment {
                Environment::LocalCheckout { branch } => repo.branch_commit(branch),
                Environment::NewWorktree {
                    base,
                    branch,
                    start,
                    ..
                } => match (repo.branch_tip(branch).map_err(git_error)?, start) {
                    (Some(tip), _) => Ok(tip),
                    (None, Some(start)) => repo.resolve(start),
                    (None, None) => repo.branch_commit(base),
                },
            };
            Ok(now.map_err(git_error)?.0)
        })
        .await
    }

    /// Where the fork works. In the user's checkout it gets a new branch at `start`, unless
    /// the source works on that checkout's branch and nothing landed there since; in a
    /// worktree its session branch starts at `start` and merges back into the source's target.
    async fn fork_environment(
        &self,
        repo: &str,
        environment: &Environment,
        place: ForkPlace,
        start: &str,
        id: &ConversationId,
        prefix: &str,
    ) -> Result<Environment> {
        let target = match environment {
            Environment::LocalCheckout { branch } => branch.clone(),
            Environment::NewWorktree { base, .. } => base.clone(),
        };
        match place {
            ForkPlace::NewWorktree => Ok(Environment::NewWorktree {
                base: target,
                branch: format!("{prefix}{}/session", id.short()),
                path: None,
                start: Some(start.to_owned()),
            }),
            ForkPlace::Workspace => {
                let (git, repo, start, same, name) = (
                    self.git.clone(),
                    repo.to_owned(),
                    Oid(start.to_owned()),
                    match environment {
                        Environment::LocalCheckout { branch } => Some(branch.clone()),
                        Environment::NewWorktree { .. } => None,
                    },
                    format!("{prefix}{}/fork", id.short()),
                );
                let at = repo.clone();
                let (environment, made) = blocking(move || {
                    let repo = git.open(Path::new(&repo)).map_err(git_error)?;
                    if let Some(branch) = same
                        && repo.branch_tip(&branch).map_err(git_error)? == Some(start.clone())
                    {
                        return Ok((Environment::LocalCheckout { branch }, None));
                    }
                    repo.create_branch(&name, &start).map_err(git_error)?;
                    Ok((
                        Environment::LocalCheckout {
                            branch: name.clone(),
                        },
                        Some(name),
                    ))
                })
                .await?;
                if let Some(name) = made {
                    self.note_branch_made(Path::new(&at), &name).await;
                }
                Ok(environment)
            }
        }
    }

    /// "Title (2)", or the next number free among the source's forks.
    fn fork_title(&self, title: &str) -> String {
        let base = match title.rsplit_once(" (") {
            Some((base, number))
                if number
                    .strip_suffix(')')
                    .is_some_and(|n| n.parse::<u32>().is_ok()) =>
            {
                base
            }
            _ => title,
        };
        let taken = self
            .core
            .catalog()
            .conversations
            .iter()
            .filter_map(|conversation| {
                let rest = conversation.title.strip_prefix(base)?;
                if rest.is_empty() {
                    return Some(1);
                }
                rest.strip_prefix(" (")?
                    .strip_suffix(')')?
                    .parse::<u32>()
                    .ok()
            })
            .max()
            .unwrap_or(1);
        format!("{base} ({})", taken.max(1) + 1)
    }
}
