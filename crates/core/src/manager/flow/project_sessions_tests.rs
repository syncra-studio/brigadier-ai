//! A project's sessions (Q73): each one task on its own worktree. A session starts another in
//! the background from its own base; sessions of one project work at once without sharing a
//! file, see each other on the board (no overlap or merge is pushed to them), and a queued
//! message may go to the sibling it belongs to.

use std::path::PathBuf;
use std::sync::Arc;

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{ConversationId, ConversationKind, Environment, Setup};

fn script<F, Fut>(f: F) -> Script
where
    F: Fn(Turn) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = Reply> + Send + 'static,
{
    Arc::new(move |turn| Box::pin(f(turn)))
}

/// A thread that commits `<name>.txt` on "Write <name>." in its own worktree. With `meet`,
/// the two "Write" turns wait for each other there, so they provably run at once.
fn writing(meet: Option<Arc<tokio::sync::Barrier>>) -> Script {
    script(move |turn| {
        let meet = meet.clone();
        async move {
            if !turn.is_orchestrator() {
                return Reply::text("Done.");
            }
            // The user's words come last (a fact before them may quote a tab's title).
            let Some(name) = turn
                .input
                .rsplit_once("Write ")
                .and_then(|(_, rest)| rest.split('.').next())
                .map(str::to_owned)
            else {
                return Reply::text("Noted.");
            };
            if let Some(meet) = &meet {
                meet.wait().await;
            }
            commit_in_workspace(
                &turn,
                &format!("{name}.txt"),
                "text\n",
                &format!("Add {name}"),
            );
            Reply::text(format!("{name} is in."))
        }
    })
}

/// A new, empty session started from `from`, as ⌘⏎ makes one (without its message).
pub(super) async fn sibling_of(flow: &Flow, from: &ConversationId) -> ConversationId {
    flow.manager
        .create_session_from(from, None)
        .await
        .unwrap()
        .id
}

pub(super) fn worktree(flow: &Flow, id: &ConversationId) -> (Option<PathBuf>, String, String) {
    match flow.core.conversation(id).unwrap().setup {
        Some(Setup::Session {
            environment:
                Environment::NewWorktree {
                    path, base, branch, ..
                },
            ..
        }) => (path.map(PathBuf::from), base, branch),
        other => panic!("not a worktree session: {other:?}"),
    }
}

/// A session starts another of its project on a new worktree from its own base; the two
/// work at once, each in its own folder on its own branch, and neither sees the other's file.
/// No workspace is made.
#[tokio::test]
async fn two_sessions_of_one_project_work_at_once_on_their_own_worktrees() {
    let meet = Arc::new(tokio::sync::Barrier::new(2));
    let flow = Flow::start("sessions-two", Options::default(), writing(Some(meet))).await;
    let first = flow.conversation.clone();
    let second = flow
        .manager
        .create_session_from(&first, None)
        .await
        .unwrap();
    assert_eq!(second.kind, ConversationKind::Session);
    assert_eq!(
        second.project_id,
        flow.core.conversation(&first).unwrap().project_id
    );
    let (_, base, branch) = worktree(&flow, &second.id);
    assert_eq!(base, "main", "from the first session's base");
    assert_ne!(branch, worktree(&flow, &first).2, "a branch of its own");

    let (one, two) = tokio::join!(
        flow.say_to(&first, "Write alpha."),
        flow.say_to(&second.id, "Write beta.")
    );
    let ((), ()) = (one, two);
    flow.settled_in(&first).await;
    flow.settled_in(&second.id).await;

    let (Some(alpha), _, alpha_branch) = worktree(&flow, &first) else {
        panic!("the first session's worktree");
    };
    let (Some(beta), _, beta_branch) = worktree(&flow, &second.id) else {
        panic!("the second session's worktree");
    };
    assert_ne!(alpha, beta, "two folders");
    assert!(alpha.join("alpha.txt").exists() && !alpha.join("beta.txt").exists());
    assert!(beta.join("beta.txt").exists() && !beta.join("alpha.txt").exists());
    let files = |branch: &str| git(&flow.repo, &["ls-tree", "-r", "--name-only", branch]);
    assert!(!files(&alpha_branch).contains("beta.txt"));
    assert!(!files(&beta_branch).contains("alpha.txt"));
    let catalog = serde_json::to_value(flow.core.catalog()).unwrap();
    assert!(catalog.get("workspaces").is_none(), "{catalog}");
    flow.stop().await;
}

