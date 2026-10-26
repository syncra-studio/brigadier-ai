//! Deciding (run2-D40, run2-D46): a message the user sends while a session or Chat works waits
//! in the queue rail, Deciding, and reaches its CLI at once as the user's own message under a
//! one-line frame naming its queue id. The session judges it with `brigadier queue <id>
//! join|wait|worker <worker-id>|new-session`, as it did through an MCP tool before
//! (027893857, #437); no tool server is involved.
//!
//! - join: it is taken into the running turn as the user's message of a request of its own,
//!   never sent again (the session already has it).
//! - wait: it stays queued and goes on its own turn once nothing works, named again.
//! - worker: Brigadier gives its words to that worker of the session's (run2-D38), and the rail
//!   keeps a notice of where it went. The item is claimed first, so no timer, turn end or
//!   pump settles it meanwhile; a hand-off that fails puts it back, waiting.
//! - new-session: it waits, offering the user "Open as a new session" or "Run here".
//!
//! Judging is optional (the earlier default, 7a3114dbd/027893857): one unjudged after
//! [`DECIDING_WAIT`] joins the working answer, and so does one still unjudged when its turn
//! completes; after a stop, a failure or a limit, and after a restart, it waits for its own
//! turn. A message the session has can't be edited; deleted, the session hears it was taken
//! back. Side chats and tab chats keep the plain queue.

use std::collections::HashMap;
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use brigadier_cli::protocol::{CliReply, ErrorCode, Judgment, QueueOutput, QueueState};
use regex::Regex;

use super::{ConvLive, SendOutcome};
use crate::manager::SessionManager;
use crate::manager::cli::{CliCaller, MIN_ID, short};
use crate::model::{Conversation, Mention, Message, MessageRole};
use crate::work::{AttachmentRef, QueuedMessage, SentToWorker};
use crate::{Result, now_ms};

/// How long a queued message waits for the session's judgment before it joins the working
/// answer: a thread inside a long command can't judge it, and the user shouldn't watch it
/// decide (7a3114dbd).
pub(crate) const DECIDING_WAIT: Duration = Duration::from_secs(if cfg!(test) { 3 } else { 20 });

/// How a queued message's frame begins in its CLI; the app's thread hides it (run2-D44).
pub(crate) const FRAME: &str = "[queued message ";

/// How many unjudged joins are remembered for a late judgment.
const JOINED_KEPT: usize = 32;

/// The line a queued message reaches the session under, to judge it.
pub(crate) fn deciding_frame(shown: &str) -> String {
    format!(
        "{FRAME}{shown} · judge it: brigadier queue {shown} join|wait|worker <id>|new-session; \
         unjudged, it joins this work]\n"
    )
}

/// The line a waited message goes on its own turn under.
pub(crate) fn own_turn_frame(shown: &str) -> String {
    format!("{FRAME}{shown}, now on its own turn]\n")
}

/// What the session hears when the user deletes a queued message it already has.
pub(crate) fn withdrawn_frame(shown: &str) -> String {
    format!("{FRAME}{shown} withdrawn: the user took it back; leave it]")
}

/// `text` without Brigadier's queue frames: the one a queued message starts with, and any word
/// that one was taken back. The app's thread reads them the same way (`cliBlocks.ts`).
pub(crate) fn without_frames(text: &str) -> std::borrow::Cow<'_, str> {
    static START: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^\[queued message \S{8}(?: · [^\]\n]*|, now on its own turn)\]\n?").unwrap()
    });
    static WITHDRAWN: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\n*\[queued message \S{8} withdrawn: [^\]\n]*\]\n*").unwrap()
    });
    if !text.contains(FRAME) {
        return text.into();
    }
    let body = START.replace(text, "");
    WITHDRAWN.replace_all(&body, "\n").trim().to_owned().into()
}

