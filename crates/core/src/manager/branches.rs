//! Editing a sent message, asking for a new answer, and moving between the branches those
//! leave behind.
//!
//! Messages form a tree: each has a parent, and the thread shows the branch that ends at the
//! conversation's head. An edit adds the new message beside the one it edits; a new answer
//! moves the head back to the user's message, so the next reply starts a branch of its own.
//!
//! A Chat has no side effects, so any message can be edited or answered again, and every
//! branch stays reachable. Its CLI session starts over with the branch shown as transcript.
//! A session's work is real (tasks, cards, commits), so only its latest request can be edited
//! or redone, and only while nothing from it has landed. What that request started is stopped
//! and its cards are closed first; the orchestrator keeps its context and is told what changed.
//! Its earlier versions can be shown again while nothing works.

use std::sync::Arc;
use std::time::Duration;

use super::SessionManager;
use super::conversation::ConvLive;
use crate::board::Board;
use crate::model::{ConversationId, ConversationKind, MessageRole};
use crate::sessions::{branch_of, display_text, one_line, parent_of};
use crate::work::{ApprovalSubject, AttachmentRef, CardState, PlanState, QuestionKind};
use crate::{Error, Result, now_ms};

/// Characters of the user's message quoted when the orchestrator is told of a switch.
const NOTE_PREVIEW_CHARS: usize = 200;

/// How long a stopped orchestrator turn may take to end before an edit gives up.
const STOP_TURN_LIMIT: Duration = Duration::from_secs(20);

impl SessionManager {
    /// Replaces a sent user message with `text`: the new message starts a branch beside it
    /// and gets an answer of its own.
    pub async fn edit_message(
        &self,
        id: ConversationId,
        message_id: String,
        text: String,
        attachments: Option<Vec<AttachmentRef>>,
    ) -> Result<()> {
        self.admit()?;
        if text.trim().is_empty() {
            return Err(Error::Invalid("the message is empty".into()));
        }
        let conv = self.conv(&id)?;
        let messages = self.core.all_messages(&id).await?;
        let at = messages
            .iter()
            .position(|message| message.id == message_id)
            .ok_or_else(|| Error::NotFound(format!("message {message_id}")))?;
        let original = messages[at].clone();
        if original.role != MessageRole::User {
            return Err(Error::Invalid(
                "only your own messages can be edited".into(),
            ));
        }
        let parent = parent_of(&messages, at);
        let note = match conv.kind {
            ConversationKind::Chat => {
                self.chat_rework(&conv).await?;
                None
            }
            ConversationKind::Session => {
                let request = original.request_id.clone().unwrap_or(original.id.clone());
                let was = self.session_rework(&conv, &request).await?;
                Some(format!(
                    "[Brigadier: the user edited their last message; it was: \"{was}\". What you started for it was stopped. Answer the new version.]"
                ))
            }
        };
        let attachments = edited_attachments(
            &text,
            attachments.as_deref().unwrap_or(&original.attachments),
        );
        let edited = self
            .core
            .append_user_message_under(
                id.clone(),
                text,
                attachments,
                original.mentions.clone(),
                Some(parent),
            )
            .await;
        match edited {
            Ok(message) => conv.carry(Some(message), note).await,
            Err(err) => {
                conv.carry(None, None).await;
                return Err(err);
            }
        }
        self.settle_requests(&id).await;
        self.kick(&conv);
        Ok(())
    }

    /// Answers a request again: the thread goes back to its user message, and the next reply
    /// starts a new branch there.
    pub async fn regenerate(&self, id: ConversationId, request_id: String) -> Result<()> {
        self.admit()?;
        let conv = self.conv(&id)?;
        let messages = self.core.all_messages(&id).await?;
        let message = messages
            .iter()
            .find(|message| message.id == request_id && message.role == MessageRole::User)
            .cloned()
            .ok_or_else(|| Error::NotFound(format!("request {request_id}")))?;
        let note = match conv.kind {
            ConversationKind::Chat => {
                self.chat_rework(&conv).await?;
                None
            }
            ConversationKind::Session => {
                self.session_rework(&conv, &request_id).await?;
                Some(
                    "[Brigadier: the user asked for a new answer to this message; what you started for it before was stopped. Start over.]"
                        .to_owned(),
                )
            }
        };
        let moved = async {
            self.core.switch_branch(&id, message.id.clone()).await?;
            self.core.restart_request(&id, &request_id).await
        }
        .await;
        match moved {
            Ok(()) => conv.carry(Some(message), note).await,
            Err(err) => {
                conv.carry(None, None).await;
                return Err(err);
            }
        }
        self.settle_requests(&id).await;
        self.kick(&conv);
        Ok(())
    }

    /// Shows another branch: the one that ends at `head`. New messages continue it.
    ///
    /// A session switches only while nothing works (the orchestrator's turn and every worker
    /// are over), so no work is left running for a version the thread no longer shows. Its
    /// orchestrator keeps its context and is told, with the next message, which version the
    /// user went back to.
    pub async fn switch_branch(&self, id: ConversationId, head: String) -> Result<()> {
        self.admit()?;
        let conv = self.conv(&id)?;
        let messages = self.core.all_messages(&id).await?;
        let branch = branch_of(&messages, &head);
        if branch.is_empty() {
            return Err(Error::NotFound(format!("message {head}")));
        }
        let note = match conv.kind {
            ConversationKind::Chat => {
                self.chat_rework(&conv).await?;
                None
            }
            ConversationKind::Session => {
                let board = self.core.board(&id).await?;
                if conv.turn_running().await
                    || board.tasks.values().any(|task| !task.state.is_final())
                {
                    return Err(Error::Invalid(
                        "wait for the work in progress to finish, or stop it, before switching versions".into(),
                    ));
                }
                let shown = branch
                    .iter()
                    .rev()
                    .find(|message| message.role == MessageRole::User)
                    .map(|message| {
                        one_line(
                            &display_text(&message.text, &message.attachments),
                            NOTE_PREVIEW_CHARS,
                        )
                    })
                    .unwrap_or_default();
                Some(format!(
                    "[Brigadier: the user switched the thread back to another version of their message: \"{shown}\". Messages from now on continue from that version.]"
                ))
            }
        };
        let switched = self.core.switch_branch(&id, head).await;
        conv.carry(None, note).await;
        switched
    }

