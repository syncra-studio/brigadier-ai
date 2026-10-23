//! STRIP-DOWN-PLAN S30: sessions message each other and read every project. `brigadier send
//! <session>` puts a message before another session's CLI as that session's, mid-turn like a
//! worker's; `brigadier wait <session>` returns its turn end; `sessions --all`, `show` and
//! `read` reach every project. A message is kept before it is sent and reaches its CLI once,
//! across a crash before it was queued, before its CLI took it and after.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use brigadier_cli::protocol::{CliCall, CliReply, CliRequest, GRANT_ENV, PROTOCOL, SESSION_ENV};
use brigadier_providers::ProviderKind;
use serde_json::Value;

use super::cli_tests::{brigadier, wait};
use super::{FakeBehavior, Flow, Options, Reply, Script, Turn};
use crate::manager::session_messages::{FILE, HEADER};
use crate::model::{ConversationId, PermissionLevel};

#[derive(Default)]
struct Seen {
    /// Each session's turns' inputs, in order.
    inputs: Mutex<Vec<(ConversationId, String)>>,
    /// Each session's `brigadier` caller: its session and grant.
    callers: Mutex<HashMap<ConversationId, (String, String)>>,
    /// What `brigadier` printed in a session's turn, by what the turn asked.
    printed: Mutex<HashMap<String, Value>>,
    /// Lets a holding turn end.
    release: AtomicBool,
}

impl Seen {
    fn inputs_of(&self, id: &ConversationId) -> Vec<String> {
        self.inputs
            .lock()
            .unwrap()
            .iter()
            .filter(|(of, _)| of == id)
            .map(|(_, input)| input.clone())
            .collect()
    }

    /// How many inputs of session `id` carry `text`.
    fn took(&self, id: &ConversationId, text: &str) -> usize {
        self.inputs_of(id)
            .iter()
            .filter(|input| input.contains(text))
            .count()
    }

    fn printed(&self, what: &str) -> Value {
        self.printed.lock().unwrap()[what].clone()
    }
}

fn script(seen: Arc<Seen>) -> Script {
    Arc::new(move |turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if turn.worker().is_some() {
                return Reply::text("Finished it.");
            }
            let Some(id) = turn.conversation() else {
                return Reply::text("ok");
            };
            session(turn, id, seen).await
        })
    })
}

/// A session: `Hold.` keeps its turn running until released (or its CLI closes);
/// `Hold-steer.` until a message is steered in; `Spawn.` starts a worker; `Message <id> then
/// wait` sends session `id` a message asking it to answer, then waits for its turn end; a
/// session's message asking for an answer is answered.
async fn session(turn: Turn, id: ConversationId, seen: Arc<Seen>) -> Reply {
    seen.inputs
        .lock()
        .unwrap()
        .push((id.clone(), turn.input.clone()));
    seen.callers.lock().unwrap().insert(
        id,
        (
            turn.env(SESSION_ENV).unwrap().to_owned(),
            turn.env(GRANT_ENV).unwrap().to_owned(),
        ),
    );
    let said = turn.input.clone();
    if said.contains("Hold-steer.") {
        turn.until_steered().await;
        return Reply::text("Held.");
    }
    if said.contains("Hold.") {
        while !seen.release.load(Ordering::SeqCst) && !turn.stops.0.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        return Reply::text("Held.");
    }
    if said.contains("Fail.") {
        return Reply::failed("The API said no.");
    }
    if said.contains("Spawn.") {
        turn.start_worker("Helper", "Helper.").await;
        return Reply::text("Spawned.");
    }
    if let Some(rest) = said.strip_prefix("Message ") {
        let to = rest.split_whitespace().next().unwrap().to_owned();
        let sent = brigadier(&turn, send(&to, "Hello. Answer back.")).await;
        seen.printed.lock().unwrap().insert("send".into(), sent);
        let waited = brigadier(&turn, wait(&[to.as_str()], Some(60))).await;
        seen.printed.lock().unwrap().insert("wait".into(), waited);
        return Reply::text("Messaged.");
    }
    if said.starts_with(HEADER) && said.contains("Answer back.") {
        let from = said.split(") · ").next().unwrap();
        let from = &from[from.rfind(" (").unwrap() + 2..];
        let sent = brigadier(&turn, send(from, "Thanks from the other side.")).await;
        seen.printed.lock().unwrap().insert("answer".into(), sent);
        return Reply::text("Answered.");
    }
    Reply::text("Got it.")
}