/// ⌘⏎: a new session starts on the message in the background, on its own worktree, titled by
/// it; one started from a session on a local checkout starts from that checkout's branch.
#[tokio::test]
async fn a_background_session_starts_on_its_message() {
    let flow = Flow::start("sessions-start", Options::default(), writing(None)).await;
    let (session, _) = flow
        .manager
        .start_session(
            &flow.conversation,
            None,
            "Write gamma.".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(session.title, "Write gamma.", "titled by its message");
    flow.settled_in(&session.id).await;
    let (Some(path), _, _) = worktree(&flow, &session.id) else {
        panic!("the session's worktree");
    };
    assert!(path.join("gamma.txt").exists());
    assert_ne!(Some(path), worktree(&flow, &flow.conversation).0);

    // From a session on a local checkout: a worktree from its branch.
    let first = flow.core.conversation(&flow.conversation).unwrap();
    let Some(Setup::Session {
        repo,
        permission,
        orchestrator,
        ..
    }) = first.setup.clone()
    else {
        unreachable!()
    };
    git(&flow.repo, &["branch", "feature"]);
    let local = flow
        .manager
        .create_conversation(
            ConversationKind::Session,
            first.project_id.clone(),
            Some("Local".into()),
            Some(crate::model::SetupRequest::Session {
                repo,
                environment: crate::model::EnvironmentRequest::LocalCheckout {
                    branch: "feature".into(),
                    create_from: None,
                },
                permission,
                orchestrator,
                plan_mode: false,
            }),
        )
        .await
        .unwrap();
    let from_local = sibling_of(&flow, &local.id).await;
    assert_eq!(worktree(&flow, &from_local).1, "feature");
    flow.stop().await;
}

/// One session at a time works in a project's own checkout (#522): another is refused there,
/// whether it starts, gets its environment late or is restored, while one in a new worktree
/// starts; once the first is archived and cleaned up, one of two asking at once gets in.
#[tokio::test]
async fn one_session_works_in_the_checkout_at_once() {
    use crate::model::{EnvironmentRequest, SetupRequest};
    let flow = Flow::start("sessions-one-local", Options::default(), writing(None)).await;
    let first = flow.core.conversation(&flow.conversation).unwrap();
    let Some(Setup::Session {
        repo,
        permission,
        orchestrator,
        ..
    }) = first.setup.clone()
    else {
        unreachable!()
    };
    let project = first.project_id.clone();
    let local = || SetupRequest::Session {
        repo: repo.clone(),
        environment: EnvironmentRequest::LocalCheckout {
            branch: "main".into(),
            create_from: None,
        },
        permission,
        orchestrator: orchestrator.clone(),
        plan_mode: false,
    };
    let taken = "“Here” already works in this checkout.";
    let here = flow
        .manager
        .create_conversation(
            ConversationKind::Session,
            project.clone(),
            Some("Here".into()),
            Some(local()),
        )
        .await
        .unwrap();
    let refused = flow
        .manager
        .create_conversation(
            ConversationKind::Session,
            project.clone(),
            Some("Also here".into()),
            Some(local()),
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains(taken), "{refused}");
    flow.manager
        .create_conversation(
            ConversationKind::Session,
            project.clone(),
            Some("Elsewhere".into()),
            Some(SetupRequest::Session {
                repo: repo.clone(),
                environment: EnvironmentRequest::NewWorktree {
                    base: "main".into(),
                    branch: None,
                },
                permission,
                orchestrator: orchestrator.clone(),
                plan_mode: false,
            }),
        )
        .await
        .unwrap();

    // A session with no setup yet is refused the checkout as it gets one.
    let late = flow
        .manager
        .create_conversation(ConversationKind::Session, project.clone(), None, None)
        .await
        .unwrap();
    let refused = flow
        .manager
        .set_setup(
            late.id.clone(),
            Setup::Session {
                repo: repo.clone(),
                environment: Environment::LocalCheckout {
                    branch: "main".into(),
                },
                permission,
                orchestrator: orchestrator.clone(),
                workers_see_uncommitted: None,
                plan_mode: false,
            },
        )
        .await
        .unwrap_err();
    assert!(refused.to_string().contains(taken), "{refused}");

    // The checkout is free once the archive's cleanup is done; of two at once, one gets it.
    flow.manager.archive(here.id.clone()).await.unwrap();
    super::eventually("the archive cleaned up", || {
        !flow.core.conversation(&here.id).unwrap().cleanup_pending
    })
    .await;
    let (one, two) = tokio::join!(
        flow.manager.create_conversation(
            ConversationKind::Session,
            project.clone(),
            Some("One".into()),
            Some(local()),
        ),
        flow.manager.create_conversation(
            ConversationKind::Session,
            project.clone(),
            Some("Two".into()),
            Some(local()),
        ),
    );
    assert!(one.is_ok() != two.is_ok(), "{one:?} {two:?}");
    let refused = flow.manager.restore(here.id.clone()).await.unwrap_err();
    assert!(
        refused
            .to_string()
            .contains("already works in this checkout"),
        "{refused}"
    );
    flow.stop().await;
}

/// A background session whose message can't go leaves no empty session behind; a chat starts
/// none.
#[tokio::test]
async fn a_background_session_whose_message_fails_goes() {
    let flow = Flow::start("sessions-start-fails", Options::default(), writing(None)).await;
    let before = flow.core.catalog().conversations.len();
    assert!(
        flow.manager
            .start_session(
                &flow.conversation,
                None,
                "  ".into(),
                Vec::new(),
                Vec::new()
            )
            .await
            .is_err()
    );
    super::eventually("the empty session gone", || {
        flow.core.catalog().conversations.len() == before
    })
    .await;
    let chat = flow
        .manager
        .create_conversation(ConversationKind::Chat, None, None, None)
        .await
        .unwrap();
    assert!(
        flow.manager
            .start_session(&chat.id, None, "Write x.".into(), Vec::new(), Vec::new())
            .await
            .is_err()
    );
    flow.stop().await;
}

/// Whether the user's words, which come last in a turn's input, are `words`: a fact before
/// them may quote another tab's title.
fn said(turn: &Turn, words: &str) -> bool {
    turn.input.trim_end().ends_with(words)
}

pub(super) type Inputs = Arc<std::sync::Mutex<Vec<(PathBuf, String)>>>;

/// A thread that, besides [`writing`], commits `shared.txt` on "Share." and removes it on
/// "Unshare.", and merges on the user's click. Every thread input is logged by worktree.
pub(super) fn sharing(inputs: Inputs) -> Script {
    let write = writing(None);
    script(move |turn| {
        let (inputs, write) = (inputs.clone(), write.clone());
        async move {
            if !turn.is_orchestrator() {
                return Reply::text("Done.");
            }
            inputs
                .lock()
                .unwrap()
                .push((turn.cwd.clone(), turn.input.clone()));
            if turn.input.contains("[merge] The user clicked") {
                let reply = turn.call("finish_session", serde_json::json!({})).await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Merged.");
            }
            if said(&turn, "Unshare.") {
                let folder = &turn.cwd;
                git(folder, &["rm", "-q", "shared.txt"]);
                git(folder, &["commit", "-q", "-m", "Unshare"]);
                return Reply::text("Unshared.");
            }
            if said(&turn, "Edit main.") {
                let folder = &turn.cwd;
                std::fs::create_dir_all(folder.join("src")).unwrap();
                let text = format!("fn main() {{}} // {}\n", crate::now_ms());
                std::fs::write(folder.join("src/main.rs"), text).unwrap();
                git(folder, &["add", "src/main.rs"]);
                git(folder, &["commit", "-q", "-m", "Edit main"]);
                return Reply::text("Edited.");
            }
            if said(&turn, "Share.") {
                let text = format!("{} {}\n", turn.cwd.display(), crate::now_ms());
                commit_in_workspace(&turn, "shared.txt", &text, "Share");
                return Reply::text("Shared.");
            }
            write(turn).await
        }
    })
}

/// The thread inputs of the tab working in `folder` that hold `what`.
fn heard(inputs: &Inputs, folder: &std::path::Path, what: &str) -> usize {
    inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|(at, input)| at == folder && input.contains(what))
        .count()
}

