//! Canonical touched PR discovery and a daemon-wide account-aware observation scheduler.
use super::{SessionManager, blocking, tracked_work::Change};
use crate::{
    ConversationId, Error, PrHistoryEvent, PrIdentity, PullRequest, PullRequestCi,
    PullRequestState, Result, TrackedPullRequest, TrackedWork,
};
use brigadier_providers::{ProviderEvent, session_file};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::Duration;

#[derive(Default)]
pub(super) struct Poller {
    state: Mutex<HashMap<String, PollSlot>>,
}
#[derive(Default)]
struct PollSlot {
    next: i64,
    busy: bool,
    cached: Option<TrackedPullRequest>,
}
struct Observation {
    pr: PullRequest,
    viewer: String,
    status: String,
    complete: bool,
    next: i64,
}

impl SessionManager {
    pub async fn watch_pull_request(
        &self,
        id: &ConversationId,
        url: &str,
        account: Option<String>,
        actor: &str,
    ) -> Result<TrackedPullRequest> {
        let identity = PrIdentity::parse(url).ok_or_else(|| {
            Error::Invalid("use a canonical https://host/owner/repo/pull/number URL".into())
        })?;
        if let Some(account) = &account
            && (account.is_empty()
                || !account
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-'))
        {
            return Err(Error::Invalid("invalid GitHub account login".into()));
        }
        self.touch_pr(id, identity, account, actor, "watching", None, true)
            .await
    }
    async fn touch_pr(
        &self,
        id: &ConversationId,
        identity: PrIdentity,
        account: Option<String>,
        actor: &str,
        action: &str,
        source: Option<String>,
        manual: bool,
    ) -> Result<TrackedPullRequest> {
        let _guard = self.tracked_writes.lock().await;
        let work = self.tracked_work(id).await?;
        let existing = work
            .pull_requests
            .into_iter()
            .find(|p| p.identity == identity);
        // A forget is durable suppression, including new automatic observations.
        if let Some(old) = &existing
            && (old.forgotten && !manual
                || source
                    .as_ref()
                    .is_some_and(|s| work.pr_history.iter().any(|e| &e.id == s)))
        {
            return Ok(old.clone());
        }
        let now = crate::now_ms();
        let mut pr = existing.unwrap_or(TrackedPullRequest {
            identity: identity.clone(),
            account: None,
            viewer: None,
            pull_request: None,
            touched_at_ms: now,
            observed_at_ms: None,
            actor: actor.into(),
            action: action.into(),
            status: "Checking status".into(),
            complete: false,
            forgotten: false,
        });
        if let Some(account) = account
            && pr.account.as_ref() != Some(&account)
        {
            pr.account = Some(account);
            pr.viewer = None;
            pr.complete = false;
            pr.observed_at_ms = None;
            pr.pull_request = None;
            pr.status = "Checking status for selected account".into();
        }
        pr.forgotten = false;
        pr.touched_at_ms = now;
        pr.actor = actor.into();
        pr.action = action.into();
        let event = PrHistoryEvent {
            id: source.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            at_ms: now,
            identity,
            actor: actor.into(),
            action: action.into(),
            detail: pr.status.clone(),
        };
        self.keep_tracked(
            id,
            &Change::Pr {
                pr: pr.clone(),
                event: Some(event),
            },
        )
        .await?;
        Ok(pr)
    }
    pub async fn forget_pull_request(&self, id: &ConversationId, key: &str) -> Result<()> {
        let _guard = self.tracked_writes.lock().await;
        let mut pr = self
            .tracked_work(id)
            .await?
            .pull_requests
            .into_iter()
            .find(|p| p.identity.key() == key || p.identity.url() == key)
            .ok_or_else(|| Error::NotFound("pull request in this session".into()))?;
        if !pr.forgotten {
            pr.forgotten = true;
            let event = PrHistoryEvent {
                id: uuid::Uuid::new_v4().to_string(),
                at_ms: crate::now_ms(),
                identity: pr.identity.clone(),
                actor: "session".into(),
                action: "forgotten".into(),
                detail: "Removed from watched pull requests".into(),
            };
            self.keep_tracked(
                id,
                &Change::Pr {
                    pr,
                    event: Some(event),
                },
            )
            .await?;
        }
        Ok(())
    }
    /// Backfill every recorded generation of both the lead and its workers. Reads only.
    pub async fn discover_pull_requests(&self, id: &ConversationId) -> Result<()> {
        let mut records = vec![("session".to_owned(), self.session_dir(id).record())];
        for task in self.core.tasks(id).await? {
            records.push((format!("worker {}", task.number), self.worker_record(&task)));
        }
        let candidates = blocking(move || {
            let mut found = Vec::new();
            for (actor, record) in records {
                for generation in super::session_dir::read_lineage(&record) {
                    let Some(file) = generation.file else {
                        continue;
                    };
                    let path =
                        session_file::locate(generation.provider, std::path::Path::new(&file));
                    for entry in session_file::read(generation.provider, &path).unwrap_or_default()
                    {
                        if let Some((identity, account, action, item)) = candidate(&entry.event) {
                            let source = format!(
                                "touch:{}",
                                blake3::hash(
                                    format!(
                                        "{actor}:{}:{item}:{}:{action}",
                                        generation.session_id,
                                        identity.key()
                                    )
                                    .as_bytes()
                                )
                                .to_hex()
                            );
                            found.push((actor.clone(), identity, account, action, source));
                        }
                    }
                }
            }
            Ok(found)
        })
        .await?;
        for (actor, identity, account, action, source) in candidates {
            self.touch_pr(id, identity, account, &actor, &action, Some(source), false)
                .await?;
        }
        Ok(())
    }
    pub async fn tracked_work_status(&self, id: &ConversationId) -> Result<TrackedWork> {
        self.discover_pull_requests(id).await?;
        self.refresh_tracked_prs(id).await?;
        self.tracked_work(id).await
    }
    pub(super) fn start_tracked_polling(&self) {
        let weak = self.me.clone();
        self.spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                let Some(manager) = weak.upgrade() else {
                    return;
                };
                if manager.admit().is_err() {
                    return;
                }
                for conversation in manager.core.catalog().conversations {
                    if conversation.lifecycle != crate::Lifecycle::Active
                        || conversation.tab_chat.is_some()
                    {
                        continue;
                    }
                    let _ = manager.deliver_waiting_wakes(&conversation.id).await;
                    if !manager.app.is_open() {
                        continue;
                    }
                    let _ = manager.discover_pull_requests(&conversation.id).await;
                    let _ = manager.refresh_tracked_prs(&conversation.id).await;
                }
            }
        });
    }
    async fn refresh_tracked_prs(&self, id: &ConversationId) -> Result<()> {
        if !self.app.is_open() {
            return Ok(());
        }
        let Some(gh) = self.gh() else {
            return Ok(());
        };
        for mut pr in self.tracked_work(id).await?.pull_requests {
            if pr.forgotten
                || pr.pull_request.as_ref().is_some_and(|p| {
                    matches!(p.state, PullRequestState::Merged | PullRequestState::Closed)
                })
            {
                continue;
            }
            let now = crate::now_ms();
            let key = format!(
                "{}:{}",
                pr.identity.key(),
                pr.account.as_deref().unwrap_or("unresolved")
            );
            let account_key = format!(
                "account:{}:{}",
                pr.identity.host,
                pr.account.as_deref().unwrap_or("unresolved")
            );
            let cached = {
                let mut state = self.pr_poll.state.lock().unwrap();
                if state
                    .get(&account_key)
                    .is_some_and(|s| s.next > now || s.busy)
                {
                    continue;
                }
                let slot = state.entry(key.clone()).or_default();
                if slot.busy {
                    continue;
                }
                if slot.next > now {
                    slot.cached.clone()
                } else {
                    slot.busy = true;
                    None
                }
            };
            let result = if let Some(cached) = cached {
                pr.pull_request = cached.pull_request;
                pr.viewer = cached.viewer;
                pr.status = cached.status;
                pr.complete = cached.complete;
                pr.observed_at_ms = cached.observed_at_ms;
                None
            } else {
                // Ambient credentials carry no intended identity: ask for an explicit account.
                let observed = if let Some(account) = &pr.account {
                    if let Some(named) = gh.for_account(&pr.identity.host, account).await {
                        observe(&named, &pr, now).await
                    } else {
                        Err(
                            "Account unavailable; use gh auth login for the selected account"
                                .into(),
                        )
                    }
                } else {
                    Err("Account needed: brigadier pr watch <url> --account <login>".into())
                };
                Some(observed)
            };
            let mut next = now
                + if now - pr.touched_at_ms < 30 * 60 * 1000 {
                    30_000
                } else {
                    120_000
                };
            if let Some(result) = result {
                match result {
                    Ok(seen) => {
                        pr.pull_request = Some(seen.pr);
                        pr.viewer = Some(seen.viewer);
                        pr.status = seen.status;
                        pr.complete = seen.complete;
                        pr.observed_at_ms = Some(now);
                        next = next.max(seen.next);
                    }
                    Err(reason) => {
                        pr.status = reason;
                        pr.complete = false;
                        next = now + 120_000;
                    }
                }
                let mut state = self.pr_poll.state.lock().unwrap();
                state.insert(
                    key,
                    PollSlot {
                        next,
                        busy: false,
                        cached: Some(pr.clone()),
                    },
                );
                // Shared primary/secondary backoff applies across this host/account.
                if next > now + 120_000 {
                    state.insert(
                        account_key,
                        PollSlot {
                            next,
                            busy: false,
                            cached: None,
                        },
                    );
                }
            }
            let _guard = self.tracked_writes.lock().await;
            let current = self
                .tracked_work(id)
                .await?
                .pull_requests
                .into_iter()
                .find(|p| p.identity == pr.identity);
            let Some(current) = current else { continue };
            if current.forgotten || current.account != pr.account {
                continue;
            }
            // Preserve touches that arrived while the network request was in flight.
            pr.touched_at_ms = current.touched_at_ms;
            pr.actor = current.actor.clone();
            pr.action = current.action.clone();
            if current != pr {
                let event = (current.status != pr.status
                    || current.pull_request.as_ref().map(|p| p.state)
                        != pr.pull_request.as_ref().map(|p| p.state))
                .then(|| PrHistoryEvent {
                    id: uuid::Uuid::new_v4().to_string(),
                    at_ms: now,
                    identity: pr.identity.clone(),
                    actor: "GitHub".into(),
                    action: "status changed".into(),
                    detail: pr.status.clone(),
                });
                // Terminal display removal never bypasses own-branch merge bookkeeping.
                if pr
                    .pull_request
                    .as_ref()
                    .is_some_and(|p| p.state == PullRequestState::Merged)
                    && self
                        .core
                        .board(id)
                        .await?
                        .pull_request
                        .as_ref()
                        .is_some_and(|p| p.url == pr.identity.url())
                    && let Ok(conv) = self.conv(id)
                {
                    conv.pr_merge_due
                        .store(true, std::sync::atomic::Ordering::SeqCst);
                    self.kick(&conv);
                }
                self.keep_tracked(id, &Change::Pr { pr, event }).await?;
            }
        }
        Ok(())
    }
}

