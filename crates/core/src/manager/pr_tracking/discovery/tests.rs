use super::*;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("brigadier-pr-discovery-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
    fn repo(&self) -> PathBuf {
        let repo = self.0.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        for args in [
            vec!["init", "-q", "-b", "today"],
            vec!["remote", "add", "origin", "git@github.com:org/repo.git"],
        ] {
            assert!(
                std::process::Command::new("git")
                    .arg("-C")
                    .arg(&repo)
                    .args(args)
                    .status()
                    .unwrap()
                    .success()
            );
        }
        repo
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn event(command: &str, output: &str, cwd: Option<&Path>, exit: i32) -> ProviderEvent {
    ProviderEvent::Command {
        item_id: "action".into(),
        command: command.into(),
        cwd: cwd.map(|p| p.display().to_string()),
        status: if exit == 0 {
            ItemStatus::Completed
        } else {
            ItemStatus::Failed
        },
        exit_code: Some(exit),
        output: Some(output.into()),
        duration_ms: None,
        kept: None,
    }
}
fn read(
    reader: &mut Reader,
    command: &str,
    output: &str,
    cwd: Option<&Path>,
    at: i64,
) -> Option<Action> {
    reader.observe(
        &event(command, output, cwd, 0),
        None,
        Some(at),
        Some(at),
        None,
    )
}
fn at() -> i64 {
    crate::now_ms() + 100
}
fn key(action: &Action) -> String {
    action
        .identity
        .as_ref()
        .map(PrIdentity::key)
        .unwrap_or_default()
}
fn repo() -> Repo {
    Repo::parse("org/repo", "github.com").unwrap()
}
fn head() -> String {
    "b".repeat(40)
}
fn pr(number: u32, branch: &str, oid: &str, source: &str) -> Value {
    json!({"number":number,"url":format!("https://github.com/org/repo/pull/{number}"),"headRefName":branch,"headRefOid":oid,"createdAt":"2000-01-01T00:00:00Z","closedAt":null,"headRepository":{"url":format!("https://github.com/{source}"),"nameWithOwner":source}})
}
fn graph(nodes: Vec<Value>) -> Value {
    json!({"data":{"viewer":{"login":"org-user"},"rateLimit":{"cost":3,"limit":5000,"remaining":4900,"resetAt":"2099-01-01T00:00:00Z"},"repository":{"pullRequests":{"totalCount":nodes.len(),"nodes":nodes,"pageInfo":{"hasNextPage":false,"endCursor":null}}}}})
}
fn stub(scratch: &Scratch, pages: Vec<Value>) -> Gh {
    let file = scratch.0.join("gh");
    std::fs::write(scratch.0.join("pages.json"), json!(pages).to_string()).unwrap();
    std::fs::write(
        &file,
        r#"#!/usr/bin/env python3
import sys,json,pathlib
root=pathlib.Path(__file__).parent
if sys.argv[1:3]==['auth','token']:
 print('fixture-private-token');sys.exit(0)
body=json.loads(sys.stdin.read() or '{}')
count=root/'count'
i=int(count.read_text()) if count.exists() else 0
count.write_text(str(i+1))
pages=json.loads((root/'pages.json').read_text())
print('HTTP/2.0 200\r\n\r\n'+json.dumps(pages[min(i,len(pages)-1)]))
"#,
    )
    .unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).unwrap();
    Gh {
        program: file,
        vars: vec![("PATH".into(), "/usr/bin:/bin".into())],
    }
}

#[test]
fn action_receipts_and_explicit_targets_cover_pr_mutations() {
    for (command, output, action) in [
        (
            "gh pr create --title 'Changes' --reviewer jane --assignee user",
            "https://github.com/org/repo/pull/7",
            "opened",
        ),
        (
            "gh pr review --approve",
            "✓ Approved pull request org/repo#7 (Changes)",
            "reviewed",
        ),
        (
            "gh pr comment --body hello",
            "https://github.com/org/repo/pull/7#issuecomment-123",
            "commented",
        ),
        ("gh pr edit -R org/repo 7 --add-label ready", "", "updated"),
        ("gh pr ready 7 -R org/repo", "", "updated"),
        ("gh pr merge -R org/repo 7 --squash --auto", "", "merged"),
        (
            "gh pr close https://github.com/org/repo/pull/7",
            "",
            "closed",
        ),
        ("gh pr reopen 7 --repo=org/repo", "", "reopened"),
        (
            "gh pr checkout 7 -R org/repo --branch review-7",
            "",
            "checked out",
        ),
        ("gh pr checks 7 -R org/repo --watch", "", "watched"),
        (
            "until gh pr checks 7 -R org/repo; do sleep 30; done",
            "",
            "watched",
        ),
    ] {
        let a = read(&mut Reader::default(), command, output, None, at()).unwrap();
        assert_eq!(key(&a), "github.com/org/repo/7", "{command}");
        assert_eq!(a.action, action);
    }
}
#[test]
fn api_write_routes_and_canonical_response_are_not_ticket_guesses() {
    for (command, output, action) in [
        (
            "gh api -X POST repos/org/repo/pulls",
            r#"{"number":7,"html_url":"https://github.com/org/repo/pull/7"}"#,
            "opened",
        ),
        (
            "gh api --method=POST repos/org/repo/pulls --jq .number",
            "7",
            "opened",
        ),
        (
            "gh api repos/org/repo/pulls/7/reviews -f body=nice",
            "{}",
            "reviewed",
        ),
        (
            "gh api -X POST repos/org/repo/pulls/7/reviews/123/events -f event=APPROVE",
            "{}",
            "reviewed",
        ),
        (
            "gh api -X POST repos/org/repo/pulls/7/comments -f body=nice",
            "{}",
            "commented",
        ),
        (
            "gh api -X PUT https://api.github.com/repos/org/repo/pulls/7/merge",
            "{}",
            "merged",
        ),
        (
            "gh api -X PATCH repos/org/repo/pulls/7 -f title=New",
            "{}",
            "updated",
        ),
        (
            "gh api -X PATCH repos/org/repo/pulls/comments/333 -f body=updated",
            r#"{"pull_request_url":"https://api.github.com/repos/org/repo/pulls/7"}"#,
            "commented",
        ),
    ] {
        let a = read(&mut Reader::default(), command, output, None, at()).unwrap();
        assert_eq!(key(&a), "github.com/org/repo/7", "{command}");
        assert_eq!(a.action, action);
    }
    for command in [
        "gh issue comment 7 -R org/repo --body nice",
        "gh api -X POST repos/org/repo/issues/7/comments -f body=nice",
    ] {
        let a = read(&mut Reader::default(), command, "{}", None, at()).unwrap();
        assert!(a.identity.is_none());
        assert!(matches!(a.lookup, Some(Lookup::Number(_, 7))));
    }
}
#[test]
fn reads_masked_failures_docs_and_dynamic_refs_never_touch() {
    for command in [
        "gh pr view 7 -R org/repo",
        "gh pr diff https://github.com/org/repo/pull/7",
        "gh pr list -R org/repo",
        "gh pr checks 7 -R org/repo",
        "gh api repos/org/repo/pulls/7",
        "gh api -X GET repos/org/repo/pulls/7 -f body=ignored",
        "echo 'gh pr review https://github.com/org/repo/pull/7'",
        "printf '%s' 'https://github.com/org/repo/pull/7'",
        "false && gh pr create",
        "gh pr review 7 -R org/repo; echo done",
        "git push origin HEAD:topic; echo done",
        "git push origin HEAD:$BRANCH",
        "cd $(pwd) && gh pr review 7 -R org/repo",
        "GH_TOKEN=literal-secret gh pr review 7 -R org/repo",
        "mystery gh pr review 7 -R org/repo",
    ] {
        assert!(
            read(
                &mut Reader::default(),
                command,
                "https://github.com/org/repo/pull/7",
                None,
                at()
            )
            .is_none(),
            "{command}"
        );
    }
    assert!(
        read(
            &mut Reader::default(),
            "gh pr create --title 'https://github.com/org/repo/pull/7'",
            "Done",
            None,
            at()
        )
        .is_none()
    );
    assert!(
        Reader::default()
            .observe(
                &event("gh pr review 7 -R org/repo", "", None, 1),
                None,
                Some(at()),
                Some(at()),
                None
            )
            .is_none()
    );
}
#[test]
fn cwd_origin_and_literal_cd_are_scoped_and_historical_head_is_not_current_head() {
    let scratch = Scratch::new();
    let cwd = scratch.repo();
    let when = at();
    let full = read(
        &mut Reader::default(),
        "gh pr review https://github.com/other/project/pull/8 --approve",
        "",
        Some(&cwd),
        when,
    )
    .unwrap();
    assert_eq!(
        key(&full),
        "github.com/other/project/8",
        "an explicit URL precedes cwd origin"
    );
    let a = read(
        &mut Reader::default(),
        "gh pr review 7 --approve",
        "",
        Some(&cwd),
        when,
    )
    .unwrap();
    assert_eq!(key(&a), "github.com/org/repo/7");
    let command = format!("cd '{}' && gh pr comment 7 --body hello", cwd.display());
    assert_eq!(
        key(&read(&mut Reader::default(), &command, "", None, when).unwrap()),
        "github.com/org/repo/7"
    );
    // HEAD in this real repository says 'today'. No transcript witness => no current-PR lookup.
    assert!(
        read(
            &mut Reader::default(),
            "gh pr review -R org/repo --approve",
            "",
            Some(&cwd),
            when
        )
        .is_none()
    );
    let mut reader = Reader::default();
    read(
        &mut reader,
        "git branch --show-current",
        "historical",
        Some(&cwd),
        when,
    );
    read(&mut reader, "git rev-parse HEAD", &head(), Some(&cwd), when);
    let a = read(
        &mut reader,
        "gh pr review -R org/repo --approve",
        "",
        Some(&cwd),
        when + 1,
    )
    .unwrap();
    assert!(
        matches!(a.lookup,Some(Lookup::Branch{branch,oid,..}) if branch=="historical"&&oid==head())
    );
    reader.observe(
        &event("git checkout newer; false", "", Some(&cwd), 1),
        None,
        Some(when + 2),
        Some(when + 2),
        None,
    );
    assert!(
        read(
            &mut reader,
            "gh pr review -R org/repo --approve",
            "",
            Some(&cwd),
            when + 3
        )
        .is_none()
    );
    // A changed config cannot be used as origin evidence for an earlier action.
    assert!(
        read(
            &mut Reader::default(),
            "gh pr review 7 --approve",
            "",
            Some(&cwd),
            0
        )
        .is_none()
    );
}
#[test]
fn push_receipt_refspec_and_head_are_required_and_git_c_is_literal() {
    let output = "To git@github.com:org/repo.git\n  aaaaaaa..bbbbbbb feature -> target\n";
    for command in [
        "git push origin feature:target",
        "git push -u origin HEAD:refs/heads/target",
        "git -C /recorded/path push origin feature:target",
    ] {
        let a = read(&mut Reader::default(), command, output, None, at()).unwrap();
        assert_eq!(a.action, "pushed");
        assert!(
            matches!(a.lookup,Some(Lookup::Branch{branch,oid,head_repo,..})if branch=="target"&&oid=="bbbbbbb"&&head_repo==repo())
        );
    }
    let porcelain = "To https://github.com/org/repo.git\n \trefs/heads/feature:refs/heads/target\taaaaaaa..bbbbbbb\nDone\n";
    assert!(
        read(
            &mut Reader::default(),
            "git push --porcelain origin feature:target",
            porcelain,
            None,
            at()
        )
        .is_some()
    );
    for (command, output) in [
        ("git push origin other:target", output),
        ("git push origin feature:other", output),
        ("git push --delete origin target", output),
        ("git push origin feature:target", "Everything up-to-date"),
        (
            "git push origin feature:target",
            "To https://github.com/org/repo.git\naaaaaaa..bbbbbbb feature -> target\naaaaaaa..ccccccc second -> second",
        ),
        (
            "git push origin HEAD:target",
            "To https://github.com/org/repo.git\n * [new branch] HEAD -> target",
        ),
    ] {
        assert!(
            read(&mut Reader::default(), command, output, None, at()).is_none(),
            "{command}"
        );
    }
}
#[test]
fn checkout_to_review_uses_recorded_head_but_is_not_a_posted_review() {
    let scratch = Scratch::new();
    let cwd = scratch.repo();
    let when = at();
    for command in [
        "git checkout feature",
        "git worktree add -b review-7 /tmp/review-7 origin/feature",
    ] {
        let a = read(
            &mut Reader::default(),
            command,
            "Preparing worktree\nHEAD is now at bbbbbbb changes",
            Some(&cwd),
            when,
        )
        .unwrap();
        assert_eq!(a.action, "checked out");
        assert!(
            matches!(a.lookup,Some(Lookup::Branch{branch,oid,..})if branch=="feature"&&oid=="bbbbbbb")
        );
    }
    assert!(
        read(
            &mut Reader::default(),
            "git checkout feature",
            "Switched to branch 'feature'",
            Some(&cwd),
            when
        )
        .is_none()
    );
}
#[test]
fn named_account_bindings_and_proven_wrappers_do_not_store_or_execute_tokens() {
    let command = "GH_CONFIG_DIR=/tmp/gh-org GH_TOKEN=\"$(gh auth token --user org-user)\" gh pr review 7 -R org/repo --approve";
    let a = read(&mut Reader::default(), command, "", None, at()).unwrap();
    assert_eq!(a.account.as_deref(), Some("org-user"));
    let command = "hpgh() { GH_TOKEN=$(gh auth token --user org-user) gh \"$@\"; }; hpgh pr comment 7 -R org/repo --body hello";
    let a = read(&mut Reader::default(), command, "", None, at()).unwrap();
    assert_eq!(a.account.as_deref(), Some("org-user"));
    assert_eq!(a.action, "commented");
    let scratch = Scratch::new();
    let script = scratch.0.join("hpgh");
    std::fs::write(
        &script,
        "#!/bin/sh\nGH_TOKEN=$(gh auth token --user org-user) gh \"$@\"\n",
    )
    .unwrap();
    let when = at();
    let command = format!("{} pr review 7 -R org/repo --approve", script.display());
    let a = read(&mut Reader::default(), &command, "", None, when).unwrap();
    assert_eq!(a.account.as_deref(), Some("org-user"));
    assert!(
        read(
            &mut Reader::default(),
            "hpgh pr review 7 -R org/repo --approve",
            "",
            None,
            when
        )
        .is_none()
    );
    let mut reader = Reader::default();
    read(
        &mut reader,
        "command -v hpgh",
        script.to_str().unwrap(),
        None,
        when,
    );
    assert_eq!(
        read(
            &mut reader,
            "hpgh pr review 7 -R org/repo --approve",
            "",
            None,
            when + 1
        )
        .unwrap()
        .account
        .as_deref(),
        Some("org-user")
    );
    assert!(read(&mut Reader::default(), &command, "", None, 0).is_none());
    let hostile = "hpgh() { echo leaked; GH_TOKEN=$(gh auth token --user org-user) gh \"$@\"; }; hpgh pr review 7 -R org/repo";
    assert!(read(&mut Reader::default(), hostile, "", None, when).is_none());
}
#[tokio::test]
async fn branch_resolution_checks_head_repo_oid_lifetime_and_uniqueness() {
    let scratch = Scratch::new();
    let a = Action {
        identity: None,
        lookup: Some(Lookup::Branch {
            repos: vec![repo()],
            branch: "target".into(),
            head_repo: repo(),
            oid: "bbbbbbb".into(),
            at: at(),
        }),
        account: Some("org-user".into()),
        action: "pushed".into(),
        item: "1".into(),
        at: Some(at()),
    };
    let gh = stub(
        &scratch,
        vec![graph(vec![pr(7, "target", &head(), "org/repo")])],
    );
    let result = resolve(&gh, &a, 0, &mut 20).await;
    assert_eq!(result.identity.unwrap().number, 7);
    for nodes in [
        vec![pr(7, "target", &"a".repeat(40), "org/repo")],
        vec![pr(7, "target", &head(), "other/repo")],
        vec![
            pr(7, "target", &head(), "org/repo"),
            pr(8, "target", &head(), "org/repo"),
        ],
    ] {
        let scratch = Scratch::new();
        let gh = stub(&scratch, vec![graph(nodes)]);
        assert!(resolve(&gh, &a, 0, &mut 20).await.identity.is_none());
    }
    let mut future = pr(7, "target", &head(), "org/repo");
    future["createdAt"] = json!("2099-01-01T00:00:00Z");
    let scratch = Scratch::new();
    let gh = stub(&scratch, vec![graph(vec![future])]);
    assert!(resolve(&gh, &a, 0, &mut 20).await.identity.is_none());
}
#[tokio::test]
async fn issue_lookup_does_not_bind_ambient_viewer_and_rate_cost_stops_pagination() {
    let a = Action {
        identity: None,
        lookup: Some(Lookup::Number(repo(), 7)),
        account: None,
        action: "commented".into(),
        item: "1".into(),
        at: Some(at()),
    };
    let scratch = Scratch::new();
    let gh = stub(
        &scratch,
        vec![json!({"data":{"viewer":{"login":"not-inferred"},"repository":{"pullRequest":null}}})],
    );
    assert!(resolve(&gh, &a, 0, &mut 20).await.identity.is_none());
    let mut page = graph(vec![pr(7, "target", &head(), "org/repo")]);
    page["data"]["rateLimit"]["remaining"] = json!(1);
    page["data"]["repository"]["pullRequests"]["pageInfo"] =
        json!({"hasNextPage":true,"endCursor":"more"});
    let scratch = Scratch::new();
    let gh = stub(&scratch, vec![page]);
    let a = Action {
        lookup: Some(Lookup::Branch {
            repos: vec![repo()],
            branch: "target".into(),
            head_repo: repo(),
            oid: head(),
            at: at(),
        }),
        ..a
    };
    let result = resolve(&gh, &a, 0, &mut 20).await;
    assert!(result.identity.is_none());
    assert!(result.retry_at.unwrap() > 0);
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("count")).unwrap(),
        "1"
    );
}

