//! The CLI's own question tool and plan mode, in the user's cards (harness D7): a question it
//! asks is the thread's question card, whose answer goes back as the tool's result; a plan it
//! proposes is the plan card, whose "Build it" leaves plan mode and tells the thread to go
//! ahead.

use std::sync::{Arc, Mutex};

use brigadier_providers::{ApprovalDecision, CliQuestion, ProviderEvent, ProviderKind};

use super::{Flow, Options, Reply, Script, Turn};
use crate::board::Board;
use crate::model::Setup;
use crate::work::{Approval, ApprovalSubject, CardState, Question};

/// What the CLI's question tool got back: each question's picks, or `None` when declined.
type Picks = Option<Vec<Vec<String>>>;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

fn question(header: &str, text: &str, labels: &[&str], multi_select: bool) -> CliQuestion {
    CliQuestion::new(
        Some(header.into()),
        text.into(),
        labels.iter().map(|label| ((*label).to_owned(), None)),
        multi_select,
    )
}

fn open_question(board: &Board) -> Option<Question> {
    board.questions.values().find(|q| q.is_open()).cloned()
}

fn plans(board: &Board) -> Vec<Approval> {
    let mut plans: Vec<Approval> = board
        .approvals
        .values()
        .filter(|approval| matches!(approval.subject, ApprovalSubject::Plan { .. }))
        .cloned()
        .collect();
    plans.sort_by_key(|approval| approval.created_at_ms);
    plans
}

fn plan_text(approval: &Approval) -> &str {
    match &approval.subject {
        ApprovalSubject::Plan { plan } => plan,
        _ => unreachable!(),
    }
}

/// The CLI's questions are one card in the thread's request, its recommendation and
/// multi-select kept; the answer goes back to the CLI, picks by pick, and never to the thread
/// as a message.
#[tokio::test]
async fn a_cli_s_own_questions_are_the_thread_s_card_and_the_answer_the_tool_s_result() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let got: Arc<Mutex<Option<Picks>>> = Arc::default();
    let (log, answers) = (inputs.clone(), got.clone());
    let flow = Flow::start(
        "cli-questions",
        Options::default(),
        script(move |turn| {
            let (log, answers) = (log.clone(), answers.clone());
            async move {
                log.lock().unwrap().push(turn.input.clone());
                let answer = turn
                    .ask_questions(
                        "toolu-1",
                        vec![
                            question(
                                "Colours",
                                "Which colours?",
                                &["red (Recommended)", "green", "blue"],
                                true,
                            ),
                            question("File", "Which file?", &["a.txt", "b.txt"], false),
                        ],
                    )
                    .await;
                *answers.lock().unwrap() = answer;
                // A next round in the same turn: the request's second card.
                turn.ask_questions(
                    "toolu-2",
                    vec![question("Size", "Which size?", &["S", "M"], false)],
                )
                .await;
                Reply::text("Wrote it.")
            }
        }),
    )
    .await;
    flow.say("Write the colours file.").await;
    let board = flow
        .until("the card", |board| open_question(board).is_some())
        .await;
    let card = open_question(&board).unwrap();
    // The thread's own card in the running request: its "Waiting for your answer" row.
    assert!(card.request_id.is_some());
    assert_eq!(card.request_id, board.run_request);
    // Numbered as the card's design shows it (harness D36); two questions, no heading of theirs.
    assert_eq!(card.title.as_deref(), Some("Round 1"));
    assert_eq!(card.items.len(), 2);
    assert!(card.items[0].multi_select);
    assert_eq!(card.items[0].recommended, Some(0));
    assert_eq!(card.items[0].options[0].label, "red");
    assert!(!card.items[1].multi_select);
    assert_eq!(card.items[1].recommended, None);

    flow.manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["red, blue".into(), "c.txt".into()],
            None,
        )
        .await
        .unwrap();
    // The request's second card is its round 2, with its one question's heading.
    let board = flow
        .until("the second card", |board| {
            open_question(board).is_some_and(|next| next.id != card.id)
        })
        .await;
    let next = open_question(&board).unwrap();
    assert_eq!(next.request_id, card.request_id);
    assert_eq!(next.title.as_deref(), Some("Round 2 · Size"));
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            next.id.clone(),
            vec!["M".into()],
            None,
        )
        .await
        .unwrap();
    flow.settled().await;
    assert_eq!(
        got.lock().unwrap().clone(),
        Some(Some(vec![
            vec!["red".to_owned(), "blue".to_owned()],
            vec!["c.txt".to_owned()],
        ]))
    );
    // One turn: the answer went as the tool's result, not as a message.
    assert_eq!(inputs.lock().unwrap().len(), 1);
    flow.stop().await;
}