/// Only a single proven-successful gh mutation or checkout. Never execute transcript text.
fn candidate(event: &ProviderEvent) -> Option<(PrIdentity, Option<String>, String, String)> {
    let ProviderEvent::Command {
        item_id,
        command,
        status: brigadier_providers::model::ItemStatus::Completed,
        exit_code: Some(0),
        output,
        ..
    } = event
    else {
        return None;
    };
    // A known credential-binding expression is parsed as provenance, never evaluated.
    // Literal tokens and unknown wrappers remain unresolved and are never retained.
    static BINDING: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(r"^(?:GH_CONFIG_DIR=[A-Za-z0-9_./-]+\s+)?(?:GH_TOKEN|GITHUB_TOKEN)=\$\(gh auth token (?P<flags>[A-Za-z0-9_ .-]+)\)\s+(?P<command>gh .+)$").unwrap()
    });
    let binding = BINDING.captures(command);
    let mut account = None;
    let text = if let Some(binding) = &binding {
        let flags = literal_words(binding.name("flags")?.as_str())?;
        let index = flags.iter().position(|w| w == "--user")?;
        account = Some(flags.get(index + 1)?.clone());
        binding.name("command")?.as_str()
    } else {
        command
    };
    let words = literal_words(text)?;
    let start = words.iter().position(|w| w == "gh")?;
    if words[..start].iter().any(|w| {
        w != "env"
            && !w.starts_with("GH_REPO=")
            && !w.starts_with("GH_HOST=")
            && !w.starts_with("GH_CONFIG_DIR=")
    }) {
        return None;
    }
    let args = &words[start + 1..];
    if args.first()?.as_str() != "pr" {
        return None;
    }
    let action = match args.get(1)?.as_str() {
        "create" => "opened",
        "review" => "reviewed",
        "comment" => "commented",
        "edit" => "updated",
        "merge" => "merged",
        "close" => "closed",
        "reopen" => "reopened",
        "checkout" => "checked out",
        _ => return None,
    };
    let mut identities: HashSet<String> = HashSet::new();
    let mut identity = None;
    let output_words = output
        .as_deref()
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    // Body/title flags may contain arbitrary PR links; only the positional selector and
    // create's canonical output identify the PR that was actually acted upon.
    if action == "opened" {
        for word in &output_words {
            if let Some(found) = PrIdentity::parse(word) {
                identities.insert(found.key());
                identity = Some(found);
            }
        }
    } else {
        let mut selector = None;
        let mut repo = None;
        let mut skip = false;
        for (index, word) in args.iter().enumerate().skip(2) {
            if skip {
                skip = false;
                continue;
            }
            if word == "-R" || word == "--repo" {
                repo = args.get(index + 1).map(String::as_str);
                skip = true;
                continue;
            }
            if let Some(value) = word.strip_prefix("--repo=") {
                repo = Some(value);
                continue;
            }
            if [
                "-b",
                "--body",
                "-F",
                "--body-file",
                "-c",
                "--comment",
                "--subject",
                "--title",
                "-t",
                "--base",
                "-B",
            ]
            .contains(&word.as_str())
            {
                skip = true;
                continue;
            }
            if word.starts_with('-') {
                continue;
            }
            if selector.replace(word.as_str()).is_some() {
                return None;
            }
        }
        if let Some(selector) = selector {
            identity = PrIdentity::parse(selector);
            if identity.is_none()
                && let Ok(number) = selector.parse::<u32>()
            {
                let repo = repo.or_else(|| {
                    words[..start]
                        .iter()
                        .find_map(|w| w.strip_prefix("GH_REPO="))
                })?;
                let host = words[..start]
                    .iter()
                    .find_map(|w| w.strip_prefix("GH_HOST="))
                    .unwrap_or("github.com");
                let repo = repo.strip_prefix("https://").unwrap_or(repo);
                let parts: Vec<_> = repo.split('/').collect();
                let url = match parts.as_slice() {
                    [owner, repo] => format!("https://{host}/{owner}/{repo}/pull/{number}"),
                    [host, owner, repo] => format!("https://{host}/{owner}/{repo}/pull/{number}"),
                    _ => return None,
                };
                identity = PrIdentity::parse(&url);
            }
        }
        if let Some(found) = &identity {
            identities.insert(found.key());
        }
    }
    if identities.len() != 1 {
        return None;
    }
    Some((identity?, account, action.into(), item_id.clone()))
}

