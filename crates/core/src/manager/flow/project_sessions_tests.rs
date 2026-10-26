//! A project's sessions (Q73): each one task on its own worktree. A session starts another in
//! the background from its own base; sessions of one project work at once without sharing a
//! file and see each other on the board (no overlap or merge is pushed to them).

use std::path::PathBuf;
use std::sync::Arc;

use super::thread_tests::commit_in_workspace;
use super::{Flow, Options, Reply, Script, Turn, git};
use crate::model::{ConversationId, ConversationKind, Environment, Setup};
use crate::work::OrchestratorStepKind;

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
    assert_eq!(session.title, "Write gamma", "titled by its message");
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
                super::thread_tests::merge_with_git(&turn);
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

/// A request that settles with the tab's branch ahead of base records one RequestReady with
/// the head; the tab reads ready, then merged after its merge with git (whose request records
/// none), and the board names those states.
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
        let (flow, id) = (&flow, id.clone());
        async move { flow.settled_state(&id).await }
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

    flow.manager
        .merge_session(&a, super::click())
        .await
        .unwrap();
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
    flow.stop().await;
}

/// #524: a fork cannot bypass the checkout admission guard, even for its source's folder.
#[tokio::test]
async fn a_fork_in_the_same_folder_respects_the_checkout_guard() {
    use crate::model::{EnvironmentRequest, ForkPlace, MessageRole, SetupRequest};
    let flow = Flow::start("fork-one-local", Options::default(), writing(None)).await;
    let first = flow.core.conversation(&flow.conversation).unwrap();
    let Some(Setup::Session {
        repo,
        permission,
        orchestrator,
        ..
    }) = first.setup
    else {
        unreachable!()
    };
    let local = flow
        .manager
        .create_conversation(
            ConversationKind::Session,
            first.project_id,
            Some("Here".into()),
            Some(SetupRequest::Session {
                repo,
                environment: EnvironmentRequest::LocalCheckout {
                    branch: "main".into(),
                    create_from: None,
                },
                permission,
                orchestrator,
                plan_mode: false,
            }),
        )
        .await
        .unwrap();
    flow.say_to(&local.id, "Hello.").await;
    flow.settled_in(&local.id).await;
    let answer = flow
        .core
        .all_messages(&local.id)
        .await
        .unwrap()
        .into_iter()
        .rfind(|message| message.role == MessageRole::Assistant)
        .unwrap()
        .id;
    let before = flow.core.catalog().conversations.len();
    let branches = git(&flow.repo, &["branch", "--list"]);
    let refused = flow
        .manager
        .fork(local.id.clone(), answer.clone(), ForkPlace::Workspace)
        .await
        .unwrap_err();
    assert_eq!(
        refused.to_string(),
        super::super::checkout_taken(&local).to_string()
    );
    assert_eq!(flow.core.catalog().conversations.len(), before);
    assert_eq!(git(&flow.repo, &["branch", "--list"]), branches);
    // Forking a worktree session into the occupied checkout is refused before making a branch.
    flow.say("Hello.").await;
    flow.settled().await;
    let branches = git(&flow.repo, &["branch", "--list"]);
    let other_answer = flow
        .core
        .all_messages(&flow.conversation)
        .await
        .unwrap()
        .into_iter()
        .rfind(|message| message.role == MessageRole::Assistant)
        .unwrap()
        .id;
    let refused = flow
        .manager
        .fork(
            flow.conversation.clone(),
            other_answer,
            ForkPlace::Workspace,
        )
        .await
        .unwrap_err();
    assert_eq!(
        refused.to_string(),
        super::super::checkout_taken(&local).to_string()
    );
    assert_eq!(git(&flow.repo, &["branch", "--list"]), branches);
    assert_eq!(flow.core.catalog().conversations.len(), before);
    // A fork on a worktree of its own remains available.
    let fork = flow
        .manager
        .fork(local.id.clone(), answer, ForkPlace::NewWorktree)
        .await
        .unwrap();
    assert!(matches!(
        fork.setup,
        Some(Setup::Session {
            environment: Environment::NewWorktree { .. },
            ..
        })
    ));
    flow.stop().await;
}

