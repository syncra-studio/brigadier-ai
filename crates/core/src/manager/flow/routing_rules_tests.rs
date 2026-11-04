//! The rules file and routing as the manager reads them, without a CLI turn, so these run on
//! every platform (Windows too): a rules line that can't be read (#762).
use std::sync::Arc;

use brigadier_providers::ProviderKind;

use super::{Flow, Options, Reply, Script};
use crate::manager::cli::CliCaller;
use crate::model::{ConversationId, ModelChoice};

fn quiet() -> Script {
    Arc::new(|_| Box::pin(async { Reply::text("") }))
}

/// #762: one typo in the rules file used to empty the Fallback rows and refuse every move, a
/// worker's own model too, with nothing said. The other rules and the Fallback rows hold; the
/// line is named in `brigadier models` and by an explicit action.
#[tokio::test]
async fn a_bad_rules_line_leaves_fallback_and_the_other_rules_on() {
    let flow = Flow::start("routing-bad-line", Options::default(), quiet()).await;
    let manager = &flow.manager;
    std::fs::write(
        manager.models_rules_path(),
        "# My rules\n\n- forbid: fable\n- effort-ceiling: hgih\n",
    )
    .unwrap();

    // Sessions and workers at a limit still have somewhere to go.
    let fallback = manager.fallback_rows();
    assert!(
        [ProviderKind::Claude, ProviderKind::Codex]
            .iter()
            .all(|agent| fallback.iter().any(|row| row.provider == *agent)),
        "{fallback:#?}"
    );
    assert!(manager.successor_model(ProviderKind::Codex).is_ok());
    // A worker's own model, judged for a retry and for a move, is not refused.
    let own = ModelChoice {
        provider: ProviderKind::Claude,
        model: Some("claude-opus-5-5".into()),
        effort: Some("high".into()),
        fast: None,
        account: None,
    };
    for moving in [false, true] {
        assert_eq!(
            manager
                .ruled_quota_choice(own.clone(), None, moving)
                .unwrap(),
            own
        );
    }
    // The rules that can be read still hold.
    assert!(
        manager
            .model_refusal(ProviderKind::Claude, Some("claude-fable-5-1"), None, None)
            .unwrap()
            .is_some_and(|rule| rule.contains("forbid: fable"))
    );

    // The line is named: by an explicit action, and in `brigadier models`.
    let refused = manager.check_rules_file().unwrap_err().to_string();
    assert!(
        refused.contains("line 4: - effort-ceiling: hgih"),
        "{refused}"
    );
    let caller = CliCaller {
        conversation_id: ConversationId::generate(),
        task_id: None,
        grant: String::new(),
    };
    let models = manager.cli_models(&caller).await.unwrap();
    assert_eq!(models.invalid_rules, ["line 4: - effort-ceiling: hgih"]);
    assert_eq!(
        models
            .rules
            .iter()
            .map(|rule| rule.rule.as_str())
            .collect::<Vec<_>>(),
        ["forbid: fable"]
    );
    let page = manager.model_rules().unwrap();
    assert_eq!(page.rules.len(), 1, "the Routing page still opens");

    std::fs::write(manager.models_rules_path(), "- effort-ceiling: high\n").unwrap();
    manager.check_rules_file().unwrap();
    flow.stop().await;
}

/// What a `brigadier start` answered, as the CLI prints it with `--json`.
fn reply(reply: brigadier_cli::protocol::CliReply) -> serde_json::Value {
    match reply {
        brigadier_cli::protocol::CliReply::Ok { output } => output,
        brigadier_cli::protocol::CliReply::Error { error } => {
            serde_json::json!({ "error": error })
        }
    }
}