/// Whether messages sent to `conversation` while it works are judged: a session's and a
/// Chat's are (run2-D46); a side chat's and a tab chat's keep the plain queue.
pub(crate) fn judges_queue(conversation: &Conversation) -> bool {
    conversation.side_of.is_none()
}

/// Queued `item` as the user's message it is read as, kept nowhere.
fn as_message(conversation_id: &crate::model::ConversationId, item: &QueuedMessage) -> Message {
    Message {
        id: item.id.clone(),
        conversation_id: conversation_id.clone(),
        seq: 0,
        role: MessageRole::User,
        text: item.text.clone(),
        blob: None,
        created_at_ms: item.queued_at_ms,
        attachments: item.attachments.clone(),
        mentions: item.mentions.clone(),
        model: None,
        request_id: None,
        parent_id: None,
        peer: None,
        answer_at_ms: None,
    }
}

/// The text a queued message goes to a worker with: the user's words with what they pasted
/// (`words`), and where its files are.
fn worker_text(words: String, files: &[(String, Option<String>)]) -> String {
    let mut text = words;
    if !files.is_empty() {
        let named: Vec<String> = files
            .iter()
            .map(|(name, path)| match path {
                Some(path) => format!("{name} ({path})"),
                None => name.clone(),
            })
            .collect();
        text.push_str(&format!("\n(attached: {})", named.join(", ")));
    }
    text
}

impl ConvLive {
    /// Sets the line message `id`'s text goes to its CLI under, once.
    fn frame(&self, id: &str, frame: String) {
        self.frames.lock().unwrap().insert(id.to_owned(), frame);
    }

    /// The line message `id`'s text goes under, if one was set; taken.
    pub(super) fn take_frame(&self, id: &str) -> Option<String> {
        self.frames.lock().unwrap().remove(id)
    }

    /// Queued `item` is sent now on a turn of its own: one the session already has is named.
    pub(super) fn goes_on_own_turn(&self, item: &QueuedMessage) {
        if item.delivered {
            self.frame(&item.id, own_turn_frame(&short(&item.id)));
        }
    }
}

impl SessionManager {
    /// Queues a message sent while the conversation works, Deciding, and gives it to its CLI to
    /// judge: steered into the running turn under its frame. When no turn can take it now, it
    /// waits plainly; unless `wake`: the session's work holds with no turn running, and a turn
    /// starts now with it to judge.
    pub(super) async fn queue_to_judge(
        &self,
        conv: &Arc<ConvLive>,
        text: String,
        attachments: Vec<AttachmentRef>,
        mentions: Vec<Mention>,
        queue_index: Option<u32>,
        wake: bool,
    ) -> Result<SendOutcome> {
        let item = conv
            .user_write(self.core.enqueue_as(
                &conv.id,
                text,
                attachments,
                mentions,
                queue_index,
                true,
            ))
            .await?;
        Ok(SendOutcome::Queued(
            self.deliver_to_judge(conv, item, wake).await,
        ))
    }

