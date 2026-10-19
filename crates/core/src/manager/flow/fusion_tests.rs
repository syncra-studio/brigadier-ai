//! Fusion (Q77): a costly research question goes to a Claude and a Codex worker on the same
//! brief; the thread gets both reports together, blind, as answers A and B, and writes one
//! answer, a disagreement only where it changes what to do (Q86). One vendor: one worker, no fusion words.
//! A restart between the reports changes nothing; a partner that ends without a report leaves
//! the other's to go alone.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use brigadier_providers::ProviderKind;
use serde_json::json;

use super::chat_thread_tests::{answered, new_chat, shown};
use super::{Flow, Options, Reply, Script, Turn};
use crate::work::TaskKind;

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A fused answer as Q86 has it: the bottom line, and the one disagreement that matters in its
/// fold, in the first person; no Confidence or Disagreement lines.
const ANSWER: &str = "Buy the hardened steel nozzle: it lasts with abrasive filament.\n\n---\n\n### Price\nI'm not fully sure of the price: one source says 25 EUR, another 40 EUR, so check it at checkout.";

/// Words that would tell the thread which vendor or model gave an answer.
const VENDOR_WORDS: &[&str] = &[
    "Claude",
    "claude",
    "Codex",
    "codex",
    "Anthropic",
    "OpenAI",
    "gpt-",
    "opus",
    "sonnet",
];

fn blind(text: &str) {
    for word in VENDOR_WORDS {
        assert!(!text.contains(word), "{word} in: {text}");
    }
}

/// A long report: over what the thread reads inline, so it is read with read_report.
fn long_summary() -> String {
    "Hardened steel lasts with carbon fibre filament. ".repeat(90)
}

/// What the Chat heard and replied: each turn's input, and its tool replies.
#[derive(Default)]
struct Heard {
    inputs: Vec<String>,
    replies: Vec<String>,
}

/// What the fused pair's Codex worker does.
#[derive(Clone, Copy, PartialEq)]
enum Codex {
    Reports,
    Stops,
    Fails,
}

/// The fused Chat's script: it fuses, reads A's long report back, and answers.
/// `codex_waits`: the Codex worker reports only once this is set (else it waits for its stop).
fn fusing(heard: Arc<Mutex<Heard>>, codex_waits: Arc<AtomicBool>, codex: Codex) -> Script {
    script(move |turn| {
        let (heard, codex_waits) = (heard.clone(), codex_waits.clone());
        async move {
            if turn.chat().is_none() && !turn.is_orchestrator() {
                if turn.provider == ProviderKind::Codex {
                    if codex == Codex::Fails {
                        return Reply::failed("the research tool broke");
                    }
                    if codex == Codex::Stops || !codex_waits.load(Ordering::SeqCst) {
                        turn.stopped().await;
                        return Reply::text("");
                    }
                    turn.call(
                        "submit_report",
                        json!({"summary": "Hardened steel, about 40 EUR."}),
                    )
                    .await;
                } else {
                    turn.call("submit_report", json!({"summary": long_summary()}))
                        .await;
                }
                return Reply::text("Reported.");
            }
            heard.lock().unwrap().inputs.push(turn.input.clone());
            if turn.input.contains("[report task-") {
                // Only the tasks whose report came in: one vendor or a lone answer has one.
                let tasks: Vec<String> = turn
                    .input
                    .split("[report task-")
                    .skip(1)
                    .map(|rest| {
                        let n: String = rest.chars().take_while(char::is_ascii_digit).collect();
                        format!("task-{n}")
                    })
                    .collect();
                for task in &tasks {
                    let read = turn.call("read_report", json!({"task": task})).await;
                    assert!(!read.is_error, "{}", read.text);
                    heard.lock().unwrap().replies.push(read.text);
                }
                // A session's fused answer doesn't rerun: its rerun would say what ran it (a
                // Chat has no rerun_task).
                if turn.is_orchestrator() && turn.input.contains("[fused answers]") {
                    let rerun = turn
                        .call("rerun_task", json!({"task": tasks[0], "why": "no sources"}))
                        .await;
                    assert!(rerun.text.contains("of a fused pair"), "{}", rerun.text);
                    heard.lock().unwrap().replies.push(rerun.text);
                }
                return Reply::text(ANSWER);
            }
            // A worker's stop note or a silence alert: nothing to add.
            if !turn.input.contains("Which nozzle") {
                return Reply::text("[quiet]");
            }
            let reply = turn
                .call(
                    "delegate_task",
                    json!({"effort": "medium", "title": "Nozzle for carbon fibre", "kind": "research",
                           "fuse": true, "spec": "Which nozzle should I buy for carbon fibre PETG?"}),
                )
                .await;
            assert!(!reply.is_error, "{}", reply.text);
            heard.lock().unwrap().replies.push(reply.text);
            Reply::text("[quiet]")
        }
    })
}

