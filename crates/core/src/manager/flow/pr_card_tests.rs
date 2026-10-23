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
        .pull_request
        .expect("the row");
    assert_eq!(pr.state, PullRequestState::Open);
    assert!(!pr.changes_requested, "approved");
    assert!(pr.approved);
    assert!(pr.mergeable);
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
        .pull_request
        .expect("the row");
    assert_eq!(pr.state, PullRequestState::Merged);
    assert_eq!(pr.merge_commit.as_deref(), Some("f00dfeed"));
    assert_eq!(pr.base.as_deref(), Some("main"));
    assert_eq!(pr.unresolved, 0);
    let calls = std::fs::read_to_string(gh.join("calls")).unwrap();
    assert!(!calls.contains("graphql"), "{calls}");
    flow.stop().await;
}

/// The open tab's next read sees GitHub's changed state without a CLI command.
#[cfg(unix)]
#[tokio::test]
async fn a_pr_card_poll_sees_approval_and_green_checks_without_a_push() {
    let flow = Flow::start(
        "pr-card-poll",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("Look around.").await;
    flow.settled().await;
    let gh = flow.repo.parent().unwrap().join("gh");
    *flow.manager.gh_override.lock().unwrap() = Some(stub_gh(&gh));
    let list = |review: &str| {
        std::fs::write(
            gh.join("pr-list"),
            json!([{
                "number": 7, "title": "Add notes", "url": "https://github.com/o/n/pull/7",
                "state": "OPEN", "isDraft": false,
                "reviewDecision": review, "mergeable": "MERGEABLE",
                "statusCheckRollup": [{ "status": "COMPLETED", "conclusion": "SUCCESS" }],
            }])
            .to_string(),
        )
        .unwrap();
    };
    list("REVIEW_REQUIRED");
    let first = flow.manager.pull_request(&flow.conversation).await.unwrap();
    assert!(!first.auth_required);
    let first = first.pull_request.unwrap();
    assert!(!first.approved, "Review needed");
    assert_eq!(first.ci, Some(PullRequestCi::Passing));
    // The open tab's timer, with no CLI item to trigger another read. Keep the fake clock
    // around the interval only: the gh subprocess uses the real clock.
    tokio::time::pause();
    let poll = tokio::time::sleep(std::time::Duration::from_secs(60));
    tokio::pin!(poll);
    list("APPROVED");
    tokio::time::advance(std::time::Duration::from_secs(59)).await;
    assert!(!poll.is_elapsed());
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    poll.await;
    tokio::time::resume();
    let next = flow
        .manager
        .pull_request(&flow.conversation)
        .await
        .unwrap()
        .pull_request
        .unwrap();
    assert!(next.approved && next.mergeable, "Ready");
    assert_eq!(next.ci, Some(PullRequestCi::Passing));
    let calls = std::fs::read_to_string(gh.join("calls")).unwrap();
    assert_eq!(
        calls
            .lines()
            .filter(|line| line.starts_with("pr list "))
            .count(),
        2
    );
    assert!(!calls.contains("auth status"));
    flow.stop().await;
}

#[cfg(unix)]
#[tokio::test]
async fn a_signed_out_gh_gives_the_pr_card_a_sign_in_hint_but_a_lookup_failure_does_not() {
    use std::os::unix::fs::PermissionsExt;
    let flow = Flow::start(
        "pr-card-auth",
        Options::default(),
        script(|_| async { Reply::text("Done.") }),
    )
    .await;
    flow.say("Look around.").await;
    flow.settled().await;
    let gh = flow.repo.parent().unwrap().join("gh");
    let stub = stub_gh(&gh);
    std::fs::write(
        &stub.program,
        r#"#!/bin/sh
D="$(dirname "$0")"
case "$1 $2" in
  "pr list") echo "To get started, run gh auth login" >&2; exit 4 ;;
  "auth status") test -f "$D/signed-in" ;;
  *) exit 1 ;;
esac
"#,
    )
    .unwrap();
    std::fs::set_permissions(&stub.program, std::fs::Permissions::from_mode(0o755)).unwrap();
    *flow.manager.gh_override.lock().unwrap() = Some(stub);
    let status = flow.manager.pull_request(&flow.conversation).await.unwrap();
    assert!(status.pull_request.is_none());
    assert!(
        status.auth_required,
        "the row must say gh auth login for PR status"
    );
    std::fs::write(gh.join("signed-in"), "yes").unwrap();
    let status = flow.manager.pull_request(&flow.conversation).await.unwrap();
    assert!(status.pull_request.is_none());
    assert!(
        !status.auth_required,
        "a failed lookup alone is not a sign-out"
    );
    flow.stop().await;
}
