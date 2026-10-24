//! Deciding (run2-D40, run2-D46): a message the user sends while a session works reaches its
//! thread at once under a line naming its queue id, waits in the rail Deciding, and the thread
//! judges it with `brigadier queue <id> join|wait|worker <id>|new-session`. The rail reads the
//! queue item's state the command wrote (G2). Unjudged, it joins the working answer after a
//! while, or with the turn that got through; after a stop or a restart it waits.

use std::sync::Arc;

use brigadier_cli::protocol::{CliRequest, Judgment};
use brigadier_providers::{ItemStatus, ProviderEvent, ProviderKind};
use serde_json::{Value, json};

use super::cli_tests::{Answers, brigadier, id_of, start};
use super::{Board, FakeBehavior, Flow, Options, Reply, Script};
use crate::model::ConversationId;
use crate::work::QueuedMessage;

const ASKED: &str = "Also check the formatting.";

fn bash(status: ItemStatus) -> ProviderEvent {
    ProviderEvent::ToolCall {
        item_id: "bash-1".into(),
        name: "Bash".into(),
        input: Some(json!({ "command": "cargo test --workspace" }).to_string()),
        status,
        output: (status != ItemStatus::InProgress).then(|| "ok".into()),
        kept: None,
    }
}

/// How the thread's "Run the tests." turn treats the message queued while it runs.
#[derive(Clone, Default)]
struct Plan {
    /// Its judgment (a worker's id is filled in with the worker the turn starts).
    judgment: Option<Judgment>,
    /// The thread reads a second message steered in (the withdrawal).
    second: bool,
    /// For a worker judgment: the worker is stopped before it is judged.
    worker_stopped: bool,
    /// Nothing is steered in to read.
    plain: bool,
    /// For a worker judgment: the worker is still starting; the judgment goes on beside the
    /// turn, which ends meanwhile.
    worker_starting: bool,
}

/// The queued message's id as its frame shows it.
fn shown_in(steered: &str) -> String {
    steered
        .strip_prefix("[queued message ")
        .expect("the frame names it")
        .chars()
        .take(8)
        .collect()
}

fn shown(item: &QueuedMessage) -> String {
    item.id[item.id.len() - 8..].to_owned()
}

