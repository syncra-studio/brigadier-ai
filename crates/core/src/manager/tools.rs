//! The tools behind `brigadier`, answered. Every reply the orchestrator reads is logged as a
//! context injection.

use std::collections::HashSet;

use brigadier_providers::{FileSearch, SearchKind};

use super::SessionManager;

use crate::model::{ConversationId, DomainEvent, MessageRole};
use crate::tools::{OrchestratorCall, ToolReply, WorkerCall};
use crate::work::{
    ApprovalSubject, InjectionKind, OrchestratorStep, OrchestratorStepKind, QuestionItem,
    QuestionOption, TaskId,
};
use crate::{Error, Result, now_ms};

/// The most feedback cards the thread offers in one request (several when the user asked, #483).
const FEEDBACK_OFFERS_MAX: usize = 10;

impl SessionManager {
    pub(crate) async fn orchestrator_call(
        &self,
        conversation_id: ConversationId,
        call: OrchestratorCall,
    ) -> ToolReply {
        let name = call.name();
        let reply = match self.run_orchestrator_call(&conversation_id, call).await {
            Ok(text) => ToolReply::ok(text),
            Err(err) => ToolReply::error(err.to_string()),
        };
        self.log_injection(
            &conversation_id,
            InjectionKind::ToolResult,
            name.into(),
            None,
            reply.text.len(),
        )
        .await;
        reply
    }

    async fn run_orchestrator_call(
        &self,
        id: &ConversationId,
        call: OrchestratorCall,
    ) -> Result<String> {
        match call {
            OrchestratorCall::RequestApproval(args) => {
                crate::heap(|| {
                    self.open_approval(
                        id,
                        None,
                        ApprovalSubject::Action {
                            action: args.action,
                            details: args.details,
                        },
                    )
                })
                .await?;
                Ok("Asked the user. The decision arrives later as a message.".into())
            }
            OrchestratorCall::OfferFeedback(args) => {
                crate::heap(|| self.offer_feedback(id, args)).await
            }
            // What the thread finds in the index counts as its own search (`super::reads`).
            OrchestratorCall::CodeSearch(args) => {
                let search = FileSearch {
                    kind: match args.kind.as_deref() {
                        Some("file") => SearchKind::Files,
                        _ => SearchKind::Content,
                    },
                    pattern: Some(args.query.clone()),
                    scope: args.path.clone(),
                    glob: args
                        .language
                        .as_ref()
                        .map(|language| format!("language:{language}")),
                    hits: Vec::new(),
                };
                let (text, hits) = crate::heap(|| self.code_search_tool(id, args)).await?;
                crate::heap(|| self.looked_in_index(id, FileSearch { hits, ..search })).await;
                Ok(text)
            }
            OrchestratorCall::CodeRefs(args) => {
                let symbol = args.symbol.clone();
                let (text, hits) = crate::heap(|| self.code_refs_tool(id, args)).await?;
                let search = FileSearch {
                    kind: SearchKind::Content,
                    pattern: Some(symbol),
                    scope: None,
                    glob: None,
                    hits,
                };
                crate::heap(|| self.looked_in_index(id, search)).await;
                Ok(text)
            }
            OrchestratorCall::ProjectMap => crate::heap(|| self.project_map_tool(id)).await,
            OrchestratorCall::ReadPage(args) => crate::heap(|| self.read_page(id, args)).await,
            OrchestratorCall::WritePage(args) => {
                crate::heap(|| self.check_plan_mode(id)).await?;
                crate::heap(|| self.write_page(id, args)).await
            }
            OrchestratorCall::SaveReport(args) => crate::heap(|| self.save_report(args)).await,
            OrchestratorCall::StartPreview(args) => {
                crate::heap(|| self.start_preview(id, args)).await
            }
            OrchestratorCall::StopPreview(args) => {
                crate::heap(|| self.stop_preview_tool(id, args.id.as_deref(), None)).await
            }
            OrchestratorCall::PreviewLog(args) => {
                crate::heap(|| {
                    self.preview_log(id, args.id.as_deref(), args.tail_lines, args.wait_secs)
                })
                .await
            }
        }
    }

