use super::*;
use crate::accounts::AccountRef;
use crate::manager::flow::{FakeBehavior, Flow, Options, Reply, Script};
use crate::sessions::Origin;
use crate::{ConversationId, ConversationKind, Core};
use brigadier_providers::ProviderKind;
use brigadier_store::{Store, StoreConfig};
use std::sync::{Arc, Mutex};

struct Data {
    dir: std::path::PathBuf,
    core: Arc<Core>,
    store: Store,
}
impl Data {
    async fn new() -> Self {
        let dir = std::env::temp_dir().join(format!("brigadier-titles-{}", uuid::Uuid::now_v7()));
        let store = Store::open(StoreConfig {
            db_path: dir.join("test.db"),
            blobs_dir: dir.join("blobs"),
            readers: 1,
        })
        .unwrap();
        let core = Core::load(store.clone()).await.unwrap();
        Self { dir, core, store }
    }
    async fn conversation(&self, kind: ConversationKind) -> ConversationId {
        let project = if kind == ConversationKind::Session {
            Some(
                self.core
                    .create_project("Titles".into(), None)
                    .await
                    .unwrap()
                    .id,
            )
        } else {
            None
        };
        self.core
            .create_conversation(
                ConversationId::generate(),
                kind,
                project,
                None,
                None,
                Origin::default(),
            )
            .await
            .unwrap()
            .id
    }
    async fn done(self) {
        self.store.shutdown().await.unwrap();
        drop(self.core);
        drop(self.store);
        let _ = std::fs::remove_dir_all(self.dir);
    }
}

#[test]
fn invalid_titles_do_not_replace_the_local_fallback() {
    for invalid in [
        "",
        "  ",
        "Fix sidebar\nExtra prose",
        "{\"title\":\"Fix sidebar\"}",
        "Fix sidebar.",
        "`Fix sidebar`",
        "One two three four five six",
        "😀",
        "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
    ] {
        assert_eq!(generated_title(invalid), None, "{invalid:?}");
    }
    for valid in [
        "Fix sidebar hover",
        "Explică procesul",
        "Locate foo_bar",
        "Fix ABC-123",
    ] {
        assert_eq!(generated_title(valid).as_deref(), Some(valid));
    }
}

#[tokio::test]
async fn provisional_is_immediate_and_first_visible_message_alone_starts_generation() {
    let data = Data::new().await;
    let seen: Arc<Mutex<Vec<TitleRequest>>> = Arc::default();
    let capture = seen.clone();
    data.core.set_title_hook(Box::new(move |request| {
        capture.lock().unwrap().push(request)
    }));
    for kind in [ConversationKind::Chat, ConversationKind::Session] {
        let id = data.conversation(kind).await;
        data.core
            .append_assistant_message(
                id.clone(),
                "bootstrap".into(),
                "Internal preparation".into(),
                None,
                None,
                None,
            )
            .await
            .unwrap();
        assert!(seen.lock().unwrap().iter().all(|request| request.id != id));
        data.core
            .append_message(id.clone(), "In this session we will fix hover".into())
            .await
            .unwrap();
        let request = seen
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.id == id)
            .cloned()
            .unwrap();
        // The call returned with a local title, despite generation not having answered.
        let provisional = data.core.conversation(&id).unwrap().title;
        assert_ne!(provisional, "New session");
        assert_ne!(provisional, "New chat");
        data.core
            .append_message(id.clone(), "Now fix search".into())
            .await
            .unwrap();
        assert_eq!(
            seen.lock().unwrap().iter().filter(|r| r.id == id).count(),
            1
        );
        assert!(
            data.core
                .finish_title(&request, "Fix sidebar hover".into())
                .await
                .unwrap()
        );
        assert!(
            !data
                .core
                .finish_title(&request, "Retitle later".into())
                .await
                .unwrap()
        );
        assert_eq!(
            data.core.conversation(&id).unwrap().title,
            "Fix sidebar hover"
        );
    }
    data.done().await;
}