/// The thread's "Run the tests." turn runs a command, reads the message steered in, judges it
/// as `plan` says once `judge` lets it, then waits for `gate` (or the user's Stop) before its
/// command ends. A worker watching the logs reads what is sent to it. Each turn's input is
/// kept in order.
fn judging(answers: Answers, gates: Gates, plan: Plan) -> Script {
    Arc::new(move |turn| {
        let (answers, plan) = (answers.clone(), plan.clone());
        let (judge, gate) = (gates.judge.clone(), gates.end.clone());
        Box::pin(async move {
            let keep = |name: &str, value: Value| {
                answers.lock().unwrap().insert(name.into(), value);
            };
            if turn.worker().is_some() {
                if turn.input.contains("Watch the logs.") {
                    keep(
                        "worker judged",
                        brigadier(
                            &turn,
                            CliRequest::Queue {
                                id: "1a2b3c4d".into(),
                                judgment: Judgment::Join,
                            },
                        )
                        .await,
                    );
                    keep("worker running", json!(true));
                    let heard = turn.steered().await.unwrap_or_default();
                    keep("worker heard", json!(heard));
                    return Reply::text("Watched.");
                }
                return Reply::text("Done.");
            }
            {
                let mut answers = answers.lock().unwrap();
                let inputs = answers.entry("inputs".into()).or_insert_with(|| json!([]));
                inputs.as_array_mut().unwrap().push(json!(turn.input));
            }
            if !turn.input.contains("Run the tests.") {
                return Reply::text("Noted.");
            }
            let worker = match &plan.judgment {
                Some(Judgment::Worker { .. }) => {
                    let prompt = if plan.worker_stopped {
                        "Say hi."
                    } else {
                        "Watch the logs."
                    };
                    let started =
                        brigadier(&turn, CliRequest::Start(Box::new(start(prompt)))).await;
                    let id = id_of(&started);
                    if plan.worker_stopped {
                        let task = turn
                            .host
                            .core
                            .tasks(&turn.conversation().unwrap())
                            .await
                            .unwrap()
                            .into_iter()
                            .find(|task| task.id.0.ends_with(&id))
                            .unwrap();
                        turn.host.stop_task(task.id).await.unwrap();
                    } else if !plan.worker_starting {
                        super::eventually("the worker to run", || {
                            answers.lock().unwrap().contains_key("worker running")
                        })
                        .await;
                    }
                    Some(id)
                }
                _ => None,
            };
            turn.emit(bash(ItemStatus::InProgress)).await;
            keep("running", json!(true));
            let steered = if plan.plain {
                String::new()
            } else {
                turn.steered().await.unwrap_or_default()
            };
            if !plan.plain {
                keep("steered", json!(steered));
            }
            if let Some(judgment) = plan.judgment {
                judge.notified().await;
                let judgment = match judgment {
                    Judgment::Worker { .. } => Judgment::Worker {
                        id: worker.unwrap(),
                    },
                    judgment => judgment,
                };
                let id = shown_in(&steered);
                let request = CliRequest::Queue { id, judgment };
                if plan.worker_starting {
                    let (turn, answers) = (turn.clone(), answers.clone());
                    tokio::spawn(async move {
                        let judged = brigadier(&turn, request).await;
                        answers.lock().unwrap().insert("judged".into(), judged);
                    });
                } else {
                    keep("judged", brigadier(&turn, request).await);
                }
            }
            if plan.second {
                let again = turn.steered().await.unwrap_or_default();
                keep("steered again", json!(again));
            }
            tokio::select! {
                () = gate.notified() => {}
                true = turn.stopped() => return Reply::text("Stopped."),
            }
            turn.emit(bash(ItemStatus::Completed)).await;
            Reply::text("The tests pass.")
        })
    })
}

/// When the thread's turn judges the message, and when its command ends.
#[derive(Clone, Default)]
struct Gates {
    judge: Arc<tokio::sync::Notify>,
    end: Arc<tokio::sync::Notify>,
}

struct Judging {
    flow: Flow,
    answers: Answers,
    gate: Arc<tokio::sync::Notify>,
    judge: Arc<tokio::sync::Notify>,
}

impl Judging {
    async fn start(name: &str, options: Options, plan: Plan) -> Judging {
        let answers = Answers::default();
        let gates = Gates::default();
        let flow = Flow::start(name, options, judging(answers.clone(), gates.clone(), plan)).await;
        Judging {
            flow,
            answers,
            gate: gates.end,
            judge: gates.judge,
        }
    }

    fn answer(&self, name: &str) -> Value {
        self.answers.lock().unwrap()[name].clone()
    }

    fn has(&self, name: &str) -> bool {
        self.answers.lock().unwrap().contains_key(name)
    }