/// A question the CLI took back (its turn stopped) keeps its card; answered later, the answer
/// reaches the thread as a message, as any answer of the thread's does.
#[tokio::test]
async fn a_withdrawn_question_s_late_answer_reaches_the_thread() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let flow = Flow::start(
        "cli-question-withdrawn",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.earlier == 0 {
                    turn.emit(ProviderEvent::QuestionsAsked {
                        id: "codex-0".into(),
                        questions: vec![question(
                            "Ship",
                            "Ship it today?",
                            &["Yes (Recommended)", "Not yet"],
                            false,
                        )],
                    })
                    .await;
                    turn.emit(ProviderEvent::QuestionsWithdrawn {
                        id: "codex-0".into(),
                    })
                    .await;
                    return Reply::text("Stopped.");
                }
                Reply::text("Shipping.")
            }
        }),
    )
    .await;
    flow.say("Ship it.").await;
    flow.settled().await;
    let board = flow.board().await;
    let card = open_question(&board).expect("the card stays");
    // One question: its heading is the card's title, after its round number (harness D36).
    assert_eq!(card.title.as_deref(), Some("Round 1 · Ship"));
    flow.manager
        .answer_question(
            flow.conversation.clone(),
            card.id.clone(),
            vec!["Yes".into()],
            None,
        )
        .await
        .unwrap();
    flow.until("the answer's turn", |_| inputs.lock().unwrap().len() == 2)
        .await;
    flow.settled().await;
    let second = inputs.lock().unwrap()[1].clone();
    assert!(
        second.contains("[answer] You asked the user: \"Ship it today?\"")
            && second.contains("The user answered: Yes"),
        "{second}"
    );
    flow.stop().await;
}

/// Plan mode is the CLI's own, set before each turn, and nothing about it is said in words;
/// the plan is a card whose "Build it" leaves plan mode and tells the thread to go ahead.
#[tokio::test]
async fn build_it_leaves_plan_mode_and_tells_the_thread_to_go_ahead() {
    let inputs: Arc<Mutex<Vec<String>>> = Arc::default();
    let log = inputs.clone();
    let options = Options {
        plan_mode: true,
        ..Options::default()
    };
    let behavior = options.behavior.clone();
    let flow = Flow::start(
        "plan-card",
        options,
        script(move |turn| {
            let log = log.clone();
            async move {
                log.lock().unwrap().push(turn.input.clone());
                if turn.earlier == 0 {
                    turn.emit(ProviderEvent::PlanProposed {
                        item_id: "toolu-plan".into(),
                        plan: "# Add a.txt\n\n- Write hi to a.txt.".into(),
                    })
                    .await;
                    return Reply::default();
                }
                Reply::text("Built it.")
            }
        }),
    )
    .await;
    flow.say("Plan a.txt.").await;
    let board = flow
        .until("the plan card", |board| !plans(board).is_empty())
        .await;
    flow.settled().await;
    let plan = plans(&board).remove(0);
    assert_eq!(plan_text(&plan), "# Add a.txt\n\n- Write hi to a.txt.");
    assert_eq!(plan.state, CardState::Pending);
    assert_eq!(*behavior.plan_modes.lock().unwrap(), [true]);
    assert!(
        !inputs.lock().unwrap()[0].contains("[plan mode]"),
        "{:?}",
        inputs.lock().unwrap()
    );

    flow.manager
        .answer_card(
            flow.conversation.clone(),
            plan.id.clone(),
            ApprovalDecision::Allow,
        )
        .await
        .unwrap();
    flow.until("the go-ahead's turn", |_| inputs.lock().unwrap().len() == 2)
        .await;
    flow.settled().await;
    assert!(inputs.lock().unwrap()[1].contains("Build it."));
    assert_eq!(*behavior.plan_modes.lock().unwrap(), [true, false]);
    assert!(matches!(
        flow.core.conversation(&flow.conversation).unwrap().setup,
        Some(Setup::Session {
            plan_mode: false,
            ..
        })
    ));
    assert!(matches!(
        plans(&flow.board().await)[0].state,
        CardState::Allowed { .. }
    ));
    flow.stop().await;
}

/// A newer plan replaces the open one, and the user's reply closes it: it can't be built
/// once the conversation moved on.
#[tokio::test]
async fn a_newer_plan_or_a_reply_closes_the_open_plan_card() {
    let flow = Flow::start(
        "plan-card-replaced",
        Options {
            plan_mode: true,
            ..Options::default()
        },
        script(|turn| async move {
            if turn.earlier == 0 {
                for plan in ["First plan.", "Second plan."] {
                    turn.emit(ProviderEvent::PlanProposed {
                        item_id: plan.into(),
                        plan: plan.into(),
                    })
                    .await;
                }
            }
            Reply::text("Noted.")
        }),
    )
    .await;
    flow.say("Plan it.").await;
    let board = flow
        .until("both plans", |board| plans(board).len() == 2)
        .await;
    flow.settled().await;
    // Opened in the same millisecond, they are told apart by their text.
    let plan = |board: &Board, text: &str| {
        plans(board)
            .into_iter()
            .find(|approval| plan_text(approval) == text)
            .unwrap()
    };
    let (first, second) = (plan(&board, "First plan."), plan(&board, "Second plan."));
    assert_eq!(
        first.state,
        CardState::Expired {
            reason: "A newer plan replaced it.".into()
        }
    );
    assert_eq!(second.state, CardState::Pending);

    flow.say("Make it smaller.").await;
    flow.settled().await;
    assert_eq!(
        plan(&flow.board().await, "Second plan.").state,
        CardState::Expired {
            reason: "You replied instead.".into()
        }
    );
    // Closed, it can't be built.
    assert!(
        flow.manager
            .answer_card(
                flow.conversation.clone(),
                second.id.clone(),
                ApprovalDecision::Allow
            )
            .await
            .is_err()
    );
    flow.stop().await;
}

