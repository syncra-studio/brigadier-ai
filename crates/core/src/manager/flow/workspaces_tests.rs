//! Workspaces (WORKSPACES-PLAN.md phase 2): a new chat in a project starts a workspace, more
//! task tabs join it, and each tab is a session of its own on its own worktree, so tabs of one
//! workspace work at once without sharing a file.

use std::path::PathBuf;
use std::sync::Arc;

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{ConversationId, ConversationKind, Environment, Setup, WorkspaceId};

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

fn workspace_of(flow: &Flow, id: &ConversationId) -> WorkspaceId {
    flow.core
        .conversation(id)
        .unwrap()
        .workspace_id
        .expect("a tab has a workspace")
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

/// A new chat in a project starts a workspace titled after it; a second tab joins it on a new
/// worktree from the same base; the two tabs work at once, each in its own folder on its own
/// branch, and neither sees the other's file.
#[tokio::test]
async fn two_tabs_of_one_workspace_work_at_once_on_their_own_worktrees() {
    let meet = Arc::new(tokio::sync::Barrier::new(2));
    let flow = Flow::start(
        "workspaces-two-tabs",
        Options::default(),
        writing(Some(meet)),
    )
    .await;
    let first = flow.conversation.clone();
    let workspace = workspace_of(&flow, &first);
    let catalog = flow.core.catalog();
    assert_eq!(catalog.workspaces.len(), 1);
    assert_eq!(catalog.workspaces[0].id, workspace);
    assert_eq!(catalog.workspaces[0].title, "Flow", "the first tab's title");
    assert_eq!(catalog.workspaces[0].base, "main");
    assert_eq!(
        catalog.workspaces[0].repos,
        vec![flow.repo.display().to_string()]
    );

    let second = flow.manager.create_tab(&workspace, None).await.unwrap();
    assert_eq!(second.workspace_id.as_ref(), Some(&workspace));
    assert_eq!(second.kind, ConversationKind::Session);
    let (_, base, branch) = worktree(&flow, &second.id);
    assert_eq!(base, "main", "from the workspace's base");
    assert_ne!(branch, worktree(&flow, &first).2, "a branch of its own");

    let (one, two) = tokio::join!(
        flow.say_to(&first, "Write alpha."),
        flow.say_to(&second.id, "Write beta.")
    );
    let ((), ()) = (one, two);
    flow.settled_in(&first).await;
    flow.settled_in(&second.id).await;

    let (Some(alpha), _, alpha_branch) = worktree(&flow, &first) else {
        panic!("the first tab's worktree");
    };
    let (Some(beta), _, beta_branch) = worktree(&flow, &second.id) else {
        panic!("the second tab's worktree");
    };
    assert_ne!(alpha, beta, "two folders");
    assert!(alpha.join("alpha.txt").exists() && !alpha.join("beta.txt").exists());
    assert!(beta.join("beta.txt").exists() && !beta.join("alpha.txt").exists());
    let files = |branch: &str| git(&flow.repo, &["ls-tree", "-r", "--name-only", branch]);
    assert!(!files(&alpha_branch).contains("beta.txt"));
    assert!(!files(&beta_branch).contains("alpha.txt"));

    let catalog = flow.core.catalog();
    assert_eq!(catalog.workspaces.len(), 1, "still one workspace");
    flow.stop().await;
}

/// ⌘⏎ starts a task in a new tab of the workspace: it runs there, on its own worktree.
#[tokio::test]
async fn a_background_tab_starts_on_its_message() {
    let flow = Flow::start("workspaces-start-tab", Options::default(), writing(None)).await;
    let workspace = workspace_of(&flow, &flow.conversation);
    let (tab, _) = flow
        .manager
        .start_tab(
            &workspace,
            None,
            "Write gamma.".into(),
            Vec::new(),
            Vec::new(),
        )
        .await
        .unwrap();
    assert_eq!(tab.title, "Write gamma.", "titled by its message");
    flow.settled_in(&tab.id).await;
    let (Some(path), _, _) = worktree(&flow, &tab.id) else {
        panic!("the tab's worktree");
    };
    assert!(path.join("gamma.txt").exists());
    assert_ne!(Some(path), worktree(&flow, &flow.conversation).0);
    flow.stop().await;
}

/// A session from before workspaces is a workspace of one tab; a new tab in it records that
/// workspace under its implicit id, and both tabs list in it.
#[tokio::test]
async fn a_tab_joins_an_older_sessions_implicit_workspace() {
    let flow = Flow::start("workspaces-implicit", Options::default(), writing(None)).await;
    let setup = flow.core.conversation(&flow.conversation).unwrap().setup;
    let project = flow
        .core
        .conversation(&flow.conversation)
        .unwrap()
        .project_id;
    let older = flow
        .core
        .create_conversation(
            ConversationId::generate(),
            ConversationKind::Session,
            project,
            Some("Older".into()),
            setup,
            crate::sessions::Origin::default(),
        )
        .await
        .unwrap();
    let implicit = WorkspaceId::implicit(&older.id);
    assert_eq!(workspace_of(&flow, &older.id), implicit);

    let tab = flow.manager.create_tab(&implicit, None).await.unwrap();
    assert_eq!(tab.workspace_id.as_ref(), Some(&implicit));
    let catalog = flow.core.catalog();
    let workspace = catalog
        .workspaces
        .iter()
        .find(|workspace| workspace.id == implicit)
        .unwrap();
    assert_eq!(workspace.title, "Older");
    let tabs = catalog
        .conversations
        .iter()
        .filter(|conversation| conversation.workspace_id.as_ref() == Some(&implicit))
        .count();
    assert_eq!(tabs, 2);
    flow.stop().await;
}

/// A new chat's first message titles its workspace, and the workspace keeps that name when
/// the tab it came from is deleted.
#[tokio::test]
async fn a_workspace_keeps_its_first_tabs_title() {
    let flow = Flow::start("workspaces-title", Options::default(), writing(None)).await;
    let first = flow.core.conversation(&flow.conversation).unwrap();
    let chat = flow
        .manager
        .create_conversation(
            ConversationKind::Session,
            first.project_id.clone(),
            None,
            Some(crate::model::SetupRequest::Session {
                repo: flow.repo.display().to_string(),
                environment: crate::model::EnvironmentRequest::NewWorktree {
                    base: "main".into(),
                    branch: None,
                },
                permission: crate::model::PermissionLevel::ApproveForMe,
                orchestrator: match first.setup {
                    Some(Setup::Session { orchestrator, .. }) => orchestrator,
                    _ => unreachable!(),
                },
                plan_mode: false,
            }),
        )
        .await
        .unwrap();
    let workspace = workspace_of(&flow, &chat.id);
    flow.say_to(&chat.id, "Redesigning the login page").await;
    let title = flow.core.workspace(&workspace).unwrap().title;
    assert_eq!(title, flow.core.conversation(&chat.id).unwrap().title);
    assert_ne!(title, "New session");

    let tab = flow.manager.create_tab(&workspace, None).await.unwrap();
    flow.settled_in(&chat.id).await;
    flow.manager.delete(chat.id.clone()).await.unwrap();
    assert_eq!(flow.core.workspace(&workspace).unwrap().title, title);
    assert_eq!(
        flow.core.conversation(&tab.id).unwrap().workspace_id,
        Some(workspace)
    );
    flow.stop().await;
}

/// A background tab whose message can't go leaves no empty tab behind.
#[tokio::test]
async fn a_background_tab_whose_message_fails_goes() {
    let flow = Flow::start("workspaces-start-fails", Options::default(), writing(None)).await;
    let workspace = workspace_of(&flow, &flow.conversation);
    let before = flow.core.catalog().conversations.len();
    assert!(
        flow.manager
            .start_tab(&workspace, None, "  ".into(), Vec::new(), Vec::new())
            .await
            .is_err()
    );
    super::eventually("the empty tab gone", || {
        flow.core.catalog().conversations.len() == before
    })
    .await;
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
                    .call("list_sessions", serde_json::json!({ "workspace": true }))
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
        "workspaces-board",
        Options::default(),
        sharing(inputs.clone()),
    )
    .await;
    let a = flow.conversation.clone();
    let workspace = workspace_of(&flow, &a);
    let b = flow.manager.create_tab(&workspace, None).await.unwrap().id;

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
    // Reading the board changed no workspace (beyond the message's own activity).
    let settled = |workspaces: Vec<crate::model::Workspace>| {
        workspaces
            .into_iter()
            .map(|workspace| crate::model::Workspace {
                updated_at_ms: 0,
                ..workspace
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        settled(flow.core.catalog().workspaces),
        settled(catalog.workspaces)
    );
    assert_eq!(
        heard(&inputs, &folder_a, "[workspace]"),
        0,
        "no overlap yet"
    );

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

/// How the next follow-up is routed (its `follow_up` id is filled in), what route_follow_up
/// answered, and the gate the routing turn waits at after it routed.
#[derive(Default)]
struct Routing {
    args: std::sync::Mutex<serde_json::Value>,
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
                .push((turn.add_dirs[0].clone(), turn.input.clone()));
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
            let block = turn.steered().await.unwrap_or_default();
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
/// keeps it queued; new_tab keeps it queued with the offer and opens nothing; tab sends it to
/// the sibling as the user's message, with a notice here that Undo takes back; tab to a
/// sibling that merged goes through with that fact; a tab that isn't there leaves it unsorted.
#[tokio::test(flavor = "multi_thread")]
async fn a_follow_up_joins_waits_offers_a_tab_or_goes_to_a_sibling() {
    let inputs: Inputs = Arc::default();
    let routes = Arc::new(Routing::default());
    let flow = Flow::start(
        "workspaces-routes",
        Options::default(),
        routing(inputs.clone(), routes.clone()),
    )
    .await;
    let a = flow.conversation.clone();
    let workspace = workspace_of(&flow, &a);
    let b = flow.manager.create_tab(&workspace, None).await.unwrap().id;
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
    let queue = last_queue(&flow, &a);
    assert_eq!(queue.items.len(), 1);
    assert!(!queue.items[0].deciding && !queue.items[0].offer_tab);
    routes.release(&flow, &a).await;

    // new_tab: the offer, and no tab opened.
    let tabs = flow.core.catalog().conversations.len();
    let offered = routes
        .route(
            &flow,
            &a,
            "Draft the release notes.",
            serde_json::json!({ "route": "new_tab" }),
        )
        .await;
    assert!(offered.contains("\"Open as a new tab?\""), "{offered}");
    let queue = last_queue(&flow, &a);
    assert_eq!(queue.items.len(), 1);
    assert!(queue.items[0].offer_tab && !queue.items[0].deciding);
    assert_eq!(
        flow.core.catalog().conversations.len(),
        tabs,
        "no tab opened"
    );
    routes.release(&flow, &a).await;

    // A tab that isn't there: a fact, and the follow-up stays the thread's to sort.
    let nowhere = routes
        .route(
            &flow,
            &a,
            "Also lint it.",
            serde_json::json!({ "route": "tab", "tab": "nope" }),
        )
        .await;
    assert!(
        nowhere.contains("No other tab of this workspace is session nope"),
        "{nowhere}"
    );
    assert!(nowhere.contains(&b.0), "lists the tabs: {nowhere}");
    // Still unsorted: deciding, or already joined by itself after the judging wait.
    let queue = last_queue(&flow, &a);
    assert!(queue.moved.is_empty() && queue.items.iter().all(|item| item.deciding));
    routes.release(&flow, &a).await;

    // tab: it goes to B as the user's message; A keeps a notice.
    let message = "Rename the beta file.";
    let moved = routes
        .route(
            &flow,
            &a,
            message,
            serde_json::json!({ "route": "tab", "tab": b.0 }),
        )
        .await;
    assert!(moved.contains("It went to tab"), "{moved}");
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
            serde_json::json!({ "route": "tab", "tab": b.0 }),
        )
        .await;
    assert!(late.contains("It went to tab"), "{late}");
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
        "workspaces-routes-revision",
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

/// A request that settles with the tab's branch ahead of base records one RequestReady with
/// the head; the tab reads ready, then merged after its merge (whose request records none),
/// and the board names those states.
#[tokio::test]
async fn a_settled_request_ahead_of_base_is_recorded_ready_once() {
    let inputs: Inputs = Arc::default();
    let flow = Flow::start(
        "workspaces-ready",
        Options::default(),
        sharing(inputs.clone()),
    )
    .await;
    let a = flow.conversation.clone();
    let workspace = workspace_of(&flow, &a);
    let b = flow.manager.create_tab(&workspace, None).await.unwrap().id;
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
