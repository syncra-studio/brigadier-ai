//! Explicit waits, with civil dates resolved once and durable wake acknowledgement.
use super::{SessionManager, tracked_work::Change};
use crate::{AddWaitingOn, ConversationId, Error, Result, WaitingOn};

pub(super) fn due(by: Option<&str>, timezone: Option<&str>) -> Result<(Option<i64>, String)> {
    let tz = match timezone {
        Some(name) => jiff::tz::TimeZone::get(name)
            .map_err(|_| Error::Invalid("unknown IANA timezone".into()))?,
        None => jiff::tz::TimeZone::system(),
    };
    let name = tz.iana_name().unwrap_or("UTC").to_owned();
    let Some(by) = by else {
        return Ok((None, name));
    };
    let at = if by.len() == 10 {
        let date: jiff::civil::Date = by
            .parse()
            .map_err(|_| Error::Invalid("--by must be YYYY-MM-DD or an RFC3339 instant".into()))?;
        date.tomorrow()
            .and_then(|d| d.at(0, 0, 0, 0).to_zoned(tz))
            .map(|z| z.timestamp())
            .map_err(|e| Error::Invalid(e.to_string()))?
    } else {
        by.parse::<jiff::Timestamp>()
            .map_err(|_| Error::Invalid("--by must be YYYY-MM-DD or an RFC3339 instant".into()))?
    };
    Ok((Some(at.as_millisecond()), name))
}
fn link(value: &str) -> Result<()> {
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .ok_or_else(|| Error::Invalid("--link must be an http or https URL".into()))?;
    let host = rest.split('/').next().unwrap_or_default();
    if host.is_empty() || rest.chars().any(char::is_whitespace) || host.contains('@') {
        return Err(Error::Invalid(
            "--link must not contain credentials or spaces".into(),
        ));
    }
    if crate::PrIdentity::parse(
        &value
            .split(['?', '#'])
            .next()
            .unwrap_or(value)
            .replacen("http://", "https://", 1),
    )
    .is_some()
    {
        return Err(Error::Invalid(
            "This is a pull request; use brigadier pr watch <url>".into(),
        ));
    }
    Ok(())
}
impl SessionManager {
    pub async fn add_waiting_on(
        &self,
        id: &ConversationId,
        actor: &str,
        args: AddWaitingOn,
    ) -> Result<WaitingOn> {
        self.core.conversation(id)?;
        let text = args.text.trim();
        if text.is_empty() || text.len() > 2000 {
            return Err(Error::Invalid("wait text must contain 1–2000 bytes".into()));
        }
        if let Some(url) = &args.link {
            link(url)?;
        }
        let (due_at_ms, timezone) = due(args.by.as_deref(), args.timezone.as_deref())?;
        let wait = WaitingOn {
            id: format!("w-{}", uuid::Uuid::new_v4().simple()),
            text: text.into(),
            actor: actor.into(),
            created_at_ms: crate::now_ms(),
            due_at_ms,
            timezone,
            by: args.by,
            then: args.then,
            link: args.link,
            done_at_ms: None,
            done_note: None,
            done_notice_delivered_at_ms: None,
            wake_delivered_at_ms: None,
        };
        let _guard = self.tracked_writes.lock().await;
        self.keep_tracked(id, &Change::Wait { wait: wait.clone() })
            .await?;
        Ok(wait)
    }
    pub async fn complete_waiting_on(
        &self,
        id: &ConversationId,
        wait_id: &str,
    ) -> Result<WaitingOn> {
        self.complete_waiting_on_with_note(id, wait_id, None).await
    }
    pub async fn complete_waiting_on_with_note(
        &self,
        id: &ConversationId,
        wait_id: &str,
        note: Option<String>,
    ) -> Result<WaitingOn> {
        let _guard = self.tracked_writes.lock().await;
        let mut wait = self
            .tracked_work(id)
            .await?
            .waiting_on
            .into_iter()
            .find(|w| w.id == wait_id)
            .ok_or_else(|| Error::NotFound("wait in this session".into()))?;
        if wait.done_at_ms.is_none() {
            wait.done_at_ms = Some(crate::now_ms());
            wait.done_note = note;
            self.keep_tracked(id, &Change::Wait { wait: wait.clone() })
                .await?;
        }
        drop(_guard);
        // A completion is durable before its notice is queued for the next turn.
        self.deliver_waiting_wakes(id).await?;
        Ok(wait)
    }
    /// Scheduler contract: queue these with a durable dedupe key `waiting:<session>:<id>`,
    /// honoring lifecycle, explicit Stop and quota. Acknowledge only after durable enqueue.
    pub async fn pending_waiting_wakes(
        &self,
        id: &ConversationId,
        now_ms: i64,
    ) -> Result<Vec<WaitingOn>> {
        Ok(self
            .tracked_work(id)
            .await?
            .waiting_on
            .into_iter()
            .filter(|w| {
                w.done_at_ms.is_none()
                    && w.wake_delivered_at_ms.is_none()
                    && w.due_at_ms.is_some_and(|d| d <= now_ms)
            })
            .collect())
    }
    pub async fn acknowledge_waiting_wake(&self, id: &ConversationId, wait_id: &str) -> Result<()> {
        let _guard = self.tracked_writes.lock().await;
        let mut wait = self
            .tracked_work(id)
            .await?
            .waiting_on
            .into_iter()
            .find(|w| w.id == wait_id)
            .ok_or_else(|| Error::NotFound("wait in this session".into()))?;
        if wait.wake_delivered_at_ms.is_none() {
            wait.wake_delivered_at_ms = Some(crate::now_ms());
            self.keep_tracked(id, &Change::Wait { wait }).await?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn date_is_overdue_after_local_day_including_dst() {
        let (at, tz) = due(Some("2026-10-25"), Some("Europe/Chisinau")).unwrap();
        assert_eq!(tz, "Europe/Chisinau");
        assert_eq!(
            at,
            Some(
                "2026-10-25T22:00:00Z"
                    .parse::<jiff::Timestamp>()
                    .unwrap()
                    .as_millisecond()
            )
        );
        assert!(due(Some("2026-02-30"), Some("UTC")).is_err());
        assert!(due(Some("2026-10-25"), Some("Bogus/Zone")).is_err());
    }
    #[test]
    fn links_do_not_confuse_issue_numbers_with_prs() {
        assert!(link("https://jira.example.com/browse/APP-2552").is_ok());
        assert!(link("https://github.com/org/repo/pull/2552").is_err());
        assert!(link("https://token@example.com/x").is_err());
    }
}

impl SessionManager {
    /// Durable domain outbox. A queued label is recovered after a crash even when its
    /// original request had finished; a taken label is never delivered again.
    pub(super) async fn deliver_waiting_wakes(&self, id: &ConversationId) -> Result<()> {
        use super::conversation::Envelope;
        use crate::work::{InjectionKind, RequestState};
        use brigadier_store::{NewEvent, StreamPage};
        let _guard = self.tracked_writes.lock().await;
        let conversation = self.core.conversation(id)?;
        if conversation.lifecycle != crate::Lifecycle::Active
            || conversation.quota_wait.is_some()
            || self.is_closing(id)
        {
            return Ok(());
        }
        let board = self.core.board(id).await?;
        if board.queue.paused
            || board.latest_request().is_some_and(|r| {
                matches!(r.state, RequestState::Stopped | RequestState::Failed { .. })
            })
        {
            return Ok(());
        }
        let Some(request) = self.request_for(id).await else {
            return Ok(());
        };
        let now = crate::now_ms();
        let due = self
            .tracked_work(id)
            .await?
            .waiting_on
            .into_iter()
            .filter(|w| {
                (w.done_at_ms.is_some() && w.done_notice_delivered_at_ms.is_none())
                    || (w.done_at_ms.is_none()
                        && w.wake_delivered_at_ms.is_none()
                        && w.due_at_ms.is_some_and(|d| d <= now))
            })
            .collect::<Vec<_>>();
        if due.is_empty() {
            return Ok(());
        }
        let mut events = Vec::new();
        let mut before = None;
        loop {
            let page = self
                .core
                .store()
                .read_stream(
                    crate::model::streams::inbox(id),
                    StreamPage {
                        before,
                        kinds: vec!["inbox.queued".into(), "inbox.taken".into()],
                        limit: 500,
                    },
                )
                .await?;
            if page.is_empty() {
                break;
            }
            before = page.last().map(|e| e.stream_seq);
            events.extend(page);
        }
        for mut wait in due {
            let completed = wait.done_at_ms.is_some();
            let label = format!(
                "waiting {} {}",
                if completed { "completed" } else { "overdue" },
                wait.id
            );
            let mut queued = None;
            let mut taken = false;
            for event in &events {
                let value: serde_json::Value =
                    serde_json::from_str(event.payload.get()).map_err(|source| Error::Corrupt {
                        seq: event.stream_seq,
                        source,
                    })?;
                if event.kind == "inbox.queued"
                    && value
                        .pointer("/envelope/label")
                        .and_then(serde_json::Value::as_str)
                        == Some(&label)
                {
                    queued = Some(value);
                } else if event.kind == "inbox.taken"
                    && value["taken"].as_array().is_some_and(|entries| {
                        entries.iter().any(|entry| {
                            entry
                                .pointer("/0/label")
                                .and_then(serde_json::Value::as_str)
                                == Some(&label)
                        })
                    })
                {
                    taken = true;
                }
            }
            if taken {
                if completed {
                    wait.done_notice_delivered_at_ms = Some(now);
                } else {
                    wait.wake_delivered_at_ms = Some(now);
                }
                self.keep_tracked(id, &Change::Wait { wait }).await?;
                continue;
            }
            let envelope = Envelope {
                kind: InjectionKind::Reminder,
                label,
                task_id: None,
                text: if completed {
                    format!(
                        "[Waiting on marked done: {}. {} {} · durable wait {}.]",
                        wait.text,
                        wait.done_note.as_deref().unwrap_or(""),
                        wait.then.as_deref().unwrap_or(""),
                        wait.id
                    )
                } else {
                    format!(
                        "[Waiting on is overdue: {}. Due {} ({}). {} · durable wait {}. Check once; complete with brigadier waiting done {} when resolved.]",
                        wait.text,
                        wait.by.as_deref().unwrap_or("the recorded instant"),
                        wait.timezone,
                        wait.then.as_deref().unwrap_or(""),
                        wait.id,
                        wait.id
                    )
                },
            };
            let conv = match self.conv(id) {
                Ok(conv) => conv,
                Err(_) => continue,
            };
            if conv.inbox_count(&envelope, Some(&request)).await > 0 {
                continue;
            }
            if queued.is_none() {
                // Persist before live delivery. The domain replays an unheard label itself.
                let event = NewEvent::new(
                    crate::model::streams::inbox(id),
                    "inbox.queued",
                    now,
                    &serde_json::json!({"envelope":envelope,"request":request}),
                )?;
                self.core.store().append(vec![event]).await?;
            }
            if let Some(conv) = self
                .queue_envelope_kept(id, envelope, Some(request.clone()), false)
                .await
            {
                if !completed {
                    self.settle_requests(id).await;
                    self.kick(&conv);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod durable_wake_tests {
    use super::super::flow::{Flow, Options, Reply};
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    #[tokio::test]
    async fn a_done_notice_is_durable_until_the_next_turn_and_not_repeated() {
        let notices = Arc::new(AtomicUsize::new(0));
        let seen = notices.clone();
        let mut flow = Flow::start(
            "waiting-done-durable",
            Options::default(),
            Arc::new(move |turn| {
                let seen = seen.clone();
                Box::pin(async move {
                    if turn.input.contains("Waiting on marked done:") {
                        seen.fetch_add(1, Ordering::SeqCst);
                    }
                    Reply::text("Done")
                })
            }),
        )
        .await;
        flow.say("Track an external dependency").await;
        flow.idle().await;
        let id = flow.conversation.clone();
        let wait = flow
            .manager
            .add_waiting_on(
                &id,
                "session",
                AddWaitingOn {
                    text: "Eric restores the license".into(),
                    by: None,
                    timezone: Some("UTC".into()),
                    then: Some("Re-run CI".into()),
                    link: None,
                },
            )
            .await
            .unwrap();
        let done = flow
            .manager
            .complete_waiting_on_with_note(&id, &wait.id, Some("License restored".into()))
            .await
            .unwrap();
        assert_eq!(done.done_note.as_deref(), Some("License restored"));
        assert_eq!(notices.load(Ordering::SeqCst), 0);
        flow.restart().await;
        flow.manager.deliver_waiting_wakes(&id).await.unwrap();
        assert_eq!(notices.load(Ordering::SeqCst), 0);
        flow.say("Continue the work").await;
        flow.idle().await;
        assert_eq!(notices.load(Ordering::SeqCst), 1);
        flow.manager.deliver_waiting_wakes(&id).await.unwrap();
        flow.restart().await;
        flow.manager.deliver_waiting_wakes(&id).await.unwrap();
        flow.say("Anything else?").await;
        flow.idle().await;
        assert_eq!(notices.load(Ordering::SeqCst), 1);
        flow.stop().await;
    }
    #[tokio::test]
    async fn overdue_wake_survives_stop_and_restart_and_is_taken_once() {
        let wakes = Arc::new(AtomicUsize::new(0));
        let observed = wakes.clone();
        let mut flow = Flow::start(
            "waiting-wake-durable",
            Options::default(),
            Arc::new(move |turn| {
                let observed = observed.clone();
                Box::pin(async move {
                    if turn.input.contains("Waiting on is overdue:") {
                        observed.fetch_add(1, Ordering::SeqCst);
                    }
                    Reply::text("Done")
                })
            }),
        )
        .await;
        flow.say("Remember the external check").await;
        flow.idle().await;
        let id = flow.conversation.clone();
        let wait = flow
            .manager
            .add_waiting_on(
                &id,
                "session",
                AddWaitingOn {
                    text: "External check".into(),
                    by: Some("2000-01-01".into()),
                    timezone: Some("UTC".into()),
                    then: None,
                    link: None,
                },
            )
            .await
            .unwrap();
        flow.core.set_queue_paused(&id, true).await.unwrap();
        flow.manager.deliver_waiting_wakes(&id).await.unwrap();
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        flow.restart().await;
        flow.manager.deliver_waiting_wakes(&id).await.unwrap();
        assert_eq!(wakes.load(Ordering::SeqCst), 0);
        flow.core.set_queue_paused(&id, false).await.unwrap();
        flow.manager.deliver_waiting_wakes(&id).await.unwrap();
        flow.idle().await;
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        flow.manager.deliver_waiting_wakes(&id).await.unwrap();
        assert_eq!(
            flow.manager
                .tracked_work(&id)
                .await
                .unwrap()
                .waiting_on
                .iter()
                .find(|w| w.id == wait.id)
                .unwrap()
                .wake_delivered_at_ms
                .is_some(),
            true
        );
        flow.restart().await;
        flow.manager.deliver_waiting_wakes(&id).await.unwrap();
        flow.idle().await;
        assert_eq!(wakes.load(Ordering::SeqCst), 1);
        flow.stop().await;
    }
}