#[tokio::test]
async fn fusion_titles_use_the_question_for_the_provisional_and_generation_input() {
    let data = Data::new().await;
    let seen: Arc<Mutex<Vec<TitleRequest>>> = Arc::default();
    let capture = seen.clone();
    data.core
        .set_title_hook(Box::new(move |r| capture.lock().unwrap().push(r)));
    let question = "For a small personal knowledge base, compare Markdown and SQLite";
    let plain = data.conversation(ConversationKind::Chat).await;
    data.core
        .append_message(plain.clone(), question.into())
        .await
        .unwrap();
    let expected = data.core.conversation(&plain).unwrap().title;
    assert_eq!(expected, "For a small personal knowledge");
    // The button and typed invocations share this path; Codex stores the $ spelling.
    for prefix in ["/fusion ", "$fusion ", "  /fusion\n\t"] {
        let id = data.conversation(ConversationKind::Chat).await;
        let text = format!("{prefix}{question}");
        let message = data
            .core
            .append_message(id.clone(), text.clone())
            .await
            .unwrap();
        assert_eq!(message.text, text);
        assert_eq!(data.core.conversation(&id).unwrap().title, expected);
        let request = seen.lock().unwrap().last().cloned().unwrap();
        assert_eq!(request.input, question);
    }
    for text in [
        "/fusionation compare options",
        "Explain /fusion",
        "$fusion-extra",
    ] {
        assert_eq!(title_text(text, &[]), text);
    }
    assert_eq!(title_text("/fusion", &[]), "");
    data.done().await;
}

#[tokio::test]
async fn every_manual_rename_wins_including_same_text_and_away_and_back() {
    let data = Data::new().await;
    let seen: Arc<Mutex<Vec<TitleRequest>>> = Arc::default();
    let capture = seen.clone();
    data.core
        .set_title_hook(Box::new(move |r| capture.lock().unwrap().push(r)));
    for away in [false, true] {
        let id = data.conversation(ConversationKind::Chat).await;
        data.core
            .append_message(id.clone(), "Fix hover".into())
            .await
            .unwrap();
        let request = seen.lock().unwrap().last().cloned().unwrap();
        let same = data.core.conversation(&id).unwrap().title;
        if away {
            data.core
                .rename_conversation(id.clone(), "My title".into())
                .await
                .unwrap();
        }
        data.core
            .rename_conversation(id.clone(), same.clone())
            .await
            .unwrap();
        assert!(
            !data
                .core
                .finish_title(&request, "Overwrite manual title".into())
                .await
                .unwrap()
        );
        assert_eq!(data.core.conversation(&id).unwrap().title, same);
    }
    // Explicitly naming a new conversation with its default text still counts as a rename.
    let id = data.conversation(ConversationKind::Chat).await;
    data.core
        .rename_conversation(id.clone(), "New chat".into())
        .await
        .unwrap();
    let before = seen.lock().unwrap().len();
    data.core
        .append_message(id.clone(), "Fix hover".into())
        .await
        .unwrap();
    assert_eq!(seen.lock().unwrap().len(), before);
    let reloaded = Core::load(data.store.clone()).await.unwrap();
    reloaded
        .append_message(id.clone(), "Do more".into())
        .await
        .unwrap();
    assert_eq!(reloaded.conversation(&id).unwrap().title, "New chat");
    drop(reloaded);
    data.done().await;
}

#[tokio::test]
async fn concurrent_manual_and_generated_renames_finish_with_the_manual_title() {
    let data = Data::new().await;
    let seen: Arc<Mutex<Vec<TitleRequest>>> = Arc::default();
    let capture = seen.clone();
    data.core
        .set_title_hook(Box::new(move |r| capture.lock().unwrap().push(r)));
    let id = data.conversation(ConversationKind::Chat).await;
    data.core
        .append_message(id.clone(), "Fix hover".into())
        .await
        .unwrap();
    let request = seen.lock().unwrap().last().cloned().unwrap();
    let (manual, generated) = tokio::join!(
        data.core.rename_conversation(id.clone(), "My title".into()),
        data.core.finish_title(&request, "Generated title".into())
    );
    manual.unwrap();
    generated.unwrap();
    assert_eq!(data.core.conversation(&id).unwrap().title, "My title");
    data.done().await;
}