    async fn deliver_to_judge(
        &self,
        conv: &Arc<ConvLive>,
        mut item: QueuedMessage,
        wake: bool,
    ) -> QueuedMessage {
        let _routing = conv.queue_routing.lock().await;
        let shown = short(&item.id);
        let mut state = conv.state.lock().await;
        let cli = state.cli.clone().filter(|_| {
            state.busy
                && !state.judging_ended
                && !state.compacting
                && !state.interrupted
                && !state.seam.held
        });
        let steered = match cli {
            Some(cli) => {
                // The message the session reads, kept nowhere until it is judged.
                let message = as_message(&conv.id, &item);
                conv.frame(&item.id, deciding_frame(&shown));
                let input = self
                    .turn_input(conv, std::slice::from_ref(&message), &[])
                    .await;
                conv.take_frame(&item.id);
                // Never a new turn (run2-D46): with none running it waits plainly.
                matches!(cli.session.steer_running(input).await, Ok(true))
            }
            None => false,
        };
        // The session's work holds with no turn running: a turn of its own wakes it to judge
        // the message, under the same frame, and goes on with that work (`ThreadTurn::joins`).
        let woke = !steered
            && wake
            && !state.busy
            && !state.closing
            && !state.compacting
            && !state.seam.held
            && state.hand_over.is_none();
        if woke {
            conv.frame(&item.id, deciding_frame(&shown));
            state.woken.insert(item.id.clone());
            // The turn serves the request it becomes once joined (its id is the item's): what
            // the turn starts before the session judges it (a job, a worker) is that request's.
            let mut message = as_message(&conv.id, &item);
            message.request_id = Some(item.id.clone());
            state.pending.push(message);
        }
        if !steered && !woke {
            drop(state);
            if let Err(err) = self
                .core
                .settle_queued(&conv.id, std::slice::from_ref(&item.id), false)
                .await
            {
                crate::logging::private_trace!(conv.private_logs, warn, "could not queue a message plainly"; conversation = %conv.id, error = %err);
            }
            // The turn may have ended meanwhile and passed it by, still Deciding.
            self.kick(conv);
            item.deciding = false;
            return item;
        }
        state.deciding.push(item.id.clone());
        // A command of the thread's running now holds it: it reads the message once that ends.
        let held = state.held_by();
        if held.is_some() {
            state.held_items.push(item.id.clone());
        }
        drop(state);
        match self
            .core
            .mark_delivered(&conv.id, &item.id, held.clone())
            .await
        {
            // Deleted as it went: the session hears it was taken back.
            Ok(false) => self.withdraw(conv, &item.id).await,
            Ok(true) => {}
            Err(err) => {
                crate::logging::private_trace!(conv.private_logs, warn, "could not mark a queued message given"; conversation = %conv.id, error = %err);
            }
        }
        // A command that ended while this was written released it first: say so again.
        if held.is_some() && !conv.state.lock().await.held_items.contains(&item.id) {
            self.mark_items_held(conv, std::slice::from_ref(&item.id), None)
                .await;
        }
        self.judge_within(conv, &item.id);
        if woke {
            self.kick(conv);
        }
        item.delivered = true;
        item.held_by = held;
        item
    }

    /// Marks Deciding queued messages as waiting behind the thread's command `held`, or as
    /// read now.
    pub(super) async fn mark_items_held(
        &self,
        conv: &ConvLive,
        items: &[String],
        held: Option<crate::work::HeldBy>,
    ) {
        if let Err(err) = self.core.mark_queued_held(&conv.id, items, held).await {
            crate::logging::private_trace!(conv.private_logs, warn, "could not mark queued messages held by a command"; conversation = %conv.id, error = %err);
        }
    }

    /// Gives the session [`DECIDING_WAIT`] to judge queued message `item_id`; one still
    /// unjudged then joins the working answer.
    fn judge_within(&self, conv: &Arc<ConvLive>, item_id: &str) {
        let (manager, conv, item_id) = (self.arc(), conv.clone(), item_id.to_owned());
        self.spawn(async move {
            tokio::time::sleep(DECIDING_WAIT).await;
            manager.join_unjudged(&conv, &item_id).await;
        });
    }

    async fn join_unjudged(&self, conv: &Arc<ConvLive>, item_id: &str) {
        let _routing = conv.queue_routing.lock().await;
        {
            let state = conv.state.lock().await;
            if !state.deciding.iter().any(|id| id == item_id) || state.claimed.contains(item_id) {
                return;
            }
        }
        match self.join_delivered(conv, item_id).await {
            Ok(()) => Self::remember_join(conv, item_id).await,
            Err(err) => {
                crate::logging::private_trace!(conv.private_logs, warn, "could not join an unjudged queued message"; conversation = %conv.id, error = %err);
            }
        }
    }