    /// Readies a Chat for another branch: nothing may be answering, and its CLI session starts
    /// over from the branch shown.
    async fn chat_rework(&self, conv: &Arc<ConvLive>) -> Result<()> {
        if conv.turn_running().await {
            return Err(Error::Invalid(
                "wait for the answer to finish, or stop it first".into(),
            ));
        }
        conv.hold().await;
        conv.close_cli().await;
        self.forget_native_session(&conv.id).await;
        conv.mark_reseed().await;
        Ok(())
    }

    /// Readies a session's latest request to be edited or redone: stops what it started and
    /// closes its cards. Refused once any of it has landed. Returns the start of its message.
    async fn session_rework(&self, conv: &Arc<ConvLive>, request: &str) -> Result<String> {
        let board = self.core.board(&conv.id).await?;
        let Some(latest) = board.latest_request().filter(|latest| latest.id == request) else {
            return Err(Error::Invalid(
                "only your latest message in a session can be edited or answered again".into(),
            ));
        };
        let preview = latest.preview.clone();
        if landed(&board, request) {
            return Err(Error::Invalid(
                "work for this message has already landed, so it can't be edited or answered again; send a new message instead".into(),
            ));
        }
        if let Some(running) = conv.running_request().await
            && running != request
        {
            return Err(Error::Invalid(
                "the orchestrator is answering an earlier request; try again when it is done"
                    .into(),
            ));
        }
        let tasks: Vec<_> = board
            .tasks
            .values()
            .filter(|task| task.request_id.as_deref() == Some(request) && !task.state.is_final())
            .cloned()
            .collect();
        conv.withdraw(request, tasks.iter().map(|task| task.id.clone()))
            .await;
        if conv.running_request().await.is_some() {
            conv.interrupt_turn().await;
            if !conv.wait_idle(STOP_TURN_LIMIT).await {
                conv.carry(None, None).await;
                return Err(Error::Invalid(
                    "the orchestrator did not stop in time; try again".into(),
                ));
            }
        }
        let of = |id: &Option<String>| id.as_deref() == Some(request);
        // Its plans close first.
        for plan in board.plans.values() {
            if of(&plan.request_id) && plan.state == PlanState::Proposed {
                let closed = self
                    .change_plan(&conv.id, &plan.id, |plan| {
                        plan.state = PlanState::Superseded;
                        plan.decided_at_ms = Some(now_ms());
                        Ok(())
                    })
                    .await;
                if let Err(err) = closed {
                    tracing::warn!(card = %plan.id, error = %err, "could not close a plan");
                }
            }
        }
        for task in tasks {
            if let Err(err) = self.stop_task(task.id.clone()).await {
                tracing::warn!(task = %task.id, error = %err, "could not stop a task for an edit");
            }
        }
        for approval in board.approvals.values() {
            if of(&approval.request_id) && approval.state == CardState::Pending {
                self.settle_approval(
                    approval,
                    CardState::Expired {
                        reason: "You changed the request that asked.".into(),
                    },
                )
                .await;
            }
        }
        for question in board.questions.values() {
            if of(&question.request_id)
                && question.kind == QuestionKind::Orchestrator
                && question.answered_at_ms.is_none()
            {
                self.withdraw_question(question).await;
            }
        }
        // What its answer so far waited for (a note, a finished worker's blocker) is over:
        // the new answer lists what it needs.
        self.rework_waiting(&conv.id, request).await;
        Ok(preview)
    }
}

/// Whether anything a request started has landed, or is landing with the user's approval.
fn landed(board: &Board, request: &str) -> bool {
    let of = |id: &Option<String>| id.as_deref() == Some(request);
    board
        .tasks
        .values()
        .any(|task| of(&task.request_id) && task.state == crate::work::TaskState::Landed)
        || board.approvals.values().any(|approval| {
            of(&approval.request_id)
                && matches!(approval.state, CardState::Allowed { .. })
                && matches!(
                    approval.subject,
                    ApprovalSubject::Landing { .. } | ApprovalSubject::FinishSession { .. }
                )
        })
}

/// Edits remove deliberately deleted inline images instead of invoking the missing-token fallback.
fn edited_attachments(text: &str, attachments: &[AttachmentRef]) -> Vec<AttachmentRef> {
    attachments
        .iter()
        .filter(|attachment| {
            !attachment.inline || text.contains(&format!("[image:{}]", attachment.id))
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_keeps_rows_and_only_inline_refs_with_tokens() {
        let row = AttachmentRef {
            id: "a".into(),
            name: "a.png".into(),
            mime: "image/png".into(),
            bytes: 1,
            pasted: false,
            inline: false,
        };
        let mut inline = row.clone();
        inline.inline = true;
        let mut removed = inline.clone();
        removed.id = "b".into();
        let attachments = vec![row.clone(), inline.clone(), removed];
        assert_eq!(
            edited_attachments("keep [image:a] twice [image:a]", &attachments),
            vec![row.clone(), inline]
        );
        assert_eq!(
            edited_attachments("removed [image:a-longer]", &attachments),
            vec![row]
        );
    }
}