/// #743: an agent switched off, or a model made unavailable, in Settings › Providers takes no
/// worker a start by its Work picks, and `brigadier models` doesn't list its models as
/// available; the automatic rows give the switched-off agent no row back. An explicit
/// `--agent` still takes that agent's own row of the order, its model and effort (D34).
#[tokio::test]
async fn a_start_by_work_skips_what_providers_keeps_out_unless_asked_for() {
    use brigadier_cli::protocol::StartRequest;
    let flow = Flow::start("routing-switched-off", Options::default(), quiet()).await;
    let manager = &flow.manager;
    std::fs::write(
        manager.models_rules_path(),
        "| Work | Agent | Model | Effort | Reason |\n| --- | --- | --- | --- | --- |\n| Docs | claude | opus | low | First. |\n| Docs | codex | astra | medium | Next. |\n| Notes | claude | opus | low | Only. |\n",
    )
    .unwrap();
    let caller = CliCaller {
        conversation_id: flow.conversation.clone(),
        task_id: None,
        grant: String::new(),
    };
    let start = |work: &str, agent: Option<ProviderKind>| StartRequest {
        title: None,
        agent,
        model: None,
        effort: None,
        work: Some(work.into()),
        reviewing: Vec::new(),
        local: true,
        repo: None,
        base: None,
        permission: None,
        mcps: Vec::new(),
        with: Vec::new(),
        mockup: false,
        prompt: "Reply once.".into(),
    };
    let started = async |work: &str, agent: Option<ProviderKind>| {
        let seen = reply(
            manager
                .cli_start(&caller, start(work, agent))
                .await
                .unwrap(),
        );
        let worker = &seen["worker"];
        (
            worker["agent"].as_str().map(str::to_owned),
            worker["model"].as_str().map(str::to_owned),
            worker["effort"].as_str().map(str::to_owned),
            seen,
        )
    };
    let owned = |agent: &str, model: &str, effort: &str| {
        (
            Some(agent.to_owned()),
            Some(model.to_owned()),
            Some(effort.to_owned()),
        )
    };

    // Claude switched off.
    let mut settings = flow.core.settings();
    settings.disabled_providers.push(ProviderKind::Claude);
    flow.core.update_settings(settings).await.unwrap();
    let automatic = manager.work_rows("Architecture and broad changes").unwrap();
    assert!(
        !automatic.is_empty()
            && automatic
                .iter()
                .all(|row| row.agent != ProviderKind::Claude),
        "{automatic:#?}"
    );
    let models = manager.cli_models(&caller).await.unwrap();
    for model in &models.models {
        assert_eq!(
            model.available,
            model.agent != ProviderKind::Claude,
            "{model:?}"
        );
    }
    let (agent, model, effort, seen) = started("Docs", None).await;
    assert_eq!(
        (agent, model, effort),
        owned("codex", "gpt-6-astra", "medium"),
        "{seen:#}"
    );
    let (agent, ..) = started("Architecture and broad changes", None).await;
    assert_eq!(agent.as_deref(), Some("codex"));
    // The automatic rows left Claude's out, yet `--agent claude` still takes Claude's own row of
    // that kind, its model and effort (D34), not Claude's default at Codex's effort.
    let (agent, model, effort, seen) =
        started("Architecture and broad changes", Some(ProviderKind::Claude)).await;
    assert_eq!(
        (agent, model, effort),
        owned("claude", "claude-opus-5-5", "high"),
        "{seen:#}"
    );
    // A kind whose only row is Claude's still starts on the agent asked for, at the row's
    // effort: that row's agent being off doesn't matter when another agent runs it.
    let (agent, _, effort, seen) = started("Notes", Some(ProviderKind::Codex)).await;
    assert_eq!(
        (agent.as_deref(), effort.as_deref()),
        (Some("codex"), Some("low")),
        "{seen:#}"
    );
    // Asked for by name, the agent's own row still starts (D34), off the routing, which says so.
    let (agent, model, effort, seen) = started("Docs", Some(ProviderKind::Claude)).await;
    assert_eq!(
        (agent, model, effort),
        owned("claude", "claude-opus-5-5", "low"),
        "{seen:#}"
    );
    assert!(
        seen["offRouting"]
            .as_str()
            .is_some_and(|line| line.contains("Codex")),
        "{seen:#}"
    );

    // Claude on, its model made unavailable: the same, and the start is on the routing.
    let mut settings = flow.core.settings();
    settings.disabled_providers.clear();
    settings.hidden_models.push(crate::model::ModelRef {
        provider: ProviderKind::Claude,
        id: "claude-opus-5-5".into(),
    });
    flow.core.update_settings(settings).await.unwrap();
    let (agent, model, effort, seen) = started("Docs", None).await;
    assert_eq!(
        (agent, model, effort),
        owned("codex", "gpt-6-astra", "medium"),
        "{seen:#}"
    );
    assert!(seen["offRouting"].is_null(), "{seen:#}");
    let (agent, model, effort, seen) = started("Docs", Some(ProviderKind::Claude)).await;
    assert_eq!(
        (agent, model, effort),
        owned("claude", "claude-opus-5-5", "low"),
        "{seen:#}"
    );

    // Nothing the order names can run: refused, saying why, not started on a row kept out.
    let mut settings = flow.core.settings();
    settings.disabled_providers.push(ProviderKind::Codex);
    flow.core.update_settings(settings).await.unwrap();
    let (agent, .., seen) = started("Docs", None).await;
    assert_eq!(agent, None);
    assert_eq!(seen["error"]["code"], "refused", "{seen:#}");
    assert_eq!(
        seen["error"]["message"],
        "refused: every row of the work \"Docs\" names an agent or a model switched off in \
         Settings › Providers; switch one on, or give --agent",
        "{seen:#}"
    );
    flow.stop().await;
}