fn send(id: &str, text: &str) -> CliRequest {
    CliRequest::Send {
        id: id.to_owned(),
        text: Some(text.to_owned()),
        answer: None,
    }
}

async fn flow(name: &str, refuse_steers: bool) -> (Flow, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let options = Options {
        permission: PermissionLevel::Full,
        behavior: Arc::new(FakeBehavior {
            session_file: true,
            take_in_steers: true,
            refuse_steers,
            ..FakeBehavior::default()
        }),
        ..Options::default()
    };
    let flow = Flow::start(name, options, script(seen.clone())).await;
    (flow, seen)
}

/// Session `id`'s `brigadier` call from outside its turns.
async fn call(flow: &Flow, seen: &Seen, id: &ConversationId, request: CliRequest) -> CliReply {
    let (session, grant) = seen.callers.lock().unwrap()[id].clone();
    let caller = flow.manager.cli_caller(&session, &grant).unwrap();
    flow.manager
        .cli_call(
            &caller,
            CliCall {
                protocol: PROTOCOL,
                request,
            },
        )
        .await
}

fn ok(reply: CliReply) -> Value {
    match reply {
        CliReply::Ok { output } => output,
        CliReply::Error { error } => panic!("brigadier: {error:?}"),
    }
}

fn refused(reply: CliReply) -> String {
    match reply {
        CliReply::Ok { output } => panic!("not refused: {output:#}"),
        CliReply::Error { error } => error.message,
    }
}

/// The message lines of session `id`'s messages' file: (id, seen).
fn kept(flow: &Flow, id: &ConversationId) -> Vec<(String, bool)> {
    let mut found: Vec<(String, bool)> = flow.manager.session_messages_of(id).into_iter().collect();
    found.sort();
    found
}

fn mail_file(flow: &Flow, id: &ConversationId) -> std::path::PathBuf {
    flow.manager.session_dir(id).record().join(FILE)
}

/// Two sessions in different projects, each having taken a turn so its caller is known; the
/// second's id.
async fn two_sessions(flow: &Flow, seen: &Seen) -> ConversationId {
    flow.say("Hi.").await;
    flow.settled().await;
    let other = flow
        .add_session("Other", Some("other"), ProviderKind::Claude)
        .await;
    flow.say_to(&other, "Hi.").await;
    flow.settled_in(&other).await;
    assert!(seen.callers.lock().unwrap().contains_key(&other));
    other
}

/// The plan's first: A sends B a message mid-turn; B's CLI is given it as from A (its title
/// and id, never as its user's); B's `send` back reaches A; `wait <B>` returns B's turn end
/// with its last message.
#[tokio::test]
async fn a_session_messages_another_mid_turn_and_hears_its_answer() {
    let (flow, seen) = flow("sessions-message", false).await;
    let a = flow.conversation.clone();
    let b = two_sessions(&flow, &seen).await;
    flow.say_to(&b, "Hold-steer.").await;
    super::eventually("B's turn to run", || seen.took(&b, "Hold-steer.") == 1).await;
    flow.say(&format!("Message {} then wait", b.0)).await;
    super::eventually("A's wait to end", || {
        seen.printed.lock().unwrap().contains_key("wait")
    })
    .await;
    flow.settled().await;
    flow.settled_in(&b).await;

    let sent = seen.printed("send");
    assert_eq!(sent["queued"], true, "{sent:#}");
    let message = sent["message"].as_str().unwrap().to_owned();
    assert!(message.starts_with("sm-"), "{sent:#}");

    let title = flow.core.conversation(&a).unwrap().title;
    let given: Vec<String> = seen
        .inputs_of(&b)
        .into_iter()
        .filter(|input| input.contains(&message))
        .collect();
    assert_eq!(given.len(), 1, "{:#?}", seen.inputs_of(&b));
    assert!(
        given[0].starts_with(&format!(
            "[message from session \"{title}\" ({}) · {message}]\nHello. Answer back.",
            a.0
        )),
        "{}",
        given[0]
    );

    let answered = seen.printed("answer");
    assert!(answered["message"].as_str().unwrap().starts_with("sm-"));
    super::eventually("A to take B's answer", || {
        seen.took(&a, "Thanks from the other side.") == 1
    })
    .await;
    let back: Vec<String> = seen
        .inputs_of(&a)
        .into_iter()
        .filter(|input| input.contains("Thanks from the other side."))
        .collect();
    assert!(back[0].contains(&format!("({}) · sm-", b.0)), "{}", back[0]);

    let waited = seen.printed("wait");
    assert_eq!(waited["timedOut"], false, "{waited:#}");
    let events = waited["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{waited:#}");
    assert_eq!(events[0]["event"], "turn-ended");
    assert_eq!(events[0]["message"], "Answered.");
    assert_eq!(kept(&flow, &b), [(message, true)]);
    flow.stop().await;
}

