//! Send feedback (#471), run: the user's note becomes reports in a throwaway writer session
//! (the commit message writer's, [`SessionManager::write_text`]), checked against Brigadier's
//! existing issues (open and closed, a second writer turn reading the likely ones), then filed
//! as the user with their `gh`: a new issue, or a comment on the issue it repeats. Signed out,
//! the app opens each report's form in the browser instead. No bot and no token of Brigadier's.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use super::SessionManager;
use super::ci::{Gh, RunError};
use crate::feedback::{
    FEEDBACK_REPO, FeedbackFiled, FeedbackFiling, FeedbackWriteUp, JUDGE_PROMPT, KnownIssue,
    judge_turn, mark_duplicates, reports_of, write_prompt, write_turn,
};
use crate::work::FeedbackKind;
use crate::{Error, Result};

/// The most issues listed for the writer.
const ISSUE_LIMIT: usize = 1000;
/// How long one read of GitHub may take.
const READ_TIMEOUT: Duration = Duration::from_secs(20);
/// The public API's page size, and the most pages read without `gh`.
const PAGE: usize = 100;
const MAX_PAGES: usize = 10;

impl SessionManager {
    /// The user's `gh` for feedback. A debug build takes a stand-in from
    /// `BRIGADIER_FEEDBACK_GH`, so the dev app files nothing real while it is tried.
    fn feedback_gh(&self) -> Option<Gh> {
        #[cfg(debug_assertions)]
        if let Some(program) = std::env::var_os("BRIGADIER_FEEDBACK_GH") {
            return Some(Gh {
                program: PathBuf::from(program),
                vars: self.runtime.cli_env().vars(),
            });
        }
        self.gh()
    }

    /// The user's `gh` when it is signed in to github.com.
    async fn signed_in_gh(&self) -> Option<Gh> {
        let gh = self.feedback_gh()?;
        let out = gh
            .output(&["auth", "status", "--hostname", "github.com"])
            .await
            .ok()?;
        out.success().then_some(gh)
    }

    /// Who Send feedback posts as: `None` when `gh` isn't signed in (the forms open in the
    /// browser instead), else the GitHub login when `gh` can say it.
    pub async fn feedback_account(&self) -> Option<Option<String>> {
        let gh = self.signed_in_gh().await?;
        let login = gh
            .output(&["api", "user", "--jq", ".login"])
            .await
            .ok()
            .filter(|out| out.success())
            .map(|out| out.stdout.trim().to_owned())
            .filter(|login| !login.is_empty());
        Some(login)
    }

    /// The note `text` as reports in the issue forms' shape, each checked against Brigadier's
    /// existing issues. Fails when no writer can run (the app then opens one form, as before).
    pub async fn write_up_feedback(&self, text: &str) -> Result<FeedbackWriteUp> {
        let text = text.trim();
        if text.is_empty() {
            return Err(Error::Invalid("write what you spotted first".into()));
        }
        let _turn = self.background_turn();
        let gh = self.signed_in_gh().await;
        let issues = self.known_issues(gh.as_ref()).await;
        let (reply, _) = self
            .write_text(
                None,
                None,
                &write_prompt(),
                &write_turn(text, issues.as_deref()),
                "the report",
            )
            .await?;
        let mut reports = reports_of(&reply, issues.as_deref().unwrap_or(&[]))?;
        let wanted: BTreeSet<u64> = reports
            .iter()
            .flat_map(|(_, candidates)| candidates.iter().copied())
            .collect();
        if !wanted.is_empty() {
            let known = issues.as_deref().unwrap_or(&[]);
            let mut bodies = Vec::new();
            for number in wanted {
                let Some(issue) = known.iter().find(|issue| issue.number == number) else {
                    continue;
                };
                // An issue whose text can't be read is judged by its title alone.
                let body = self
                    .issue_body(gh.as_ref(), number)
                    .await
                    .unwrap_or_default();
                bodies.push((issue.clone(), body));
            }
            match self
                .write_text(
                    None,
                    None,
                    JUDGE_PROMPT,
                    &judge_turn(&reports, &bodies),
                    "the duplicate check",
                )
                .await
            {
                Ok((reply, _)) => mark_duplicates(&mut reports, &reply, &bodies),
                Err(err) => {
                    tracing::warn!(error = %err, "the duplicate check didn't run; filing every report new");
                }
            }
        }
        Ok(FeedbackWriteUp {
            reports: reports.into_iter().map(|(report, _)| report).collect(),
            searched: issues.is_some(),
        })
    }