/// How the next follow-up is routed (its `follow_up` id is filled in), what route_follow_up
/// answered, and the gate the routing turn waits at after it routed.
#[derive(Default)]
struct Routing {
    args: std::sync::Mutex<serde_json::Value>,
    follow_up: std::sync::Mutex<String>,
    results: std::sync::Mutex<Vec<String>>,
    gate: tokio::sync::Notify,
}

fn bash(status: brigadier_providers::ItemStatus) -> brigadier_providers::ProviderEvent {
    brigadier_providers::ProviderEvent::ToolCall {
        item_id: "bash-1".into(),
        name: "Bash".into(),
        input: Some(serde_json::json!({ "command": "cargo test --workspace" }).to_string()),
        status,
        output: (status != brigadier_providers::ItemStatus::InProgress).then(|| "ok".into()),
        kept: None,
    }
}

/// A thread that, besides [`writing`] and merging on the user's click, runs a long command on
/// "Run the tests.", routes the follow-up it gets meanwhile as `routing` says, and waits at
/// the gate. Every thread input is logged by worktree.
fn routing(inputs: Inputs, routing: Arc<Routing>) -> Script {
    let write = writing(None);
    script(move |turn| {
        let (inputs, write, routing) = (inputs.clone(), write.clone(), routing.clone());
        async move {
            if !turn.is_orchestrator() {
                return Reply::text("Done.");
            }
            inputs
                .lock()
                .unwrap()
                .push((turn.cwd.clone(), turn.input.clone()));
            if turn.input.contains("[merge] The user clicked") {
                let reply = turn.call("finish_session", serde_json::json!({})).await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Merged.");
            }
            if !said(&turn, "Run the tests.") {
                return write(turn).await;
            }
            turn.emit(bash(brigadier_providers::ItemStatus::InProgress))
                .await;
            // An earlier follow-up that joined on its own may have left its steer unread:
            // route the one this turn is for.
            let text = routing.follow_up.lock().unwrap().clone();
            let mut block = String::new();
            while let Some(next) = turn.steered().await {
                if next.contains("[follow-up ") && next.contains(&text) {
                    block = next;
                    break;
                }
            }
            let id = block
                .split("[follow-up ")
                .nth(1)
                .and_then(|rest| rest.split(']').next())
                .unwrap_or_default()
                .to_owned();
            let mut args = routing.args.lock().unwrap().clone();
            args["follow_up"] = serde_json::Value::String(id);
            let reply = turn.call("route_follow_up", args).await;
            assert!(!reply.is_error, "{}", reply.text);
            if reply.text.starts_with("It joins this work") {
                // The joined message, steered into this turn.
                turn.steered().await;
            }
            routing.results.lock().unwrap().push(reply.text);
            routing.gate.notified().await;
            turn.emit(bash(brigadier_providers::ItemStatus::Completed))
                .await;
            Reply::text("The tests pass.")
        }
    })
}