#[tokio::test]
async fn input_excludes_image_chips_and_bounds_all_text_and_pastes() {
    let data = Data::new().await;
    let seen: Arc<Mutex<Vec<TitleRequest>>> = Arc::default();
    let capture = seen.clone();
    data.core
        .set_title_hook(Box::new(move |r| capture.lock().unwrap().push(r)));
    let mut image = data
        .core
        .add_attachment("sketch.png".into(), "image/png".into(), vec![1], false)
        .await
        .unwrap();
    image.inline = Some(1);
    let id = data.conversation(ConversationKind::Chat).await;
    data.core
        .append_user_message(id.clone(), "[Image #1]".into(), vec![image.clone()], vec![])
        .await
        .unwrap();
    assert_eq!(data.core.conversation(&id).unwrap().title, "Image");
    assert!(seen.lock().unwrap().is_empty());
    let second = data.conversation(ConversationKind::Chat).await;
    data.core
        .append_user_message(
            second.clone(),
            String::new(),
            vec![image.clone(), image.clone(), image.clone()],
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(data.core.conversation(&second).unwrap().title, "3 images");
    let mut paste = data
        .core
        .add_attachment(
            "Pasted text.md".into(),
            "text/markdown".into(),
            "nginx timeout\n".repeat(2_000).into_bytes(),
            true,
        )
        .await
        .unwrap();
    paste.inline = Some(2);
    let id = data.conversation(ConversationKind::Chat).await;
    data.core
        .append_user_message(
            id.clone(),
            "[Image #1] [Pasted text #2]".into(),
            vec![image.clone(), paste.clone()],
            vec![],
        )
        .await
        .unwrap();
    let request = seen.lock().unwrap().last().cloned().unwrap();
    assert!(request.input.contains("nginx timeout"));
    assert!(!request.input.contains("Pasted text"));
    assert!(!request.input.contains("[Image"));
    assert!(request.input.chars().count() <= 4_000);
    assert_eq!(data.core.conversation(&id).unwrap().title, "nginx timeout");
    let id = data.conversation(ConversationKind::Chat).await;
    data.core
        .append_user_message(id, "界".repeat(10_000), vec![paste.clone(), paste], vec![])
        .await
        .unwrap();
    let request = seen.lock().unwrap().last().cloned().unwrap();
    assert!(request.input.chars().count() <= 4_000);
    assert_eq!(
        request
            .input
            .chars()
            .take(2_000)
            .filter(|c| *c == '界')
            .count(),
        2_000
    );
    data.done().await;
}

fn script(reply: &'static str, hold: bool, started: Arc<tokio::sync::Notify>) -> Script {
    Arc::new(move |turn| {
        let started = started.clone();
        Box::pin(async move {
            if turn.prompt.contains("You write conversation titles") {
                started.notify_one();
                if hold {
                    turn.stopped().await;
                }
                return Reply::text(reply);
            }
            Reply::text("Main answer")
        })
    })
}
async fn title_flow(
    name: &str,
    reply: &'static str,
    hold: bool,
) -> (Flow, Arc<tokio::sync::Notify>) {
    let started = Arc::new(tokio::sync::Notify::new());
    let flow = Flow::start(
        name,
        Options {
            behavior: Arc::new(FakeBehavior {
                light_model: true,
                ..Default::default()
            }),
            ..Default::default()
        },
        script(reply, hold, started.clone()),
    )
    .await;
    (flow, started)
}
async fn route_rows(flow: &Flow, text: &str) {
    std::fs::write(flow.manager.models_rules_path(), text).unwrap();
}

#[tokio::test]
async fn title_routing_uses_work_rows_rules_quota_and_account_selection() {
    let (flow, _) = title_flow("title-routing", "Fix sidebar hover", false).await;
    route_rows(&flow, "| Conversation titles | codex | luna | low | chosen |\n| Conversation titles | claude | opus | low | explicit alternative |\n").await;
    let (choice, _) = flow.manager.title_route().await.unwrap().unwrap();
    assert_eq!(choice.provider, ProviderKind::Codex);
    assert_eq!(choice.model.as_deref(), Some("gpt-6-luna"));
    route_rows(&flow, "| Conversation titles | codex | luna | low | chosen |\n| Conversation titles | claude | opus | low | explicit alternative |\n- forbid for Conversation titles: luna\n").await;
    assert_eq!(
        flow.manager
            .title_route()
            .await
            .unwrap()
            .unwrap()
            .0
            .provider,
        ProviderKind::Claude
    );
    route_rows(&flow, "| Conversation titles | codex | luna | low | chosen |\n| Conversation titles | claude | opus | low | explicit alternative |\n").await;
    flow.manager.runtime.monitor().note(
        &AccountRef::own(ProviderKind::Codex),
        &brigadier_providers::QuotaSnapshot {
            provider: ProviderKind::Codex,
            windows: vec![],
            limit: Some(brigadier_providers::LimitHit {
                kind: brigadier_providers::LimitKind::UsageWindow,
                window: None,
                resets_at_ms: Some(crate::now_ms() + 60_000),
            }),
            observed_at_ms: crate::now_ms(),
            source: brigadier_providers::QuotaSource::Read,
        },
        crate::now_ms(),
    );
    assert_eq!(
        flow.manager
            .title_route()
            .await
            .unwrap()
            .unwrap()
            .0
            .provider,
        ProviderKind::Claude
    );
    flow.add_accounts(&[(ProviderKind::Codex, "free")], true)
        .await;
    let (choice, account) = flow.manager.title_route().await.unwrap().unwrap();
    assert_eq!(choice.provider, ProviderKind::Codex);
    assert_eq!(account.account.as_deref(), Some("free"));
    route_rows(&flow, "| Conversation titles | codex | luna | low | chosen |\n- effort-ceiling for Conversation titles: minimal\n").await;
    assert!(flow.manager.title_route().await.unwrap().is_none());
    flow.stop().await;
}

#[tokio::test]
async fn first_message_generation_is_nonblocking_and_has_no_extra_workblock() {
    let (flow, started) = title_flow("title-nonblocking", "Fix sidebar hover", true).await;
    route_rows(
        &flow,
        "| Conversation titles | codex | luna | low | chosen |\n",
    )
    .await;
    let id = flow
        .manager
        .create_conversation(
            ConversationKind::Chat,
            None,
            None,
            Some(crate::SetupRequest::Chat {
                model: crate::ModelChoice {
                    provider: ProviderKind::Claude,
                    model: Some("claude-opus-5-5".into()),
                    effort: None,
                    fast: None,
                    account: None,
                },
                permission: crate::PermissionLevel::Full,
            }),
        )
        .await
        .unwrap()
        .id;
    tokio::time::timeout(
        Duration::from_secs(5),
        flow.manager.send_message(
            id.clone(),
            "Fix the sidebar hover".into(),
            vec![],
            vec![],
            false,
            None,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        flow.core.conversation(&id).unwrap().title,
        "Fix the sidebar hover"
    );
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    flow.idle_in(&id).await;
    assert!(
        flow.core
            .list_messages(id.clone(), None, 20)
            .await
            .unwrap()
            .messages
            .iter()
            .any(|m| m.text == "Main answer")
    );
    let spec = flow
        .specs()
        .into_iter()
        .find(|(_, spec)| {
            spec.append_system_prompt
                .as_deref()
                .is_some_and(|p| p.contains("You write conversation titles"))
        })
        .unwrap()
        .1;
    assert_eq!(spec.tools, brigadier_providers::ToolSet::None);
    assert_eq!(spec.access, brigadier_providers::Access::ReadOnly);
    assert!(spec.ephemeral);
    assert!(spec.mcp_servers.is_empty());
    assert!(spec.skill_plugins.is_empty());
    assert!(spec.env.is_empty());
    let board = flow.core.board(&id).await.unwrap();
    assert_eq!(board.requests.len(), 1);
    assert!(board.tasks.is_empty());
    flow.core
        .rename_conversation(id.clone(), "Manual title".into())
        .await
        .unwrap();
    // Cleanup ends the held titler; it cannot change the manually chosen title.
    flow.stop().await;
}

#[tokio::test]
async fn timeout_and_invalid_reply_keep_the_provisional_without_retries() {
    let (flow, started) = title_flow("title-timeout", "Fix sidebar hover", true).await;
    route_rows(
        &flow,
        "| Conversation titles | codex | luna | low | chosen |\n",
    )
    .await;
    let manager = flow.manager.clone();
    let run = tokio::spawn(async move { manager.generate_title("Fix hover").await });
    tokio::time::timeout(Duration::from_secs(5), started.notified())
        .await
        .unwrap();
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(31)).await;
    assert!(run.await.unwrap().is_none());
    tokio::time::resume();
    flow.stop().await;
    let (flow, _) = title_flow("title-invalid", "{\"title\":\"Oops\"}", false).await;
    route_rows(
        &flow,
        "| Conversation titles | codex | luna | low | chosen |\n",
    )
    .await;
    let id = flow
        .core
        .create_conversation(
            ConversationId::generate(),
            ConversationKind::Chat,
            None,
            None,
            None,
            Origin::default(),
        )
        .await
        .unwrap()
        .id;
    flow.core
        .append_message(id.clone(), "Fix hover".into())
        .await
        .unwrap();
    // Wait for title transport cleanup instead of an arbitrary sleep.
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if flow.specs().iter().any(|(_, s)| {
                s.append_system_prompt
                    .as_deref()
                    .is_some_and(|p| p.contains("You write conversation titles"))
                    && !s.cwd.exists()
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(flow.core.conversation(&id).unwrap().title, "Fix hover");
    assert_eq!(
        flow.specs()
            .iter()
            .filter(|(_, s)| s
                .append_system_prompt
                .as_deref()
                .is_some_and(|p| p.contains("You write conversation titles")))
            .count(),
        1
    );
    flow.stop().await;
}

#[tokio::test]
async fn explicitly_named_workers_are_preserved_and_only_unnamed_workers_generate() {
    const NAMED: &str = "Keep my explicit worker title exactly as supplied";
    let script: Script = Arc::new(move |turn| {
        Box::pin(async move {
            if turn.prompt.contains("You write conversation titles") {
                return Reply::text("Fix unnamed worker");
            }
            if turn.worker().is_some() {
                return Reply::text("Worker done");
            }
            turn.start_worker(NAMED, "Named worker brief").await;
            turn.start_worker("", "Unnamed worker brief").await;
            Reply::text("Started both")
        })
    });
    let flow = Flow::start(
        "title-workers",
        Options {
            behavior: Arc::new(FakeBehavior {
                light_model: true,
                ..Default::default()
            }),
            ..Default::default()
        },
        script,
    )
    .await;
    route_rows(
        &flow,
        "| Conversation titles | codex | luna | low | chosen |\n",
    )
    .await;
    flow.say("Start workers").await;
    flow.until("only unnamed worker generated", |board| {
        board
            .tasks
            .values()
            .any(|t| t.title == "Fix unnamed worker")
    })
    .await;
    let board = flow.core.board(&flow.conversation).await.unwrap();
    assert!(board.tasks.values().any(|t| t.title == NAMED));
    assert_eq!(board.tasks.len(), 2);
    assert_eq!(
        flow.specs()
            .iter()
            .filter(|(_, s)| s
                .append_system_prompt
                .as_deref()
                .is_some_and(|p| p.contains("You write conversation titles")))
            .count(),
        1
    );
    flow.stop().await;
}

#[tokio::test]
async fn unavailable_shipped_light_models_never_fall_back_to_strong_models() {
    let flow = Flow::start(
        "title-no-strong-fallback",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("Done") })),
    )
    .await;
    assert!(flow.manager.title_route().await.unwrap().is_none());
    assert!(flow.manager.generate_title("Fix hover").await.is_none());
    assert!(flow.specs().iter().all(|(_, s)| {
        !s.append_system_prompt
            .as_deref()
            .is_some_and(|p| p.contains("You write conversation titles"))
    }));
    flow.stop().await;
}

