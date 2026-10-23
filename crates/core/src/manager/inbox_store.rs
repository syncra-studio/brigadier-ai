//! What waits in a thread's inbox for its next turn lives in memory, so each envelope is also
//! kept in the store as it is queued: one a stop left there before a turn took it goes again
//! at the next start, once (#210). Another session's messages have their own recovery (peers.rs); reminders are asked again from what is due.

use std::collections::HashMap;

use brigadier_store::{NewEvent, StreamPage};
use serde::{Deserialize, Serialize};

use super::SessionManager;
use super::conversation::Envelope;
use crate::model::{ConversationId, TaskId, streams};
use crate::work::{InjectionKind, RequestState};

const QUEUED: &str = "inbox.queued";
const TAKEN: &str = "inbox.taken";
const CARRIED: &str = "inbox.carried";

#[derive(Serialize, Deserialize)]
struct Queued {
    envelope: Envelope,
    request: Option<String>,
    /// The text this envelope had before more was folded into it: that one is this.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    replaces: Option<String>,
}

/// What a turn took from the inbox.
#[derive(Serialize, Deserialize)]
struct Taken {
    taken: Vec<(Envelope, Option<String>)>,
}

/// What a turn that could not start put back in front of the inbox.
#[derive(Serialize, Deserialize)]
struct Carried {
    carried: Vec<(Envelope, Option<String>)>,
}

/// Whether an envelope is kept: another session's message has its own record, a worker's
/// question is asked again by the worker's recovery, and one for no request has nothing to go
/// on for.
fn kept(envelope: &Envelope, request: Option<&str>) -> bool {
    !matches!(
        envelope.kind,
        InjectionKind::PeerMessage | InjectionKind::WorkerQuestion
    ) && request.is_some()
}

/// The envelopes of `envelopes` that are kept.
fn kept_of(envelopes: &[(Envelope, Option<String>)]) -> Vec<(Envelope, Option<String>)> {
    envelopes
        .iter()
        .filter(|(envelope, request)| kept(envelope, request.as_deref()))
        .cloned()
        .collect()
}

/// An envelope as kept: its kind, label, task, request and text.
type Key = (
    InjectionKind,
    String,
    Option<TaskId>,
    Option<String>,
    String,
);

fn key(envelope: &Envelope, request: &Option<String>, text: &str) -> Key {
    (
        envelope.kind,
        envelope.label.clone(),
        envelope.task_id.clone(),
        request.clone(),
        text.to_owned(),
    )
}

/// The inbox as its stream says it stood, replayed oldest first: each envelope queued joins
/// it, where it was queued (a fold takes the place of what it replaces), and each one a turn
/// took leaves it (one that could not start puts it back in front). What is left is owed.
#[derive(Default)]
struct Kept {
    /// When each was queued, as kept, and what.
    inbox: Vec<(i64, Key, Queued)>,
}

impl Kept {
    fn taken(&mut self, taken: &Taken) {
        for (envelope, request) in &taken.taken {
            let at = key(envelope, request, &envelope.text);
            if let Some(index) = self.inbox.iter().position(|(_, kept, _)| *kept == at) {
                self.inbox.remove(index);
            }
        }
    }

    fn carried(&mut self, at_ms: i64, carried: Carried) {
        let front = carried.carried.into_iter().map(|(envelope, request)| {
            let at = key(&envelope, &request, &envelope.text);
            let queued = Queued {
                envelope,
                request,
                replaces: None,
            };
            (at_ms, at, queued)
        });
        self.inbox.splice(0..0, front);
    }

    fn queued(&mut self, at_ms: i64, queued: Queued) {
        let (envelope, request) = (&queued.envelope, &queued.request);
        let at = key(envelope, request, &envelope.text);
        if let Some(text) = &queued.replaces {
            let was = key(envelope, request, text);
            if let Some(slot) = self.inbox.iter_mut().find(|(_, kept, _)| *kept == was) {
                (slot.1, slot.2) = (at, queued);
                return;
            }
        }
        self.inbox.push((at_ms, at, queued));
    }

    /// What is owed to the requests `open` (with when each started), in queue order.
    fn owed(self, open: &HashMap<&str, i64>) -> Vec<Queued> {
        self.inbox
            .into_iter()
            .filter(|(at_ms, _, queued)| {
                queued
                    .request
                    .as_deref()
                    .and_then(|request| open.get(request))
                    .is_some_and(|started| at_ms >= started)
            })
            .map(|(_, _, queued)| queued)
            .collect()
    }
}

