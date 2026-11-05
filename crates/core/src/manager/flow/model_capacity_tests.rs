//! A worker or session whose CLI gave up on its model (at capacity, overloaded, a server or
//! network error) goes on at once on the next model of its routing, never trying one twice in a
//! run; with none left it goes back to the model the run started on and says so.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_cli::protocol::{CliRequest, StartRequest};
use brigadier_providers::{ErrorKind, ProviderError, ProviderKind};

use super::cli_tests::{brigadier, start};
use super::{AT_CAPACITY, FakeBehavior, Flow, Options, Reply};
use crate::board::Board;
use crate::manager::fallback::{self, ErrorVerdict};
use crate::model::{ModelChoice, Setup};
use crate::work::{
    AttemptEnd, OrchestratorStepKind, ProviderTransitionCause, RequestState, Task, TaskState,
};

const THREAD_MODEL: &str = "claude-opus-5-5";

/// One turn a CLI ran.
#[derive(Clone, Debug)]
struct Seen {
    provider: ProviderKind,
    model: String,
    native_id: String,
    input: String,
}

type Turns = Arc<Mutex<Vec<Seen>>>;

struct Bench {
    flow: Flow,
    /// The models whose turns fail at capacity now.
    failing: Arc<Mutex<Vec<String>>>,
    workers: Turns,
    threads: Turns,
}

/// A flow with these routing rows (work, agent, model), "another agent when all are out" as
/// `switch_providers` says, and these models failing. `request` is the worker "Start one." starts.
async fn bench(
    name: &str,
    rows: &[(&str, &str, &str)],
    switch_providers: bool,
    failing: &[&str],
    request: Option<StartRequest>,
) -> Bench {
    bench_on(
        ProviderKind::Claude,
        name,
        rows,
        switch_providers,
        failing,
        request,
    )
    .await
}

/// [`bench`], its thread on the CLI `thread`.
async fn bench_on(
    thread: ProviderKind,
    name: &str,
    rows: &[(&str, &str, &str)],
    switch_providers: bool,
    failing: &[&str],
    request: Option<StartRequest>,
) -> Bench {
    let names: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(
        failing.iter().map(|model| (*model).to_owned()).collect(),
    ));
    let (workers, threads): (Turns, Turns) = Default::default();
    let (fails, on_workers, on_threads) = (names.clone(), workers.clone(), threads.clone());
    let flow = Flow::start(
        name,
        Options {
            behavior: Arc::new(FakeBehavior {
                session_file: true,
                ..FakeBehavior::default()
            }),
            thread,
            ..Options::default()
        },
        Arc::new(move |turn| {
            let (fails, on_workers, on_threads) =
                (fails.clone(), on_workers.clone(), on_threads.clone());
            let request = request.clone();
            Box::pin(async move {
                let thread = turn.is_orchestrator();
                let model = turn.model.clone().unwrap_or_default();
                let seen = Seen {
                    provider: turn.provider,
                    model: model.clone(),
                    native_id: turn.native_id.clone(),
                    input: turn.input.clone(),
                };
                if thread {
                    if let Some(request) = request.filter(|_| turn.input.contains("Start one.")) {
                        brigadier(&turn, CliRequest::Start(Box::new(request))).await;
                        return Reply::text("Started.");
                    }
                    on_threads.lock().unwrap().push(seen);
                } else if turn.worker().is_some() {
                    on_workers.lock().unwrap().push(seen);
                } else {
                    return Reply::text("A title");
                }
                if fails.lock().unwrap().contains(&model) {
                    return Reply::at_capacity();
                }
                Reply::text(format!("Done on {model}."))
            })
        }),
    )
    .await;
    let mut text = String::from(
        "| Work | Agent | Model | Effort | Reason |\n| --- | --- | --- | --- | --- |\n",
    );
    for (work, agent, model) in rows {
        text.push_str(&format!("| {work} | {agent} | {model} | high | test |\n"));
    }
    std::fs::write(flow.manager.models_rules_path(), text).unwrap();
    let mut settings = flow.core.settings();
    settings.switch_providers = switch_providers;
    flow.core.update_settings(settings).await.unwrap();
    Bench {
        flow,
        failing: names,
        workers,
        threads,
    }
}

fn work(prompt_work: &str) -> StartRequest {
    let mut request = start("Build it.");
    request.work = Some(prompt_work.into());
    request
}

impl Bench {
    fn models(turns: &Turns) -> Vec<String> {
        turns
            .lock()
            .unwrap()
            .iter()
            .map(|turn| turn.model.clone())
            .collect()
    }