/// Plan before building (harness D42): with the setting on, each new request starts in the
/// CLI's plan mode; "Build it" leaves it, and the next request starts in it again. Off, a
/// request starts as it is.
#[tokio::test]
async fn plan_first_starts_each_new_request_in_plan_mode() {
    let options = Options::default();
    let behavior = options.behavior.clone();
    let flow = Flow::start(
        "plan-first",
        options,
        script(|turn| async move {
            if turn.input.contains("Plan a.txt.") {
                turn.emit(ProviderEvent::PlanProposed {
                    item_id: "toolu-plan".into(),
                    plan: "# Add a.txt".into(),
                })
                .await;
                return Reply::default();
            }
            Reply::text("Done.")
        }),
    )
    .await;
    flow.say("What is a.txt?").await;
    flow.settled().await;
    assert_eq!(*behavior.plan_modes.lock().unwrap(), [false]);

    let mut settings = flow.core.settings();
    settings.plan_first = true;
    flow.core.update_settings(settings).await.unwrap();
    flow.say("Plan a.txt.").await;
    let board = flow
        .until("the plan card", |board| !plans(board).is_empty())
        .await;
    flow.settled().await;
    assert_eq!(*behavior.plan_modes.lock().unwrap(), [false, true]);
    flow.manager
        .answer_card(
            flow.conversation.clone(),
            plans(&board)[0].id.clone(),
            ApprovalDecision::Allow,
        )
        .await
        .unwrap();
    flow.until("the go-ahead's turn", |_| {
        behavior.plan_modes.lock().unwrap().len() == 3
    })
    .await;
    flow.settled().await;
    assert_eq!(*behavior.plan_modes.lock().unwrap(), [false, true, false]);

    flow.say("Now b.txt.").await;
    flow.settled().await;
    assert_eq!(
        *behavior.plan_modes.lock().unwrap(),
        [false, true, false, true]
    );
    flow.stop().await;
}

/// A message while the session's workers still work is no new request: it doesn't put the
/// session in plan mode, which would leave the thread mid-build with no plan to approve (D42).
#[tokio::test]
async fn plan_first_leaves_a_message_mid_build_as_it_is() {
    let release = Arc::new(tokio::sync::Notify::new());
    let held = release.clone();
    let options = Options::default();
    let behavior = options.behavior.clone();
    let flow = Flow::start(
        "plan-first-mid-build",
        options,
        script(move |turn| {
            let held = held.clone();
            async move {
                if turn.worker().is_some() {
                    held.notified().await;
                    return Reply::text("Worker done.");
                }
                if turn.input.contains("Start one.") {
                    turn.start_worker("one", "Wait.").await;
                }
                Reply::text("Going.")
            }
        }),
    )
    .await;
    flow.say("Start one.").await;
    flow.settled().await;
    let mut settings = flow.core.settings();
    settings.plan_first = true;
    flow.core.update_settings(settings).await.unwrap();
    assert!(flow.manager.has_running_workers(&flow.conversation).await);
    flow.say("How is it going?").await;
    flow.settled().await;
    assert_eq!(*behavior.plan_modes.lock().unwrap(), [false, false]);
    release.notify_waiters();
    flow.stop().await;
}

/// The CLIs' plan mode lets a shell command run, so `brigadier start` refuses a worker
/// while the session is in plan mode, with a plain reason (harness D42).
#[tokio::test]
async fn no_worker_starts_in_plan_mode() {
    let answers = super::cli_tests::Answers::default();
    let seen = answers.clone();
    let options = Options {
        plan_mode: true,
        ..Options::default()
    };
    let flow = Flow::start(
        "plan-mode-no-workers",
        options,
        script(move |turn| {
            let seen = seen.clone();
            async move {
                let request = brigadier_cli::protocol::CliRequest::Start(Box::new(
                    super::cli_tests::start("Write a.txt."),
                ));
                let started = super::cli_tests::brigadier(&turn, request).await;
                seen.lock().unwrap().insert("start".into(), started);
                Reply::text("Planned.")
            }
        }),
    )
    .await;
    flow.say("Build a.txt.").await;
    flow.settled().await;
    let started = answers.lock().unwrap()["start"].clone();
    assert!(
        started["error"]["message"]
            .as_str()
            .is_some_and(|error| error.contains("Plan mode is on")),
        "{started}"
    );
    assert!(flow.board().await.tasks.is_empty());
    flow.stop().await;
}