    fn inputs(&self) -> Vec<String> {
        self.answers.lock().unwrap()["inputs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|input| input.as_str().unwrap().to_owned())
            .collect()
    }

    /// Starts the turn, sends the message while its command runs, and waits until the thread
    /// read it: the item, Deciding.
    async fn send_while_working(&self) -> QueuedMessage {
        self.flow.say("Run the tests.").await;
        super::eventually("the command to run", || self.has("running")).await;
        self.flow.say(ASKED).await;
        let board = self
            .flow
            .until("the message to wait, Deciding", |board| {
                board
                    .queue
                    .items
                    .first()
                    .is_some_and(|item| item.text == ASKED && item.deciding && item.delivered)
            })
            .await;
        super::eventually("the thread to read it", || self.has("steered")).await;
        let item = board.queue.items[0].clone();
        assert_eq!(
            shown_in(self.answer("steered").as_str().unwrap()),
            shown(&item)
        );
        assert!(
            self.answer("steered").as_str().unwrap().ends_with(ASKED),
            "the user's words follow the frame"
        );
        item
    }

    /// Lets the thread judge it, and waits for the answer.
    async fn judged(&self) -> Value {
        self.judge.notify_one();
        super::eventually("the thread to judge it", || self.has("judged")).await;
        self.answer("judged")
    }
}

fn joined(board: &Board, item: &QueuedMessage) -> bool {
    board.queue.items.is_empty() && board.requests.contains_key(&item.id)
}

/// `join`: the message leaves the queue and is the user's message of a request of its own,
/// inside the running turn's; nothing sends it again.
#[tokio::test]
async fn join_takes_it_into_the_running_turn_without_sending_it_again() {
    let judging = Judging::start(
        "queue-join",
        Options::default(),
        Plan {
            judgment: Some(Judgment::Join),
            ..Plan::default()
        },
    )
    .await;
    let item = judging.send_while_working().await;
    let judged = judging.judged().await;
    assert_eq!(judged["state"], "joined", "{judged:#}");
    assert_eq!(judged["already"], false);
    assert_eq!(judged["id"], shown(&item));
    let board = judging
        .flow
        .until("it to join", |board| joined(board, &item))
        .await;
    assert!(
        board.requests[&item.id].steered_into.is_some(),
        "{:?}",
        board.requests[&item.id]
    );
    judging.gate.notify_one();
    judging.flow.settled().await;
    assert_eq!(judging.inputs().len(), 1, "{:#?}", judging.inputs());
    judging.flow.stop().await;
}

/// `wait`: it stays queued, no longer Deciding, and goes on its own turn once the turn ends,
/// named so the thread knows it is the one it already read.
#[tokio::test]
async fn wait_keeps_it_queued_until_its_own_turn() {
    let judging = Judging::start(
        "queue-wait",
        Options::default(),
        Plan {
            judgment: Some(Judgment::Wait),
            ..Plan::default()
        },
    )
    .await;
    let item = judging.send_while_working().await;
    let judged = judging.judged().await;
    assert_eq!(judged["state"], "waiting", "{judged:#}");
    let board = judging.flow.board().await;
    let queued = &board.queue.items[0];
    assert!(!queued.deciding && queued.delivered, "{queued:?}");
    judging.gate.notify_one();
    judging
        .flow
        .until("it to go on its own turn", |board| joined(board, &item))
        .await;
    judging.flow.settled().await;
    let inputs = judging.inputs();
    assert_eq!(inputs.len(), 2, "{inputs:#?}");
    assert!(
        inputs[1].contains(&format!(
            "[queued message {}, now on its own turn]\n{ASKED}",
            shown(&item)
        )),
        "{inputs:#?}"
    );
    judging.flow.stop().await;
}

/// `new-session`: it waits for the user's pick and is never sent meanwhile; "Run here" sends
/// it on its own turn.
#[tokio::test]
async fn new_session_offers_it_and_run_here_sends_it() {
    let judging = Judging::start(
        "queue-new-session",
        Options::default(),
        Plan {
            judgment: Some(Judgment::NewSession),
            ..Plan::default()
        },
    )
    .await;
    let item = judging.send_while_working().await;
    let judged = judging.judged().await;
    assert_eq!(judged["state"], "offered", "{judged:#}");
    judging.gate.notify_one();
    let board = judging.flow.settled().await;
    let queued = &board.queue.items[0];
    assert!(queued.offer_session && !queued.deciding, "{queued:?}");
    assert_eq!(judging.inputs().len(), 1, "never sent while offered");
    judging
        .flow
        .manager
        .run_queued_here(&judging.flow.conversation, &item.id)
        .await
        .unwrap();
    judging
        .flow
        .until("it to go on its own turn", |board| joined(board, &item))
        .await;
    judging.flow.settled().await;
    assert_eq!(judging.inputs().len(), 2, "{:#?}", judging.inputs());
    judging.flow.stop().await;
}

/// `worker`: Brigadier gives the user's words to that worker of the session's (run2-D38),
/// and the rail keeps a notice of where it went until the user dismisses it. A worker has no
/// queue to judge.
#[tokio::test]
async fn worker_sends_it_to_the_sessions_worker_and_keeps_a_notice() {
    let judging = Judging::start(
        "queue-worker",
        Options::default(),
        Plan {
            judgment: Some(Judgment::Worker { id: String::new() }),
            ..Plan::default()
        },
    )
    .await;
    let item = judging.send_while_working().await;
    let judged = judging.judged().await;
    assert_eq!(judged["state"], "sent-to-worker", "{judged:#}");
    super::eventually("the worker to hear it", || judging.has("worker heard")).await;
    assert_eq!(judging.answer("worker heard"), ASKED);
    assert_eq!(
        judging.answer("worker judged")["error"]["code"],
        "refused",
        "{:#}",
        judging.answer("worker judged")
    );
    let board = judging.flow.board().await;
    assert!(board.queue.items.is_empty(), "{:?}", board.queue);
    assert_eq!(board.queue.sent.len(), 1, "{:?}", board.queue);
    let sent = &board.queue.sent[0];
    assert_eq!(
        (sent.id.as_str(), sent.text.as_str()),
        (item.id.as_str(), ASKED)
    );
    assert_eq!(sent.worker_number, 1);
    assert_eq!(
        judged["worker"],
        json!(&sent.worker.0[sent.worker.0.len() - 8..])
    );
    judging.gate.notify_one();
    judging.flow.settled().await;
    // Its worker's report may come on a turn of its own; the message never does.
    let inputs = judging.inputs();
    assert!(
        inputs[1..].iter().all(|input| !input.contains(ASKED)),
        "the session got it once: {inputs:#?}"
    );
    assert!(!board.requests.contains_key(&item.id));
    let queue = judging
        .flow
        .core
        .dismiss_sent(&judging.flow.conversation, &item.id)
        .await
        .unwrap();
    assert!(queue.sent.is_empty());
    judging.flow.stop().await;
}

/// What the user pasted goes to the worker with their words: the worker never gets a bare
/// `[Pasted text #1]`.
#[tokio::test]
async fn a_worker_gets_what_the_user_pasted() {
    let judging = Judging::start(
        "queue-worker-pasted",
        Options::default(),
        Plan {
            judgment: Some(Judgment::Worker { id: String::new() }),
            ..Plan::default()
        },
    )
    .await;
    let mut pasted = judging
        .flow
        .core
        .add_attachment(
            "Pasted text.md".into(),
            "text/markdown".into(),
            b"- ten pages".to_vec(),
            true,
        )
        .await
        .unwrap();
    pasted.inline = Some(1);
    judging.flow.say("Run the tests.").await;
    super::eventually("the command to run", || judging.has("running")).await;
    judging
        .flow
        .manager
        .send_message(
            judging.flow.conversation.clone(),
            "Also check [Pasted text #1].".into(),
            vec![pasted],
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    super::eventually("the thread to read it", || judging.has("steered")).await;
    let judged = judging.judged().await;
    assert_eq!(judged["state"], "sent-to-worker", "{judged:#}");
    super::eventually("the worker to hear it", || judging.has("worker heard")).await;
    let heard = judging.answer("worker heard");
    let heard = heard.as_str().unwrap();
    assert!(
        heard.starts_with("Also check [Pasted text #1, pasted here by the user:]")
            && heard.contains("- ten pages"),
        "{heard}"
    );
    judging.gate.notify_one();
    judging.flow.settled().await;
    judging.flow.stop().await;
}

/// A hand-off to a worker that can't take it puts the message back in its slot, waiting for
/// its own turn, and tells the thread why.
#[tokio::test]
async fn a_hand_off_that_fails_puts_it_back_waiting() {
    let judging = Judging::start(
        "queue-worker-fails",
        Options::default(),
        Plan {
            judgment: Some(Judgment::Worker { id: String::new() }),
            worker_stopped: true,
            ..Plan::default()
        },
    )
    .await;
    let item = judging.send_while_working().await;
    let judged = judging.judged().await;
    assert!(
        judged["error"]["message"]
            .as_str()
            .is_some_and(|text| text.contains("waits in the user's queue")),
        "{judged:#}"
    );
    let board = judging.flow.board().await;
    let queued = &board.queue.items[0];
    assert_eq!(queued.id, item.id);
    assert!(!queued.deciding, "{queued:?}");
    assert!(board.queue.sent.is_empty());
    judging.gate.notify_one();
    judging
        .flow
        .until("it to go on its own turn", |board| joined(board, &item))
        .await;
    judging.flow.stop().await;
}

/// Unjudged for a while, it joins the working answer by itself; a judgment that comes later
/// hears it already joined.
#[tokio::test]
async fn unjudged_it_joins_the_working_answer_and_a_late_judgment_hears_so() {
    let judging = Judging::start(
        "queue-timeout",
        Options::default(),
        Plan {
            judgment: Some(Judgment::Wait),
            ..Plan::default()
        },
    )
    .await;
    let item = judging.send_while_working().await;
    let board = judging
        .flow
        .until("it to join by itself", |board| joined(board, &item))
        .await;
    assert!(board.requests[&item.id].steered_into.is_some());
    let judged = judging.judged().await;
    assert_eq!(judged["state"], "joined", "{judged:#}");
    assert_eq!(judged["already"], true);
    judging.gate.notify_one();
    judging.flow.settled().await;
    assert_eq!(judging.inputs().len(), 1, "{:#?}", judging.inputs());
    judging.flow.stop().await;
}

/// Unjudged when the user stops the turn, it waits in the queue, no longer Deciding. The
/// user's Steer then sends it on a turn, named again: the stopped turn never answered it.
#[tokio::test]
async fn unjudged_at_a_stop_it_waits() {
    let judging = Judging::start("queue-stopped", Options::default(), Plan::default()).await;
    let item = judging.send_while_working().await;
    judging
        .flow
        .manager
        .interrupt(judging.flow.conversation.clone())
        .await
        .unwrap();
    let board = judging
        .flow
        .until("it to wait after the stop", |board| {
            board
                .queue
                .items
                .first()
                .is_some_and(|queued| queued.id == item.id && !queued.deciding)
        })
        .await;
    assert!(!board.requests.contains_key(&item.id));
    judging.flow.settled().await;
    steer_sends_it_named(&judging, &item).await;
    judging.flow.stop().await;
}

/// The user's Steer on `item`, which the thread read before and judged no further in that
/// turn: it goes on a turn of its own, named, and its request is that turn's.
async fn steer_sends_it_named(judging: &Judging, item: &QueuedMessage) {
    judging
        .flow
        .manager
        .steer_queued(judging.flow.conversation.clone(), item.id.clone())
        .await
        .unwrap();
    judging
        .flow
        .until("it to go on a turn", |board| joined(board, item))
        .await;
    judging.flow.settled().await;
    let inputs = judging.inputs();
    assert_eq!(inputs.len(), 2, "{inputs:#?}");
    assert_eq!(
        inputs[1],
        format!(
            "[queued message {}, now on its own turn]\n{ASKED}",
            shown(item)
        )
    );
}

/// The user's Steer on a message the thread offered as a new session sends it, named, on a
/// turn of its own: nothing else would.
#[tokio::test]
async fn a_steer_on_an_offered_message_sends_it_named() {
    let judging = Judging::start(
        "queue-steer-offered",
        Options::default(),
        Plan {
            judgment: Some(Judgment::NewSession),
            ..Plan::default()
        },
    )
    .await;
    let item = judging.send_while_working().await;
    assert_eq!(judging.judged().await["state"], "offered");
    judging.gate.notify_one();
    judging.flow.settled().await;
    steer_sends_it_named(&judging, &item).await;
    judging.flow.stop().await;
}

/// A message still being given to the session can't be edited either: the words it reads are
/// fixed once Deciding.
#[tokio::test]
async fn a_message_being_given_to_judge_cant_be_edited() {
    let judging = Judging::start("queue-edit-deciding", Options::default(), Plan::default()).await;
    let conversation = judging.flow.conversation.clone();
    let item = judging
        .flow
        .core
        .enqueue_as(
            &conversation,
            ASKED.into(),
            Vec::new(),
            Vec::new(),
            None,
            true,
        )
        .await
        .unwrap();
    let edited = judging
        .flow
        .manager
        .edit_queued(
            &conversation,
            &item.id,
            "Something else.".into(),
            Vec::new(),
            Vec::new(),
        )
        .await;
    assert!(edited.is_err(), "{edited:?}");
    judging.flow.stop().await;
}

/// The user can't edit a message the thread already has; deleting it tells the thread it was
/// taken back, in its running turn.
#[tokio::test]
async fn once_the_thread_has_it_an_edit_is_refused_and_a_delete_withdraws_it() {
    let judging = Judging::start(
        "queue-withdrawn",
        Options::default(),
        Plan {
            second: true,
            ..Plan::default()
        },
    )
    .await;
    let item = judging.send_while_working().await;
    let conversation = judging.flow.conversation.clone();
    let edited = judging
        .flow
        .manager
        .edit_queued(
            &conversation,
            &item.id,
            "Something else.".into(),
            Vec::new(),
            Vec::new(),
        )
        .await;
    assert!(edited.is_err(), "{edited:?}");
    judging
        .flow
        .manager
        .delete_queued(&conversation, &item.id)
        .await
        .unwrap();
    super::eventually("the thread to hear it withdrawn", || {
        judging.has("steered again")
    })
    .await;
    assert_eq!(
        judging.answer("steered again"),
        format!(
            "[queued message {} withdrawn: the user took it back; leave it]",
            shown(&item)
        )
    );
    judging.gate.notify_one();
    let board = judging.flow.settled().await;
    assert!(board.queue.items.is_empty());
    assert!(!board.requests.contains_key(&item.id));
    assert_eq!(judging.inputs().len(), 1);
    judging.flow.stop().await;
}

/// Still Deciding when the daemon stops, it waits for its own turn after the restart (#449).
#[tokio::test]
async fn a_restart_settles_what_was_deciding() {
    let mut judging = Judging::start("queue-restart", Options::default(), Plan::default()).await;
    let item = judging.send_while_working().await;
    judging.gate.notify_one();
    judging.flow.restart().await;
    judging
        .flow
        .until("it to stop deciding", |board| {
            board.queue.items.iter().all(|queued| !queued.deciding)
        })
        .await;
    let board = judging.flow.board().await;
    assert!(
        board
            .queue
            .items
            .iter()
            .all(|queued| queued.id != item.id || !queued.deciding),
        "{:?}",
        board.queue
    );
    judging.flow.stop().await;
}

/// A Codex session judges the same way: its message reaches the running turn through Codex's
/// own steering.
#[tokio::test]
async fn a_codex_session_judges_it_too() {
    let judging = Judging::start(
        "queue-codex",
        Options {
            thread: ProviderKind::Codex,
            ..Options::default()
        },
        Plan {
            judgment: Some(Judgment::Join),
            ..Plan::default()
        },
    )
    .await;
    let item = judging.send_while_working().await;
    assert_eq!(judging.judged().await["state"], "joined");
    judging
        .flow
        .until("it to join", |board| joined(board, &item))
        .await;
    judging.gate.notify_one();
    judging.flow.settled().await;
    assert_eq!(judging.inputs().len(), 1, "{:#?}", judging.inputs());
    judging.flow.stop().await;
}

/// With no Codex turn running to take it (the session is busy, its turn not yet started), it
/// waits plainly: Codex is never asked to start a turn for it (run2-D46).
#[tokio::test]
async fn with_no_codex_turn_running_it_waits_plainly() {
    let took = Arc::new(tokio::sync::Notify::new());
    let go = Arc::new(tokio::sync::Notify::new());
    let behavior = Arc::new(FakeBehavior::default());
    let judging = Judging::start(
        "queue-codex-no-turn",
        Options {
            thread: ProviderKind::Codex,
            behavior: behavior.clone(),
            ..Options::default()
        },
        Plan::default(),
    )
    .await;
    *behavior.hold_send.lock().unwrap() = Some((took.clone(), go.clone()));
    judging.flow.say("Hold on.").await;
    took.notified().await;
    judging.flow.say(ASKED).await;
    let board = judging
        .flow
        .until("it to wait", |board| !board.queue.items.is_empty())
        .await;
    let queued = &board.queue.items[0];
    assert!(!queued.deciding && !queued.delivered, "{queued:?}");
    go.notify_one();
    judging
        .flow
        .until("it to go on its own turn", |board| {
            board.queue.items.is_empty()
        })
        .await;
    judging.flow.settled().await;
    let inputs = judging.inputs();
    assert_eq!(inputs.len(), 2, "{inputs:#?}");
    assert!(inputs[1].contains(ASKED) && !inputs[1].contains("[queued message"));
    judging.flow.stop().await;
}

/// A normal Chat judges the same way (run2-D46): its message waits Deciding, reaches the running
/// turn under its frame, and the Chat's own `brigadier queue` joins it.
#[tokio::test]
async fn a_chat_judges_it_too() {
    let (judging, chat, item) = chat_judging("queue-chat", Judgment::Join).await;
    assert_eq!(judging.judged().await["state"], "joined");
    judging
        .flow
        .until_in(&chat, "it to join", |board| joined(board, &item))
        .await;
    judging.gate.notify_one();
    judging.flow.settled_in(&chat).await;
    assert_eq!(judging.inputs().len(), 1, "{:#?}", judging.inputs());
    judging.flow.stop().await;
}

/// A Chat has no project to open a session in: its `new-session` is refused, and the message
/// stays Deciding, to be judged otherwise or join by itself.
#[tokio::test]
async fn a_chat_offers_no_new_session() {
    let (judging, chat, item) = chat_judging("queue-chat-new-session", Judgment::NewSession).await;
    let judged = judging.judged().await;
    assert_eq!(judged["error"]["code"], "refused", "{judged:#}");
    let board = judging.flow.board_of(&chat).await;
    assert!(
        board
            .queue
            .items
            .iter()
            .all(|queued| queued.id != item.id || !queued.offer_session),
        "{:?}",
        board.queue
    );
    judging.gate.notify_one();
    judging
        .flow
        .until_in(&chat, "it to join with the turn", |board| {
            joined(board, &item)
        })
        .await;
    judging.flow.stop().await;
}

/// A Chat whose "Run the tests." turn runs, with the message sent meanwhile Deciding and read
/// by the turn, to be judged `judgment`.
async fn chat_judging(name: &str, judgment: Judgment) -> (Judging, ConversationId, QueuedMessage) {
    let judging = Judging::start(
        name,
        Options::default(),
        Plan {
            judgment: Some(judgment),
            ..Plan::default()
        },
    )
    .await;
    let chat = super::chat_thread_tests::new_chat(&judging.flow).await;
    judging.flow.say_to(&chat, "Run the tests.").await;
    super::eventually("the command to run", || judging.has("running")).await;
    judging.flow.say_to(&chat, ASKED).await;
    let board = judging
        .flow
        .until_in(&chat, "the message to wait, Deciding", |board| {
            board
                .queue
                .items
                .first()
                .is_some_and(|item| item.text == ASKED && item.deciding && item.delivered)
        })
        .await;
    let item = board.queue.items[0].clone();
    super::eventually("the thread to read it", || judging.has("steered")).await;
    let steered = judging.answer("steered");
    assert_eq!(shown_in(steered.as_str().unwrap()), shown(&item));
    (judging, chat, item)
}

/// A side chat keeps the plain queue: nothing reaches its turn until that ends.
#[tokio::test]
async fn a_side_chat_keeps_the_plain_queue() {
    let judging = Judging::start(
        "queue-side-chat",
        Options::default(),
        Plan {
            plain: true,
            ..Plan::default()
        },
    )
    .await;
    let side = judging
        .flow
        .manager
        .open_side_chat(&judging.flow.conversation, None)
        .await
        .unwrap()
        .id;
    judging.flow.say_to(&side, "Run the tests.").await;
    super::eventually("the command to run", || judging.has("running")).await;
    judging.flow.say_to(&side, ASKED).await;
    let board = judging
        .flow
        .until_in(&side, "it to wait", |board| !board.queue.items.is_empty())
        .await;
    let queued = &board.queue.items[0];
    assert!(!queued.deciding && !queued.delivered, "{queued:?}");
    judging.gate.notify_one();
    judging
        .flow
        .until_in(&side, "it to go on its own turn", |board| {
            board.queue.items.is_empty()
        })
        .await;
    side_inputs_end(&judging, &side).await;
    judging.flow.stop().await;
}

async fn side_inputs_end(judging: &Judging, side: &ConversationId) {
    judging.flow.settled_in(side).await;
    let inputs = judging.inputs();
    assert!(
        inputs
            .last()
            .is_some_and(|input| input.contains(ASKED) && !input.contains("[queued message")),
        "{inputs:#?}"
    );
}

/// A hand-off to a worker still starting holds the message: neither the wait for a judgment
/// nor the turn's end settles it meanwhile, and once the worker takes it, it is gone from the
/// queue with its notice.
#[tokio::test]
async fn a_message_going_to_a_starting_worker_is_left_to_that_hand_off() {
    let started = Arc::new(tokio::sync::Notify::new());
    let go = Arc::new(tokio::sync::Notify::new());
    let behavior = Arc::new(FakeBehavior::default());
    let judging = Judging::start(
        "queue-worker-starting",
        Options {
            behavior: behavior.clone(),
            ..Options::default()
        },
        Plan {
            judgment: Some(Judgment::Worker { id: String::new() }),
            worker_starting: true,
            ..Plan::default()
        },
    )
    .await;
    // The thread's own CLI first, so the hold is the worker's.
    judging.flow.say("Hello.").await;
    judging.flow.settled().await;
    *behavior.hold_start.lock().unwrap() = Some((started.clone(), go.clone()));
    let item = judging.send_while_working().await;
    started.notified().await;
    judging.judge.notify_one();
    // Past the 3 s a test's message waits to be judged, with the turn over meanwhile.
    tokio::time::sleep(std::time::Duration::from_secs(4)).await;
    judging.gate.notify_one();
    let board = judging.flow.settled().await;
    assert!(!judging.has("judged"), "{:#}", judging.answer("judged"));
    assert!(
        board.queue.items.iter().any(|queued| queued.id == item.id),
        "{:?}",
        board.queue
    );
    assert!(!board.requests.contains_key(&item.id), "it never joined");
    let inputs = judging.inputs();
    assert!(
        inputs.iter().all(|input| !input.contains(ASKED)),
        "it never went on its own turn: {inputs:#?}"
    );
    go.notify_one();
    super::eventually("the hand-off to end", || judging.has("judged")).await;
    assert_eq!(judging.answer("judged")["state"], "sent-to-worker");
    super::eventually("the worker to hear it", || judging.has("worker heard")).await;
    assert_eq!(judging.answer("worker heard"), ASKED);
    let board = judging
        .flow
        .until("its notice", |board| board.queue.sent.len() == 1)
        .await;
    assert!(board.queue.items.is_empty(), "{:?}", board.queue);
    judging.flow.stop().await;
}
