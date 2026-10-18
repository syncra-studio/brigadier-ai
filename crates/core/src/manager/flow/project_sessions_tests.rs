//! A project's sessions (Q73): each one task on its own worktree. A session starts another in
//! the background from its own base; sessions of one project work at once without sharing a
//! file, see each other on the board, hear of overlaps in the same repository and of a
//! sibling's merge, and a queued message may go to the sibling it belongs to.

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
async fn sibling_of(flow: &Flow, from: &ConversationId) -> ConversationId {
    flow.manager
        .create_session_from(from, None)
        .await
        .unwrap()
        .id
}

fn worktree(flow: &Flow, id: &ConversationId) -> (Option<PathBuf>, String, String) {
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

type Inputs = Arc<std::sync::Mutex<Vec<(PathBuf, String)>>>;

/// A thread that, besides [`writing`], commits `shared.txt` on "Share." and removes it on
/// "Unshare.", reads the board on "List tabs.", messages session ‹id› on "Ping ‹id›.", and
/// merges on the user's click. Every thread input is logged by worktree.
fn sharing(inputs: Inputs) -> Script {
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
                .push((turn.add_dirs[0].clone(), turn.input.clone()));
            if turn.input.contains("[merge] The user clicked") {
                let reply = turn.call("finish_session", serde_json::json!({})).await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Merged.");
            }
            if said(&turn, "List tabs.") {
                let reply = turn
                    .call("list_sessions", serde_json::json!({ "project": true }))
                    .await;
                return Reply::text(reply.text);
            }
            // A sibling's message, or the reply to one: its header holds the sender's title.
            if turn.input.contains("Hello from a sibling tab.")
                || turn.input.contains("Seen by the sibling.")
            {
                return Reply::text("Seen by the sibling.");
            }
            if let Some(to) = turn
                .input
                .split("Ping ")
                .nth(1)
                .and_then(|rest| rest.split('.').next())
            {
                let reply = turn
                    .call(
                        "message_session",
                        serde_json::json!({ "to": to, "text": "Hello from a sibling tab." }),
                    )
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Pinged.");
            }
            if said(&turn, "Unshare.") {
                let folder = &turn.add_dirs[0];
                git(folder, &["rm", "-q", "shared.txt"]);
                git(folder, &["commit", "-q", "-m", "Unshare"]);
                return Reply::text("Unshared.");
            }
            if said(&turn, "Edit main.") {
                let folder = &turn.add_dirs[0];
                std::fs::create_dir_all(folder.join("src")).unwrap();
                let text = format!("fn main() {{}} // {}\n", crate::now_ms());
                std::fs::write(folder.join("src/main.rs"), text).unwrap();
                git(folder, &["add", "src/main.rs"]);
                git(folder, &["commit", "-q", "-m", "Edit main"]);
                return Reply::text("Edited.");
            }
            if said(&turn, "Share.") {
                let text = format!("{} {}\n", turn.add_dirs[0].display(), crate::now_ms());
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

/// The board, read-only: a tab lists its sibling's title, state, branch, base and changed
/// files; when both tabs change the same file each hears it once, as a plain fact; a merge of
/// one is told to the other once; reading the board writes nothing.
#[tokio::test]
async fn the_board_lists_siblings_and_tells_an_overlap_and_a_merge_once() {
    let inputs: Inputs = Arc::default();
    let flow = Flow::start(
        "sessions-board",
        Options::default(),
        sharing(inputs.clone()),
    )
    .await;
    let a = flow.conversation.clone();
    let b = sibling_of(&flow, &a).await;

    flow.say_to(&a, "Share.").await;
    flow.settled_in(&a).await;
    flow.say_to(&b, "Write beta.").await;
    flow.settled_in(&b).await;
    let (Some(folder_a), _, _) = worktree(&flow, &a) else {
        panic!("a's worktree")
    };
    let (Some(folder_b), _, branch_b) = worktree(&flow, &b) else {
        panic!("b's worktree")
    };

    // The board as data.
    let catalog = flow.core.catalog();
    flow.say_to(&a, "List tabs.").await;
    flow.settled_in(&a).await;
    let answer = flow
        .core
        .conversation_view(a.clone(), 20)
        .await
        .unwrap()
        .messages
        .messages
        .last()
        .unwrap()
        .text
        .clone();
    assert!(answer.contains(&b.0), "{answer}");
    assert!(
        answer.contains(&format!("\"branch\": \"{branch_b}\"")),
        "{answer}"
    );
    assert!(answer.contains("\"base\": \"main\""), "{answer}");
    assert!(
        answer.contains("beta.txt") && !answer.contains("alpha.txt"),
        "{answer}"
    );
    assert!(
        answer.contains(&format!("\"repository\": \"{}\"", flow.repo.display())),
        "{answer}"
    );
    // Reading the board changed no session (beyond the message's own activity).
    let settled = |mut conversations: Vec<crate::model::Conversation>| {
        conversations.sort_by(|a, b| a.id.cmp(&b.id));
        conversations
            .into_iter()
            .map(|conversation| crate::model::Conversation {
                updated_at_ms: 0,
                finished_at_ms: None,
                ..conversation
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        settled(flow.core.catalog().conversations),
        settled(catalog.conversations)
    );
    assert_eq!(heard(&inputs, &folder_a, "[project]"), 0, "no overlap yet");

    // Both change shared.txt: each hears it once, at its next turn.
    flow.say_to(&b, "Share.").await;
    flow.settled_in(&b).await;
    flow.say_to(&a, "Write alpha.").await;
    flow.settled_in(&a).await;
    flow.say_to(&b, "Write gamma.").await;
    flow.settled_in(&b).await;
    let fact = "also changed `shared.txt`";
    assert_eq!(heard(&inputs, &folder_a, fact), 1);
    assert_eq!(heard(&inputs, &folder_b, fact), 1);
    // The same overlap again is no news.
    flow.say_to(&a, "Share.").await;
    flow.settled_in(&a).await;
    flow.say_to(&a, "Write delta.").await;
    flow.settled_in(&a).await;
    flow.say_to(&b, "Write epsilon.").await;
    flow.settled_in(&b).await;
    assert_eq!(heard(&inputs, &folder_a, fact), 1, "told once");
    assert_eq!(heard(&inputs, &folder_b, fact), 1, "told once");
    // An overlap that ended and came back is news again.
    flow.say_to(&a, "Unshare.").await;
    flow.settled_in(&a).await;
    flow.say_to(&a, "Share.").await;
    flow.settled_in(&a).await;
    flow.say_to(&a, "Write theta.").await;
    flow.settled_in(&a).await;
    assert_eq!(heard(&inputs, &folder_a, fact), 2, "told again");

    // B merges: A hears it once.
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
    // A turn that no user message starts (a sibling's message) carries the fact too.
    let pings = heard(&inputs, &folder_a, "Hello from a sibling tab.");
    flow.say_to(&b, &format!("Ping {}.", a.0)).await;
    flow.settled_in(&b).await;
    let pinged = || {
        inputs.lock().unwrap().iter().any(|(at, input)| {
            at == &folder_a
                && input.contains("Hello from a sibling tab.")
                && input.contains("merged into `main` at")
        })
    };
    super::eventually("a's turn on the sibling's message", || {
        heard(&inputs, &folder_a, "Hello from a sibling tab.") > pings
    })
    .await;
    flow.settled_in(&a).await;
    assert!(pinged(), "the merge fact rides on the peer turn");
    flow.say_to(&a, "Write eta.").await;
    flow.settled_in(&a).await;
    let merged = "merged into `main` at";
    assert_eq!(heard(&inputs, &folder_a, merged), 1);
    assert_eq!(
        heard(&inputs, &folder_b, merged),
        0,
        "not told of its own merge"
    );
    flow.stop().await;
}

/// A project of two repositories: the same path changed in each is two files, so only the
/// sessions in the same repository hear of the overlap, and only they hear of each other's
/// merge; the board and the facts name each session's repository.
#[tokio::test]
async fn sessions_in_two_repositories_of_a_project_overlap_only_within_one() {
    let inputs: Inputs = Arc::default();
    let flow = Flow::start(
        "sessions-two-repos",
        Options::default(),
        sharing(inputs.clone()),
    )
    .await;
    let other = flow.dir.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let other = other.canonicalize().unwrap();
    git(&other, &["init", "-q", "-b", "main"]);
    std::fs::write(other.join("README.md"), "# Other\n").unwrap();
    git(&other, &["add", "-A"]);
    git(&other, &["commit", "-q", "-m", "Start"]);
    let first = flow.core.conversation(&flow.conversation).unwrap();
    let project = first.project_id.clone().unwrap();
    flow.core
        .update_project(
            project.clone(),
            crate::model::ProjectPatch {
                name: None,
                repos: Some(vec![
                    flow.repo.display().to_string(),
                    other.display().to_string(),
                ]),
                prefs: None,
            },
        )
        .await
        .unwrap();
    let Some(Setup::Session {
        permission,
        orchestrator,
        ..
    }) = first.setup.clone()
    else {
        unreachable!()
    };
    let a = flow.conversation.clone();
    let b = flow
        .manager
        .create_session_from(
            &a,
            Some(crate::model::SetupRequest::Session {
                repo: other.display().to_string(),
                environment: crate::model::EnvironmentRequest::NewWorktree {
                    base: "main".into(),
                    branch: None,
                },
                permission,
                orchestrator,
                plan_mode: false,
            }),
        )
        .await
        .unwrap()
        .id;
    let c = sibling_of(&flow, &a).await;
    let repo_of = |id: &ConversationId| match flow.core.conversation(id).unwrap().setup {
        Some(Setup::Session { repo, .. }) => repo,
        _ => unreachable!(),
    };
    assert_eq!(repo_of(&b), other.display().to_string());
    assert_eq!(repo_of(&c), flow.repo.display().to_string());

    for id in [&a, &b, &c] {
        flow.say_to(id, "Edit main.").await;
        flow.settled_in(id).await;
    }
    for id in [&a, &b, &c] {
        flow.say_to(id, "Write next.").await;
        flow.settled_in(id).await;
    }
    let folder = |id: &ConversationId| worktree(&flow, id).0.unwrap();
    let fact = "also changed `src/main.rs`";
    assert_eq!(heard(&inputs, &folder(&a), fact), 1);
    assert_eq!(heard(&inputs, &folder(&c), fact), 1);
    assert_eq!(
        heard(&inputs, &folder(&b), fact),
        0,
        "another repository's file"
    );
    assert_eq!(
        heard(&inputs, &folder(&a), &format!("(session {c}, repository `")),
        1,
        "the fact names the repository"
    );

    // The board names each one's repository.
    flow.say_to(&b, "List tabs.").await;
    flow.settled_in(&b).await;
    let answer = flow
        .core
        .conversation_view(b.clone(), 20)
        .await
        .unwrap()
        .messages
        .messages
        .last()
        .unwrap()
        .text
        .clone();
    assert!(
        answer.contains(&format!("\"repository\": \"{}\"", flow.repo.display())),
        "{answer}"
    );

    // C merges: A hears it, B (another repository) doesn't.
    flow.manager
        .merge_session(&c, super::click())
        .await
        .unwrap();
    flow.until_in(&c, "c's merge", |board| {
        board
            .orchestrator_steps
            .iter()
            .any(|step| matches!(step.kind, crate::work::OrchestratorStepKind::Merged { .. }))
    })
    .await;
    flow.settled_in(&c).await;
    for id in [&a, &b] {
        flow.say_to(id, "Write after.").await;
        flow.settled_in(id).await;
    }
    let merged = "merged into `main` at";
    assert_eq!(heard(&inputs, &folder(&a), merged), 1);
    assert_eq!(heard(&inputs, &folder(&b), merged), 0);
    flow.stop().await;
}

/// How the next queued message is routed (its `follow_up` id is filled in), what
/// route_follow_up answered, and the gates the routing turn waits at: for the queued message,
/// and after it routed.
#[derive(Default)]
struct Routing {
    args: std::sync::Mutex<serde_json::Value>,
    queued: std::sync::Mutex<String>,
    ready: tokio::sync::Notify,
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
/// "Run the tests.", routes the message queued meanwhile as `routing` says, and waits at the
/// gate. Every thread input is logged by worktree.
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
                .push((turn.add_dirs[0].clone(), turn.input.clone()));
            if turn.input.contains("[merge] The user clicked") {
                let reply = turn.call("finish_session", serde_json::json!({})).await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text("Merged.");
            }
            // As a thread's cached schema from before sessions still calls it.
            if said(&turn, "List the tabs the old way.") {
                let reply = turn
                    .call("list_sessions", serde_json::json!({ "workspace": true }))
                    .await;
                assert!(!reply.is_error, "{}", reply.text);
                return Reply::text(reply.text);
            }
            if !said(&turn, "Run the tests.") {
                return write(turn).await;
            }
            turn.emit(bash(brigadier_providers::ItemStatus::InProgress))
                .await;
            routing.ready.notified().await;
            let id = routing.queued.lock().unwrap().clone();
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
    /// Runs one routed message on `tab`: "Run the tests.", then `follow_up` waiting in the
    /// queue while it works (as one queued while paused, or by an older build), routed by
    /// `args`. Returns route_follow_up's answer, with the turn still at the gate.
    async fn route(
        &self,
        flow: &Flow,
        tab: &ConversationId,
        follow_up: &str,
        args: serde_json::Value,
    ) -> String {
        *self.args.lock().unwrap() = args;
        let before = self.results.lock().unwrap().len();
        flow.say_to(tab, "Run the tests.").await;
        flow.until_in(tab, "the command to run", |board| {
            board.orchestrator_steps.iter().any(|step| {
                matches!(&step.kind, crate::work::OrchestratorStepKind::Tool { status, .. } if *status == brigadier_providers::ItemStatus::InProgress)
            })
        })
        .await;
        let item = flow
            .core
            .enqueue(tab, follow_up.into(), Vec::new(), Vec::new(), None)
            .await
            .unwrap();
        *self.queued.lock().unwrap() = item.id;
        self.ready.notify_one();
        super::eventually("the follow-up routed", || {
            self.results.lock().unwrap().len() > before
        })
        .await;
        self.results.lock().unwrap()[before].clone()
    }

    /// Lets the routing turn end, and what waited in the queue go.
    async fn release(&self, flow: &Flow, tab: &ConversationId) {
        self.gate.notify_one();
        flow.until_in(tab, "the queue to empty", |board| {
            board.queue.items.is_empty()
        })
        .await;
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
/// keeps it queued; new_session keeps it queued too and starts nothing; session
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
    routes.release(&flow, &a).await;

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
    assert_eq!(last_queue(&flow, &a).items.len(), 1);
    routes.release(&flow, &a).await;

    // new_session: it waits too, and no session starts.
    let tabs = flow.core.catalog().conversations.len();
    let notes = "Draft the release notes.";
    let heard_before = heard(&inputs, &folder_a, notes);
    let offered = routes
        .route(
            &flow,
            &a,
            notes,
            serde_json::json!({ "route": "new_session" }),
        )
        .await;
    assert!(offered.starts_with("It waits in the queue"), "{offered}");
    assert!(!offered.contains("Run here"), "{offered}");
    assert_eq!(last_queue(&flow, &a).items.len(), 1);
    assert_eq!(
        flow.core.catalog().conversations.len(),
        tabs,
        "no session started"
    );
    routes.release(&flow, &a).await;
    assert_eq!(
        heard(&inputs, &folder_a, notes),
        heard_before + 1,
        "it ran here once the work was done"
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
    // Still waiting in the queue.
    let queue = last_queue(&flow, &a);
    assert!(
        queue.moved.is_empty() && queue.items.len() == 1,
        "{queue:?}"
    );
    routes.release(&flow, &a).await;

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
    routes.release(&flow, &a).await;
    flow.settled_in(&b).await;
    assert_eq!(heard(&inputs, &folder_b, message), 1, "B got it");
    assert_eq!(
        heard(&inputs, &folder_b, "whose thread sent it on to you"),
        1,
        "with where it came from"
    );
    let a_heard = heard(&inputs, &folder_a, message);

    // Undo: it is A's again, and B hears so at its next turn.
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
    assert_eq!(heard(&inputs, &folder_b, "took their message"), 1);

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
    routes.release(&flow, &a).await;
    flow.settled_in(&b).await;
    assert_eq!(heard(&inputs, &folder_b, "Also tidy the beta file."), 1);
    flow.stop().await;
}

/// Rule R: a thread told the revision before resumes its session at its next turn, hears the
/// routes' note once, and its older `joins` call still routes.
#[tokio::test(flavor = "multi_thread")]
async fn an_open_thread_resumes_with_the_routes_note_and_joins_still_routes() {
    use crate::manager::prompts::{WORKSPACE_ROUTES_REVISION, revision_note};
    let inputs: Inputs = Arc::default();
    let routes = Arc::new(Routing::default());
    let mut flow = Flow::start(
        "sessions-routes-revision",
        Options::default(),
        routing(inputs.clone(), routes.clone()),
    )
    .await;
    let a = flow.conversation.clone();
    flow.say_to(&a, "Write alpha.").await;
    flow.settled_in(&a).await;
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&a),
            crate::model::DomainEvent::OrchestratorLogged {
                conversation_id: a.clone(),
                entry: crate::work::OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            revision: Some(WORKSPACE_ROUTES_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;
    let joined = routes
        .route(
            &flow,
            &a,
            "Also check the docs.",
            serde_json::json!({ "joins": true }),
        )
        .await;
    assert!(joined.starts_with("It joins this work"), "{joined}");
    routes.release(&flow, &a).await;
    let specs = flow.thread_specs();
    assert!(
        matches!(
            specs.last().unwrap().1.origin,
            brigadier_providers::model::Origin::Resume { .. }
        ),
        "{specs:#?}"
    );
    let note = revision_note(WORKSPACE_ROUTES_REVISION - 1, false).unwrap();
    let told: Vec<String> = inputs
        .lock()
        .unwrap()
        .iter()
        .map(|(_, input)| input.clone())
        .filter(|input| input.contains(&note))
        .collect();
    assert_eq!(told.len(), 1, "told once: {told:#?}");
    assert_eq!(told[0].matches("[instructions]").count(), 1);
    flow.say_to(&a, "Write beta.").await;
    flow.settled_in(&a).await;
    let last = inputs.lock().unwrap().last().unwrap().1.clone();
    assert!(!last.contains("[instructions]"), "{last}");
    flow.stop().await;
}

/// Rule R for sessions instead of workspaces: a thread told the revision before resumes its
/// session at its next turn and hears the note once; its cached calls by the older names
/// (route "new_tab", route "tab" with `tab`, list_sessions with workspace true) still work.
#[tokio::test(flavor = "multi_thread")]
async fn an_open_thread_resumes_with_the_sessions_note_and_the_older_names_still_work() {
    use crate::manager::prompts::{SESSIONS_REVISION, revision_note};
    let inputs: Inputs = Arc::default();
    let routes = Arc::new(Routing::default());
    let mut flow = Flow::start(
        "sessions-revision",
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
    flow.core
        .record(vec![(
            crate::model::streams::orchestrator(&a),
            crate::model::DomainEvent::OrchestratorLogged {
                conversation_id: a.clone(),
                entry: crate::work::OrchestratorEntry::Injection {
                    injection: crate::work::ContextInjection {
                        kind: crate::work::InjectionKind::Instructions,
                        bytes: 100,
                        tokens_estimate: 25,
                        label: "instructions revision".into(),
                        task_id: None,
                        told: Some(crate::work::Told {
                            revision: Some(SESSIONS_REVISION - 1),
                            ..crate::work::Told::default()
                        }),
                    },
                },
            },
        )])
        .await
        .unwrap();
    flow.restart().await;

    let offered = routes
        .route(
            &flow,
            &a,
            "Draft the release notes.",
            serde_json::json!({ "route": "new_tab" }),
        )
        .await;
    assert!(offered.starts_with("It waits in the queue"), "{offered}");
    routes.gate.notify_one();
    flow.settled_in(&a).await;
    let queue = last_queue(&flow, &a);
    if let Some(item) = queue.items.first() {
        flow.core.take_queued(&a, &item.id).await.unwrap();
    }
    let specs = flow.thread_specs();
    assert!(
        matches!(
            specs.last().unwrap().1.origin,
            brigadier_providers::model::Origin::Resume { .. }
        ),
        "{specs:#?}"
    );

    let moved = routes
        .route(
            &flow,
            &a,
            "Rename the beta file.",
            serde_json::json!({ "route": "tab", "tab": b.0 }),
        )
        .await;
    assert!(moved.contains("It went to session"), "{moved}");
    routes.release(&flow, &a).await;

    flow.say_to(&a, "List the tabs the old way.").await;
    flow.settled_in(&a).await;
    let listed = flow
        .core
        .conversation_view(a.clone(), 20)
        .await
        .unwrap()
        .messages
        .messages
        .last()
        .unwrap()
        .text
        .clone();
    assert!(
        listed.starts_with("The other sessions of this project") && listed.contains(&b.0),
        "{listed}"
    );

    let note = revision_note(SESSIONS_REVISION - 1, false).unwrap();
    let told = inputs
        .lock()
        .unwrap()
        .iter()
        .filter(|(_, input)| input.contains(&note))
        .count();
    assert_eq!(told, 1, "told once");
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
    assert_eq!(state(&a).await, Some(crate::work::TabState::Ready));
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

    flow.say_to(&b, "List tabs.").await;
    flow.settled_in(&b).await;
    let listed = inputs.lock().unwrap().len();
    assert!(listed > 0);
    let answer = |flow: &Flow, id: &ConversationId| {
        let core = flow.core.clone();
        let id = id.clone();
        async move {
            core.conversation_view(id, 20)
                .await
                .unwrap()
                .messages
                .messages
                .last()
                .unwrap()
                .text
                .clone()
        }
    };
    let board_text = answer(&flow, &b).await;
    assert!(
        board_text.contains("\"state\": \"ready to merge\""),
        "{board_text}"
    );

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
    assert_eq!(state(&a).await, Some(crate::work::TabState::Merged));
    assert_eq!(
        settled(&flow).await,
        [(a.clone(), crate::work::TabState::Merged)]
    );
    assert_eq!(
        flow.core.board(&a).await.unwrap().ready.len(),
        1,
        "the merge's own request is not ready"
    );
    flow.say_to(&b, "List tabs.").await;
    flow.settled_in(&b).await;
    let board_text = answer(&flow, &b).await;
    assert!(board_text.contains("\"state\": \"merged\""), "{board_text}");
    flow.stop().await;
}