impl Routing {
    /// Runs one routed follow-up on `tab`: "Run the tests.", then `follow_up` while it works,
    /// routed by `args`. Returns route_follow_up's answer, with the turn still at the gate.
    async fn route(
        &self,
        flow: &Flow,
        tab: &ConversationId,
        follow_up: &str,
        args: serde_json::Value,
    ) -> String {
        *self.args.lock().unwrap() = args;
        *self.follow_up.lock().unwrap() = follow_up.to_owned();
        let before = self.results.lock().unwrap().len();
        flow.say_to(tab, "Run the tests.").await;
        flow.until_in(tab, "the command to run", |board| {
            board.orchestrator_steps.iter().any(|step| {
                matches!(&step.kind, crate::work::OrchestratorStepKind::Tool { status, .. } if *status == brigadier_providers::ItemStatus::InProgress)
            })
        })
        .await;
        flow.say_to(tab, follow_up).await;
        super::eventually("the follow-up routed", || {
            self.results.lock().unwrap().len() > before
        })
        .await;
        self.results.lock().unwrap()[before].clone()
    }

    /// Lets the routing turn end, and waits for what stayed queued to reach its next turn.
    async fn release(&self, flow: &Flow, tab: &ConversationId, inputs: &Inputs) {
        let folder = worktree(flow, tab).0.expect("the session's worktree");
        let waiting: Vec<_> = flow
            .board_of(tab)
            .await
            .queue
            .items
            .into_iter()
            .map(|item| {
                let before = heard(inputs, &folder, &item.text);
                (item.text, before)
            })
            .collect();
        self.gate.notify_one();
        flow.until_in(tab, "the queue to empty", |board| {
            board.queue.items.is_empty()
        })
        .await;
        // The queue empties before the next request is recorded as working. The old
        // requests can all be settled in that gap, before the script has read the message.
        for (text, before) in waiting {
            super::eventually("the queued follow-up's turn to start", || {
                heard(inputs, &folder, &text) > before
            })
            .await;
        }
        flow.settled_in(tab).await;
    }
}