async fn generation(record: PathBuf, name: &str, commands: &[(&str, &str)], cwd: &Path) {
    use super::super::super::session_dir::{LineageEntry, note_lineage_at};
    std::fs::create_dir_all(&record).unwrap();
    let file = record.join(format!("{name}.jsonl"));
    let now = at();
    let lines=commands.iter().enumerate().map(|(index,(command,output))|json!({"type":"event_msg","timestamp":jiff::Timestamp::from_millisecond(now+index as i64).unwrap().to_string(),"payload":{"type":"item_completed","item":{"type":"CommandExecution","id":format!("call-{index}"),"command":command,"cwd":cwd.display().to_string(),"status":"completed","exit_code":0,"aggregated_output":output}}}).to_string()).collect::<Vec<_>>().join("\n");
    std::fs::write(&file, format!("{lines}\n")).unwrap();
    note_lineage_at(
        record,
        LineageEntry {
            provider: brigadier_providers::ProviderKind::Codex,
            session_id: name.into(),
            file: Some(file.display().to_string()),
            started_at: now,
            forgotten: false,
        },
    )
    .await
    .unwrap();
}
#[tokio::test]
async fn recorded_actions_cache_positive_negative_retry_and_forget_across_restart() {
    use super::super::super::flow::{Flow, Options, Reply};
    let scratch = Scratch::new();
    let mut flow = Flow::start(
        "pr-discovery-cache",
        Options::default(),
        Arc::new(|_| Box::pin(async { Reply::text("Done") })),
    )
    .await;
    let id = flow.conversation.clone();
    let choice = flow
        .core
        .conversation(&id)
        .unwrap()
        .setup
        .unwrap()
        .choice()
        .clone();
    let plain = serde_json::from_value(json!({"permission":"full","local":true})).unwrap();
    let task = flow
        .manager
        .create_plain_task(
            &id,
            "Worker".into(),
            "Read only".into(),
            choice,
            plain,
            None,
        )
        .await
        .unwrap();
    flow.until("worker finished", |board| {
        board.tasks.get(&task.id).is_some_and(|t| {
            !matches!(
                t.state,
                crate::TaskState::Queued | crate::TaskState::Starting | crate::TaskState::Running
            )
        })
    })
    .await;
    let h = head();
    let record = flow.manager.session_dir(&id).record();
    generation(
        record.clone(),
        "lead-discovery",
        &[
            ("git remote get-url origin", "git@github.com:org/repo.git"),
            ("git branch --show-current", "topic"),
            ("git rev-parse HEAD", &h),
            (
                "GH_TOKEN=$(gh auth token --user org-user) gh pr review --approve",
                "",
            ),
        ],
        &flow.repo,
    )
    .await;
    generation(
        flow.manager.worker_record(&task),
        "worker-discovery",
        &[
            (
                "git push origin HEAD:worker",
                "To git@github.com:org/repo.git\naaaaaaa..bbbbbbb HEAD -> worker",
            ),
            (
                "gh api -X POST repos/org/repo/issues/9/comments -f body=hello",
                "{}",
            ),
        ],
        &flow.repo,
    )
    .await;
    let gh = stub(
        &scratch,
        vec![
            graph(vec![pr(42, "topic", &h, "org/repo")]),
            graph(vec![pr(43, "worker", &h, "org/repo")]),
            json!({"data":{"viewer":{"login":"not-inferred"},"repository":{"pullRequest":null}}}),
        ],
    );
    *flow.manager.gh_override.lock().unwrap() = Some(gh.clone());
    let _app = flow.manager.subscribe_app();
    flow.manager.discover_pull_requests(&id).await.unwrap();
    let first = flow.manager.tracked_work(&id).await.unwrap();
    assert_eq!(first.pull_requests.len(), 2);
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("count")).unwrap(),
        "3"
    );
    assert_eq!(
        first
            .pull_requests
            .iter()
            .find(|p| p.identity.number == 42)
            .unwrap()
            .account
            .as_deref(),
        Some("org-user")
    );
    assert!(
        first
            .pull_requests
            .iter()
            .find(|p| p.identity.number == 43)
            .unwrap()
            .account
            .is_none(),
        "ambient viewer is not account provenance"
    );
    assert!(
        first
            .pull_requests
            .iter()
            .find(|p| p.identity.number == 43)
            .unwrap()
            .actor
            .starts_with("worker ")
    );
    assert!(
        !serde_json::to_string(&first)
            .unwrap()
            .contains("fixture-private-token")
    );
    flow.manager.discover_pull_requests(&id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("count")).unwrap(),
        "3"
    );
    for p in &first.pull_requests {
        flow.manager
            .forget_pull_request(&id, &p.identity.key())
            .await
            .unwrap();
    }
    flow.restart().await;
    *flow.manager.gh_override.lock().unwrap() = Some(gh.clone());
    let _app2 = flow.manager.subscribe_app();
    flow.manager.discover_pull_requests(&id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("count")).unwrap(),
        "3",
        "positive AND issue-not-PR results are durable"
    );
    assert!(
        flow.manager
            .tracked_work(&id)
            .await
            .unwrap()
            .pull_requests
            .iter()
            .all(|pr| pr.forgotten)
    );
    let limited = json!({"data":{"rateLimit":{"cost":12,"limit":5000,"remaining":0,"resetAt":"2099-01-01T00:00:00Z"},"repository":null},"errors":[{"type":"RATE_LIMITED"}]});
    std::fs::write(scratch.0.join("pages.json"), json!([limited]).to_string()).unwrap();
    generation(
        record.clone(),
        "rate-limited-action",
        &[("gh issue comment 10 -R org/repo --body next", "")],
        &flow.repo,
    )
    .await;
    flow.manager.discover_pull_requests(&id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("count")).unwrap(),
        "4"
    );
    flow.manager.discover_pull_requests(&id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("count")).unwrap(),
        "4"
    );
    flow.restart().await;
    *flow.manager.gh_override.lock().unwrap() = Some(gh);
    let _app3 = flow.manager.subscribe_app();
    generation(
        record,
        "another-action-on-blocked-account",
        &[("gh issue comment 11 -R org/repo --body next", "")],
        &flow.repo,
    )
    .await;
    flow.manager.discover_pull_requests(&id).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("count")).unwrap(),
        "4",
        "rate reset persists and gates other historical lookups too"
    );
    flow.stop().await;
}
#[tokio::test]
async fn actual_request_budget_counts_pagination_not_just_action_candidates() {
    let scratch = Scratch::new();
    let mut page = graph(vec![pr(7, "target", &head(), "org/repo")]);
    page["data"]["repository"]["pullRequests"]["pageInfo"] =
        json!({"hasNextPage":true,"endCursor":"next"});
    let gh = stub(&scratch, vec![page]);
    let action = Action {
        identity: None,
        lookup: Some(Lookup::Branch {
            repos: vec![repo()],
            branch: "target".into(),
            head_repo: repo(),
            oid: head(),
            at: at(),
        }),
        account: None,
        action: "pushed".into(),
        item: "1".into(),
        at: Some(at()),
    };
    let mut budget = 1;
    let resolved = resolve(&gh, &action, 0, &mut budget).await;
    assert!(resolved.identity.is_none());
    assert_eq!(budget, 0);
    assert_eq!(
        std::fs::read_to_string(scratch.0.join("count")).unwrap(),
        "1"
    );
}
#[tokio::test]
async fn old_worker_head_capture_cannot_cross_native_id_generations() {
    let scratch = Scratch::new();
    let record = scratch.0.join("record");
    let command = "git push origin HEAD:target";
    generation(record.clone(), "one", &[(command, "")], &scratch.0).await;
    std::fs::write(
        record.join("commands.jsonl"),
        json!({"line":"ran","item":"call-0","command":command,"head":head()}).to_string() + "\n",
    )
    .unwrap();
    assert_eq!(captured_heads(&record).len(), 1);
    generation(record.clone(), "two", &[(command, "")], &scratch.0).await;
    assert!(captured_heads(&record).is_empty());
}
