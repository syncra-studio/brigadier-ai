//! The pinned card's pull request row: what its buttons go by (checks, review decision,
//! conflicts, and unresolved review threads from every page of GitHub's GraphQL answer, so an
//! approved pull request with a comment still waiting shows it).

use serde_json::json;

use super::auto_merge_tests::{script, stub_gh};
use super::{Flow, Options, Reply};
use crate::work::{PullRequestCi, PullRequestState};

#[cfg(unix)]
#[tokio::test]
async fn an_approved_pull_request_with_an_unresolved_thread_has_comments_to_address() {
    let flow = Flow::start(
        "pr-card",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("Look around.").await;
    flow.settled().await;
    let gh = flow.repo.parent().unwrap().join("gh");
    *flow.manager.gh_override.lock().unwrap() = Some(stub_gh(&gh));
    let list = |state: &str, merge_commit: Option<&str>| {
        std::fs::write(
            gh.join("pr-list"),
            json!([{
                "number": 7,
                "title": "Add notes",
                "url": "https://github.com/o/n/pull/7",
                "state": state,
                "isDraft": false,
                "baseRefName": "main",
                "mergeCommit": merge_commit.map(|oid| json!({ "oid": oid })),
                "reviewDecision": "APPROVED",
                "mergeable": "MERGEABLE",
                "statusCheckRollup": [{ "__typename": "CheckRun", "status": "COMPLETED", "conclusion": "SUCCESS" }],
            }])
            .to_string(),
        )
        .unwrap();
    };
    let page = |resolved: &[bool]| {
        json!({ "data": { "repository": { "pullRequest": { "reviewThreads": {
            "nodes": resolved.iter().map(|done| json!({ "isResolved": done })).collect::<Vec<_>>(),
            "pageInfo": { "hasNextPage": false, "endCursor": null },
        } } } } })
    };
    std::fs::write(
        gh.join("threads"),
        json!([page(&[true, true]), page(&[false])]).to_string(),
    )
    .unwrap();

    list("OPEN", None);
    let pr = flow
        .manager
        .pull_request(&flow.conversation)
        .await
        .unwrap()
        .expect("the row");
    assert_eq!(pr.state, PullRequestState::Open);
    assert!(!pr.changes_requested, "approved");
    assert_eq!(pr.unresolved, 1, "a thread on the second page waits");
    assert_eq!(pr.ci, Some(PullRequestCi::Passing));
    assert!(!pr.conflicts);
    let calls = std::fs::read_to_string(gh.join("calls")).unwrap();
    assert!(
        calls.lines().any(|call| call
            .starts_with("api graphql --hostname github.com --paginate --slurp")
            && call.contains("owner=o")
            && call.contains("name=n")
            && call.contains("number=7")),
        "{calls}"
    );

    // Merged: Revert goes by its merge commit and base; no threads are looked up.
    std::fs::remove_file(gh.join("calls")).unwrap();
    list("MERGED", Some("f00dfeed"));
    let pr = flow
        .manager
        .pull_request(&flow.conversation)
        .await
        .unwrap()
        .expect("the row");
    assert_eq!(pr.state, PullRequestState::Merged);
    assert_eq!(pr.merge_commit.as_deref(), Some("f00dfeed"));
    assert_eq!(pr.base.as_deref(), Some("main"));
    assert_eq!(pr.unresolved, 0);
    let calls = std::fs::read_to_string(gh.join("calls")).unwrap();
    assert!(!calls.contains("graphql"), "{calls}");
    flow.stop().await;
}