fn last_queue(flow: &Flow, id: &ConversationId) -> crate::work::MessageQueue {
    let core = flow.core.clone();
    let id = id.clone();
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current()
            .block_on(async move { core.board(&id).await.unwrap().queue })
    })
}

/// Queued routes: the thread's choice is carried out as it says. join steers it in; wait
/// keeps it queued; new_session keeps it queued with the offer and starts nothing; session
/// sends it to the sibling as the user's message, with a notice here that Undo takes back;
/// session to a sibling that merged goes through with that fact; a session that isn't there
/// leaves it unsorted.
#[tokio::test(flavor = "multi_thread")]
async fn a_follow_up_joins_waits_offers_a_session_or_goes_to_a_sibling() {
    let inputs: Inputs = Arc::default();
    let routes = Arc::new(Routing::default());
    let flow = Flow::start(
        "sessions-routes",
        Options::default(),
        routing(inputs.clone(), routes.clone()),
    )
    .await;
    let a = flow.conversation.clone();
    let b = sibling_of(&flow, &a).await;
    flow.say_to(&a, "Write alpha.").await;
    flow.settled_in(&a).await;
    flow.say_to(&b, "Write beta.").await;
    flow.settled_in(&b).await;
    let (Some(folder_a), _, _) = worktree(&flow, &a) else {
        panic!("a's worktree")
    };
    let (Some(folder_b), _, _) = worktree(&flow, &b) else {
        panic!("b's worktree")
    };

    // join
    let joined = routes
        .route(
            &flow,
            &a,
            "Also check the docs.",
            serde_json::json!({ "route": "join" }),
        )
        .await;
    assert!(joined.starts_with("It joins this work"), "{joined}");
    assert!(last_queue(&flow, &a).items.is_empty());
    routes.release(&flow, &a, &inputs).await;

    // wait
    let waits = routes
        .route(
            &flow,
            &a,
            "Then bump the version.",
            serde_json::json!({ "route": "wait" }),
        )
        .await;
    assert!(waits.starts_with("It waits in the queue"), "{waits}");
    let queue = last_queue(&flow, &a);
    assert_eq!(queue.items.len(), 1);
    assert!(!queue.items[0].deciding && !queue.items[0].offer_tab);
    routes.release(&flow, &a, &inputs).await;

    // new_session: the offer, and no session started.
    let tabs = flow.core.catalog().conversations.len();
    let offered = routes
        .route(
            &flow,
            &a,
            "Draft the release notes.",
            serde_json::json!({ "route": "new_session" }),
        )
        .await;
    assert!(
        offered.contains("\"Open as a new session\" or \"Run here\""),
        "{offered}"
    );
    let queue = last_queue(&flow, &a);
    assert_eq!(queue.items.len(), 1);
    assert!(queue.items[0].offer_tab && !queue.items[0].deciding);
    assert_eq!(
        flow.core.catalog().conversations.len(),
        tabs,
        "no session started"
    );
    // The turn ends and the offer stays until the user picks; "Run here" sends it here.
    routes.gate.notify_one();
    flow.settled_in(&a).await;
    let queue = last_queue(&flow, &a);
    assert!(
        queue.items.len() == 1 && queue.items[0].offer_tab,
        "still offered"
    );
    let notes = "Draft the release notes.";
    let heard_before = heard(&inputs, &folder_a, notes);
    let requests_before = flow.board_of(&a).await.requests.len();
    flow.manager
        .run_queued_here(&a, &queue.items[0].id)
        .await
        .unwrap();
    // Its request too: the queue drops the item a write before the request shows.
    flow.until_in(&a, "the queued message to run", |board| {
        board.queue.items.is_empty() && board.requests.len() > requests_before
    })
    .await;
    flow.settled_in(&a).await;
    assert_eq!(
        heard(&inputs, &folder_a, notes),
        heard_before + 1,
        "ran here"
    );

    // A session that isn't there: a fact, and the follow-up stays the thread's to sort.
    let nowhere = routes
        .route(
            &flow,
            &a,
            "Also lint it.",
            serde_json::json!({ "route": "session", "session": "nope" }),
        )
        .await;
    assert!(
        nowhere.contains("No other session of this project is session nope"),
        "{nowhere}"
    );
    assert!(nowhere.contains(&b.0), "lists the sessions: {nowhere}");
    // Still unsorted: deciding, or already joined by itself after the judging wait.
    let queue = last_queue(&flow, &a);
    assert!(queue.moved.is_empty() && queue.items.iter().all(|item| item.deciding));
    routes.release(&flow, &a, &inputs).await;

    // session: it goes to B as the user's message; A keeps a notice.
    let message = "Rename the beta file.";
    let moved = routes
        .route(
            &flow,
            &a,
            message,
            serde_json::json!({ "route": "session", "session": b.0 }),
        )
        .await;
    assert!(moved.contains("It went to session"), "{moved}");
    assert!(moved.contains("notice with Undo"), "{moved}");
    assert!(!moved.contains("already merged"), "{moved}");
    let queue = last_queue(&flow, &a);
    assert!(queue.items.is_empty(), "{:?}", queue.items);
    assert_eq!(queue.moved.len(), 1);
    assert_eq!(queue.moved[0].to, b);
    assert_eq!(queue.moved[0].text, message);
    routes.release(&flow, &a, &inputs).await;
    flow.settled_in(&b).await;
    assert_eq!(heard(&inputs, &folder_b, message), 1, "B got it");
    assert_eq!(
        heard(&inputs, &folder_b, "whose thread sent it on to you"),
        0,
        "the message only, as the user wrote it"
    );
    let a_heard = heard(&inputs, &folder_a, message);

    // Undo: it is A's again; B is told nothing.
    let item = last_queue(&flow, &a).moved[0].id.clone();
    flow.manager.settle_moved(&a, &item, true).await.unwrap();
    flow.settled_in(&a).await;
    assert!(last_queue(&flow, &a).moved.is_empty());
    assert_eq!(
        heard(&inputs, &folder_a, message),
        a_heard + 1,
        "sent here again"
    );
    flow.say_to(&b, "Write gamma.").await;
    flow.settled_in(&b).await;
    assert_eq!(heard(&inputs, &folder_b, "took their message"), 0);

    // B merges; a follow-up routed to it goes, with the fact attached.
    flow.manager
        .merge_session(&b, super::click())
        .await
        .unwrap();
    flow.until_in(&b, "b's merge", |board| {
        board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, crate::work::OrchestratorStepKind::Merged { .. }))
    })
    .await;
    flow.settled_in(&b).await;
    let late = routes
        .route(
            &flow,
            &a,
            "Also tidy the beta file.",
            serde_json::json!({ "route": "session", "session": b.0 }),
        )
        .await;
    assert!(late.contains("It went to session"), "{late}");
    assert!(
        late.contains("has already merged into `main` at "),
        "{late}"
    );
    routes.release(&flow, &a, &inputs).await;
    flow.settled_in(&b).await;
    assert_eq!(heard(&inputs, &folder_b, "Also tidy the beta file."), 1);
    flow.stop().await;
}