    /// Files a row for the thread under the request the orchestrator serves.
    pub(crate) async fn orchestrator_step(&self, id: &ConversationId, kind: OrchestratorStepKind) {
        let request = self.request_for(id).await;
        self.orchestrator_step_in(id, request, kind).await;
    }

    /// Says a moment of the conversation (a usage limit, a switch of account) as a divider in
    /// the thread, under the request it happened in.
    pub(crate) async fn moment(&self, id: &ConversationId, text: String) {
        let request = self.request_for(id).await;
        self.orchestrator_step_in(id, request, OrchestratorStepKind::Moment { text })
            .await;
    }

    /// Files a row for the thread under `request`.
    pub(crate) async fn orchestrator_step_in(
        &self,
        id: &ConversationId,
        request: Option<String>,
        kind: OrchestratorStepKind,
    ) {
        let step = OrchestratorStep {
            request_id: request,
            kind,
            at_ms: now_ms(),
            position: 0,
        };
        if let Err(err) = self
            .core
            .record_conversation(id, vec![DomainEvent::OrchestratorStepped { step }])
            .await
        {
            tracing::warn!(conversation = %id, error = %err, "could not store an orchestrator step");
        }
    }

    pub(crate) async fn worker_call(
        &self,
        conversation_id: ConversationId,
        task_id: TaskId,
        call: WorkerCall,
    ) -> ToolReply {
        let result = match call {
            WorkerCall::StartPreview(args) => {
                crate::heap(|| self.start_worker_preview(&conversation_id, &task_id, args)).await
            }
            WorkerCall::StopPreview(args) => {
                crate::heap(|| {
                    self.stop_preview_tool(&conversation_id, args.id.as_deref(), Some(&task_id))
                })
                .await
            }
            WorkerCall::PreviewLog(args) => {
                crate::heap(|| {
                    self.worker_preview_log(
                        &conversation_id,
                        &task_id,
                        args.id.as_deref(),
                        args.tail_lines,
                        args.wait_secs,
                    )
                })
                .await
            }
        };
        match result {
            Ok(text) => ToolReply::ok(text),
            Err(err) => ToolReply::error(err.to_string()),
        }
    }

    /// In plan mode nothing changes until the user turns it off: a page write is refused,
    /// whatever the permission level.
    async fn check_plan_mode(&self, id: &ConversationId) -> Result<()> {
        if self.plan_mode(id) {
            return Err(Error::Invalid(
                "Plan mode is on: change nothing yet. Writing works again once the user turns plan mode off.".into(),
            ));
        }
        Ok(())
    }

