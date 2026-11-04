//! `brigadier browser screenshot --out` follows the session's permission level: below Full
//! access the file must lie in the thread's checkout, outside its settings, and anything else
//! is refused before the tool runs; at Full access it goes wherever a file write may.

use std::sync::{Arc, Mutex};

use brigadier_cli::protocol::{
    CliCall, CliReply, CliRequest, GRANT_ENV, PROTOCOL, SESSION_ENV, Tool,
};
use serde_json::{Value, json};

use super::{Flow, Options, Reply, Script, Turn};
use crate::model::PermissionLevel;

/// `brigadier browser screenshot --tab browser:x --out <out>` as the turn's shell runs it: as
/// its session, with its grant, speaking `protocol`. An error comes back as `{"error": …}`.
async fn screenshot(turn: &Turn, out: &std::path::Path, protocol: u32) -> Value {
    let session = turn.env(SESSION_ENV).expect("the CLI knows its session");
    let grant = turn.env(GRANT_ENV).expect("the CLI has its grant");
    let caller = turn
        .host
        .cli_caller(session, grant)
        .expect("the session's own grant is accepted");
    let call = CliCall {
        protocol,
        request: CliRequest::Tool {
            tool: Tool::Browser,
            action: "screenshot".into(),
            args: json!({"tab": "browser:x"}),
            out: Some(out.display().to_string()),
        },
    };
    match turn.host.cli_call(&caller, call).await {
        CliReply::Ok { output } => output,
        CliReply::Error { error } => json!({ "error": serde_json::to_value(error).unwrap() }),
    }
}

/// At Accept edits a file in the checkout is let through to the tool, one beside the checkout
/// or in its `.claude` is refused, saying why; at Full access none is refused. A call of the
/// protocol before `--out` was checked is refused by its version, not run.
#[tokio::test]
async fn out_follows_the_sessions_permission_level() {
    for permission in [PermissionLevel::AcceptEdits, PermissionLevel::Full] {
        let answers: Arc<Mutex<Vec<(&'static str, Value)>>> = Arc::default();
        let script: Script = {
            let answers = answers.clone();
            Arc::new(move |turn| {
                let answers = answers.clone();
                Box::pin(async move {
                    if !turn.input.contains("Save the shots.") {
                        return Reply::text("Noted.");
                    }
                    let checkout = turn.cwd.clone();
                    for (name, out) in [
                        ("inside", checkout.join("shots/shot.png")),
                        ("outside", checkout.parent().unwrap().join("outside.png")),
                        ("settings", checkout.join(".claude/settings.json")),
                    ] {
                        let answer = screenshot(&turn, &out, PROTOCOL).await;
                        answers.lock().unwrap().push((name, answer));
                    }
                    // A `brigadier` from before `--out` was sent, which the daemon couldn't
                    // check, is told to use the app's.
                    let old = screenshot(&turn, &checkout.join("shot.png"), 2).await;
                    answers.lock().unwrap().push(("old", old));
                    Reply::text("Saved them.")
                })
            })
        };
        let options = Options {
            permission,
            ..Options::default()
        };
        let flow = Flow::start("cli-out", options, script).await;
        flow.say("Save the shots.").await;
        flow.until("it saved them", |_| answers.lock().unwrap().len() == 4)
            .await;
        flow.settled().await;
        let answers = answers.lock().unwrap().clone();
        for (name, answer) in &answers {
            if *name == "old" {
                assert_eq!(
                    answer["error"]["code"], "version",
                    "{permission:?}: {answer:#}"
                );
                let message = answer["error"]["message"].as_str().unwrap_or_default();
                assert!(message.contains("speaks protocol 2"), "{message}");
                continue;
            }
            let refused = answer["error"]["code"] == "refused"
                && answer["error"]["message"]
                    .as_str()
                    .is_some_and(|message| message.contains("--out"));
            let expected = permission != PermissionLevel::Full && *name != "inside";
            assert_eq!(refused, expected, "{permission:?} {name}: {answer:#}");
        }
        flow.stop().await;
    }
}