/// The latest request of the flow's thread.
async fn latest_request(flow: &Flow) -> String {
    flow.board()
        .await
        .latest_request()
        .expect("a request")
        .id
        .clone()
}

/// Records a session offer in the flow's thread as the thread recorded one (an old record's
/// too: the event is the same), at `at_ms`.
async fn record_offer(flow: &Flow, request: Option<String>, id: &str, task: &str, at_ms: i64) {
    flow.core
        .record_conversation(
            &flow.conversation,
            vec![crate::model::DomainEvent::OrchestratorStepped {
                step: crate::work::OrchestratorStep {
                    request_id: request,
                    kind: OrchestratorStepKind::SessionOffered {
                        id: id.into(),
                        title: task.trim_end_matches('.').into(),
                        task: task.into(),
                    },
                    at_ms,
                    position: 0,
                },
            }],
        )
        .await
        .unwrap();
}

/// How many times the offer `id` was taken in the flow's thread.
async fn taken(flow: &Flow, id: &str) -> usize {
    flow.board()
        .await
        .orchestrator_steps
        .iter()
        .filter(|step| matches!(&step.kind, OrchestratorStepKind::SessionOfferTaken { id: taken, .. } if taken == id))
        .count()
}

/// Start as session (Q73): the thread offers work to run apart as a card
/// (`brigadier session propose`), the same task once a request; nothing starts by itself. The
/// user's click starts it as a session of its own, on its own worktree, with the task as its
/// first message, and the offer is spent.
#[tokio::test]
async fn an_offered_session_starts_only_on_the_users_click() {
    let inner = writing(None);
    let thread = script(move |turn| {
        let inner = inner.clone();
        async move {
            if !turn.is_orchestrator() || !turn.input.contains("Fix the header.") {
                return inner(turn).await;
            }
            let offered = turn
                .call(
                    "offer_session",
                    serde_json::json!({ "task": "Write delta.", "title": "Write delta" }),
                )
                .await;
            assert!(!offered.is_error, "{}", offered.text);
            assert!(
                offered.text.contains("Don't start it yourself"),
                "{}",
                offered.text
            );
            let again = turn
                .call(
                    "offer_session",
                    serde_json::json!({ "task": "Write delta." }),
                )
                .await;
            assert!(
                again.is_error && again.text.contains("already offered"),
                "{}",
                again.text
            );
            let empty = turn
                .call("offer_session", serde_json::json!({ "task": "  " }))
                .await;
            assert!(empty.is_error, "{}", empty.text);
            Reply::text("The header is fixed.")
        }
    });
    let flow = Flow::start("sessions-offer", Options::default(), thread).await;
    let before = flow.core.catalog().conversations.len();
    flow.say("Fix the header.").await;
    let board = flow.settled().await;
    let offers: Vec<_> = board
        .orchestrator_steps
        .iter()
        .filter_map(|step| match &step.kind {
            OrchestratorStepKind::SessionOffered { id, title, task } => {
                Some((id.clone(), title.clone(), task.clone()))
            }
            _ => None,
        })
        .collect();
    let [(offer, title, task)] = offers.try_into().expect("one offer");
    assert_eq!(
        (title.as_str(), task.as_str()),
        ("Write delta", "Write delta.")
    );
    assert_eq!(
        flow.core.catalog().conversations.len(),
        before,
        "nothing starts by itself"
    );

    let (session, _) = flow
        .manager
        .start_offered_session(&flow.conversation, &offer)
        .await
        .unwrap();
    assert_eq!(
        session.title, "Write delta",
        "its task is its first message"
    );
    assert_eq!(
        session.project_id,
        flow.core
            .conversation(&flow.conversation)
            .unwrap()
            .project_id
    );
    flow.settled_in(&session.id).await;
    let (Some(path), _, _) = worktree(&flow, &session.id) else {
        panic!("the session's worktree");
    };
    assert!(path.join("delta.txt").exists());
    assert_ne!(Some(path), worktree(&flow, &flow.conversation).0);
    let board = flow.board().await;
    assert!(board.orchestrator_steps.iter().any(|step| matches!(
        &step.kind,
        OrchestratorStepKind::SessionOfferTaken { id, session: started } if *id == offer && *started == session.id
    )));
    assert!(
        flow.manager
            .start_offered_session(&flow.conversation, &offer)
            .await
            .is_err(),
        "once only"
    );
    flow.stop().await;
}