    /// `offer_feedback`: a card in the thread with a report to Brigadier's developers, which
    /// the user sends or dismisses (the thread judges they are annoyed, or that they asked;
    /// this only plumbs). One offer per incident: none while a card of the latest request that
    /// offered any is unanswered or dismissed unless they asked for this one, one in a request
    /// unless they asked (then up to [`FEEDBACK_OFFERS_MAX`], each title once, #483). An offer
    /// from an attempt the user asked to redo, or from a message they edited away, counts for nothing.
    pub(crate) async fn offer_feedback(
        &self,
        id: &ConversationId,
        args: crate::tools::OfferFeedback,
    ) -> Result<String> {
        let (title, body) = (args.title.trim(), args.body.trim());
        if title.is_empty() || body.is_empty() {
            return Err(Error::Invalid(
                "`title` and `body` must both be written".into(),
            ));
        }
        let request = self.request_for(id).await;
        let conv = self.conv(id)?;
        let _offer = conv.feedback_offer.lock().await;
        let board = self.core.board(id).await?;
        let messages = match &board.head {
            Some((head, _)) => self.core.branch(id, head).await?,
            None => self.core.all_messages(id).await?,
        };
        let shown: HashSet<String> = messages
            .into_iter()
            .filter(|message| message.role == MessageRole::User)
            .map(|message| message.request_id.unwrap_or(message.id))
            .collect();
        // The latest earlier request that offered any (`None` before one did), its cards with
        // their answers; and this request's titles.
        let mut last = None;
        let mut cards: Vec<(&str, Option<bool>)> = Vec::new();
        let mut here: Vec<&str> = Vec::new();
        // An offer from a request's earlier attempt (before a Try again) or from a message the
        // user edited away is out of view: it counts for nothing, as the thread shows it.
        let current = |step: &&crate::work::OrchestratorStep| match &step.request_id {
            Some(request) => {
                shown.contains(request)
                    && board
                        .requests
                        .get(request)
                        .is_none_or(|request| step.at_ms >= request.started_at_ms)
            }
            None => true,
        };
        for step in board.orchestrator_steps.iter().filter(current) {
            match &step.kind {
                OrchestratorStepKind::FeedbackOffered {
                    id, title: offered, ..
                } => {
                    if step.request_id == request {
                        here.push(offered);
                        continue;
                    }
                    if last != Some(&step.request_id) {
                        last = Some(&step.request_id);
                        cards.clear();
                    }
                    cards.push((id, None));
                }
                OrchestratorStepKind::FeedbackAnswered { id, sent, .. } => {
                    if let Some((_, answer)) = cards.iter_mut().find(|(offered, _)| offered == id) {
                        *answer = Some(*sent);
                    }
                }
                _ => {}
            }
        }
        if !here.is_empty() && !args.asked {
            return Err(Error::Invalid(
                "Feedback was already offered for this request: don't offer it again.".into(),
            ));
        }
        if here
            .iter()
            .any(|offered| offered.eq_ignore_ascii_case(title))
        {
            return Err(Error::Invalid(
                "This report was already offered for this request: don't offer it again.".into(),
            ));
        }
        if here.len() >= FEEDBACK_OFFERS_MAX {
            return Err(Error::Invalid(format!(
                "At most {FEEDBACK_OFFERS_MAX} reports are offered in one request: tell the user which ones are left, so they can ask for them next."
            )));
        }
        if !args.asked && cards.iter().any(|(_, answer)| answer.is_none()) {
            return Err(Error::Invalid(
                "The user hasn't answered the feedback already offered in this session: don't offer another.".into(),
            ));
        }
        if !args.asked && cards.iter().any(|(_, answer)| *answer == Some(false)) {
            return Err(Error::Invalid(
                "The user dismissed the feedback offered earlier in this session: don't offer it again.".into(),
            ));
        }
        self.orchestrator_step_in(
            id,
            request,
            OrchestratorStepKind::FeedbackOffered {
                id: uuid::Uuid::now_v7().to_string(),
                kind: args.kind,
                title: title.to_owned(),
                body: body.to_owned(),
            },
        )
        .await;
        Ok("The user sees the report as a card under your answer, to send or dismiss. Don't mention it again, nor say whether it is sent: they may send it after your answer, and the card shows it. Carry on with your answer.".into())
    }

    /// The user's answer to the feedback offer `offer_id` in this conversation: sent (opened
    /// on GitHub, with what became of its reports) or dismissed. Once only, but a sent one
    /// again when a Try again filed the rest: its `results` take the first's place.
    pub async fn answer_feedback_offer(
        &self,
        id: &ConversationId,
        offer_id: &str,
        sent: bool,
        results: Vec<crate::feedback::FeedbackSent>,
    ) -> Result<()> {
        let conv = self.conv(id)?;
        let _offer = conv.feedback_offer.lock().await;
        let board = self.core.board(id).await?;
        let offer = board.orchestrator_steps.iter().find(|step| {
            matches!(&step.kind, OrchestratorStepKind::FeedbackOffered { id, .. } if id == offer_id)
        });
        let Some(offer) = offer else {
            return Err(Error::NotFound(format!("feedback offer {offer_id}")));
        };
        if board.orchestrator_steps.iter().any(|step| {
            matches!(&step.kind, OrchestratorStepKind::FeedbackAnswered { id, sent: was, .. } if id == offer_id && !(*was && sent))
        }) {
            return Err(Error::Invalid("this feedback was already answered".into()));
        }
        let request = offer.request_id.clone();
        self.orchestrator_step_in(
            id,
            request,
            OrchestratorStepKind::FeedbackAnswered {
                id: offer_id.to_owned(),
                sent,
                results: if sent { results } else { Vec::new() },
            },
        )
        .await;
        Ok(())
    }
}

