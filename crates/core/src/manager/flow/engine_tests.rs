//! The thread engine's first start (THREAD-PLAN Q14): what the earlier engine left goes, once.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::{Flow, Options, Reply, Scratch, Script, Turn};
use crate::model::{ConversationId, ConversationKind, DomainEvent, PermissionLevel, streams};
use crate::sessions::Origin;

/// The conversations of `pre-flow-events.jsonl`: two sessions and two Chats.
const OLD: [&str; 4] = [
    "01a10853-80c6-75fc-9788-21c770d02049",
    "01a10864-5c97-75de-aabb-2364f6e33343",
    "01a10865-0bed-76df-8d1a-cd1e1bc41e4d",
    "01a108f1-a812-73cd-98ec-4eee23126abe",
];
const OLD_PROJECT: &str = "01a0fef8-d9ca-735d-9668-e9d157360d9f";

/// What every scripted CLI was asked.
type Heard = Arc<Mutex<Vec<String>>>;

fn listening(heard: &Heard) -> Script {
    let heard = heard.clone();
    Arc::new(move |turn: Turn| {
        heard.lock().unwrap().push(turn.input.clone());
        Box::pin(async { Reply::text("[quiet]") })
    })
}

/// The store an earlier version left (real records, their text redacted): a landing that
/// waited for the user, a task mid-landing, and an overnight run still running, with `more`
/// lines after it.
fn old_store(more: &[Value]) -> &'static str {
    old_store_in(more, "/tmp/redacted/brigadier-ai")
}

/// [`old_store`], its sessions' repository at `repo`.
fn old_store_in(more: &[Value], repo: &str) -> &'static str {
    let mut lines: Vec<String> = include_str!("fixtures/pre-flow-events.jsonl")
        .replace("\"/tmp/redacted/brigadier-ai\"", &format!("{repo:?}"))
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter(|line| {
            let event: Value = serde_json::from_str(line).unwrap();
            // The run stops at "running": a restart would carry it on.
            event["kind"] != "overnight.updated"
                || matches!(
                    event["payload"]["run"]["state"].as_str(),
                    Some("proposed" | "preparing" | "running")
                )
        })
        .map(str::to_owned)
        .collect();
    lines.extend(more.iter().map(Value::to_string));
    Box::leak(lines.join("\n").into_boxed_str())
}

fn catalog_line(payload: Value) -> Value {
    let kind = match payload["type"].as_str().unwrap() {
        "settingsChanged" => "settings.changed",
        "conversationLifecycleChanged" => "conversation.lifecycle",
        "conversationCreated" => "conversation.created",
        "conversationDeleting" => "conversation.deleting",
        "conversationDeleted" => "conversation.deleted",
        "engineSwitching" => "engine.switching",
        other => panic!("no kind for {other}"),
    };
    let stream = if kind == "settings.changed" {
        streams::SETTINGS
    } else {
        streams::CATALOG
    };
    json!({ "stream": stream, "kind": kind, "payload": payload })
}

fn chat(id: &str) -> Value {
    catalog_line(json!({
        "type": "conversationCreated",
        "conversation": {
            "id": id, "kind": "chat", "projectId": null, "title": "Made later",
            "pinnedAtMs": null, "createdAtMs": 1791200000000_i64, "updatedAtMs": 1791200000000_i64,
            "setup": null, "lifecycle": "active",
        },
    }))
}