#[tokio::test]
async fn shipped_title_work_selects_a_small_model_and_obeys_the_model_switch() {
    let (flow, _) = title_flow("title-shipped-routing", "Fix hover", false).await;
    let (choice, _) = flow.manager.title_route().await.unwrap().unwrap();
    assert_eq!(choice.provider, ProviderKind::Codex);
    assert_eq!(choice.model.as_deref(), Some("gpt-6-luna"));
    assert_eq!(choice.effort.as_deref(), Some("low"));
    assert_eq!(
        flow.manager.generate_title("Fix hover").await.as_deref(),
        Some("Fix hover")
    );
    let mut settings = flow.core.settings();
    settings.hidden_models.push(crate::model::ModelRef {
        provider: ProviderKind::Codex,
        id: "gpt-6-luna".into(),
    });
    flow.core.update_settings(settings).await.unwrap();
    assert!(flow.manager.title_route().await.unwrap().is_none());
    flow.stop().await;
}

#[tokio::test]
async fn shipped_routing_weighs_quota_small_model_cost_and_work_suitability() {
    let flow = Flow::start(
        "title-heat-and-fit",
        Options {
            behavior: Arc::new(FakeBehavior {
                title_models: true,
                ..Default::default()
            }),
            ..Default::default()
        },
        Arc::new(|_| Box::pin(async { Reply::text("Done") })),
    )
    .await;
    let selected = flow.manager.title_route().await.unwrap().unwrap().0;
    // 5.6 Luna is Light, whereas 6 Luna is Standard in the registry. Operate-only
    // Haiku 5.5 must never take this short-text job despite being the newer Haiku.
    assert_eq!(selected.model.as_deref(), Some("gpt-5.6-luna"));
    let note_heat = |provider, used_percent| {
        flow.manager.runtime.monitor().note(
            &AccountRef::own(provider),
            &brigadier_providers::QuotaSnapshot {
                provider,
                windows: vec![brigadier_providers::QuotaWindow {
                    id: "week".into(),
                    label: "Week".into(),
                    used_percent,
                    resets_at_ms: Some(crate::now_ms() + 60_000),
                    window_minutes: Some(10080),
                    bucket: None,
                    model: None,
                }],
                limit: None,
                observed_at_ms: crate::now_ms(),
                source: brigadier_providers::QuotaSource::Read,
            },
            crate::now_ms(),
        );
    };
    note_heat(ProviderKind::Codex, 90.0);
    let selected = flow.manager.title_route().await.unwrap().unwrap().0;
    assert_eq!(selected.provider, ProviderKind::Claude);
    assert_eq!(selected.model.as_deref(), Some("claude-haiku-4-5-20251001"));
    note_heat(ProviderKind::Codex, 5.0);
    note_heat(ProviderKind::Claude, 99.0);
    assert_eq!(
        flow.manager
            .title_route()
            .await
            .unwrap()
            .unwrap()
            .0
            .provider,
        ProviderKind::Codex
    );
    flow.stop().await;
}