impl SessionManager {
    /// Keeps an envelope just queued for `request`, or (`replaces`, its text before) as it
    /// stands once more was folded into it.
    pub(super) async fn keep_queued(
        &self,
        id: &ConversationId,
        envelope: &Envelope,
        request: Option<&str>,
        replaces: Option<String>,
    ) {
        if !kept(envelope, request) {
            return;
        }
        let queued = Queued {
            envelope: envelope.clone(),
            request: request.map(str::to_owned),
            replaces,
        };
        let stored = match NewEvent::new(streams::inbox(id), QUEUED, crate::now_ms(), &queued) {
            Ok(event) => self.core.store().append(vec![event]).await.map(drop),
            Err(err) => Err(err),
        };
        if let Err(err) = stored {
            tracing::warn!(conversation = %id, error = %err, label = %envelope.label, "could not keep a queued message");
        }
    }

    /// Notes what a turn took from the inbox, so none of it goes again after a restart.
    pub(super) async fn keep_taken(
        &self,
        id: &ConversationId,
        envelopes: &[(Envelope, Option<String>)],
    ) {
        let taken = kept_of(envelopes);
        if !taken.is_empty() {
            self.keep_batch(id, TAKEN, &Taken { taken }).await;
        }
    }

    /// Notes what a turn that could not start, or one a limit handed over, put
    /// back in front of the inbox.
    pub(super) async fn keep_carried(
        &self,
        id: &ConversationId,
        envelopes: &[(Envelope, Option<String>)],
    ) {
        let carried = kept_of(envelopes);
        if !carried.is_empty() {
            self.keep_batch(id, CARRIED, &Carried { carried }).await;
        }
    }

    async fn keep_batch(&self, id: &ConversationId, kind: &str, batch: &impl Serialize) {
        let stored = match NewEvent::new(streams::inbox(id), kind, crate::now_ms(), batch) {
            Ok(event) => self.core.store().append(vec![event]).await.map(drop),
            Err(err) => Err(err),
        };
        if let Err(err) = stored {
            tracing::warn!(conversation = %id, error = %err, kind, "could not keep what the inbox did");
        }
    }