    async fn remember_join(conv: &ConvLive, item_id: &str) {
        let mut state = conv.state.lock().await;
        state.joined_unjudged.push(item_id.to_owned());
        let over = state.joined_unjudged.len().saturating_sub(JOINED_KEPT);
        state.joined_unjudged.drain(..over);
    }

    /// Queued message `item_id`, which the session already has, joins the running turn as the
    /// user's message of a request of its own, never sent again; with no turn running it is
    /// kept as that request, which ends at once. The caller holds `queue_routing`.
    async fn join_delivered(&self, conv: &Arc<ConvLive>, item_id: &str) -> Result<()> {
        {
            let mut state = conv.state.lock().await;
            state.deciding.retain(|id| id != item_id);
            state.held_items.retain(|id| id != item_id);
        }
        let message = conv
            .user_write(async {
                let item = self.core.take_queued(&conv.id, item_id).await?;
                self.core.append_queued(conv.id.clone(), item).await
            })
            .await?;
        let (into, held) = {
            let mut state = conv.state.lock().await;
            if state.busy {
                state.in_turn.push(message.clone());
                let held = state.held_by();
                if held.is_some()
                    && let Some(request) = &message.request_id
                {
                    state.held_requests.push(request.clone());
                }
                // The rest of the turn answers it: its own request.
                (
                    std::mem::replace(&mut state.request, message.request_id.clone()),
                    held,
                )
            } else {
                // Woken to be judged, its turn not begun yet: that turn carries it as the
                // user's message now, unframed.
                if state.woken.remove(item_id) {
                    conv.take_frame(item_id);
                    for pending in &mut state.pending {
                        if pending.id == item_id {
                            *pending = message.clone();
                        }
                    }
                }
                (None, None)
            }
        };
        self.note_steered(conv, message.request_id.as_deref(), into)
            .await;
        if let (Some(held), Some(request)) = (held, &message.request_id) {
            self.hold(conv, std::slice::from_ref(request), held).await;
        }
        self.settle_requests(&conv.id).await;
        Ok(())
    }

    /// The user's Steer on queued message `item_id`: one the running turn has and still judges
    /// joins at once without going again, and one being handed to a worker is left to that
    /// (true). Any other is left to the plain steer (false), named again if the session read
    /// it before: its judgment was wait or a new session, or its turn is over.
    pub(super) async fn steered_delivered(
        &self,
        conv: &Arc<ConvLive>,
        item_id: &str,
    ) -> Result<bool> {
        let deciding = {
            let state = conv.state.lock().await;
            if state.claimed.contains(item_id) {
                return Ok(true);
            }
            state.deciding.iter().any(|id| id == item_id)
        };
        if deciding {
            self.join_delivered(conv, item_id).await?;
        }
        Ok(deciding)
    }

    /// The turn ended: the queued messages it left unjudged join it when it got through
    /// (`completed`), else wait for their own turn. Those being handed to a worker are left to
    /// that hand-off.
    pub(super) async fn end_judging(&self, conv: &Arc<ConvLive>, completed: bool) {
        let _routing = conv.queue_routing.lock().await;
        let unjudged: Vec<String> = {
            let mut state = conv.state.lock().await;
            state.judging_ended = true;
            state.woken.clear();
            let claimed = state.claimed.clone();
            let (kept, unjudged) = std::mem::take(&mut state.deciding)
                .into_iter()
                .partition(|id| claimed.contains(id));
            state.deciding = kept;
            state.held_items.retain(|id| claimed.contains(id));
            unjudged
        };
        if unjudged.is_empty() {
            return;
        }
        if completed {
            for item_id in &unjudged {
                match self.join_delivered(conv, item_id).await {
                    Ok(()) => Self::remember_join(conv, item_id).await,
                    Err(err) => {
                        crate::logging::private_trace!(conv.private_logs, warn, "could not join an unjudged queued message"; conversation = %conv.id, error = %err);
                    }
                }
            }
        } else if let Err(err) = self.core.settle_queued(&conv.id, &unjudged, false).await {
            crate::logging::private_trace!(conv.private_logs, warn, "could not settle queued messages"; conversation = %conv.id, error = %err);
        }
    }