/// The most questions one card asks, and the most options one question offers.
const ROUND_MAX: usize = 6;
const OPTIONS_MAX: usize = 4;

/// An `ask_user` round as the card's questions, or why it can't be asked as it is.
pub(super) fn ask_round(questions: Vec<crate::tools::AskQuestion>) -> Result<Vec<QuestionItem>> {
    if questions.is_empty() || questions.len() > ROUND_MAX {
        return Err(Error::Invalid(format!(
            "ask_user takes 1 to {ROUND_MAX} questions in one round; ask the rest in the next round."
        )));
    }
    questions
        .into_iter()
        .map(|question| {
            let text = question.question.trim().to_owned();
            if text.is_empty() {
                return Err(Error::Invalid("a question of the round is empty".into()));
            }
            // A blank option goes; the recommendation keeps pointing at the option it named.
            let mut recommended = None;
            let mut options: Vec<QuestionOption> = Vec::new();
            for (index, option) in question.options.into_iter().enumerate() {
                let label = option.label.trim();
                if label.is_empty() {
                    continue;
                }
                if question.recommended == Some(index as u32) {
                    recommended = Some(options.len() as u32);
                }
                options.push(QuestionOption {
                    label: label.to_owned(),
                    description: option
                        .description
                        .map(|line| line.trim().to_owned())
                        .filter(|line| !line.is_empty()),
                });
            }
            if options.len() == 1 || options.len() > OPTIONS_MAX {
                return Err(Error::Invalid(format!(
                    "\"{text}\": give 2 to {OPTIONS_MAX} options (or none, for a free answer)."
                )));
            }
            if !options.is_empty() && recommended.is_none() {
                return Err(Error::Invalid(format!(
                    "\"{text}\": say which option you recommend in `recommended` (its 0-based index)."
                )));
            }
            Ok(QuestionItem {
                text,
                options,
                recommended,
                multi_select: false,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_blank_option_leaves_the_recommendation_on_the_option_it_named() {
        let round = |options: serde_json::Value, recommended: u32| {
            ask_round(vec![
                serde_json::from_value(serde_json::json!({
                    "question": "Which format?",
                    "options": options,
                    "recommended": recommended,
                }))
                .unwrap(),
            ])
        };
        let options =
            serde_json::json!([{ "label": " " }, { "label": "CSV" }, { "label": "JSON" }]);
        let [item] = &round(options.clone(), 1).unwrap()[..] else {
            panic!("one question")
        };
        assert_eq!(item.options.len(), 2);
        assert_eq!(item.recommended, Some(0));
        assert_eq!(item.options[0].label, "CSV");
        // Recommending the blank one recommends nothing: the thread is asked to say which.
        let blank = round(options, 0).unwrap_err().to_string();
        assert!(blank.contains("recommend"), "{blank}");
    }

    /// Every await of `run_orchestrator_call` goes through [`crate::heap`]: an unoptimized
    /// build keeps all of a function's awaited futures in its stack frame at once, and before
    /// that this dispatch's frame took 0.9 MB of a runtime thread's 2 MiB stack. Its future
    /// was 44 KB then and is 12 KB now (its locals); one past 16 KB means an engine future is
    /// awaited around it again.
    #[test]
    fn the_orchestrator_tools_dispatch_keeps_its_futures_on_the_heap() {
        fn future_size<R>(
            _: fn(&'static SessionManager, &'static ConversationId, OrchestratorCall) -> R,
        ) -> usize {
            std::mem::size_of::<R>()
        }
        let size = future_size(SessionManager::run_orchestrator_call);
        assert!(
            size <= 16 << 10,
            "run_orchestrator_call's future is {size} bytes"
        );
    }
}
