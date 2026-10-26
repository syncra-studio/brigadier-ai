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
    round: tokio::sync::Mutex<()>,
}
#[derive(Default)]
struct PollSlot {
    next: i64,
    failures: u32,
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
        source: Option<(String, Option<i64>)>,
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
                    .is_some_and(|s| work.pr_history.iter().any(|e| e.id == s.0)))
        {
            return Ok(old.clone());
        }
        let now = source
            .as_ref()
            .and_then(|(_, at)| *at)
            .unwrap_or_else(crate::now_ms);
        let action_account = account.clone();
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
            && now >= pr.touched_at_ms
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
        if now >= pr.touched_at_ms {
            pr.touched_at_ms = now;
            pr.actor = actor.into();
            pr.action = action.into();
        }
        let event = PrHistoryEvent {
            id: source
                .map(|(id, _)| id)
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            at_ms: now,
            identity,
            account: action_account,
            actor: actor.into(),
            action: action.into(),
            detail: "Successful action".into(),
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
                account: pr.account.clone(),
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
                            found.push((
                                actor.clone(),
                                identity,
                                account,
                                action,
                                source,
                                entry.updated_at_ms.or(entry.at_ms),
                            ));
                        }
                    }
                }
            }
            Ok(found)
        })
        .await?;
        for (actor, identity, account, action, source, at) in candidates {
            self.touch_pr(
                id,
                identity,
                account,
                &actor,
                &action,
                Some((source, at)),
                false,
            )
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
                }
                if let Some(conversation) = manager.core.catalog().conversations.first() {
                    let _ = manager.refresh_tracked_prs(&conversation.id).await;
                }
                drop(manager);
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        });
    }
    async fn refresh_tracked_prs(&self, _id: &ConversationId) -> Result<()> {
        if !self.app.is_open() {
            return Ok(());
        }
        let Ok(_round) = self.pr_poll.round.try_lock() else {
            return Ok(());
        };
        let Some(gh) = self.gh() else {
            return Ok(());
        };
        let now = crate::now_ms();
        // Gather every owner before issuing any requests: mounting another session cannot
        // create another poller or a duplicate first-page request for a shared identity.
        let mut targets: HashMap<String, (TrackedPullRequest, Vec<ConversationId>)> =
            HashMap::new();
        for conversation in self.core.catalog().conversations {
            if conversation.lifecycle != crate::Lifecycle::Active || conversation.tab_chat.is_some()
            {
                continue;
            }
            for pr in self.tracked_work(&conversation.id).await?.pull_requests {
                if pr.forgotten
                    || pr.pull_request.as_ref().is_some_and(|p| {
                        matches!(p.state, PullRequestState::Merged | PullRequestState::Closed)
                    })
                {
                    continue;
                }
                let key = poll_key(&pr);
                let target = targets
                    .entry(key)
                    .or_insert_with(|| (pr.clone(), Vec::new()));
                if pr.touched_at_ms > target.0.touched_at_ms {
                    target.0 = pr;
                }
                target.1.push(conversation.id.clone());
            }
        }
        let mut groups: HashMap<
            (String, String),
            Vec<(String, TrackedPullRequest, Vec<ConversationId>)>,
        > = HashMap::new();
        for (key, (mut pr, owners)) in targets {
            let cached = self
                .pr_poll
                .state
                .lock()
                .unwrap()
                .get(&key)
                .and_then(|slot| {
                    let touched = slot
                        .cached
                        .as_ref()
                        .is_some_and(|old| old.touched_at_ms < pr.touched_at_ms);
                    (slot.next > now && !touched)
                        .then(|| slot.cached.clone())
                        .flatten()
                });
            if let Some(cached) = cached {
                for owner in owners {
                    self.apply_pr_observation(&owner, &cached, now).await?;
                }
                continue;
            }
            let Some(account) = pr.account.clone() else {
                pr.status = "Account needed: brigadier pr watch <url> --account <login>".into();
                pr.complete = false;
                self.pr_poll.state.lock().unwrap().insert(
                    key,
                    PollSlot {
                        next: now + 120_000,
                        failures: 0,
                        cached: Some(pr.clone()),
                    },
                );
                for owner in owners {
                    self.apply_pr_observation(&owner, &pr, now).await?;
                }
                continue;
            };
            groups
                .entry((pr.identity.host.clone(), account))
                .or_default()
                .push((key, pr, owners));
        }
        for ((host, account), group) in groups {
            let account_key = format!("account:{host}:{account}");
            if self
                .pr_poll
                .state
                .lock()
                .unwrap()
                .get(&account_key)
                .is_some_and(|slot| slot.next > now)
            {
                continue;
            }
            if !self.app.is_open() {
                break;
            }
            let named = gh.for_account(&host, &account).await;
            // Bounded aliases keep the initial query modest; only a PR with more pages
            // performs its own cursor requests. One viewer and rateLimit per batch.
            for chunk in group.chunks(20) {
                if !self.app.is_open() {
                    break;
                }
                let prs = chunk
                    .iter()
                    .map(|(_, pr, _)| pr.clone())
                    .collect::<Vec<_>>();
                let results = if let Some(named) = &named {
                    observe_batch(named, &prs, now, || self.app.is_open()).await
                } else {
                    prs.iter()
                        .map(|_| {
                            Err(
                                "Account unavailable; use gh auth login for the selected account"
                                    .into(),
                            )
                        })
                        .collect()
                };
                let mut account_next = now;
                let mut failed = false;
                for ((key, mut pr, owners), result) in chunk.iter().cloned().zip(results) {
                    let mut next = now
                        + if now - pr.touched_at_ms < 30 * 60 * 1000 {
                            30_000
                        } else {
                            120_000
                        };
                    match result {
                        Ok(seen) => {
                            account_next = account_next.max(seen.next);
                            failed |= seen.status.starts_with("GitHub unavailable");
                            pr.pull_request = Some(seen.pr);
                            pr.viewer = Some(seen.viewer);
                            pr.status = seen.status;
                            pr.complete = seen.complete;
                            pr.observed_at_ms = Some(now);
                            next = next.max(seen.next);
                        }
                        Err(reason) => {
                            failed = true;
                            pr.status = reason;
                            pr.complete = false;
                            next = now + 120_000;
                        }
                    }
                    self.pr_poll.state.lock().unwrap().insert(
                        key,
                        PollSlot {
                            next,
                            failures: 0,
                            cached: Some(pr.clone()),
                        },
                    );
                    for owner in owners {
                        self.apply_pr_observation(&owner, &pr, now).await?;
                    }
                }
                {
                    let mut state = self.pr_poll.state.lock().unwrap();
                    let slot = state.entry(account_key.clone()).or_default();
                    if failed {
                        slot.failures = slot.failures.saturating_add(1);
                        account_next = account_next
                            .max(now + (120_000i64 << slot.failures.saturating_sub(1).min(5)));
                    } else {
                        slot.failures = 0;
                    }
                    slot.next = account_next;
                }
                if account_next > now {
                    break;
                }
            }
        }
        Ok(())
    }
    async fn apply_pr_observation(
        &self,
        id: &ConversationId,
        observed: &TrackedPullRequest,
        now: i64,
    ) -> Result<()> {
        let _guard = self.tracked_writes.lock().await;
        let Some(current) = self
            .tracked_work(id)
            .await?
            .pull_requests
            .into_iter()
            .find(|p| p.identity == observed.identity)
        else {
            return Ok(());
        };
        if current.forgotten || current.account != observed.account {
            return Ok(());
        }
        let mut pr = observed.clone();
        pr.touched_at_ms = current.touched_at_ms;
        pr.actor = current.actor.clone();
        pr.action = current.action.clone();
        if current == pr {
            return Ok(());
        }
        let event = (current.status != pr.status
            || current.pull_request.as_ref().map(|p| p.state)
                != pr.pull_request.as_ref().map(|p| p.state))
        .then(|| PrHistoryEvent {
            id: uuid::Uuid::new_v4().to_string(),
            at_ms: now,
            identity: pr.identity.clone(),
            account: pr.account.clone(),
            actor: "GitHub".into(),
            action: "status changed".into(),
            detail: pr.status.clone(),
        });
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
        self.keep_tracked(id, &Change::Pr { pr, event }).await
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
#[cfg(test)]
async fn observe(
    gh: &super::gh::Gh,
    tracked: &TrackedPullRequest,
    now: i64,
) -> std::result::Result<Observation, String> {
    observe_pages(gh, tracked, now, None, || true).await
}

struct FirstPage {
    value: Value,
    headers: String,
    success: bool,
}
async fn observe_pages(
    gh: &super::gh::Gh,
    tracked: &TrackedPullRequest,
    now: i64,
    mut first: Option<FirstPage>,
    app_open: impl Fn() -> bool,
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
        if !app_open() {
            return Err("App closed; status is stale".into());
        }
        let (value, headers, success) = if let Some(first) = first.take() {
            (first.value, first.headers, first.success)
        } else {
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
            (
                serde_json::from_str::<Value>(payload).unwrap_or(Value::Null),
                headers.to_owned(),
                output.success(),
            )
        };
        if let Some(delay) = retry_after(&headers, now) {
            next = next.max(delay);
        }
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
        if !success || value.is_null() || value.get("errors").is_some() {
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
        if let Some(state) = c["state"].as_str() {
            state != "SUCCESS"
        } else {
            c["status"] != "COMPLETED"
                || !matches!(
                    c["conclusion"].as_str(),
                    Some("SUCCESS" | "NEUTRAL" | "SKIPPED")
                )
        }
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
        let value = value.trim();
        if let Ok(seconds) = value.parse::<i64>() {
            return (seconds >= 0).then(|| now.saturating_add(seconds.saturating_mul(1000)));
        }
        jiff::civil::DateTime::strptime("%a, %d %b %Y %H:%M:%S GMT", value)
            .ok()
            .and_then(|date| date.to_zoned(jiff::tz::TimeZone::UTC).ok())
            .map(|date| date.timestamp().as_millisecond().max(now))
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
        assert_eq!(
            retry_after("Retry-After: Mon, 26 Oct 2026 12:00:00 GMT", 0),
            Some(
                "2026-10-26T12:00:00Z"
                    .parse::<jiff::Timestamp>()
                    .unwrap()
                    .as_millisecond()
            )
        );
        assert_eq!(
            retry_after("Retry-After: 9223372036854775807", 1),
            Some(i64::MAX)
        );
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
    async fn two_prs_share_one_first_page_round_trip_and_keep_separate_states() {
        let mut green = page(
            vec![json!({"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"})],
            false,
        );
        green["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
            ["contexts"]["totalCount"] = json!(1);
        let mut red = green["data"]["repository"].clone();
        red["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"]
            [0]["conclusion"] = json!("ACTION_REQUIRED");
        let batch = json!({"data":{"viewer":{"login":"alice"},"rateLimit":{"cost":9,"limit":5000,"remaining":4000,"resetAt":"2099-01-01T00:00:00Z"},"p0":green["data"]["repository"],"p1":red}});
        let (dir, gh) = stub(vec![batch]);
        let first = tracked();
        let mut second = tracked();
        second.identity.number = 8;
        let result = observe_batch(&gh, &[first, second], 0, || true).await;
        assert_eq!(result.len(), 2);
        let a = result[0].as_ref().unwrap();
        let b = result[1].as_ref().unwrap();
        assert!(a.complete);
        assert!(b.complete);
        assert_eq!(a.status, "Ready to merge");
        assert_eq!(b.status, "CI failing");
        assert_eq!(a.pr.number, 7);
        assert_eq!(b.pr.number, 8);
        assert_eq!(std::fs::read_to_string(dir.join("count")).unwrap(), "1");
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn an_alias_error_does_not_poison_another_pr_or_claim_readiness() {
        let mut first = page(
            vec![json!({"name":"CI","status":"COMPLETED","conclusion":"SUCCESS"})],
            false,
        );
        first["data"]["repository"]["pullRequest"]["commits"]["nodes"][0]["commit"]["statusCheckRollup"]
            ["contexts"]["totalCount"] = json!(1);
        let batch = json!({"data":{"viewer":{"login":"alice"},"rateLimit":{"cost":9,"limit":5000,"remaining":4000,"resetAt":"2099-01-01T00:00:00Z"},"p0":first["data"]["repository"],"p1":null},"errors":[{"path":["p1","pullRequest"],"message":"not readable"}]});
        let (dir, gh) = stub(vec![batch]);
        let a = tracked();
        let mut b = tracked();
        b.identity.number = 8;
        let results = observe_batch(&gh, &[a, b], 0, || true).await;
        assert_eq!(results[0].as_ref().unwrap().status, "Ready to merge");
        assert!(!results[1].as_ref().unwrap().complete);
        assert_eq!(std::fs::read_to_string(dir.join("count")).unwrap(), "1");
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
    pub(super) async fn observe_own_branch_pr(
        &self,
        id: &ConversationId,
        found: &PullRequest,
    ) -> Result<()> {
        let Some(identity) = PrIdentity::parse(&found.url) else {
            return Ok(());
        };
        let source = format!("branch:{}", identity.key());
        self.touch_pr(
            id,
            identity.clone(),
            None,
            "session",
            "branch pull request",
            Some((source, None)),
            false,
        )
        .await?;
        let _guard = self.tracked_writes.lock().await;
        let Some(mut pr) = self
            .tracked_work(id)
            .await?
            .pull_requests
            .into_iter()
            .find(|p| p.identity == identity)
        else {
            return Ok(());
        };
        if pr.forgotten {
            return Ok(());
        }
        let terminal = matches!(
            found.state,
            PullRequestState::Merged | PullRequestState::Closed
        );
        if pr.pull_request.is_some()
            && (!terminal
                || pr
                    .pull_request
                    .as_ref()
                    .is_some_and(|p| p.state == found.state))
        {
            return Ok(());
        }
        pr.pull_request = Some(found.clone());
        pr.complete = false;
        pr.observed_at_ms = Some(crate::now_ms());
        pr.status = match found.state {
            PullRequestState::Merged => "Merged",
            PullRequestState::Closed => "Closed",
            _ if pr.account.is_none() => "Account needed for review status",
            _ => "Checking status",
        }
        .into();
        let event = PrHistoryEvent {
            id: uuid::Uuid::new_v4().to_string(),
            at_ms: crate::now_ms(),
            identity,
            account: pr.account.clone(),
            actor: "GitHub".into(),
            action: "status changed".into(),
            detail: pr.status.clone(),
        };
        self.keep_tracked(
            id,
            &Change::Pr {
                pr,
                event: Some(event),
            },
        )
        .await
    }
}

#[cfg(test)]
mod replay_tests {
    use super::super::flow::{Flow, Options, Reply};
    use super::super::session_dir::{LineageEntry, note_lineage_at};
    use super::*;
    use std::sync::Arc;
    async fn generation(record: std::path::PathBuf, name: &str, command: &str, output: &str) {
        std::fs::create_dir_all(&record).unwrap();
        let path = record.join(format!("{name}.jsonl"));
        let line = json!({"type":"event_msg","timestamp":"2026-10-26T10:00:00Z","payload":{"type":"item_completed","item":{"type":"CommandExecution","id":"reused-call-id","command":command,"status":"completed","exit_code":0,"cwd":"/tmp","aggregated_output":output}}});
        std::fs::write(&path, format!("{line}\n")).unwrap();
        note_lineage_at(
            record,
            LineageEntry {
                provider: brigadier_providers::ProviderKind::Codex,
                session_id: name.into(),
                file: Some(path.display().to_string()),
                started_at: 1,
                forgotten: false,
            },
        )
        .await
        .unwrap();
    }
    #[tokio::test]
    async fn lead_and_worker_generations_replay_once_and_forget_stays_suppressed() {
        let mut flow = Flow::start(
            "pr-transcript-replay",
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
                    crate::TaskState::Queued
                        | crate::TaskState::Starting
                        | crate::TaskState::Running
                )
            })
        })
        .await;
        let record = flow.manager.session_dir(&id).record();
        generation(
            record.clone(),
            "lead-1",
            "gh pr create --title Changes",
            "https://github.com/org/main/pull/7",
        )
        .await;
        generation(
            record,
            "lead-2",
            "gh pr view https://github.com/org/read-only/pull/8",
            "",
        )
        .await;
        generation(
            flow.manager.worker_record(&task),
            "worker-1",
            "GH_TOKEN=$(gh auth token --user org-user) gh pr review 9 -R org/other --approve",
            "",
        )
        .await;
        flow.manager.discover_pull_requests(&id).await.unwrap();
        let first = flow.manager.tracked_work(&id).await.unwrap();
        assert_eq!(first.pull_requests.len(), 2);
        assert_eq!(first.pr_history.len(), 2);
        assert!(first.pr_history.iter().all(|event| {
            event.at_ms
                == "2026-10-26T10:00:00Z"
                    .parse::<jiff::Timestamp>()
                    .unwrap()
                    .as_millisecond()
        }));
        assert_eq!(
            first
                .pr_history
                .iter()
                .find(|event| event.identity.repo == "other")
                .unwrap()
                .account
                .as_deref(),
            Some("org-user")
        );
        let worker = first
            .pull_requests
            .iter()
            .find(|p| p.identity.repo == "other")
            .unwrap();
        assert_eq!(worker.account.as_deref(), Some("org-user"));
        assert_eq!(worker.actor, format!("worker {}", task.number));
        flow.manager.discover_pull_requests(&id).await.unwrap();
        assert_eq!(flow.manager.tracked_work(&id).await.unwrap(), first);
        flow.manager
            .forget_pull_request(&id, &worker.identity.key())
            .await
            .unwrap();
        flow.restart().await;
        flow.manager.discover_pull_requests(&id).await.unwrap();
        let replay = flow.manager.tracked_work(&id).await.unwrap();
        assert_eq!(replay.pull_requests.len(), 2);
        assert!(
            replay
                .pull_requests
                .iter()
                .find(|p| p.identity.repo == "other")
                .unwrap()
                .forgotten
        );
        assert_eq!(replay.pr_history.len(), 3);
        flow.stop().await;
    }
}

fn poll_key(pr: &TrackedPullRequest) -> String {
    format!(
        "{}:{}",
        pr.identity.key(),
        pr.account.as_deref().unwrap_or("unresolved")
    )
}

/// Initial pages share one account identity and one charged rateLimit result. Alias errors
/// affect only their PR; global/auth/rate errors conservatively invalidate the whole batch.
async fn observe_batch(
    gh: &super::gh::Gh,
    prs: &[TrackedPullRequest],
    now: i64,
    app_open: impl Fn() -> bool,
) -> Vec<std::result::Result<Observation, String>> {
    if prs.is_empty() {
        return Vec::new();
    }
    let host = &prs[0].identity.host;
    if prs.len() > 20
        || prs
            .iter()
            .any(|pr| &pr.identity.host != host || pr.account != prs[0].account)
    {
        return prs
            .iter()
            .map(|_| Err("Invalid GitHub account batch".into()))
            .collect();
    }
    if !app_open() {
        return prs
            .iter()
            .map(|_| Err("App closed; status is stale".into()))
            .collect();
    }
    let template = &QUERY[QUERY.find("repository(owner:").unwrap()..QUERY.len() - 1];
    let mut query = String::from("query{viewer{login} rateLimit{cost limit remaining resetAt} ");
    for (index, pr) in prs.iter().enumerate() {
        let identity = &pr.identity;
        let mut alias = template
            .replace("$owner", &json!(identity.owner).to_string())
            .replace("$repo", &json!(identity.repo).to_string())
            .replace("$number", &identity.number.to_string());
        for cursor in ["checks", "threads", "reviews", "requests"] {
            alias = alias.replace(&format!(",after:${cursor}"), "");
        }
        query.push_str(&format!("p{index}:{alias} "));
    }
    query.push('}');
    let body = json!({"query":query}).to_string();
    let output = match gh
        .output_with_input(
            &[
                "api",
                "graphql",
                "--hostname",
                host,
                "--include",
                "--input",
                "-",
            ],
            &body,
        )
        .await
    {
        Ok(output) => output,
        Err(_) => {
            return prs
                .iter()
                .map(|_| Err("GitHub unavailable; status is stale".into()))
                .collect();
        }
    };
    let (headers, payload) = split_headers(&output.stdout);
    let value: Value = serde_json::from_str(payload).unwrap_or(Value::Null);
    let mut results = Vec::new();
    for (index, pr) in prs.iter().enumerate() {
        let alias = format!("p{index}");
        let mut normalized = json!({"data":{"viewer":value.pointer("/data/viewer"),"rateLimit":value.pointer("/data/rateLimit"),"repository":value.get("data").and_then(|d|d.get(&alias))}});
        let errors = value["errors"]
            .as_array()
            .map(|errors| {
                errors
                    .iter()
                    .filter(
                        |error| match error.pointer("/path/0").and_then(Value::as_str) {
                            Some(path)
                                if path.starts_with('p')
                                    && path[1..].bytes().all(|b| b.is_ascii_digit()) =>
                            {
                                path == alias
                            }
                            _ => true,
                        },
                    )
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        if !errors.is_empty() {
            normalized["errors"] = json!(errors);
        }
        results.push(
            observe_pages(
                gh,
                pr,
                now,
                Some(FirstPage {
                    value: normalized,
                    headers: headers.into(),
                    success: output.success() && !value.is_null(),
                }),
                &app_open,
            )
            .await,
        );
    }
    results
}