    /// After a start: queued messages still Deciding when the daemon stopped wait for their own
    /// turn. Their judging (the timer, the turn) didn't outlive the stop (#449).
    pub(crate) async fn settle_unjudged(&self, id: &crate::model::ConversationId) {
        let Ok(board) = self.core.board(id).await else {
            return;
        };
        let deciding: Vec<String> = board
            .queue
            .items
            .iter()
            .filter(|item| item.deciding)
            .map(|item| item.id.clone())
            .collect();
        if deciding.is_empty() {
            return;
        }
        if let Err(err) = self.core.settle_queued(id, &deciding, false).await {
            tracing::warn!(conversation = %id, error = %err, "could not settle queued messages after a restart");
        }
    }

    /// The user deleted queued message `item_id`. One the session already has is withdrawn:
    /// the session hears so, in its running turn or with its next one (run2-D46).
    pub async fn delete_queued(
        &self,
        id: &crate::model::ConversationId,
        item_id: &str,
    ) -> Result<()> {
        let conv = self.conv(id)?;
        let _routing = conv.queue_routing.lock().await;
        if conv.state.lock().await.claimed.contains(item_id) {
            return Err(crate::Error::Invalid(
                "this message is being sent to a worker".into(),
            ));
        }
        let item = self.core.take_queued(id, item_id).await?;
        if item.delivered {
            self.withdraw(&conv, item_id).await;
        }
        Ok(())
    }

    /// Tells the session the user took back queued message `item_id`: steered into its running
    /// turn (never starting one), else with its next turn.
    async fn withdraw(&self, conv: &Arc<ConvLive>, item_id: &str) {
        let text = withdrawn_frame(&short(item_id));
        let mut state = conv.state.lock().await;
        state.deciding.retain(|id| id != item_id);
        state.held_items.retain(|id| id != item_id);
        // Woken to be judged, its turn not begun yet: the session never had it.
        if state.woken.remove(item_id) {
            state.pending.retain(|pending| pending.id != item_id);
            conv.take_frame(item_id);
            return;
        }
        let cli = state
            .cli
            .clone()
            .filter(|_| state.busy && !state.compacting && !state.interrupted && !state.seam.held);
        let steered = match cli {
            Some(cli) => matches!(
                cli.session
                    .steer_running(brigadier_providers::TurnInput::text(text.clone()))
                    .await,
                Ok(true)
            ),
            None => false,
        };
        if !steered {
            state.notes.push(text.clone());
        }
        drop(state);
    }

    /// The user's "Run here" on a message the session offered as a new session.
    pub async fn run_queued_here(
        &self,
        id: &crate::model::ConversationId,
        item_id: &str,
    ) -> Result<crate::work::MessageQueue> {
        let conv = self.conv(id)?;
        let queue = self.core.run_queued_here(id, item_id).await?;
        self.kick(&conv);
        Ok(queue)
    }