fn literal_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut quote = None;
    let mut escape = false;
    for c in command.chars() {
        if escape {
            word.push(c);
            escape = false;
            continue;
        }
        if c == '\\' && quote != Some('\'') {
            escape = true;
            continue;
        }
        if let Some(q) = quote {
            if c == q {
                quote = None;
            } else if (c == '$' || c == '`') && q == '"' {
                return None;
            } else {
                word.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => quote = Some(c),
            ';' | '&' | '|' | '>' | '<' | '$' | '`' | '\n' | '(' | ')' | '{' | '}' => return None,
            c if c.is_whitespace() => {
                if !word.is_empty() {
                    words.push(std::mem::take(&mut word));
                }
            }
            _ => word.push(c),
        }
    }
    if quote.is_some() || escape {
        return None;
    }
    if !word.is_empty() {
        words.push(word);
    }
    Some(words)
}

const QUERY: &str = r#"query($owner:String!,$repo:String!,$number:Int!,$checks:String,$threads:String,$reviews:String,$requests:String){viewer{login} rateLimit{cost limit remaining resetAt} repository(owner:$owner,name:$repo){pullRequest(number:$number){number title url state isDraft headRefOid baseRefName mergeCommit{oid} author{login} mergedBy{login} reviewDecision mergeable mergeStateStatus reviewRequests(first:100,after:$requests){totalCount pageInfo{hasNextPage endCursor} nodes{requestedReviewer{... on User{login} ... on Team{name}}}} latestOpinionatedReviews(first:100,after:$reviews){totalCount pageInfo{hasNextPage endCursor} nodes{state author{login}}} reviewThreads(first:100,after:$threads){totalCount pageInfo{hasNextPage endCursor} nodes{isResolved}} commits(last:1){nodes{commit{statusCheckRollup{state contexts(first:100,after:$checks){totalCount pageInfo{hasNextPage endCursor} nodes{... on CheckRun{name status conclusion} ... on StatusContext{context state}}}}}}}}}}"#;
async fn observe(
    gh: &super::gh::Gh,
    tracked: &TrackedPullRequest,
    now: i64,
) -> std::result::Result<Observation, String> {
    let identity = &tracked.identity;
    let mut cursors = json!({"owner":identity.owner,"repo":identity.repo,"number":identity.number});
    let mut checks = Vec::new();
    let mut threads = Vec::new();
    let mut reviews = Vec::new();
    let mut requests = Vec::new();
    let mut seen_cursors = HashSet::new();
    let mut next = now;
    let mut complete = false;
    let mut newest = Value::Null;
    let mut viewer = String::new();
    let mut head = None;
    // Hard bound protects secondary limits; incomplete results remain explicitly partial.
    for _ in 0..20 {
        let body = json!({"query":QUERY,"variables":cursors}).to_string();
        let output = gh
            .output_with_input(
                &[
                    "api",
                    "graphql",
                    "--hostname",
                    &identity.host,
                    "--include",
                    "--input",
                    "-",
                ],
                &body,
            )
            .await
            .map_err(|_| "GitHub unavailable; status is stale".to_string())?;
        let (headers, payload) = split_headers(&output.stdout);
        if let Some(delay) = retry_after(headers, now) {
            next = next.max(delay);
        }
        let value: Value = serde_json::from_str(payload)
            .map_err(|_| "GitHub unavailable; status is stale".to_string())?;
        if let Some(rate) = value.pointer("/data/rateLimit") {
            // Use actual returned cost, limit, remaining and reset, never a node estimate.
            let cost = rate["cost"].as_i64().unwrap_or(1);
            let remaining = rate["remaining"].as_i64().unwrap_or(0);
            let limit = rate["limit"].as_i64().unwrap_or(0);
            if remaining < cost.max((limit / 20).max(1))
                && let Some(reset) = rate["resetAt"]
                    .as_str()
                    .and_then(|s| s.parse::<jiff::Timestamp>().ok())
            {
                next = next.max(reset.as_millisecond());
            }
        }
        if !output.success() || value.get("errors").is_some() {
            // The caller retains the previous observation. No raw stderr or API messages.
            return Ok(Observation {
                pr: tracked
                    .pull_request
                    .clone()
                    .unwrap_or_else(|| unknown_pr(identity)),
                viewer: tracked.viewer.clone().unwrap_or_default(),
                status: "GitHub unavailable; status is stale".into(),
                complete: false,
                next: next.max(now + 120_000),
            });
        }
        viewer = value
            .pointer("/data/viewer/login")
            .and_then(Value::as_str)
            .ok_or("GitHub account identity unavailable")?
            .into();
        if tracked
            .account
            .as_deref()
            .is_some_and(|a| !a.eq_ignore_ascii_case(&viewer))
        {
            return Err("Selected GitHub account identity changed; sign in again".into());
        }
        let pr = value
            .pointer("/data/repository/pullRequest")
            .ok_or("Pull request unavailable for this account")?;
        if pr.is_null() {
            return Err("Pull request unavailable for this account".into());
        }
        let current_head = pr["headRefOid"].as_str().map(str::to_owned);
        if head
            .as_ref()
            .is_some_and(|h| Some(h) != current_head.as_ref())
        {
            newest = pr.clone();
            break;
        }
        if head.is_none() {
            head = current_head;
        }
        let connections = [
            (
                "checks",
                pr.pointer("/commits/nodes/0/commit/statusCheckRollup/contexts"),
                &mut checks,
            ),
            ("threads", pr.get("reviewThreads"), &mut threads),
            ("reviews", pr.get("latestOpinionatedReviews"), &mut reviews),
            ("requests", pr.get("reviewRequests"), &mut requests),
        ];
        let mut more = false;
        let mut valid = true;
        for (name, connection, nodes) in connections {
            // No rollup is valid: there are no checks yet, never interpreted as passing.
            let Some(connection) = connection else {
                if name != "checks" {
                    valid = false;
                }
                continue;
            };
            let Some(page) = connection.get("pageInfo") else {
                valid = false;
                continue;
            };
            if !cursors.get(name).is_some_and(Value::is_null) {
                nodes.extend(connection["nodes"].as_array().cloned().unwrap_or_default());
            }
            if page["hasNextPage"] == true {
                let Some(cursor) = page["endCursor"].as_str() else {
                    valid = false;
                    continue;
                };
                if !seen_cursors.insert(format!("{name}:{cursor}")) {
                    valid = false;
                    continue;
                }
                cursors[name] = json!(cursor);
                more = true;
            } else {
                cursors[name] = Value::Null;
            }
            if !more
                && connection["totalCount"]
                    .as_u64()
                    .is_some_and(|n| n > nodes.len() as u64)
            {
                valid = false;
            }
        }
        newest = pr.clone();
        if !valid {
            break;
        }
        if !more {
            complete = true;
            break;
        }
        if next > now {
            break;
        }
    }
    let pr = &newest;
    let state = match pr["state"].as_str() {
        Some("MERGED") => PullRequestState::Merged,
        Some("CLOSED") => PullRequestState::Closed,
        _ if pr["isDraft"] == true => PullRequestState::Draft,
        _ => PullRequestState::Open,
    };
    let failing = checks.iter().any(|c| {
        [
            "FAILURE",
            "ERROR",
            "TIMED_OUT",
            "CANCELLED",
            "ACTION_REQUIRED",
            "STARTUP_FAILURE",
            "STALE",
        ]
        .contains(
            &c["conclusion"]
                .as_str()
                .or(c["state"].as_str())
                .unwrap_or(""),
        )
    });
    let pending = checks.iter().any(|c| {
        c["status"].as_str().is_some_and(|s| s != "COMPLETED")
            || c["state"] == "PENDING"
            || c["conclusion"].is_null() && c["state"].is_null()
    });
    let ci = if failing {
        Some(PullRequestCi::Failing)
    } else if !complete || pending {
        Some(PullRequestCi::Pending)
    } else if !checks.is_empty() {
        Some(PullRequestCi::Passing)
    } else {
        None
    };
    let unresolved = threads.iter().filter(|t| t["isResolved"] == false).count() as u32;
    let approved = complete && pr["reviewDecision"] == "APPROVED";
    let changes = pr["reviewDecision"] == "CHANGES_REQUESTED";
    let own_changes = reviews.iter().any(|r| {
        r["state"] == "CHANGES_REQUESTED"
            && r.pointer("/author/login")
                .and_then(Value::as_str)
                .is_some_and(|a| a.eq_ignore_ascii_case(&viewer))
    });
    let status = match state {
        PullRequestState::Merged => format!(
            "Merged{}",
            pr.pointer("/mergedBy/login")
                .and_then(Value::as_str)
                .map(|s| format!(" by {s}"))
                .unwrap_or_default()
        ),
        PullRequestState::Closed => "Closed".into(),
        PullRequestState::Draft => "Draft".into(),
        _ if failing => "CI failing".into(),
        _ if !complete => "Status incomplete; checking remaining pages".into(),
        _ if changes
            && own_changes
            && pr.pointer("/author/login").and_then(Value::as_str) != Some(viewer.as_str()) =>
        {
            "Waiting on author: you asked for changes".into()
        }
        _ if changes => "Changes requested".into(),
        _ if unresolved > 0 => format!("{unresolved} unresolved review threads"),
        _ if pending => "CI running".into(),
        _ if pr["mergeable"] == "CONFLICTING" => "Merge conflicts".into(),
        _ if !requests.is_empty() => format!(
            "Waiting on {}",
            requests
                .iter()
                .filter_map(|r| r
                    .pointer("/requestedReviewer/login")
                    .or_else(|| r.pointer("/requestedReviewer/name")))
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ if approved
            && ci == Some(PullRequestCi::Passing)
            && pr["mergeable"] == "MERGEABLE"
            && pr["mergeStateStatus"] == "CLEAN" =>
        {
            "Ready to merge".into()
        }
        _ if !approved => "Waiting on review".into(),
        _ => "Checking merge requirements".into(),
    };
    Ok(Observation {
        pr: PullRequest {
            number: identity.number,
            title: pr["title"].as_str().unwrap_or("Pull request").into(),
            url: identity.url(),
            state,
            base: pr["baseRefName"].as_str().map(str::to_owned),
            merge_commit: pr
                .pointer("/mergeCommit/oid")
                .and_then(Value::as_str)
                .map(str::to_owned),
            ci,
            approved,
            mergeable: complete
                && pr["mergeable"] == "MERGEABLE"
                && pr["mergeStateStatus"] == "CLEAN",
            conflicts: pr["mergeable"] == "CONFLICTING",
            changes_requested: changes,
            unresolved,
        },
        viewer,
        status,
        complete,
        next,
    })
}
fn unknown_pr(identity: &PrIdentity) -> PullRequest {
    PullRequest {
        number: identity.number,
        title: "Pull request".into(),
        url: identity.url(),
        state: PullRequestState::Open,
        base: None,
        merge_commit: None,
        ci: None,
        approved: false,
        mergeable: false,
        conflicts: false,
        changes_requested: false,
        unresolved: 0,
    }
}
fn split_headers(output: &str) -> (&str, &str) {
    if output.starts_with("HTTP/") {
        if let Some((headers, body)) = output
            .split_once("\r\n\r\n")
            .or_else(|| output.split_once("\n\n"))
        {
            return (headers, body);
        }
    }
    ("", output)
}
fn retry_after(headers: &str, now: i64) -> Option<i64> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        if !key.eq_ignore_ascii_case("retry-after") {
            return None;
        }
        value
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|s| *s >= 0)
            .map(|s| now + s.saturating_mul(1000))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compound_commands_never_prove_a_touch() {
        for c in [
            "gh pr review https://github.com/o/r/pull/1; echo ok",
            "false && gh pr create",
            "gh pr review 1 | cat",
            "echo 'gh pr review 1'",
        ] {
            assert!(
                literal_words(c).is_none()
                    || !literal_words(c).unwrap().first().is_some_and(|w| w == "gh")
            );
        }
    }
    fn command(text: &str, output: &str, exit: i32) -> ProviderEvent {
        ProviderEvent::Command {
            item_id: "call-1".into(),
            command: text.into(),
            cwd: None,
            status: brigadier_providers::model::ItemStatus::Completed,
            exit_code: Some(exit),
            output: Some(output.into()),
            duration_ms: None,
            kept: None,
        }
    }
    #[test]
    fn discovery_requires_success_and_actual_selector() {
        let url = "https://github.com/o/r/pull/12";
        assert!(candidate(&command("gh pr view 12", url, 0)).is_none());
        assert!(candidate(&command(&format!("gh pr review {url}"), "", 1)).is_none());
        assert!(candidate(&command(&format!("gh pr review {url}; echo done"), "", 0)).is_none());
        assert!(candidate(&command(&format!("gh pr comment --body '{url}' 7"), "", 0)).is_none());
        let (pr, account, action, _) = candidate(&command(
            "GH_TOKEN=$(gh auth token --user org-user) gh pr review 12 -R org/repo --approve",
            "",
            0,
        ))
        .unwrap();
        assert_eq!(pr.key(), "github.com/org/repo/12");
        assert_eq!(account.as_deref(), Some("org-user"));
        assert_eq!(action, "reviewed");
        assert!(
            candidate(&command(
                "GH_TOKEN=secret gh pr review 12 -R org/repo",
                "",
                0
            ))
            .is_none()
        );
        let (pr, _, action, _) =
            candidate(&command("gh pr create --title 'Changes'", url, 0)).unwrap();
        assert_eq!(pr.number, 12);
        assert_eq!(action, "opened");
    }
    #[test]
    fn header_backoff_is_safe_and_exact() {
        let (h, b) = split_headers("HTTP/2.0 429\r\nRetry-After: 90\r\n\r\n{}");
        assert_eq!(b, "{}");
        assert_eq!(retry_after(h, 1000), Some(91000));
    }
}

