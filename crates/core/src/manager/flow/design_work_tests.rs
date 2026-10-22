//! #503: work that makes designs or images (a mockup, a design file edited in Pencil) is routed
//! as design work, which the user's routing rules can name: "Never use X for Make designs and
//! images" keeps X off it, and off nothing else.

use std::sync::Arc;

use brigadier_router::{OverrideEffect, OverrideRule, OverrideTarget, TaskCategory};
use serde_json::json;

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::{ModelChoice, RoutePreviewOutcome};

/// What routing would run `category` on now.
async fn picked(flow: &Flow, category: TaskCategory) -> ModelChoice {
    let preview = flow
        .manager
        .preview_routes(None, Vec::new(), None)
        .await
        .into_iter()
        .find(|preview| preview.category == category)
        .expect("a preview per kind of work");
    match preview.outcome {
        RoutePreviewOutcome::Chosen { choice, .. } => choice,
        other => panic!("{category:?} would wait: {other:?}"),
    }
}

/// The thread's first turn delegates a mockup, a Pencil design and a code change.
fn delegating() -> Script {
    Arc::new(|turn: Turn| {
        Box::pin(async move {
            if !turn.is_orchestrator() {
                return Reply::text("Working.");
            }
            if turn.earlier > 0 {
                return Reply::text("");
            }
            for args in [
                json!({"effort": "medium", "title": "Settings mockup", "kind": "implement",
                       "mockup": true, "spec": "Mock up the new Settings page."}),
                json!({"effort": "medium", "title": "Sidebar design", "kind": "implement",
                       "design": true,
                       "spec": "Draw the sidebar's footer in app.pen with the Pencil tools."}),
                json!({"effort": "medium", "title": "Parser fix", "kind": "implement",
                       "spec": "Fix the parser in src/lib.rs."}),
            ] {
                let reply = turn.call("delegate_task", args).await;
                assert!(!reply.is_error, "{}", reply.text);
            }
            Reply::text("")
        })
    })
}

#[tokio::test]
async fn mockups_and_designs_route_as_design_work_and_a_never_rule_keeps_its_model_off() {
    let flow = Flow::start("design-work", Options::default(), delegating()).await;
    let first = picked(&flow, TaskCategory::Design).await;
    let model = first.model.clone().expect("a model id");
    let mut settings = flow.core.settings();
    settings.routing_overrides.push(OverrideRule {
        id: "no-designs".into(),
        effect: OverrideEffect::Never,
        target: OverrideTarget::Model {
            provider: first.provider,
            id: model.clone(),
        },
        categories: vec![TaskCategory::Design],
        areas: Vec::new(),
        project_id: None,
        created_at_ms: 0,
    });
    flow.core.update_settings(settings).await.unwrap();
    let after = picked(&flow, TaskCategory::Design).await;
    assert_ne!(
        (after.provider, after.model.as_deref()),
        (first.provider, Some(model.as_str())),
        "the rule keeps {model} off design work"
    );

    flow.say("Mock up the Settings page, draw the sidebar and fix the parser.")
        .await;
    let board = flow
        .until("the three tasks", |board| board.tasks.len() == 3)
        .await;
    for number in [1, 2] {
        let task = Flow::task(&board, number);
        assert!(task.design, "task-{number} makes designs");
        assert_eq!(
            super::super::workers::task_category(task),
            TaskCategory::Design
        );
        assert!(
            task.route.reason.contains("for designs and images"),
            "task-{number}: {}",
            task.route.reason
        );
        assert_ne!(
            task.route.choice.model.as_deref(),
            Some(model.as_str()),
            "task-{number} is kept off {model}"
        );
    }
    let code = Flow::task(&board, 3);
    assert!(!code.design);
    assert!(
        code.route.reason.contains("for implementation"),
        "{}",
        code.route.reason
    );
    flow.stop().await;
}