    /// `brigadier queue <id> <judgment>`: the calling session's judgment of a message in its
    /// queue. The rail reads the item's state this writes (G2).
    pub(crate) async fn cli_queue(
        &self,
        caller: &CliCaller,
        id: &str,
        judgment: Judgment,
    ) -> Result<CliReply> {
        if caller.task_id.is_some() {
            return Ok(CliReply::error(
                ErrorCode::Refused,
                "refused: a worker has no queue; its session judges the user's messages",
            ));
        }
        let conv = self.conv(&caller.conversation_id)?;
        let board = self.core.board(&caller.conversation_id).await?;
        let found: Vec<&QueuedMessage> = board
            .queue
            .items
            .iter()
            .filter(|item| item.id == id || (id.len() >= MIN_ID && short(&item.id).starts_with(id)))
            .collect();
        let item = match found.as_slice() {
            [item] => (*item).clone(),
            [] => {
                let _routing = conv.queue_routing.lock().await;
                return Ok(Self::queue_gone(&conv, id).await);
            }
            _ => {
                return Ok(CliReply::error(
                    ErrorCode::Ambiguous,
                    format!("{id} starts several queued messages' ids"),
                ));
            }
        };
        #[cfg(test)]
        {
            let pause = conv.queue_judgment_pause.lock().unwrap().clone();
            if let Some((reached, release)) = pause {
                reached.notify_one();
                release.notified().await;
            }
        }
        let routing = conv.queue_routing.lock().await;
        // The lookup above can outlive an automatic join, a steer or a delete (#630).
        // Reserve only an item that still exists while all routing paths are serialized.
        let current = self.core.board(&conv.id).await?;
        let Some(item) = current
            .queue
            .items
            .iter()
            .find(|queued| queued.id == item.id)
            .cloned()
        else {
            return Ok(Self::queue_gone(&conv, &item.id).await);
        };
        let shown = short(&item.id);
        if conv.state.lock().await.claimed.contains(&item.id) {
            return Ok(CliReply::error(
                ErrorCode::Failed,
                format!("{shown} is being sent to a worker now"),
            ));
        }
        let state = match judgment {
            Judgment::Join => {
                if item.delivered {
                    self.join_delivered(&conv, &item.id).await?;
                } else {
                    drop(routing);
                    Box::pin(self.steer_queued(conv.id.clone(), item.id.clone())).await?;
                }
                QueueState::Joined
            }
            Judgment::Wait => {
                self.unjudge(&conv, &item.id).await;
                self.core
                    .settle_queued(&conv.id, std::slice::from_ref(&item.id), false)
                    .await?;
                if item.offer_session {
                    self.core.run_queued_here(&conv.id, &item.id).await?;
                }
                self.kick(&conv);
                QueueState::Waiting
            }
            Judgment::NewSession => {
                if self.core.conversation(&conv.id)?.kind != crate::model::ConversationKind::Session
                {
                    return Ok(CliReply::error(
                        ErrorCode::Refused,
                        "refused: a Chat opens no sessions; join it, let it wait, or give it to a \
                         worker",
                    ));
                }
                self.unjudge(&conv, &item.id).await;
                self.core
                    .offer_queued_as_session(&conv.id, &item.id)
                    .await?;
                QueueState::Offered
            }
            Judgment::Worker { id: worker } => {
                return self
                    .queued_to_worker(caller, &conv, item, &worker, routing)
                    .await;
            }
        };
        Ok(CliReply::ok(&QueueOutput {
            id: shown,
            state,
            already: false,
            worker: None,
        }))
    }

    /// A late judgment hears whether the message auto-joined, including at turn end.
    async fn queue_gone(conv: &ConvLive, id: &str) -> CliReply {
        let state = conv.state.lock().await;
        match state.joined_unjudged.iter().find(|joined| {
            joined.as_str() == id || (id.len() >= MIN_ID && short(joined).starts_with(id))
        }) {
            Some(joined) => CliReply::ok(&QueueOutput {
                id: short(joined),
                state: QueueState::Joined,
                already: true,
                worker: None,
            }),
            None => CliReply::error(
                ErrorCode::NotFound,
                format!("no message {id} waits in the user's queue"),
            ),
        }
    }

    /// Ends the judging of queued message `item_id` in the live state.
    async fn unjudge(&self, conv: &ConvLive, item_id: &str) {
        let mut state = conv.state.lock().await;
        state.deciding.retain(|id| id != item_id);
        state.held_items.retain(|id| id != item_id);
    }