#[cfg(all(test, unix))]
mod query_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn tracked() -> TrackedPullRequest {
        TrackedPullRequest {
            identity: PrIdentity::parse("https://github.com/org/repo/pull/7").unwrap(),
            account: Some("alice".into()),
            viewer: None,
            pull_request: None,
            touched_at_ms: 0,
            observed_at_ms: None,
            actor: "worker 1".into(),
            action: "reviewed".into(),
            status: "Checking".into(),
            complete: false,
            forgotten: false,
        }
    }
    fn stub(pages: Vec<Value>) -> (std::path::PathBuf, super::super::gh::Gh) {
        let dir = std::env::temp_dir().join(format!("brigadier-pr-query-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (i, page) in pages.iter().enumerate() {
            std::fs::write(dir.join(format!("page-{i}.json")), page.to_string()).unwrap();
        }
        let script = dir.join("gh");
        std::fs::write(
            &script,
            r#"#!/usr/bin/env python3
import sys,pathlib
root=pathlib.Path(__file__).parent
if sys.argv[1:3]==['auth','token']:
 print('private-test-token');sys.exit(0)
_ = sys.stdin.read()
count=root/'count'
i=int(count.read_text()) if count.exists() else 0
count.write_text(str(i+1))
print('HTTP/2.0 200\r\n\r\n'+(root/('page-'+str(i)+'.json')).read_text())
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let gh = super::super::gh::Gh {
            program: script,
            vars: std::env::vars_os().collect(),
        };
        (dir, gh)
    }
    fn page(checks: Vec<Value>, has_next: bool) -> Value {
        let conn = |nodes: Vec<Value>, total: usize, more: bool| json!({"nodes":nodes,"totalCount":total,"pageInfo":{"hasNextPage":more,"endCursor":if more {Some("next")}else{None}}});
        json!({"data":{"viewer":{"login":"alice"},"rateLimit":{"cost":17,"limit":5000,"remaining":4000,"resetAt":"2099-01-01T00:00:00Z"},"repository":{"pullRequest":{"number":7,"title":"Changes","url":"https://github.com/org/repo/pull/7","state":"OPEN","isDraft":false,"reviewDecision":"APPROVED","mergeable":"MERGEABLE","mergeStateStatus":"CLEAN","reviewThreads":conn(vec![],0,false),"reviewRequests":conn(vec![],0,false),"latestOpinionatedReviews":conn(vec![],0,false),"commits":{"nodes":[{"commit":{"statusCheckRollup":{"state":"SUCCESS","contexts":conn(checks,101,has_next)}}}]}}}}})
    }
    #[tokio::test]
    async fn paginated_failure_cannot_be_ready_and_named_account_is_private() {
        let first=page((0..100).map(|i|json!({"name":format!("check-{i}"),"status":"COMPLETED","conclusion":"SUCCESS"})).collect(),true);
        let second = page(
            vec![json!({"name":"last","status":"COMPLETED","conclusion":"STARTUP_FAILURE"})],
            false,
        );
        let (dir, gh) = stub(vec![first, second]);
        let named = gh.for_account("github.com", "alice").await.unwrap();
        assert!(!format!("{named:?}").contains("private-test-token"));
        let seen = observe(&named, &tracked(), 0).await.unwrap();
        assert!(seen.complete);
        assert_eq!(seen.pr.ci, Some(PullRequestCi::Failing));
        assert_eq!(seen.status, "CI failing");
        assert_eq!(std::fs::read_to_string(dir.join("count")).unwrap(), "2");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn incomplete_connections_and_wrong_viewer_never_claim_ready() {
        let mut missing = page(vec![], false);
        missing["data"]["repository"]["pullRequest"]["reviewThreads"] = json!({"nodes":[]});
        let (dir, gh) = stub(vec![missing]);
        let seen = observe(&gh, &tracked(), 0).await.unwrap();
        assert!(!seen.complete);
        assert!(!seen.pr.approved);
        assert!(!seen.pr.mergeable);
        std::fs::remove_dir_all(dir).unwrap();
        let mut wrong = page(vec![], false);
        wrong["data"]["viewer"]["login"] = json!("bob");
        let (dir, gh) = stub(vec![wrong]);
        assert!(observe(&gh, &tracked(), 0).await.is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}

impl SessionManager {
    /// The own-branch lookup is canonical discovery. Keep its terminal transition even
    /// when it disappears from the summary; never revive a suppressed identity.
    pub(super) async fn observe_own_branch_pr(&self,id:&ConversationId,found:&PullRequest)->Result<()> {
        let Some(identity)=PrIdentity::parse(&found.url) else{return Ok(())};
        let source=format!("branch:{}",identity.key());
        self.touch_pr(id,identity.clone(),None,"session","branch pull request",Some(source),false).await?;
        let _guard=self.tracked_writes.lock().await;
        let Some(mut pr)=self.tracked_work(id).await?.pull_requests.into_iter().find(|p|p.identity==identity) else{return Ok(())};
        if pr.forgotten {return Ok(());}
        let terminal=matches!(found.state,PullRequestState::Merged|PullRequestState::Closed);
        if pr.pull_request.is_some() && (!terminal || pr.pull_request.as_ref().is_some_and(|p|p.state==found.state)) {return Ok(());}
        pr.pull_request=Some(found.clone());pr.complete=false;pr.observed_at_ms=Some(crate::now_ms());
        pr.status=match found.state {PullRequestState::Merged=>"Merged",PullRequestState::Closed=>"Closed",_ if pr.account.is_none()=>"Account needed for review status",_=>"Checking status"}.into();
        let event=PrHistoryEvent{id:uuid::Uuid::new_v4().to_string(),at_ms:crate::now_ms(),identity,actor:"GitHub".into(),action:"status changed".into(),detail:pr.status.clone()};
        self.keep_tracked(id,&Change::Pr{pr,event:Some(event)}).await
    }
}