/// Waits until none of `ids` is in the catalog any more.
async fn gone(flow: &Flow, ids: &[&str]) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    loop {
        let left: Vec<_> = flow
            .core
            .catalog()
            .conversations
            .into_iter()
            .filter(|conversation| ids.contains(&conversation.id.0.as_str()))
            .map(|conversation| conversation.id)
            .collect();
        if left.is_empty() {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "still there: {left:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// The kinds of the catalog's events, oldest first.
async fn catalog_kinds(flow: &Flow) -> Vec<String> {
    let mut events = flow
        .core
        .store()
        .read_stream(
            streams::CATALOG.into(),
            brigadier_store::StreamPage {
                limit: 10_000,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    events.reverse();
    events.into_iter().map(|event| event.kind).collect()
}

/// The first start deletes every conversation there was (sessions and Chats, an archived one
/// too) through the normal delete, puts the default permission back to Full access, keeps the
/// projects and their remembered choices, and resumes nothing of what it deletes.
#[tokio::test]
async fn the_first_start_deletes_every_old_conversation() {
    let heard: Heard = Arc::default();
    let seed = old_store(&[
        catalog_line(json!({
            "type": "conversationLifecycleChanged",
            "id": OLD[1],
            "lifecycle": "archived",
        })),
        catalog_line(json!({
            "type": "settingsChanged",
            "settings": { "defaultPermission": "askForApproval", "onboarded": true },
        })),
    ]);
    let flow = Flow::start(
        "engine-first",
        Options {
            seed: Some(seed),
            ..Options::default()
        },
        listening(&heard),
    )
    .await;
    // Gone for the user before any client could connect.
    let visible = flow.core.visible_catalog().conversations;
    assert_eq!(visible.len(), 1, "only the session made after: {visible:?}");
    assert_eq!(visible[0].id, flow.conversation);
    gone(&flow, &OLD).await;
    let settings = flow.core.settings();
    assert_eq!(settings.default_permission, PermissionLevel::FullAccess);
    assert!(settings.onboarded, "the other settings stay");
    let project = flow
        .core
        .project(&crate::model::ProjectId(OLD_PROJECT.into()))
        .expect("the project stays");
    assert_eq!(project.prefs.permission, Some(PermissionLevel::FullAccess));
    assert_eq!(
        flow.core.engine().as_deref(),
        Some(super::super::engine::ENGINE)
    );
    assert!(flow.core.engine_switch().is_none());
    // Their streams are purged.
    for id in OLD {
        let events = flow
            .core
            .store()
            .read_stream(
                streams::conversation(&ConversationId(id.into())),
                brigadier_store::StreamPage::default(),
            )
            .await
            .unwrap();
        assert!(events.is_empty(), "{id} keeps {} events", events.len());
    }
    // Nothing of them resumed: no landing redone, no run carried on.
    assert!(!flow.manager.overnight_active());
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        heard.lock().unwrap().is_empty(),
        "{:?}",
        heard.lock().unwrap()
    );
    let kinds = catalog_kinds(&flow).await;
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| *kind == "engine.switched")
            .count(),
        1
    );
    flow.stop().await;
}

/// Records only the earlier engine wrote (a run in Phase 0, a whole-phase verifier, a
/// per-phase injection and rebirth) still read: a conversation holding them whose delete
/// can't finish at once keeps a board, so the next launch can still release its run and tasks.
#[tokio::test]
async fn the_earlier_engines_own_records_still_read() {
    let heard: Heard = Arc::default();
    let repo = Scratch::new("engine-old-records");
    let seed = old_store_in(&[], repo.to_str().unwrap())
        .replace("\"role\":\"worker\"", "\"role\":\"phaseVerifier\"")
        .replace(
            "\"state\":\"running\",\"windDownAtMs\"",
            "\"state\":\"planning\",\"windDownAtMs\"",
        );
    let flow = Flow::start(
        "engine-old-records",
        Options {
            seed: Some(Box::leak(seed.into_boxed_str())),
            ..Options::default()
        },
        listening(&heard),
    )
    .await;
    let session = ConversationId(OLD[3].into());
    let board = flow.core.board(&session).await.expect("its board reads");
    assert_eq!(board.runs.len(), 1, "its run");
    let task = &board.tasks[&crate::model::TaskId("01a108f3-9296-70a4-a507-9d369072785a".into())];
    assert_eq!(
        task.run.as_ref().map(|context| context.role),
        Some(crate::overnight::RunRole::Check)
    );
    // An injection and a rebirth of an earlier run's phase.
    let injection = serde_json::from_str::<crate::work::InjectionKind>("\"phase\"");
    assert_eq!(injection.ok(), Some(crate::work::InjectionKind::Run));
    let rebirth = serde_json::from_str::<crate::knowledge::RebirthTrigger>("\"phase\"");
    assert_eq!(
        rebirth.ok(),
        Some(crate::knowledge::RebirthTrigger::Recovery)
    );
    flow.stop().await;
}

/// A fresh store only gets the marker; a Chat and a session made after the first start, and
/// a permission chosen since, stay through later starts.
#[tokio::test]
async fn later_starts_keep_what_was_made_after_the_first() {
    let heard: Heard = Arc::default();
    let mut flow = Flow::start("engine-later", Options::default(), listening(&heard)).await;
    assert_eq!(
        flow.core.engine().as_deref(),
        Some(super::super::engine::ENGINE)
    );
    let kinds = catalog_kinds(&flow).await;
    assert_eq!(kinds.first().map(String::as_str), Some("engine.switched"));
    assert!(!kinds.iter().any(|kind| kind == "engine.switching"));
    let chat = flow
        .core
        .create_conversation(
            ConversationId::generate(),
            ConversationKind::Chat,
            None,
            Some("Kept".into()),
            None,
            Origin::default(),
        )
        .await
        .unwrap();
    let mut settings = flow.core.settings();
    settings.default_permission = PermissionLevel::AskForApproval;
    flow.core.update_settings(settings).await.unwrap();
    for _ in 0..2 {
        flow.restart().await;
        for id in [&chat.id, &flow.conversation] {
            let conversation = flow.core.conversation(id).expect("still there");
            assert!(!conversation.deleting);
        }
        assert_eq!(
            flow.core.settings().default_permission,
            PermissionLevel::AskForApproval
        );
    }
    let kinds = catalog_kinds(&flow).await;
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| *kind == "engine.switched")
            .count(),
        1
    );
    flow.stop().await;
}