    /// At startup, once the turn the stop cut off reads so and before anything else can
    /// settle its requests: each envelope kept for a request that still works, since that
    /// request (last) started, queued more often than turns took it, and not in the inbox
    /// again already, is queued again, in the order it was first queued. The caller holds
    /// the thread's turns until the reviews' recovery has seen what is queued, so a review
    /// isn't told twice.
    pub(super) async fn recover_unheard_envelopes(&self, id: &ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let open: HashMap<&str, i64> = board
            .requests
            .values()
            .filter(|request| {
                matches!(request.state, RequestState::Working | RequestState::Waiting)
            })
            .map(|request| (request.id.as_str(), request.started_at_ms))
            .collect();
        let Some(since) = open.values().copied().min() else {
            return;
        };
        // Read newest first, back to when the oldest of those requests started.
        let mut records = Vec::new();
        let mut before = None;
        loop {
            let Ok(page) = self
                .core
                .store()
                .read_stream(
                    streams::inbox(id),
                    StreamPage {
                        before,
                        kinds: vec![QUEUED.into(), TAKEN.into(), CARRIED.into()],
                        limit: 200,
                    },
                )
                .await
            else {
                break;
            };
            let Some(last) = page.last() else {
                break;
            };
            before = Some(last.stream_seq);
            let mut older = false;
            for stored in &page {
                if stored.at_ms < since {
                    older = true;
                    continue;
                }
                records.push((stored.at_ms, stored.kind.clone(), stored.payload.clone()));
            }
            if older {
                break;
            }
        }
        let mut kept = Kept::default();
        for (at_ms, kind, payload) in records.into_iter().rev() {
            if kind == TAKEN {
                if let Ok(taken) = serde_json::from_str(payload.get()) {
                    kept.taken(&taken);
                }
            } else if kind == CARRIED {
                if let Ok(carried) = serde_json::from_str(payload.get()) {
                    kept.carried(at_ms, carried);
                }
            } else if let Ok(queued) = serde_json::from_str(payload.get()) {
                kept.queued(at_ms, queued);
            }
        }
        let Ok(conv) = self.conv(id) else {
            return;
        };
        // What is in the inbox again already counts against what is owed of it.
        let mut held: HashMap<Key, usize> = HashMap::new();
        for queued in kept.owed(&open) {
            let envelope = &queued.envelope;
            let at = key(envelope, &queued.request, &envelope.text);
            let already = match held.get_mut(&at) {
                Some(already) => already,
                None => {
                    let count = conv.inbox_count(envelope, queued.request.as_deref()).await;
                    held.entry(at).or_insert(count)
                }
            };
            if *already > 0 {
                *already -= 1;
                continue;
            }
            tracing::info!(conversation = %id, label = %envelope.label, "a message the restart lost goes to the thread again");
            // Kept already: queued again as it was, not as more owed.
            self.queue_envelope_kept(id, queued.envelope, queued.request, false)
                .await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(text: &str) -> Envelope {
        Envelope {
            kind: InjectionKind::Decision,
            label: "build server".into(),
            task_id: None,
            text: text.into(),
        }
    }

    fn queued(text: &str, replaces: Option<&str>) -> Queued {
        Queued {
            envelope: note(text),
            request: Some("r1".into()),
            replaces: replaces.map(str::to_owned),
        }
    }

    /// What recovery owes after `stream` (oldest first, as appended).
    fn owed(stream: Vec<(i64, Result<Queued, Taken>)>) -> Vec<String> {
        let mut kept = Kept::default();
        for (at_ms, record) in stream {
            match record {
                Ok(queued) => kept.queued(at_ms, queued),
                Err(taken) => kept.taken(&taken),
            }
        }
        let open = HashMap::from([("r1", 0)]);
        kept.owed(&open)
            .into_iter()
            .map(|queued| queued.envelope.text)
            .collect()
    }

    fn taken(text: &str) -> Taken {
        Taken {
            taken: vec![(note(text), Some("r1".into()))],
        }
    }

    #[test]
    fn a_message_a_turn_took_is_not_owed() {
        assert_eq!(owed(vec![(1, Ok(queued("A", None)))]), ["A"]);
        // Put back at a start without a record of its own, then taken: nothing more is owed.
        assert!(owed(vec![(1, Ok(queued("A", None))), (2, Err(taken("A")))]).is_empty());
        // Carried back by a failed turn: owed again.
        assert_eq!(
            owed(vec![
                (1, Ok(queued("A", None))),
                (2, Err(taken("A"))),
                (3, Ok(queued("A", None))),
            ]),
            ["A"]
        );
    }

    #[test]
    fn only_the_last_fold_of_a_message_is_owed() {
        let stream = vec![
            (1, Ok(queued("A", None))),
            (2, Ok(queued("A\nB", Some("A")))),
            (3, Ok(queued("A\nB\nC", Some("A\nB")))),
        ];
        assert_eq!(owed(stream), ["A\nB\nC"]);
        let stream = vec![
            (1, Ok(queued("A", None))),
            (2, Ok(queued("A\nB", Some("A")))),
            (3, Ok(queued("A\nB\nC", Some("A\nB")))),
            (4, Err(taken("A\nB\nC"))),
        ];
        assert!(owed(stream).is_empty());
    }

    #[test]
    fn messages_are_owed_in_the_order_first_queued() {
        let stream = vec![
            (1, Ok(queued("A", None))),
            (2, Ok(queued("X", None))),
            (3, Ok(queued("Y", None))),
            (4, Ok(queued("A\nB", Some("A")))),
        ];
        assert_eq!(owed(stream), ["A\nB", "X", "Y"]);
    }

    #[test]
    fn a_message_queued_again_after_a_turn_took_it_goes_where_it_was_queued_again() {
        let stream = vec![
            (1, Ok(queued("A", None))),
            (2, Err(taken("A"))),
            (3, Ok(queued("X", None))),
            (4, Ok(queued("A", None))),
        ];
        assert_eq!(owed(stream), ["X", "A"]);
    }

    #[test]
    fn what_a_turn_could_not_start_with_goes_back_in_front() {
        let mut kept = Kept::default();
        kept.queued(1, queued("A", None));
        kept.taken(&taken("A"));
        kept.queued(3, queued("B", None));
        kept.carried(
            4,
            Carried {
                carried: vec![(note("A"), Some("r1".into()))],
            },
        );
        let open = HashMap::from([("r1", 0)]);
        let owed: Vec<_> = kept
            .owed(&open)
            .into_iter()
            .map(|queued| queued.envelope.text)
            .collect();
        assert_eq!(owed, ["A", "B"]);
    }

    #[test]
    fn two_alike_messages_are_both_owed_until_both_are_taken() {
        let twice = || vec![(1, Ok(queued("A", None))), (2, Ok(queued("A", None)))];
        assert_eq!(owed(twice()), ["A", "A"]);
        let mut stream = twice();
        stream.push((3, Err(taken("A"))));
        assert_eq!(owed(stream), ["A"]);
    }
}
