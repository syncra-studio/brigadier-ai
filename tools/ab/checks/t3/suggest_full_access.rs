//! T3, hidden from both arms: the thread has a `suggest_full_access { reason }` tool; a worker
//! doesn't.

use brigadier_core::model::{ConversationId, TaskId};
use brigadier_core::tools::{Role, RunTools};
use brigadier_mcp_server::tools_for;

fn thread(run: RunTools) -> Role {
    Role::Orchestrator {
        conversation_id: ConversationId("c1".into()),
        run,
    }
}

#[test]
fn t3_the_thread_can_suggest_full_access_with_a_reason() {
    for run in [RunTools::None, RunTools::Run, RunTools::WithEscalation] {
        let tools = tools_for(&thread(run));
        let tool = tools
            .iter()
            .find(|tool| tool.name == "suggest_full_access")
            .unwrap_or_else(|| panic!("no suggest_full_access for a thread with {run:?}"));
        let schema = serde_json::Value::Object((*tool.input_schema).clone());
        assert_eq!(
            schema["properties"]["reason"]["type"], "string",
            "reason is not a string: {schema}"
        );
        let required = schema["required"].as_array().cloned().unwrap_or_default();
        assert!(
            required.iter().any(|name| name == "reason"),
            "reason is not required: {schema}"
        );
    }
    let worker = Role::Worker {
        conversation_id: ConversationId("c1".into()),
        task_id: TaskId("t1".into()),
        checks: false,
    };
    assert!(
        !tools_for(&worker)
            .iter()
            .any(|tool| tool.name == "suggest_full_access"),
        "a worker has suggest_full_access"
    );
}