/// Two clicks of Start as session at once, on an offer an older version recorded: one session
/// starts, the offer is taken once, and the other click is refused.
#[tokio::test]
async fn two_clicks_on_an_old_records_offer_start_one_session() {
    let flow = Flow::start("sessions-offer-clicks", Options::default(), writing(None)).await;
    flow.say("Note it.").await;
    flow.settled().await;
    let request = latest_request(&flow).await;
    record_offer(
        &flow,
        Some(request),
        "old",
        "Write epsilon.",
        crate::now_ms(),
    )
    .await;
    let before = flow.core.catalog().conversations.len();
    let (first, second) = tokio::join!(
        flow.manager
            .start_offered_session(&flow.conversation, "old"),
        flow.manager
            .start_offered_session(&flow.conversation, "old"),
    );
    let started: Vec<_> = [first, second].into_iter().filter_map(Result::ok).collect();
    let [(session, _)] = started.try_into().expect("one click starts it");
    assert_eq!(flow.core.catalog().conversations.len(), before + 1);
    assert_eq!(taken(&flow, "old").await, 1);
    flow.settled_in(&session.id).await;
    flow.stop().await;
}

/// Start as session starts nothing from an offer out of view (from a request's earlier attempt,
/// or a message the user edited away), an unknown offer, or a session archived; workers and
/// Chats have no `offer_session`, and a Chat opens no session.
#[tokio::test]
async fn an_offer_out_of_view_or_from_an_archived_session_starts_nothing() {
    use crate::tools::{OfferSession, OrchestratorCall, Role, ToolCall, ToolHost};
    let flow = Flow::start("sessions-offer-refused", Options::default(), writing(None)).await;
    flow.say("Note it.").await;
    flow.settled().await;
    let request = latest_request(&flow).await;
    record_offer(&flow, Some(request.clone()), "earlier", "Write zeta.", 0).await;
    record_offer(
        &flow,
        Some("edited-away".into()),
        "edited",
        "Write eta.",
        crate::now_ms(),
    )
    .await;
    record_offer(
        &flow,
        Some(request),
        "current",
        "Write theta.",
        crate::now_ms(),
    )
    .await;
    let before = flow.core.catalog().conversations.len();
    for offer in ["earlier", "edited"] {
        let err = flow
            .manager
            .start_offered_session(&flow.conversation, offer)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("earlier answer"), "{offer}: {err}");
    }
    assert!(matches!(
        flow.manager
            .start_offered_session(&flow.conversation, "unknown")
            .await,
        Err(crate::Error::NotFound(_))
    ));
    assert_eq!(flow.core.catalog().conversations.len(), before);

    let offer = |task: &str| OfferSession {
        task: task.into(),
        title: None,
    };
    for role in [
        Role::Worker {
            conversation_id: flow.conversation.clone(),
            task_id: crate::work::TaskId("t-offer".into()),
        },
        Role::Chat {
            conversation_id: flow.conversation.clone(),
        },
    ] {
        let grant = flow.manager.grants.issue("test:offer", role);
        let reply = ToolHost::call(
            &*flow.manager,
            &grant,
            ToolCall::Orchestrator(OrchestratorCall::OfferSession(offer("Write iota."))),
        )
        .await;
        assert!(
            reply.is_error && reply.text.contains("not available"),
            "{}",
            reply.text
        );
    }
    let chat = flow
        .manager
        .create_conversation(ConversationKind::Chat, None, None, None)
        .await
        .unwrap();
    assert!(
        flow.manager
            .offer_session(&chat.id, offer("Write iota."))
            .await
            .is_err()
    );

    flow.manager
        .archive(flow.conversation.clone())
        .await
        .unwrap();
    assert!(
        flow.manager
            .offer_session(&flow.conversation, offer("Write kappa."))
            .await
            .is_err()
    );
    assert!(
        flow.manager
            .start_offered_session(&flow.conversation, "current")
            .await
            .is_err()
    );
    assert_eq!(taken(&flow, "current").await, 0);
    assert_eq!(
        flow.core.catalog().conversations.len(),
        before + 1,
        "only the Chat"
    );
    flow.stop().await;
}