/// A Chat fuses a costly question: one worker per vendor, both reports in one message as A and
/// B with no vendor or model named (read_report and the stored full report too), and the
/// answer with no Confidence or Disagreement lines (Q86).
#[tokio::test]
async fn a_chat_fuses_a_costly_question_blind() {
    let heard: Arc<Mutex<Heard>> = Arc::default();
    let flow = Flow::start(
        "fusion-blind",
        Options::default(),
        fusing(
            heard.clone(),
            Arc::new(AtomicBool::new(true)),
            Codex::Reports,
        ),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(
        &chat,
        "Which nozzle should I buy for carbon fibre PETG? It's 40 EUR.",
    )
    .await;
    let board = answered(&flow, &chat).await;
    let mut providers: Vec<ProviderKind> = board
        .tasks
        .values()
        .map(|task| task.route.choice.provider)
        .collect();
    providers.sort_by_key(|provider| provider.label());
    assert_eq!(providers, vec![ProviderKind::Claude, ProviderKind::Codex]);
    let mut labels: Vec<String> = board
        .tasks
        .values()
        .map(|task| {
            assert_eq!(task.kind, TaskKind::Research);
            let fusion = task.fusion.as_ref().expect("a fused task");
            assert!(fusion.delivered);
            fusion.label.clone()
        })
        .collect();
    labels.sort();
    assert_eq!(labels, vec!["A", "B"]);
    let heard = std::mem::take(&mut *heard.lock().unwrap());
    let reports: Vec<&String> = heard
        .inputs
        .iter()
        .filter(|input| input.contains("[report task-"))
        .collect();
    assert_eq!(
        reports.len(),
        1,
        "both reports in one message: {reports:#?}"
    );
    let reports = reports[0];
    assert_eq!(reports.matches("[report task-").count(), 2, "{reports}");
    assert!(reports.find("answer A").unwrap() < reports.find("answer B").unwrap());
    assert!(reports.contains("[fused answers]"));
    assert!(reports.contains("name where they disagree"));
    // How the answer shows the judging (Q86): no report on it, a disagreement only where it counts.
    assert!(reports.contains(crate::manager::prompts::FUSED_ANSWER));
    assert!(reports.contains("never write `Confidence:` or `Disagreement:` lines"));
    assert!(!reports.contains("`Disagreement: none`"));
    blind(reports);
    for reply in &heard.replies {
        blind(reply);
    }
    assert!(heard.replies[0].contains("answer A") && heard.replies[0].contains("answer B"));
    // The long one is read whole from its stored report, blind too.
    assert!(
        heard.replies[1..]
            .iter()
            .any(|reply| reply.contains(&long_summary()[..200])),
        "{:#?}",
        heard.replies
    );
    for task in board.tasks.values() {
        for artifact in &task.report.as_ref().unwrap().artifacts {
            let bytes = flow
                .core
                .store()
                .blobs()
                .get(artifact.id.parse().unwrap())
                .await
                .unwrap()
                .unwrap();
            blind(&String::from_utf8_lossy(&bytes));
        }
    }
    let answer = shown(&flow, &chat).await;
    assert_eq!(answer, vec![ANSWER.to_owned()]);
    flow.stop().await;
}

/// A session's thread fuses the same way: both reports in one message, blind, and its answer.
#[tokio::test]
async fn a_session_fuses_a_costly_question_too() {
    let heard: Arc<Mutex<Heard>> = Arc::default();
    let flow = Flow::start(
        "fusion-session",
        Options::default(),
        fusing(
            heard.clone(),
            Arc::new(AtomicBool::new(true)),
            Codex::Reports,
        ),
    )
    .await;
    let id = flow.conversation.clone();
    flow.say_to(&id, "Which nozzle should I buy for carbon fibre PETG?")
        .await;
    let board = answered(&flow, &id).await;
    assert_eq!(board.tasks.len(), 2);
    assert!(
        board
            .tasks
            .values()
            .all(|task| task.fusion.as_ref().is_some_and(|fusion| fusion.delivered))
    );
    let heard = std::mem::take(&mut *heard.lock().unwrap());
    let reports: Vec<&String> = heard
        .inputs
        .iter()
        .filter(|input| input.contains("[report task-"))
        .collect();
    assert_eq!(reports.len(), 1, "{:#?}", heard.inputs);
    assert_eq!(reports[0].matches("[report task-").count(), 2);
    assert!(reports[0].contains("[fused answers]"));
    blind(reports[0]);
    // Its rerun is refused, blind too.
    assert!(
        heard
            .replies
            .iter()
            .any(|reply| reply.contains("of a fused pair")),
        "{:#?}",
        heard.replies
    );
    for reply in &heard.replies {
        blind(reply);
    }
    assert_eq!(shown(&flow, &id).await, vec![ANSWER.to_owned()]);
    flow.stop().await;
}

/// With one vendor switched off there is no pair: one worker, a plain report, and nothing
/// that says fusion.
#[tokio::test]
async fn one_vendor_runs_one_worker_and_says_nothing_of_fusion() {
    let heard: Arc<Mutex<Heard>> = Arc::default();
    let flow = Flow::start(
        "fusion-one-vendor",
        Options::default(),
        fusing(
            heard.clone(),
            Arc::new(AtomicBool::new(true)),
            Codex::Reports,
        ),
    )
    .await;
    let mut settings = flow.core.settings();
    settings.disabled_providers.push(ProviderKind::Codex);
    flow.core.update_settings(settings).await.unwrap();
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Which nozzle should I buy for carbon fibre PETG?")
        .await;
    let board = answered(&flow, &chat).await;
    assert_eq!(board.tasks.len(), 1);
    let task = board.tasks.values().next().unwrap();
    assert_eq!(task.route.choice.provider, ProviderKind::Claude);
    assert!(task.fusion.is_none());
    let heard = std::mem::take(&mut *heard.lock().unwrap());
    for text in heard.inputs.iter().chain(&heard.replies) {
        for word in [
            "answer A",
            "answer B",
            "[fused answers]",
            "[one answer]",
            "fuse",
        ] {
            assert!(!text.contains(word), "{word} in: {text}");
        }
    }
    flow.stop().await;
}

/// A restart between the two reports: the first stays held, and the second, after the
/// restart, brings both in one message.
#[tokio::test]
async fn a_restart_between_the_reports_still_brings_both_together() {
    let heard: Arc<Mutex<Heard>> = Arc::default();
    let codex_waits = Arc::new(AtomicBool::new(false));
    let mut flow = Flow::start(
        "fusion-restart",
        Options::default(),
        fusing(heard.clone(), codex_waits.clone(), Codex::Reports),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Which nozzle should I buy for carbon fibre PETG?")
        .await;
    flow.until_in(&chat, "the first report, held", |board| {
        board.tasks.values().any(|task| {
            task.report.is_some() && task.fusion.as_ref().is_some_and(|fusion| !fusion.delivered)
        })
    })
    .await;
    assert!(
        !heard
            .lock()
            .unwrap()
            .inputs
            .iter()
            .any(|input| input.contains("[report task-"))
    );
    codex_waits.store(true, Ordering::SeqCst);
    flow.restart().await;
    let board = answered(&flow, &chat).await;
    assert!(
        board
            .tasks
            .values()
            .all(|task| task.fusion.as_ref().is_some_and(|fusion| fusion.delivered))
    );
    let heard = std::mem::take(&mut *heard.lock().unwrap());
    let reports: Vec<&String> = heard
        .inputs
        .iter()
        .filter(|input| input.contains("[report task-"))
        .collect();
    assert_eq!(reports.len(), 1, "{reports:#?}");
    assert_eq!(reports[0].matches("[report task-").count(), 2);
    blind(reports[0]);
    flow.stop().await;
}

/// The Codex worker is stopped before it reports: the held answer goes alone, with one plain
/// line, and the Chat answers from it.
#[tokio::test]
async fn a_partner_stopped_without_a_report_leaves_the_other_to_go_alone() {
    let heard: Arc<Mutex<Heard>> = Arc::default();
    let flow = Flow::start(
        "fusion-alone",
        Options::default(),
        fusing(
            heard.clone(),
            Arc::new(AtomicBool::new(false)),
            Codex::Stops,
        ),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Which nozzle should I buy for carbon fibre PETG?")
        .await;
    let board = flow
        .until_in(&chat, "the first report, held", |board| {
            board.tasks.values().any(|task| {
                task.report.is_some()
                    && task.fusion.as_ref().is_some_and(|fusion| !fusion.delivered)
            })
        })
        .await;
    let codex = board
        .tasks
        .values()
        .find(|task| task.route.choice.provider == ProviderKind::Codex)
        .unwrap()
        .id
        .clone();
    flow.manager.stop_task_by_user(codex).await.unwrap();
    answered(&flow, &chat).await;
    let heard = std::mem::take(&mut *heard.lock().unwrap());
    let reports: Vec<&String> = heard
        .inputs
        .iter()
        .filter(|input| input.contains("[report task-"))
        .collect();
    assert_eq!(reports.len(), 1, "{:#?}", heard.inputs);
    assert_eq!(reports[0].matches("[report task-").count(), 1);
    assert!(reports[0].contains("[one answer]"));
    assert!(!reports[0].contains("[fused answers]"));
    blind(reports[0]);
    flow.stop().await;
}

/// The Codex worker fails: the other answer goes alone, and nothing reruns the failed one.
#[tokio::test]
async fn a_partner_that_fails_leaves_the_other_to_go_alone() {
    let heard: Arc<Mutex<Heard>> = Arc::default();
    let flow = Flow::start(
        "fusion-fails",
        Options::default(),
        fusing(
            heard.clone(),
            Arc::new(AtomicBool::new(false)),
            Codex::Fails,
        ),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Which nozzle should I buy for carbon fibre PETG?")
        .await;
    let board = answered(&flow, &chat).await;
    assert_eq!(board.tasks.len(), 2, "no rerun of the failed one");
    let heard = std::mem::take(&mut *heard.lock().unwrap());
    let reports: Vec<&String> = heard
        .inputs
        .iter()
        .filter(|input| input.contains("[report task-"))
        .collect();
    assert_eq!(reports.len(), 1, "{:#?}", heard.inputs);
    assert_eq!(reports[0].matches("[report task-").count(), 1);
    assert!(reports[0].contains("[one answer]"));
    blind(reports[0]);
    assert_eq!(shown(&flow, &chat).await, vec![ANSWER.to_owned()]);
    flow.stop().await;
}

/// A fused brief keeps what `delegate_task` was given: both workers get the user's file and
/// the quality floor asked for.
#[tokio::test]
async fn a_fused_pair_keeps_the_files_and_quality_asked_for() {
    let given: Arc<Mutex<String>> = Arc::default();
    let flow = Flow::start(
        "fusion-inputs",
        Options::default(),
        script({
            let given = given.clone();
            move |turn| {
                let given = given.clone();
                async move {
                    if turn.chat().is_none() && !turn.is_orchestrator() {
                        turn.call("submit_report", json!({"summary": "Hardened steel."}))
                            .await;
                        return Reply::text("Reported.");
                    }
                    if turn.input.contains("[report task-") {
                        return Reply::text(ANSWER);
                    }
                    if !turn.input.contains("Which nozzle") {
                        return Reply::text("[quiet]");
                    }
                    let id = given.lock().unwrap().clone();
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Nozzle for my printer", "kind": "research",
                                   "fuse": true, "quality": "high", "attachments": [id],
                                   "spec": "Which nozzle fits the printer in the attached spec sheet?"}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    Reply::text("[quiet]")
                }
            }
        }),
    )
    .await;
    let chat = new_chat(&flow).await;
    let sheet = flow
        .core
        .add_attachment(
            "printer-spec.txt".into(),
            "text/plain".into(),
            b"Hotend: 0.4 mm, M6 thread.".to_vec(),
            false,
        )
        .await
        .unwrap();
    *given.lock().unwrap() = sheet.id.clone();
    flow.manager
        .send_message(
            chat.clone(),
            "Which nozzle should I buy for carbon fibre PETG?".into(),
            vec![sheet.clone()],
            Vec::new(),
            false,
            None,
        )
        .await
        .unwrap();
    let board = answered(&flow, &chat).await;
    assert_eq!(board.tasks.len(), 2);
    for task in board.tasks.values() {
        assert!(task.fusion.is_some());
        let names: Vec<&str> = task.attachments.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, vec!["printer-spec.txt"]);
        assert_eq!(task.floor, brigadier_router::QualityTier::Frontier);
    }
    flow.stop().await;
}

/// #378: both of a fused pair write the same file name. Each report gives the link to its own
/// copy, blind, and the answer's links open the right one.
#[tokio::test]
async fn a_fused_pair_s_files_of_one_name_each_open_from_the_answer() {
    let heard: Arc<Mutex<String>> = Arc::default();
    let flow = Flow::start(
        "fusion-files",
        Options::default(),
        script({
            let heard = heard.clone();
            move |turn| {
                let heard = heard.clone();
                async move {
                    if turn.chat().is_none() && !turn.is_orchestrator() {
                        let price = match turn.provider {
                            ProviderKind::Claude => "25",
                            ProviderKind::Codex => "40",
                        };
                        std::fs::create_dir_all(turn.cwd.join("outputs")).unwrap();
                        std::fs::write(
                            turn.cwd.join("outputs/compare.csv"),
                            format!("nozzle,price\nhardened steel,{price}\n"),
                        )
                        .unwrap();
                        turn.call(
                            "submit_report",
                            json!({"summary": "The table is in outputs/compare.csv."}),
                        )
                        .await;
                        return Reply::text("Reported.");
                    }
                    if turn.input.contains("[report task-") {
                        *heard.lock().unwrap() = turn.input.clone();
                        let links: Vec<&str> = turn
                            .input
                            .split("link it as ")
                            .skip(1)
                            .filter_map(|rest| rest.split_whitespace().next())
                            .collect();
                        return Reply::text(format!("The tables: {}.", links.join(" and ")));
                    }
                    if !turn.input.contains("Which nozzle") {
                        return Reply::text("[quiet]");
                    }
                    let reply = turn
                        .call(
                            "delegate_task",
                            json!({"effort": "medium", "title": "Nozzle prices", "kind": "research",
                                   "fuse": true, "spec": "Compare nozzle prices in a CSV."}),
                        )
                        .await;
                    assert!(!reply.is_error, "{}", reply.text);
                    Reply::text("[quiet]")
                }
            }
        }),
    )
    .await;
    let chat = new_chat(&flow).await;
    flow.say_to(&chat, "Which nozzle is cheapest? Make me a table.")
        .await;
    let board = answered(&flow, &chat).await;
    let input = heard.lock().unwrap().clone();
    blind(&input);
    let answer = shown(&flow, &chat).await.join("\n");
    let mut prices = Vec::new();
    for task in board.tasks.values() {
        let link = format!("outputs/task-{}/compare.csv", task.number);
        assert!(
            input.contains(&format!("link it as [compare.csv]({link})")),
            "{input}"
        );
        assert!(answer.contains(&link), "{answer}");
        let stored = flow
            .manager
            .stored_output(&chat, &link, None)
            .await
            .unwrap()
            .expect("the answer's link opens a stored copy");
        assert_eq!(stored.task_id, task.id, "{link} opens its own task's copy");
        let bytes = flow
            .core
            .store()
            .blobs()
            .get(stored.artifact.id.parse().unwrap())
            .await
            .unwrap()
            .unwrap();
        prices.push(String::from_utf8(bytes).unwrap());
    }
    prices.sort();
    assert_eq!(
        prices,
        [
            "nozzle,price\nhardened steel,25\n",
            "nozzle,price\nhardened steel,40\n"
        ]
    );
    flow.stop().await;
}