/// A first start a quit cut off (its list recorded, one of them deleted already) deletes
/// exactly the rest of its list at the next start, never a conversation made after the list.
#[tokio::test]
async fn a_first_start_cut_off_deletes_only_what_it_listed() {
    let heard: Heard = Arc::default();
    let later = "01a20000-0000-7000-8000-000000000001";
    let mut more = vec![catalog_line(json!({
        "type": "engineSwitching",
        "engine": super::super::engine::ENGINE,
        "conversations": OLD,
    }))];
    more.extend(
        OLD.iter()
            .map(|id| catalog_line(json!({ "type": "conversationDeleting", "id": id }))),
    );
    more.push(catalog_line(
        json!({ "type": "conversationDeleted", "id": OLD[2] }),
    ));
    more.push(chat(later));
    let flow = Flow::start(
        "engine-cut-off",
        Options {
            seed: Some(old_store(&more)),
            ..Options::default()
        },
        listening(&heard),
    )
    .await;
    gone(&flow, &OLD).await;
    let kept = flow
        .core
        .conversation(&ConversationId(later.into()))
        .expect("made after the list");
    assert!(!kept.deleting);
    assert_eq!(
        flow.core.engine().as_deref(),
        Some(super::super::engine::ENGINE)
    );
    assert!(flow.core.engine_switch().is_none());
    assert!(!flow.manager.overnight_active());
    assert!(
        heard.lock().unwrap().is_empty(),
        "{:?}",
        heard.lock().unwrap()
    );
    flow.stop().await;
}

/// A conversation whose delete can't finish (its repository is a folder git can't open)
/// stays marked as being deleted, and recovery passes over it: its interrupted landing isn't
/// handed back to the orchestrator, nothing of it runs, and the next launch tries again.
#[tokio::test]
async fn an_old_conversation_whose_delete_fails_resumes_nothing() {
    let heard: Heard = Arc::default();
    let repo = Scratch::new("engine-not-a-repo");
    let seed = old_store_in(&[], repo.to_str().unwrap());
    let flow = Flow::start(
        "engine-stuck",
        Options {
            seed: Some(seed),
            ..Options::default()
        },
        listening(&heard),
    )
    .await;
    let session = ConversationId(OLD[0].into());
    let landing = crate::work::TaskId("01a10888-d9e7-764b-9843-72f98b3b7d1d".into());
    // Its delete stopped the task, then failed on the branches.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while flow.core.board(&session).await.unwrap().tasks[&landing].state
        != crate::work::TaskState::Stopped
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "its delete winds it down"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    let conversation = flow.core.conversation(&session).unwrap();
    assert!(conversation.deleting, "still being deleted");
    assert!(
        !flow
            .core
            .visible_catalog()
            .conversations
            .iter()
            .any(|conversation| conversation.id == session)
    );
    // What this start recorded for it: never recovery's hand-back of the landing.
    let recorded = flow
        .core
        .store()
        .read_stream(
            streams::conversation(&session),
            brigadier_store::StreamPage {
                limit: 10_000,
                ..Default::default()
            },
        )
        .await
        .unwrap()
        .into_iter()
        .filter(|event| event.at_ms > 1_760_000_000_000)
        .map(|event| crate::sessions::decode(&event).unwrap())
        .collect::<Vec<_>>();
    assert!(!recorded.is_empty(), "its delete recorded its wind-down");
    for event in &recorded {
        if let DomainEvent::TaskUpdated { task } = event {
            assert_ne!(
                task.state,
                crate::work::TaskState::Reported,
                "recovery handed task-{} back",
                task.number
            );
        }
    }
    assert!(!flow.manager.overnight_active());
    assert!(
        heard.lock().unwrap().is_empty(),
        "{:?}",
        heard.lock().unwrap()
    );
    flow.stop().await;
}

/// The marker and the list fold as recorded: the list until the marker, the marker for good.
#[test]
fn the_switch_folds_into_the_catalog() {
    let mut projection = crate::projection::Projection::default();
    let id = ConversationId("c1".into());
    projection.apply(
        &DomainEvent::EngineSwitching {
            engine: "thread-1".into(),
            conversations: vec![id.clone()],
        },
        1,
        0,
    );
    assert_eq!(projection.engine_switch, Some(vec![id]));
    assert_eq!(projection.engine, None);
    projection.apply(
        &DomainEvent::EngineSwitched {
            engine: "thread-1".into(),
        },
        2,
        0,
    );
    assert_eq!(projection.engine.as_deref(), Some("thread-1"));
    assert_eq!(projection.engine_switch, None);
}
