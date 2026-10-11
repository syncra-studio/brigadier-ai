//! No long silences (SESSION-FIXES-PLAN.md Q9): while a request works and the user has seen
//! nothing new in it for 30 minutes, the thread is told `[status due]` and writes one status
//! line. After that, one at most every 30 minutes, and only when something changed since.

use std::collections::HashMap;
use std::time::Duration;

use super::SessionManager;
use super::conversation::Envelope;
use crate::model::{ConversationId, ConversationKind, MessageRole};
use crate::now_ms;
use crate::work::{InjectionKind, RequestState, Task, TaskState};

/// How long a working request may show nothing new before the thread writes a status line.
pub(super) const SILENCE_MS: i64 = 30 * 60 * 1000;

/// How often the requests are looked at.
const TICK: Duration = Duration::from_secs(60);

/// Per request: when the user last saw something new in it, and when the thread was last told
/// `[status due]`.
#[derive(Default)]
pub(crate) struct StatusClock {
    shown: HashMap<String, i64>,
    /// When, and what its workers were doing then.
    told: HashMap<String, (i64, Vec<String>)>,
}

impl StatusClock {
    /// The user saw a message of `request`'s at `at_ms`: its silence starts again.
    pub(super) fn shown(&mut self, request: &str, at_ms: i64) {
        self.shown.insert(request.to_owned(), at_ms);
    }

    /// Tests: when the user last saw something new in `request`.
    #[cfg(test)]
    pub(crate) fn shown_at(&self, request: &str) -> Option<i64> {
        self.shown.get(request).copied()
    }
}

impl SessionManager {
    /// Looks at the working requests every minute.
    pub(super) fn start_status_timer(&self) {
        let manager = self.me.clone();
        self.spawn(async move {
            let mut tick = tokio::time::interval(TICK);
            tick.tick().await;
            loop {
                tick.tick().await;
                let Some(manager) = manager.upgrade() else {
                    return;
                };
                if manager.admit().is_err() {
                    return;
                }
                manager.status_due(now_ms()).await;
            }
        });
    }

    /// Tells each session's thread `[status due]` for a request that works and has shown
    /// nothing new for [`SILENCE_MS`] at `now`: the first time whatever changed, then only
    /// when its workers changed since the last time.
    pub(crate) async fn status_due(&self, now: i64) {
        let convs: Vec<_> = self.convs_lock().values().cloned().collect();
        for conv in convs {
            if conv.kind != ConversationKind::Session {
                continue;
            }
            let (Ok(board), Ok(tasks)) = (
                self.core.board(&conv.id).await,
                self.core.tasks(&conv.id).await,
            ) else {
                continue;
            };
            for request in board
                .requests
                .values()
                .filter(|request| request.state == RequestState::Working)
            {
                // After a restart the clock is empty: the request's last reply the user saw,
                // stored, is where its silence starts, looked up once it would be due.
                let unknown = {
                    let clock = conv.status.lock().unwrap();
                    !clock.shown.contains_key(&request.id)
                        && now - request.started_at_ms >= SILENCE_MS
                };
                if unknown {
                    let at = self
                        .last_reply_at(&conv.id, &request.id)
                        .await
                        .unwrap_or(request.started_at_ms);
                    conv.status
                        .lock()
                        .unwrap()
                        .shown
                        .entry(request.id.clone())
                        .or_insert(at);
                }
                let text = {
                    let mut clock = conv.status.lock().unwrap();
                    let told = clock.told.get(&request.id);
                    let since = [
                        Some(request.started_at_ms),
                        clock.shown.get(&request.id).copied(),
                        told.map(|(at, _)| *at),
                    ]
                    .into_iter()
                    .flatten()
                    .max()
                    .unwrap_or(now);
                    if now - since < SILENCE_MS {
                        continue;
                    }
                    let workers = workers(&tasks, &request.id);
                    // Since the last status: what is new in its workers; before it, what
                    // started or ended since the user last saw something.
                    let changed: Vec<String> = match told {
                        Some((_, before)) => workers
                            .iter()
                            .filter(|worker| !before.contains(worker))
                            .cloned()
                            .collect(),
                        None => changes(&tasks, &request.id, since),
                    };
                    if told.is_some() && changed.is_empty() {
                        continue;
                    }
                    clock.told.insert(request.id.clone(), (now, workers));
                    let minutes = (now - since) / 60_000;
                    format!(
                        "[status due] {minutes} min with nothing new for the user.{} Write one status line now: what runs, why, and roughly what's left. Then reply as before.",
                        if changed.is_empty() {
                            String::new()
                        } else {
                            format!(" Since then: {}.", changed.join("; "))
                        }
                    )
                };
                self.deliver_for(
                    &conv.id,
                    Envelope {
                        kind: InjectionKind::Reminder,
                        label: "status".into(),
                        task_id: None,
                        text,
                    },
                    Some(request.id.clone()),
                )
                .await;
            }
        }
    }
}

impl SessionManager {
    /// When the thread's last stored reply for `request` was written, among the
    /// conversation's latest messages.
    async fn last_reply_at(&self, id: &ConversationId, request: &str) -> Option<i64> {
        let page = self.core.list_messages(id.clone(), None, 200).await.ok()?;
        page.messages
            .iter()
            .rev()
            .find(|message| {
                message.role == MessageRole::Assistant
                    && message.request_id.as_deref() == Some(request)
            })
            .map(|message| message.created_at_ms)
    }
}

/// What changed in `request`'s workers after `since`: started, reported or ended.
fn changes(tasks: &[Task], request: &str, since: i64) -> Vec<String> {
    let mut changed = Vec::new();
    for task in of_request(tasks, request) {
        if task.created_at_ms > since {
            changed.push(format!("{} started", name(task)));
        }
        if task.updated_at_ms > since
            && let Some(end) = end(task.state)
        {
            changed.push(format!("{} {end}", name(task)));
        }
    }
    changed
}

/// `request`'s workers as they are: each "started", and how it reported or ended if it did.
fn workers(tasks: &[Task], request: &str) -> Vec<String> {
    of_request(tasks, request)
        .flat_map(|task| {
            let started = format!("{} started", name(task));
            // A task that reported and then finished without landing is no news.
            let ended = match task.state {
                TaskState::Done => Some("reported"),
                state => end(state),
            }
            .map(|end| format!("{} {end}", name(task)));
            std::iter::once(started).chain(ended)
        })
        .collect()
}

fn of_request<'a>(tasks: &'a [Task], request: &'a str) -> impl Iterator<Item = &'a Task> {
    tasks
        .iter()
        .filter(move |task| task.request_id.as_deref() == Some(request))
}

fn name(task: &Task) -> String {
    format!("task-{} \"{}\"", task.number, task.title)
}

fn end(state: TaskState) -> Option<&'static str> {
    match state {
        TaskState::Landed => Some("landed"),
        TaskState::Failed => Some("failed"),
        TaskState::Stopped => Some("was stopped"),
        TaskState::Done => Some("finished"),
        TaskState::Reported => Some("reported"),
        _ => None,
    }
}