    /// Files `items` as the user with their `gh`, one by one: `None` when `gh` isn't signed in
    /// (nothing is filed; the app opens the forms instead), else what became of each.
    pub async fn file_feedback(&self, items: &[FeedbackFiling]) -> Option<Vec<FeedbackFiled>> {
        let gh = self.signed_in_gh().await?;
        let mut results = Vec::with_capacity(items.len());
        for item in items {
            results.push(file_one(&gh, item).await);
        }
        Some(results)
    }

    /// Brigadier's issues, open and closed: through `gh` when it is signed in, else the public
    /// API; `None` when neither answers.
    async fn known_issues(&self, gh: Option<&Gh>) -> Option<Vec<KnownIssue>> {
        if let Some(gh) = gh {
            #[derive(serde::Deserialize)]
            struct Listed {
                number: u64,
                title: String,
                state: String,
                url: String,
                #[serde(default)]
                labels: Vec<Label>,
            }
            #[derive(serde::Deserialize)]
            struct Label {
                name: String,
            }
            let limit = ISSUE_LIMIT.to_string();
            let args = [
                "issue",
                "list",
                "--repo",
                FEEDBACK_REPO,
                "--state",
                "all",
                "--limit",
                &limit,
                "--json",
                "number,title,state,url,labels",
            ];
            let out = tokio::time::timeout(READ_TIMEOUT, gh.output(&args))
                .await
                .ok()?
                .ok()?;
            if !out.success() {
                tracing::warn!(stderr = %out.stderr.trim(), "gh couldn't list the issues");
                return None;
            }
            let listed: Vec<Listed> = serde_json::from_str(&out.stdout).ok()?;
            return Some(
                listed
                    .into_iter()
                    .map(|issue| KnownIssue {
                        kind: KnownIssue::kind_of(
                            &issue.title,
                            issue.labels.iter().map(|label| label.name.as_str()),
                        ),
                        number: issue.number,
                        title: issue.title,
                        open: issue.state.eq_ignore_ascii_case("open"),
                        url: issue.url,
                    })
                    .collect(),
            );
        }
        let mut issues = Vec::new();
        for page in 1..=MAX_PAGES {
            let path = format!("issues?state=all&per_page={PAGE}&page={page}");
            let serde_json::Value::Array(items) = self.public_api(&path).await? else {
                return None;
            };
            let full = items.len() == PAGE;
            // The public list has pull requests in it too.
            issues.extend(
                items
                    .iter()
                    .filter(|item| item.get("pull_request").is_none())
                    .filter_map(api_issue),
            );
            if !full {
                break;
            }
        }
        Some(issues)
    }

    /// The text of issue `number`, for the duplicate check.
    async fn issue_body(&self, gh: Option<&Gh>, number: u64) -> Option<String> {
        if let Some(gh) = gh {
            let number = number.to_string();
            let args = [
                "issue",
                "view",
                &number,
                "--repo",
                FEEDBACK_REPO,
                "--json",
                "body",
            ];
            let out = tokio::time::timeout(READ_TIMEOUT, gh.output(&args))
                .await
                .ok()?
                .ok()?;
            if !out.success() {
                return None;
            }
            let value: serde_json::Value = serde_json::from_str(&out.stdout).ok()?;
            return value.get("body")?.as_str().map(str::to_owned);
        }
        let value = self.public_api(&format!("issues/{number}")).await?;
        Some(value.get("body")?.as_str().unwrap_or_default().to_owned())
    }