    /// `brigadier queue <id> worker <worker-id>`: claimed, then given to the worker as
    /// `brigadier send` gives it, then settled: out of the queue with a notice of where it
    /// went, or, when the worker didn't take it, back in its slot waiting for its own turn.
    async fn queued_to_worker(
        &self,
        caller: &CliCaller,
        conv: &Arc<ConvLive>,
        item: QueuedMessage,
        worker: &str,
        routing: tokio::sync::MutexGuard<'_, ()>,
    ) -> Result<CliReply> {
        let shown = short(&item.id);
        let task = match self.own_worker(caller, worker).await {
            Ok(task) => task,
            Err(reply) => return Ok(reply),
        };
        // Reserve in both live state and the queue under the driver's lock: the pump
        // either already took it, or sees it Deciding and leaves it to this hand-off.
        let ids = std::slice::from_ref(&item.id);
        {
            let mut state = conv.state.lock().await;
            self.core.claim_queued(&conv.id, &item.id).await?;
            state.claimed.insert(item.id.clone());
        }
        drop(routing);
        let mut files = Vec::new();
        let mut copied = HashMap::new();
        for attachment in item.attachments.iter().filter(|a| !a.pasted) {
            let path = self
                .copied_file(&super::FilesAt::Conversation(conv), attachment, &mut copied)
                .await;
            files.push((
                attachment.name.clone(),
                path.map(|path| path.display().to_string()),
            ));
        }
        let words = self.full_words(&as_message(&conv.id, &item)).await;
        let sent = self
            .cli_send(caller, worker, Some(worker_text(words, &files)), None)
            .await;
        let failed = match &sent {
            Ok(CliReply::Ok { .. }) => None,
            Ok(CliReply::Error { error }) => Some(error.message.clone()),
            Err(err) => Some(err.to_string()),
        };
        let _routing = conv.queue_routing.lock().await;
        if let Some(why) = failed {
            // Back in its slot, waiting for its own turn.
            {
                let mut state = conv.state.lock().await;
                state.claimed.remove(&item.id);
                state.deciding.retain(|id| id != &item.id);
                state.held_items.retain(|id| id != &item.id);
            }
            self.core.settle_queued(&conv.id, ids, false).await?;
            self.kick(conv);
            return Ok(CliReply::error(
                ErrorCode::Failed,
                format!(
                    "{shown} could not go to worker {}: {why}. It waits in the user's queue for \
                     its own turn.",
                    short(&task.id.0)
                ),
            ));
        }
        self.core.take_queued(&conv.id, &item.id).await?;
        let kept = self
            .core
            .keep_sent(
                &conv.id,
                SentToWorker {
                    id: item.id.clone(),
                    text: item.text.clone(),
                    worker: task.id.clone(),
                    worker_number: task.number,
                    worker_title: task.title.clone(),
                    sent_at_ms: now_ms(),
                },
            )
            .await;
        {
            let mut state = conv.state.lock().await;
            state.claimed.remove(&item.id);
            state.deciding.retain(|id| id != &item.id);
            state.held_items.retain(|id| id != &item.id);
        }
        kept?;
        self.kick(conv);
        Ok(CliReply::ok(&QueueOutput {
            id: shown,
            state: QueueState::SentToWorker,
            already: false,
            worker: Some(short(&task.id.0)),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The frames name the queue id and the command, each on one line, all with one start the
    /// app's thread recognises (run2-D44).
    #[test]
    fn the_frames_name_the_id_and_the_command() {
        let deciding = deciding_frame("1a2b3c4d");
        assert!(deciding.starts_with(FRAME) && deciding.ends_with("]\n"));
        assert_eq!(deciding.lines().count(), 1);
        assert!(deciding.contains("brigadier queue 1a2b3c4d join|wait|worker <id>|new-session"));
        assert_eq!(
            own_turn_frame("1a2b3c4d"),
            "[queued message 1a2b3c4d, now on its own turn]\n"
        );
        assert!(withdrawn_frame("1a2b3c4d").starts_with("[queued message 1a2b3c4d withdrawn"));
    }
}