/// A request that settles with the tab's branch ahead of base records one RequestReady with
/// the head; the tab reads ready, then merged after its merge (whose request records none),
/// and the board names those states.
#[tokio::test]
async fn a_settled_request_ahead_of_base_is_recorded_ready_once() {
    let inputs: Inputs = Arc::default();
    let flow = Flow::start(
        "sessions-ready",
        Options::default(),
        sharing(inputs.clone()),
    )
    .await;
    let a = flow.conversation.clone();
    let b = sibling_of(&flow, &a).await;
    let state = |id: &ConversationId| {
        let tab = flow.core.conversation(id).unwrap();
        let manager = flow.manager.clone();
        async move { manager.tab_state(&tab).await }
    };
    assert_eq!(state(&b).await, None, "a new tab has no state");

    flow.say_to(&a, "Write alpha.").await;
    flow.settled_in(&a).await;
    // Recorded before the request reads done.
    let board = flow.core.board(&a).await.unwrap();
    assert_eq!(board.ready.len(), 1, "{:?}", board.ready);
    let (_, _, branch) = worktree(&flow, &a);
    let head = git(&flow.repo, &["rev-parse", &branch]);
    assert_eq!(board.ready.values().next().unwrap().0, head);
    // The request reads done a moment before its turn's run does (#296).
    super::eventually_async("the tab to read ready", || async {
        state(&a).await == Some(crate::work::TabState::Ready)
    })
    .await;
    let settled = |flow: &Flow| {
        let manager = flow.manager.clone();
        async move {
            manager
                .settled_tabs()
                .await
                .into_iter()
                .map(|tab| (tab.conversation_id, tab.state))
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        settled(&flow).await,
        [(a.clone(), crate::work::TabState::Ready)],
        "the app reads the ready tab, not the untouched one"
    );
    // Settles at once record it once.
    let request = board.ready.keys().next().unwrap().clone();
    let mut settles = tokio::task::JoinSet::new();
    for _ in 0..8 {
        let (manager, a, request) = (flow.manager.clone(), a.clone(), request.clone());
        settles.spawn(async move { manager.note_ready(&a, &request).await });
    }
    while settles.join_next().await.is_some() {}
    let recorded = flow
        .events_of(&a)
        .await
        .into_iter()
        .filter(|event| matches!(event, crate::model::DomainEvent::RequestReady { .. }))
        .count();
    assert_eq!(recorded, 1);

    let words = |flow: &Flow, id: &ConversationId| {
        let tab = flow.core.conversation(id).unwrap();
        let manager = flow.manager.clone();
        async move { manager.tab_words(&tab).await }
    };
    assert_eq!(words(&flow, &a).await, "ready to merge");

    flow.manager
        .merge_session(&a, super::click())
        .await
        .unwrap();
    flow.until_in(&a, "a's merge", |board| {
        board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, crate::work::OrchestratorStepKind::Merged { .. }))
    })
    .await;
    flow.settled_in(&a).await;
    super::eventually_async("the tab to read merged", || async {
        state(&a).await == Some(crate::work::TabState::Merged)
    })
    .await;
    assert_eq!(
        settled(&flow).await,
        [(a.clone(), crate::work::TabState::Merged)]
    );
    assert_eq!(
        flow.core.board(&a).await.unwrap().ready.len(),
        1,
        "the merge's own request is not ready"
    );
    assert_eq!(words(&flow, &a).await, "merged");
    flow.stop().await;
}