    /// GET `path` under the repository on GitHub's public API, with `curl`; `None` on any
    /// failure (offline, rate limited).
    async fn public_api(&self, path: &str) -> Option<serde_json::Value> {
        let curl = self.curl()?;
        let url = format!("https://api.github.com/repos/{FEEDBACK_REPO}/{path}");
        let out = tokio::time::timeout(
            READ_TIMEOUT,
            tokio::process::Command::new(curl)
                .args([
                    "-sSfL",
                    "--max-time",
                    "15",
                    "-H",
                    "Accept: application/vnd.github+json",
                    &url,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .output(),
        )
        .await
        .ok()?
        .ok()?;
        if !out.status.success() {
            tracing::warn!(%url, stderr = %String::from_utf8_lossy(&out.stderr).trim(), "GitHub's public API didn't answer");
            return None;
        }
        serde_json::from_slice(&out.stdout).ok()
    }

    fn curl(&self) -> Option<PathBuf> {
        #[cfg(test)]
        if let Some(curl) = self.curl_override.lock().unwrap().clone() {
            return Some(curl);
        }
        self.runtime.cli_env().which("curl")
    }
}

/// An issue as the public API lists it.
fn api_issue(item: &serde_json::Value) -> Option<KnownIssue> {
    let title = item.get("title")?.as_str()?;
    let labels = item.get("labels").and_then(|labels| labels.as_array());
    Some(KnownIssue {
        kind: KnownIssue::kind_of(
            title,
            labels
                .into_iter()
                .flatten()
                .filter_map(|label| label.get("name")?.as_str()),
        ),
        number: item.get("number")?.as_u64()?,
        title: title.to_owned(),
        open: item.get("state")?.as_str()? == "open",
        url: item.get("html_url")?.as_str()?.to_owned(),
    })
}

/// Files one item: a comment on the issue it repeats, else a new issue with its form's label,
/// filed again without it only when GitHub refused the label itself.
async fn file_one(gh: &Gh, item: &FeedbackFiling) -> FeedbackFiled {
    if let Some(number) = item.comment_on {
        let issue = number.to_string();
        let args = [
            "issue",
            "comment",
            &issue,
            "--repo",
            FEEDBACK_REPO,
            "--body-file",
            "-",
        ];
        return match gh.output_with_input(&args, &item.body).await {
            Ok(out) if out.success() => FeedbackFiled::Commented {
                number,
                url: last_url(&out.stdout).unwrap_or_else(|| {
                    format!("https://github.com/{FEEDBACK_REPO}/issues/{number}")
                }),
            },
            Ok(out) => failed(&out.stderr),
            Err(err) => not_run(err),
        };
    }
    let label = match item.kind {
        FeedbackKind::Bug => "bug",
        FeedbackKind::Idea => "proposal",
    };
    let mut args = vec![
        "issue",
        "create",
        "--repo",
        FEEDBACK_REPO,
        "--title",
        &item.title,
        "--body-file",
        "-",
        "--label",
        label,
    ];
    let mut out = gh.output_with_input(&args, &item.body).await;
    if matches!(&out, Ok(done) if !done.success() && label_refused(&done.stderr)) {
        tracing::info!(label, "GitHub refused the label; filing without it");
        args.truncate(args.len() - 2);
        out = gh.output_with_input(&args, &item.body).await;
    }
    match out {
        Ok(out) if out.success() => match last_url(&out.stdout).and_then(|url| {
            let number = url.rsplit('/').next()?.parse().ok()?;
            Some((number, url))
        }) {
            Some((number, url)) => FeedbackFiled::Filed { number, url },
            None => FeedbackFiled::Failed {
                reason: "gh filed it but didn't say where".into(),
            },
        },
        Ok(out) => failed(&out.stderr),
        Err(err) => not_run(err),
    }
}

/// Whether `gh` failed on the label alone (missing, or the user may not set it).
fn label_refused(stderr: &str) -> bool {
    let stderr = stderr.to_lowercase();
    stderr.contains("label")
        && (stderr.contains("not found")
            || stderr.contains("could not add")
            || stderr.contains("does not have permission"))
}

/// The last link `gh` printed: what it filed.
fn last_url(stdout: &str) -> Option<String> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with("https://"))
        .map(str::to_owned)
}

fn failed(stderr: &str) -> FeedbackFiled {
    let line = stderr
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("it failed");
    FeedbackFiled::Failed {
        reason: format!("GitHub didn't take it: {line}"),
    }
}

fn not_run(err: RunError) -> FeedbackFiled {
    FeedbackFiled::Failed {
        reason: match err {
            RunError::Timeout => "gh took too long".into(),
            RunError::Spawn(err) => format!("gh couldn't run: {err}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_refused_label_files_again_without_it() {
        assert!(label_refused("could not add label: 'bug' not found"));
        assert!(label_refused(
            "GraphQL: user does not have permission to add Label"
        ));
        assert!(!label_refused("GraphQL: Could not resolve to a Repository"));
        assert!(!label_refused("HTTP 502: Bad gateway (label service)"));
    }

    #[test]
    fn the_filed_link_is_the_last_one_gh_printed() {
        assert_eq!(
            last_url("Creating issue in o/r\n\nhttps://github.com/o/r/issues/481\n").as_deref(),
            Some("https://github.com/o/r/issues/481")
        );
        assert_eq!(last_url("nothing"), None);
    }
}
