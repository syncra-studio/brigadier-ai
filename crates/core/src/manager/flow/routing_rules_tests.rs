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