    fn worker_models(&self) -> Vec<String> {
        Self::models(&self.workers)
    }

    fn thread_models(&self) -> Vec<String> {
        Self::models(&self.threads)
    }

    fn fail(&self, models: &[&str]) {
        *self.failing.lock().unwrap() = models.iter().map(|model| (*model).to_owned()).collect();
    }

    async fn task(&self) -> Task {
        Flow::task(&self.flow.board().await, 1).clone()
    }

    /// Starts the worker and waits for it to report.
    async fn start_worker(&self) -> Task {
        self.flow.say("Start one.").await;
        self.reported(1).await
    }

    /// Waits until this many of its turns ran and it reported the last one.
    async fn reported(&self, turns: usize) -> Task {
        let seen = self.workers.clone();
        loop {
            let board = self
                .flow
                .until("the worker reported", |board| {
                    board
                        .tasks
                        .values()
                        .any(|task| task.state == TaskState::Reported)
                })
                .await;
            if seen.lock().unwrap().len() >= turns {
                // Past any move still under way.
                tokio::time::sleep(Duration::from_millis(300)).await;
                let task = self.task().await;
                if task.state == TaskState::Reported {
                    return task;
                }
            }
            drop(board);
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    async fn message_worker(&self, text: &str) {
        let task = self.task().await;
        let before = self.workers.lock().unwrap().len();
        self.flow
            .manager
            .message_worker(&task.conversation_id, &task, text.into())
            .await
            .unwrap();
        let seen = self.workers.clone();
        for _ in 0..200 {
            if seen.lock().unwrap().len() > before {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the worker took no turn");
    }

    fn setup_model(&self) -> ModelChoice {
        match self
            .flow
            .core
            .conversation(&self.flow.conversation)
            .unwrap()
            .setup
        {
            Some(Setup::Session { orchestrator, .. }) => orchestrator,
            other => panic!("no session setup: {other:?}"),
        }
    }

    /// Waits until the session runs on `model`, and past what follows it.
    async fn until_setup(&self, model: &str) {
        for _ in 0..400 {
            if self.setup_model().model.as_deref() == Some(model) {
                self.flow.settled().await;
                tokio::time::sleep(Duration::from_millis(300)).await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the session never ran on {model}: {:?}", self.setup_model());
    }

    /// Waits until the session's turn failed, and past what follows it.
    async fn until_failed(&self) -> String {
        let board = self
            .flow
            .until("the request failed", |board| failed(board).is_some())
            .await;
        self.flow.settled().await;
        tokio::time::sleep(Duration::from_millis(400)).await;
        failed(&board).unwrap()
    }
}

fn failed(board: &Board) -> Option<String> {
    board
        .requests
        .values()
        .find_map(|request| match &request.state {
            RequestState::Failed { error } => Some(error.clone()),
            _ => None,
        })
}

/// The automatic and manual "Switched" rows: their cause and the model each went to.
fn rows(board: &Board) -> Vec<(ProviderTransitionCause, Option<String>)> {
    board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::ProviderTransition { cause, to, .. } => {
                Some((*cause, to.model.clone()))
            }
            _ => None,
        })
        .collect()
}

fn automatic(model: &str) -> (ProviderTransitionCause, Option<String>) {
    (
        ProviderTransitionCause::AutomaticFallback,
        Some(model.to_owned()),
    )
}

fn strings(models: &[&str]) -> Vec<String> {
    models.iter().map(|model| (*model).to_owned()).collect()
}

fn error(kind: ErrorKind, will_retry: bool) -> ProviderError {
    ProviderError {
        kind,
        message: AT_CAPACITY.into(),
        will_retry,
        limit: None,
        code: None,
    }
}

/// These kinds move on once the CLI has given up, and only those.
#[test]
fn only_a_model_failure_the_cli_gave_up_on_moves_on() {
    for kind in [ErrorKind::Overloaded, ErrorKind::Server, ErrorKind::Network] {
        assert!(matches!(
            fallback::verdict(&error(kind, false)),
            ErrorVerdict::Transient
        ));
        assert!(fallback::model_failed(&error(kind, false)));
        assert!(
            !fallback::model_failed(&error(kind, true)),
            "it still retries"
        );
    }
    for kind in [
        ErrorKind::UsageLimit,
        ErrorKind::RateLimit,
        ErrorKind::Auth,
        ErrorKind::Billing,
        ErrorKind::ContextWindow,
        ErrorKind::InvalidRequest,
        ErrorKind::Policy,
        ErrorKind::Other,
    ] {
        assert!(!fallback::model_failed(&error(kind, false)), "{kind:?}");
    }
}

#[tokio::test]
async fn a_worker_goes_on_with_the_works_next_row_in_its_cli_session() {
    let bench = bench(
        "capacity-worker-row",
        &[("Two", "codex", "gpt-a"), ("Two", "codex", "gpt-b")],
        true,
        &["gpt-a"],
        Some(work("Two")),
    )
    .await;
    bench.flow.say("Start one.").await;
    let task = bench.reported(2).await;
    assert_eq!(bench.worker_models(), strings(&["gpt-a", "gpt-b"]));
    let turns = bench.workers.lock().unwrap().clone();
    assert_eq!(
        turns[0].native_id, turns[1].native_id,
        "the same CLI session"
    );
    assert!(
        turns[1].input.contains("Continue where you stopped."),
        "{turns:#?}"
    );
    assert_eq!(task.route.choice.model.as_deref(), Some("gpt-b"));
    assert_eq!(task.error, None);
    assert!(matches!(
        &task.attempts[0].end,
        Some(AttemptEnd::Error { kind: ErrorKind::Overloaded, message }) if message == AT_CAPACITY
    ));
    assert_eq!(rows(&bench.flow.board().await), vec![automatic("gpt-b")]);
    bench.flow.stop().await;
}

/// #744: a row naming a family word runs as the model it names, so a failure moves to the next
/// row rather than onto its own model under the short word.
#[tokio::test]
async fn a_worker_on_a_family_word_row_moves_to_the_next_row() {
    let bench = bench(
        "capacity-worker-family-word",
        &[("Two", "codex", "astra"), ("Two", "codex", "gpt-b")],
        true,
        &["gpt-6-astra"],
        Some(work("Two")),
    )
    .await;
    bench.flow.say("Start one.").await;
    bench.reported(2).await;
    assert_eq!(bench.worker_models(), strings(&["gpt-6-astra", "gpt-b"]));
    bench.flow.stop().await;
}

/// The loop guard: past the Work's rows to Fallback, on another agent, whose first row fails too;
/// no model comes round again.
#[tokio::test]
async fn a_worker_goes_on_to_fallback_on_another_agent_trying_each_model_once() {
    let bench = bench(
        "capacity-worker-fallback",
        &[
            ("One", "codex", "gpt-a"),
            ("Fallback", "claude", "claude-c"),
            ("Fallback", "claude", "claude-d"),
            ("Fallback", "codex", "gpt-a"),
        ],
        true,
        &["gpt-a", "claude-c"],
        Some(work("One")),
    )
    .await;
    bench.flow.say("Start one.").await;
    let task = bench.reported(3).await;
    assert_eq!(
        bench.worker_models(),
        strings(&["gpt-a", "claude-c", "claude-d"])
    );
    assert_eq!(
        bench.workers.lock().unwrap()[2].provider,
        ProviderKind::Claude
    );
    assert_eq!(task.route.choice.model.as_deref(), Some("claude-d"));
    assert_eq!(task.error, None);
    assert_eq!(
        rows(&bench.flow.board().await),
        vec![automatic("claude-c"), automatic("claude-d")]
    );
    bench.flow.stop().await;
}

/// Nothing can take it: each model once, back on the one it started on, saying so; its next
/// message tries that first and switches again if it is still full.
#[tokio::test]
async fn a_worker_nothing_can_take_goes_back_and_its_next_message_tries_that_first() {
    let bench = bench(
        "capacity-worker-nothing",
        &[
            ("Two", "codex", "gpt-a"),
            ("Two", "codex", "gpt-b"),
            ("Fallback", "claude", "claude-c"),
        ],
        true,
        &["gpt-a", "gpt-b", "claude-c"],
        Some(work("Two")),
    )
    .await;
    bench.flow.say("Start one.").await;
    let task = bench.reported(3).await;
    assert_eq!(
        bench.worker_models(),
        strings(&["gpt-a", "gpt-b", "claude-c"])
    );
    assert_eq!(task.route.choice.provider, ProviderKind::Codex);
    assert_eq!(task.route.choice.model.as_deref(), Some("gpt-a"));
    let said = task.error.clone().unwrap();
    assert!(said.starts_with(AT_CAPACITY), "{said}");
    assert!(
        said.contains("Nothing else in its routing can take it now"),
        "{said}"
    );
    assert!(said.contains("It is back on Codex gpt-a"), "{said}");
    assert_eq!(
        rows(&bench.flow.board().await).last(),
        Some(&automatic("gpt-a")),
        "the move back shows"
    );
    // Its next message: the one it started on first, then on again.
    bench.fail(&["gpt-a"]);
    bench.message_worker("Try again.").await;
    let task = bench.reported(5).await;
    assert_eq!(
        bench.worker_models(),
        strings(&["gpt-a", "gpt-b", "claude-c", "gpt-a", "gpt-b"])
    );
    assert_eq!(task.route.choice.model.as_deref(), Some("gpt-b"));
    assert_eq!(task.error, None);
    bench.flow.stop().await;
}

/// A turn that got through ends the run: the model that failed before may be tried again.
#[tokio::test]
async fn a_worker_turn_that_gets_through_resets_the_loop_guard() {
    let bench = bench(
        "capacity-worker-reset",
        &[("Two", "codex", "gpt-a"), ("Two", "codex", "gpt-b")],
        true,
        &["gpt-a"],
        Some(work("Two")),
    )
    .await;
    bench.flow.say("Start one.").await;
    bench.reported(2).await;
    bench.fail(&["gpt-b"]);
    bench.message_worker("More.").await;
    let task = bench.reported(4).await;
    assert_eq!(
        bench.worker_models(),
        strings(&["gpt-a", "gpt-b", "gpt-b", "gpt-a"])
    );
    assert_eq!(task.route.choice.model.as_deref(), Some("gpt-a"));
    bench.flow.stop().await;
}

#[tokio::test]
async fn a_worker_goes_past_a_target_that_fails_to_launch() {
    let bench = bench(
        "capacity-worker-launch",
        &[
            ("Three", "codex", "gpt-a"),
            ("Three", "codex", "gpt-b"),
            ("Three", "codex", "gpt-c"),
        ],
        true,
        &["gpt-a"],
        Some(work("Three")),
    )
    .await;
    bench
        .flow
        .behavior
        .fail_start_models
        .lock()
        .unwrap()
        .push("gpt-b".into());
    bench.flow.say("Start one.").await;
    let task = bench.reported(2).await;
    assert_eq!(bench.worker_models(), strings(&["gpt-a", "gpt-c"]));
    assert_eq!(task.route.choice.model.as_deref(), Some("gpt-c"));
    assert_eq!(task.error, None);
    assert_eq!(rows(&bench.flow.board().await), vec![automatic("gpt-c")]);
    bench.flow.stop().await;
}

/// #748: the CLI it moved to started and refused its first message: no turn runs, so the worker
/// isn't left busy and Running, and the next model goes on.
#[tokio::test]
async fn a_worker_goes_past_a_target_that_refuses_its_first_message() {
    let bench = bench(
        "capacity-worker-refused",
        &[
            ("One", "claude", "claude-a"),
            ("Fallback", "codex", "gpt-b"),
            ("Fallback", "claude", "claude-c"),
        ],
        true,
        &["claude-a"],
        Some(work("One")),
    )
    .await;
    bench
        .flow
        .behavior
        .refuse_sends
        .lock()
        .unwrap()
        .push(ProviderKind::Codex);
    bench.flow.say("Start one.").await;
    let task = bench.reported(2).await;
    assert_eq!(bench.worker_models(), strings(&["claude-a", "claude-c"]));
    assert_eq!(task.route.choice.model.as_deref(), Some("claude-c"));
    bench.flow.stop().await;
}

#[tokio::test]
async fn a_worker_whose_last_target_fails_to_launch_goes_back_and_says_so() {
    let bench = bench(
        "capacity-worker-launch-last",
        &[
            ("Two", "codex", "gpt-a"),
            ("Two", "codex", "gpt-b"),
            ("Fallback", "codex", "gpt-b"),
        ],
        true,
        &["gpt-a"],
        Some(work("Two")),
    )
    .await;
    bench
        .flow
        .behavior
        .fail_start_models
        .lock()
        .unwrap()
        .push("gpt-b".into());
    bench.flow.say("Start one.").await;
    let task = bench.reported(1).await;
    assert_eq!(bench.worker_models(), strings(&["gpt-a"]));
    assert_eq!(task.route.choice.model.as_deref(), Some("gpt-a"));
    let said = task.error.unwrap();
    assert!(
        said.contains("Nothing else in its routing can take it now"),
        "{said}"
    );
    assert!(said.contains("It is back on Codex gpt-a"), "{said}");
    bench.flow.stop().await;
}

/// `--agent` holds: no row of another agent, by itself or by a plain switch.
#[tokio::test]
async fn a_workers_agent_pin_holds() {
    let mut request = work("One");
    request.agent = Some(ProviderKind::Codex);
    let bench = bench(
        "capacity-pin-agent",
        &[
            ("One", "codex", "gpt-a"),
            ("Fallback", "claude", "claude-c"),
        ],
        true,
        &["gpt-a"],
        Some(request),
    )
    .await;
    bench.flow.say("Start one.").await;
    let task = bench.reported(1).await;
    assert_eq!(bench.worker_models(), strings(&["gpt-a"]));
    assert_eq!(task.route.choice.model.as_deref(), Some("gpt-a"));
    let said = task.error.clone().unwrap();
    assert!(
        said.contains("Nothing else in its routing can take it now"),
        "{said}"
    );
    assert!(!said.contains("It is back on"), "it never left: {said}");
    assert!(rows(&bench.flow.board().await).is_empty());
    let refused = bench.flow.manager.switch_worker(&task, None).await;
    assert!(refused.is_err(), "{refused:?}");
    bench.flow.stop().await;
}

/// `--model` holds: not the Work's next row either.
#[tokio::test]
async fn a_workers_model_pin_holds() {
    let mut request = work("Two");
    request.model = Some("gpt-a".into());
    let bench = bench(
        "capacity-pin-model",
        &[("Two", "codex", "gpt-a"), ("Two", "codex", "gpt-b")],
        true,
        &["gpt-a"],
        Some(request),
    )
    .await;
    bench.flow.say("Start one.").await;
    let task = bench.reported(1).await;
    assert_eq!(bench.worker_models(), strings(&["gpt-a"]));
    assert!(task.error.unwrap().contains("Nothing else in its routing"));
    bench.flow.stop().await;
}

/// "Another agent when all are out" off: the same agent's rows only.
#[tokio::test]
async fn a_worker_with_the_setting_off_stays_on_its_agent() {
    let bench = bench(
        "capacity-off-worker",
        &[
            ("One", "codex", "gpt-a"),
            ("Fallback", "claude", "claude-c"),
            ("Fallback", "codex", "gpt-b"),
        ],
        false,
        &["gpt-a"],
        Some(work("One")),
    )
    .await;
    bench.flow.say("Start one.").await;
    let task = bench.reported(2).await;
    assert_eq!(bench.worker_models(), strings(&["gpt-a", "gpt-b"]));
    assert_eq!(task.route.choice.model.as_deref(), Some("gpt-b"));
    // Now that one too: no Claude row, and it goes back.
    bench.fail(&["gpt-a", "gpt-b"]);
    bench.message_worker("More.").await;
    let task = bench.reported(4).await;
    assert_eq!(
        bench.worker_models(),
        strings(&["gpt-a", "gpt-b", "gpt-b", "gpt-a"])
    );
    assert!(task.error.unwrap().contains("Nothing else in its routing"));
    assert_eq!(task.route.choice.model.as_deref(), Some("gpt-b"));
    bench.flow.stop().await;
}

/// The refused case (worker fabe89d3): a plain `brigadier switch` of a worker whose Work has one
/// row finds the Fallback row.
#[tokio::test]
async fn a_plain_switch_finds_the_fallback_rows() {
    let bench = bench(
        "capacity-plain-switch",
        &[
            ("Solo", "codex", "gpt-s"),
            ("Fallback", "claude", "claude-c"),
        ],
        true,
        &[],
        Some(work("Solo")),
    )
    .await;
    let task = bench.start_worker().await;
    let (target, _) = bench.flow.manager.switch_worker(&task, None).await.unwrap();
    assert_eq!(target.provider, ProviderKind::Claude);
    assert_eq!(target.model.as_deref(), Some("claude-c"));
    assert_eq!(
        bench.task().await.route.choice.model.as_deref(),
        Some("claude-c")
    );
    assert_eq!(
        rows(&bench.flow.board().await),
        vec![(
            ProviderTransitionCause::ManualSwitch,
            Some("claude-c".to_owned())
        )]
    );
    bench.flow.stop().await;
}

/// With the setting off its Fallback row on another agent isn't a target.
#[tokio::test]
async fn a_plain_switch_with_the_setting_off_refuses_another_agents_row() {
    let bench = bench(
        "capacity-plain-off",
        &[
            ("Solo", "codex", "gpt-s"),
            ("Fallback", "claude", "claude-c"),
        ],
        false,
        &[],
        Some(work("Solo")),
    )
    .await;
    let task = bench.start_worker().await;
    let refused = bench.flow.manager.switch_worker(&task, None).await;
    assert!(
        refused.unwrap_err().to_string().contains("can take it now"),
        "the refusal names why"
    );
    bench.flow.stop().await;
}

#[tokio::test]
async fn a_session_goes_on_with_the_next_row_of_its_agent_in_its_cli_session() {
    let bench = bench(
        "capacity-session-row",
        &[
            ("Fallback", "claude", "claude-d"),
            ("Fallback", "codex", "gpt-b"),
        ],
        true,
        &[THREAD_MODEL],
        None,
    )
    .await;
    bench.flow.say("Go on with it.").await;
    bench.until_setup("claude-d").await;
    assert_eq!(bench.thread_models(), strings(&[THREAD_MODEL, "claude-d"]));
    let turns = bench.threads.lock().unwrap().clone();
    assert_eq!(
        turns[0].native_id, turns[1].native_id,
        "the same CLI session"
    );
    assert!(
        turns[1].input.contains("its model could not serve it"),
        "{turns:#?}"
    );
    assert!(!turns[1].input.contains("usage limit"), "{turns:#?}");
    let board = bench.flow.settled().await;
    assert_eq!(failed(&board), None);
    assert_eq!(rows(&board), vec![automatic("claude-d")]);
    bench.flow.stop().await;
}

#[tokio::test]
async fn a_session_goes_on_on_another_agent_with_its_message() {
    let bench = bench(
        "capacity-session-agent",
        &[("Fallback", "codex", "gpt-b")],
        true,
        &[THREAD_MODEL],
        None,
    )
    .await;
    bench.flow.say("Go on with MARK.").await;
    bench.until_setup("gpt-b").await;
    assert_eq!(bench.thread_models(), strings(&[THREAD_MODEL, "gpt-b"]));
    let turns = bench.threads.lock().unwrap().clone();
    assert_eq!(turns[1].provider, ProviderKind::Codex);
    assert!(turns[1].input.contains("MARK"), "{turns:#?}");
    let board = bench.flow.settled().await;
    assert_eq!(failed(&board), None);
    assert_eq!(rows(&board), vec![automatic("gpt-b")]);
    bench.flow.stop().await;
}

/// Nothing can take it: each model once, the request fails and says so, the session is back on
/// its model, nothing runs by itself; its next message tries that first and switches again.
#[tokio::test]
async fn a_session_nothing_can_take_goes_back_and_its_next_message_tries_that_first() {
    let bench = bench(
        "capacity-session-nothing",
        &[
            ("Fallback", "claude", "claude-d"),
            ("Fallback", "codex", "gpt-b"),
        ],
        true,
        &[THREAD_MODEL, "claude-d", "gpt-b"],
        None,
    )
    .await;
    bench.flow.say("Go on with it.").await;
    let said = bench.until_failed().await;
    assert!(said.starts_with(AT_CAPACITY), "{said}");
    assert!(
        said.contains("Nothing else in its routing can take it now"),
        "{said}"
    );
    assert!(
        said.contains(&format!("It is back on Claude Code {THREAD_MODEL}")),
        "{said}"
    );
    assert_eq!(
        bench.thread_models(),
        strings(&[THREAD_MODEL, "claude-d", "gpt-b"]),
        "each once, and none by itself after"
    );
    let setup = bench.setup_model();
    assert_eq!(setup.provider, ProviderKind::Claude);
    assert_eq!(setup.model.as_deref(), Some(THREAD_MODEL));
    assert_eq!(
        rows(&bench.flow.board().await).last(),
        Some(&automatic(THREAD_MODEL)),
        "the move back shows"
    );
    bench.fail(&[THREAD_MODEL]);
    bench.flow.say("Try again.").await;
    bench.until_setup("claude-d").await;
    assert_eq!(
        bench.thread_models(),
        strings(&[THREAD_MODEL, "claude-d", "gpt-b", THREAD_MODEL, "claude-d"])
    );
    bench.flow.stop().await;
}

#[tokio::test]
async fn a_session_turn_that_gets_through_resets_the_loop_guard() {
    let bench = bench(
        "capacity-session-reset",
        &[
            ("Fallback", "claude", THREAD_MODEL),
            ("Fallback", "claude", "claude-d"),
        ],
        true,
        &[THREAD_MODEL],
        None,
    )
    .await;
    bench.flow.say("Go on with it.").await;
    bench.until_setup("claude-d").await;
    bench.fail(&["claude-d"]);
    bench.flow.say("More.").await;
    bench.until_setup(THREAD_MODEL).await;
    assert_eq!(
        bench.thread_models(),
        strings(&[THREAD_MODEL, "claude-d", "claude-d", THREAD_MODEL])
    );
    assert_eq!(failed(&bench.flow.board().await), None);
    bench.flow.stop().await;
}

#[tokio::test]
async fn a_session_goes_past_a_target_that_fails_to_launch() {
    let bench = bench(
        "capacity-session-launch",
        &[
            ("Fallback", "claude", "claude-c"),
            ("Fallback", "claude", "claude-d"),
        ],
        true,
        &[THREAD_MODEL],
        None,
    )
    .await;
    bench
        .flow
        .behavior
        .fail_start_models
        .lock()
        .unwrap()
        .push("claude-c".into());
    bench.flow.say("Go on with it.").await;
    bench.until_setup("claude-d").await;
    assert_eq!(bench.thread_models(), strings(&[THREAD_MODEL, "claude-d"]));
    let board = bench.flow.settled().await;
    assert_eq!(failed(&board), None);
    assert_eq!(rows(&board), vec![automatic("claude-d")]);
    bench.flow.stop().await;
}

/// Its last target fails to launch: back on its model, and the message that kept doesn't run
/// again by itself (that would come round for ever with every model failing).
#[tokio::test]
async fn a_session_whose_last_target_fails_to_launch_goes_back_without_a_loop() {
    let bench = bench(
        "capacity-session-launch-last",
        &[("Fallback", "claude", "claude-c")],
        true,
        &[THREAD_MODEL],
        None,
    )
    .await;
    bench
        .flow
        .behavior
        .fail_start_models
        .lock()
        .unwrap()
        .push("claude-c".into());
    bench.flow.say("Go on with it.").await;
    let said = bench.until_failed().await;
    assert!(
        said.contains("Nothing else in its routing can take it now"),
        "{said}"
    );
    assert!(
        said.contains(&format!("It is back on Claude Code {THREAD_MODEL}")),
        "{said}"
    );
    assert_eq!(bench.setup_model().model.as_deref(), Some(THREAD_MODEL));
    tokio::time::sleep(Duration::from_millis(600)).await;
    assert_eq!(bench.thread_models(), strings(&[THREAD_MODEL]));
    // Its turn was handed over and nothing carried it on: it reads failed, not working for good.
    let view = bench
        .flow
        .manager
        .thread_view(&bench.flow.conversation)
        .unwrap();
    assert_eq!(
        view.turns.last().and_then(|turn| turn.end),
        Some(crate::thread_view::TurnEnd::Failed),
        "{view:#?}"
    );
    // It reads as the start that failed, not as what handed it over.
    let last = view.turns.last().unwrap();
    assert_eq!(last.error_kind, None, "{view:#?}");
    assert_eq!(last.limit, None, "{view:#?}");
    // What it kept for the next try isn't work: its block closes, or it would read working for good.
    tokio::time::sleep(Duration::from_millis(700)).await;
    let view = bench
        .flow
        .manager
        .thread_view(&bench.flow.conversation)
        .unwrap();
    assert!(
        view.block.is_some_and(|block| block.ended_at_ms.is_some()),
        "its work block is still open"
    );
    bench.flow.stop().await;
}

/// What a failed start kept for the next try doesn't stop a worker's word: it reaches the thread,
/// or the worker's end would never be heard and its block would wait out the hold for nothing.
#[tokio::test]
async fn a_workers_word_wakes_a_session_whose_start_failed() {
    let bench = bench(
        "capacity-session-launch-wake",
        &[("Fallback", "claude", "claude-c")],
        true,
        &[THREAD_MODEL],
        None,
    )
    .await;
    bench
        .flow
        .behavior
        .fail_start_models
        .lock()
        .unwrap()
        .push("claude-c".into());
    bench.flow.say("Go on with it.").await;
    bench.until_failed().await;
    bench.fail(&[]);
    let worker = crate::work::TaskId("t-word".into());
    bench.flow.manager.worker_event(
        &bench.flow.conversation,
        &worker,
        None,
        crate::manager::plain_workers::event(
            &worker,
            brigadier_cli::protocol::EventKind::TurnEnded,
        ),
    );
    super::eventually("the worker's word to reach the thread", || {
        bench
            .threads
            .lock()
            .unwrap()
            .iter()
            .any(|turn| turn.input.contains("[worker events wn-"))
    })
    .await;
    bench.flow.stop().await;
}

const EVERY_CLI: [(ProviderKind, &str); 3] = [
    (ProviderKind::Claude, "claude"),
    (ProviderKind::Codex, "codex"),
    (ProviderKind::OpenCode, "opencode"),
];

impl Bench {
    /// A worker's word for the thread, waited for until a turn of the thread carried it.
    async fn workers_word(&self) {
        let worker = crate::work::TaskId("t-word".into());
        self.flow.manager.worker_event(
            &self.flow.conversation,
            &worker,
            None,
            crate::manager::plain_workers::event(
                &worker,
                brigadier_cli::protocol::EventKind::TurnEnded,
            ),
        );
        let threads = self.threads.clone();
        super::eventually("the worker's word to reach the thread", || {
            threads
                .lock()
                .unwrap()
                .iter()
                .any(|turn| turn.input.contains("[worker events wn-"))
        })
        .await;
    }

    /// The thread's turns that were given the user's message as something to do.
    fn sent_as_new(&self, words: &str) -> usize {
        self.threads
            .lock()
            .unwrap()
            .iter()
            .filter(|turn| turn.input.contains(words) && !turn.input.contains("began on it"))
            .count()
    }
}

/// The CLI began the turn, so it took the message (it may have started workers already); the
/// switch's target then fails to start. A later turn doesn't hand it the message as new: real
/// work would run twice. It still reads what was asked, as what it already began.
#[tokio::test]
async fn a_message_its_cli_took_is_not_sent_again_after_a_failed_start() {
    for (thread, agent) in EVERY_CLI {
        let bench = bench_on(
            thread,
            &format!("capacity-took-{agent}"),
            &[("Fallback", agent, "next-model")],
            true,
            &[],
            None,
        )
        .await;
        let model = bench.setup_model().model.unwrap_or_default();
        bench.fail(&[&model]);
        bench
            .flow
            .behavior
            .fail_start_models
            .lock()
            .unwrap()
            .push("next-model".into());
        bench.flow.say("Go on with it.").await;
        bench.until_failed().await;
        assert_eq!(
            bench.sent_as_new("Go on with it."),
            1,
            "{agent}: its first turn"
        );
        bench.fail(&[]);
        bench.workers_word().await;
        bench.flow.settled().await;
        assert_eq!(
            bench.sent_as_new("Go on with it."),
            1,
            "{agent}: sent again as new"
        );
        bench.flow.stop().await;
    }
}

/// The start failed before its CLI ever had the message: the next turn delivers it, so it isn't
/// lost.
#[tokio::test]
async fn a_message_its_cli_never_had_goes_with_the_next_turn_after_a_failed_start() {
    for (thread, agent) in EVERY_CLI {
        let bench = bench_on(
            thread,
            &format!("capacity-never-{agent}"),
            &[],
            true,
            &[],
            None,
        )
        .await;
        bench
            .flow
            .behavior
            .fail_starts
            .store(true, std::sync::atomic::Ordering::SeqCst);
        bench.flow.say("Go on with it.").await;
        bench.until_failed().await;
        assert_eq!(
            bench.sent_as_new("Go on with it."),
            0,
            "{agent}: no CLI had it"
        );
        bench
            .flow
            .behavior
            .fail_starts
            .store(false, std::sync::atomic::Ordering::SeqCst);
        bench.workers_word().await;
        bench.flow.settled().await;
        assert_eq!(
            bench.sent_as_new("Go on with it."),
            1,
            "{agent}: delivered once"
        );
        bench.flow.stop().await;
    }
}

/// "Another agent when all are out" off: its only other row is another agent's, so it stays.
#[tokio::test]
async fn a_session_with_the_setting_off_stays_on_its_agent() {
    let bench = bench(
        "capacity-session-off",
        &[("Fallback", "codex", "gpt-b")],
        false,
        &[THREAD_MODEL],
        None,
    )
    .await;
    bench.flow.say("Go on with it.").await;
    let said = bench.until_failed().await;
    assert!(
        said.contains("Nothing else in its routing can take it now"),
        "{said}"
    );
    assert!(!said.contains("It is back on"), "it never left: {said}");
    assert_eq!(bench.thread_models(), strings(&[THREAD_MODEL]));
    assert_eq!(bench.setup_model().model.as_deref(), Some(THREAD_MODEL));
    assert!(rows(&bench.flow.board().await).is_empty());
    bench.flow.stop().await;
}