/// `wait <session>` returns that turn's own end: a turn that failed before it answered says
/// its error, never an earlier turn's answer.
#[tokio::test]
async fn a_session_wait_returns_the_turns_own_end() {
    let (flow, seen) = flow("sessions-wait-own-turn", false).await;
    let a = flow.conversation.clone();
    let b = two_sessions(&flow, &seen).await;
    let waited = {
        let (flow, seen, a, b) = (&flow, &seen, a.clone(), b.0.clone());
        async move { ok(call(flow, seen, &a, wait(&[b.as_str()], Some(30))).await) }
    };
    let failing = async {
        tokio::time::sleep(Duration::from_millis(200)).await;
        flow.say_to(&b, "Fail.").await;
    };
    let (waited, ()) = tokio::join!(waited, failing);
    let events = waited["events"].as_array().unwrap();
    assert_eq!(events.len(), 1, "{waited:#}");
    assert_eq!(events[0]["event"], "failed", "{waited:#}");
    let message = events[0]["message"].as_str().unwrap_or_default();
    assert!(!message.contains("Got it."), "{waited:#}");
    assert!(message.contains("The API said no."), "{waited:#}");
    flow.stop().await;
}

/// The plan's second: `sessions --all` lists another project's session with its project, and
/// `read` and `show` reach it; plain `sessions` keeps to the caller's project.
#[tokio::test]
async fn sessions_all_lists_every_project_and_read_reaches_them() {
    let (flow, seen) = flow("sessions-all", false).await;
    let a = flow.conversation.clone();
    let b = two_sessions(&flow, &seen).await;
    let short = &b.0[b.0.len() - 8..];

    let all = ok(call(&flow, &seen, &a, CliRequest::Sessions { all: true }).await);
    let rows = all["sessions"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|row| row["id"] == short)
        .unwrap_or_else(|| panic!("{all:#}"));
    assert_eq!(row["project"], "other");
    assert!(rows.iter().any(|row| row["caller"] == true));

    let own = ok(call(&flow, &seen, &a, CliRequest::Sessions { all: false }).await);
    assert!(
        own["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|row| row["id"] != short),
        "{own:#}"
    );

    let read = ok(call(
        &flow,
        &seen,
        &a,
        CliRequest::Read {
            id: short.to_owned(),
            last: None,
            screen: false,
        },
    )
    .await);
    assert!(read.to_string().contains("Got it."), "{read:#}");
    ok(call(&flow, &seen, &a, CliRequest::Show { id: short.into() }).await);
    flow.stop().await;
}

/// The plan's third: another session's worker is out of reach (send, wait and close are
/// refused); a session can't message or wait on itself, and wait takes workers or sessions,
/// not both.
#[tokio::test]
async fn another_sessions_worker_is_out_of_reach() {
    let (flow, seen) = flow("sessions-refused", false).await;
    let a = flow.conversation.clone();
    let b = two_sessions(&flow, &seen).await;
    flow.say_to(&b, "Spawn.").await;
    flow.settled_in(&b).await;
    let board = flow.board_of(&b).await;
    let worker = board.tasks.keys().next().unwrap().0.clone();

    let refusal = refused(call(&flow, &seen, &a, send(&worker, "Hi.")).await);
    assert!(refusal.contains("another session's worker"), "{refusal}");
    let refusal = refused(call(&flow, &seen, &a, wait(&[worker.as_str()], Some(1))).await);
    assert!(refusal.contains("another session's worker"), "{refusal}");
    let refusal = refused(call(&flow, &seen, &a, CliRequest::Close { id: worker }).await);
    assert!(refusal.contains("another session's worker"), "{refusal}");

    let refusal = refused(call(&flow, &seen, &a, send(&a.0, "Me.")).await);
    assert!(refusal.contains("is this session"), "{refusal}");
    let refusal = refused(call(&flow, &seen, &a, CliRequest::Close { id: b.0.clone() }).await);
    assert!(refusal.contains("is a session"), "{refusal}");

    flow.say("Spawn.").await;
    flow.settled().await;
    let own = flow.board().await.tasks.keys().next().unwrap().0.clone();
    let refusal = refused(
        call(
            &flow,
            &seen,
            &a,
            wait(&[own.as_str(), b.0.as_str()], Some(1)),
        )
        .await,
    );
    assert!(refusal.contains("not both"), "{refusal}");
    flow.stop().await;
}

/// Kept, then the daemon died before anything sent it: it reaches the recipient's CLI once
/// after the restart, and not again after another.
#[tokio::test]
async fn a_message_kept_before_it_was_queued_reaches_its_session_once_after_a_crash() {
    let (mut flow, seen) = flow("sessions-crash-kept", false).await;
    let a = flow.conversation.clone();
    let b = two_sessions(&flow, &seen).await;
    let text = format!("{HEADER}Main\" ({}) · sm-0123456789ab]\nKept only.", a.0);
    let line = serde_json::json!({
        "line": "message", "id": "sm-0123456789ab", "from": a.0, "text": text, "at_ms": 1,
    });
    std::fs::write(mail_file(&flow, &b), format!("{line}\n")).unwrap();

    flow.crash().await;
    super::eventually("B to take the kept message", || {
        seen.took(&b, "Kept only.") == 1
    })
    .await;
    flow.settled_in(&b).await;
    assert_eq!(kept(&flow, &b), [("sm-0123456789ab".into(), true)]);

    flow.crash().await;
    flow.settled_in(&b).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(seen.took(&b, "Kept only."), 1, "{:#?}", seen.inputs_of(&b));
    flow.stop().await;
}

/// Queued while the recipient worked and its CLI refused the steer, then the daemon died:
/// it reaches the recipient once after the restart.
#[tokio::test]
async fn a_message_its_cli_never_took_reaches_it_once_after_a_crash() {
    let (mut flow, seen) = flow("sessions-crash-queued", true).await;
    let a = flow.conversation.clone();
    let b = two_sessions(&flow, &seen).await;
    flow.say_to(&b, "Hold.").await;
    super::eventually("B's turn to run", || seen.took(&b, "Hold.") == 1).await;
    let sent = ok(call(&flow, &seen, &a, send(&b.0, "Queued one.")).await);
    let message = sent["message"].as_str().unwrap().to_owned();
    assert_eq!(kept(&flow, &b), [(message.clone(), false)]);

    flow.crash().await;
    seen.release.store(true, Ordering::SeqCst);
    super::eventually("B to take the queued message", || {
        seen.took(&b, "Queued one.") == 1
    })
    .await;
    flow.settled_in(&b).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(seen.took(&b, "Queued one."), 1, "{:#?}", seen.inputs_of(&b));
    assert_eq!(kept(&flow, &b), [(message, true)]);
    flow.stop().await;
}

/// Its CLI took it mid-turn, then the daemon died: it is not sent again, even with the line
/// that said so lost, since the recipient's own session file holds it.
#[tokio::test]
async fn a_message_its_cli_took_is_not_sent_again_after_a_crash() {
    let (mut flow, seen) = flow("sessions-crash-taken", false).await;
    let a = flow.conversation.clone();
    let b = two_sessions(&flow, &seen).await;
    flow.say_to(&b, "Hold.").await;
    super::eventually("B's turn to run", || seen.took(&b, "Hold.") == 1).await;
    let sent = ok(call(&flow, &seen, &a, send(&b.0, "Steered one.")).await);
    let message = sent["message"].as_str().unwrap().to_owned();
    assert_eq!(sent["queued"], true);
    assert_eq!(kept(&flow, &b), [(message.clone(), true)]);

    flow.crash().await;
    seen.release.store(true, Ordering::SeqCst);
    flow.settled_in(&b).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(kept(&flow, &b), [(message.clone(), true)]);

    // The seen line lost: B's session file still holds it, so it is not sent again.
    let file = mail_file(&flow, &b);
    let only_message: String = std::fs::read_to_string(&file)
        .unwrap()
        .lines()
        .filter(|line| line.contains("\"line\":\"message\""))
        .map(|line| format!("{line}\n"))
        .collect();
    std::fs::write(&file, only_message).unwrap();
    assert_eq!(kept(&flow, &b), [(message.clone(), false)]);
    let before = seen.took(&b, "Steered one.");

    flow.crash().await;
    flow.settled_in(&b).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(kept(&flow, &b), [(message, true)]);
    assert_eq!(
        seen.took(&b, "Steered one."),
        before,
        "{:#?}",
        seen.inputs_of(&b)
    );
    flow.stop().await;
}
