//! Send feedback (#471): the note becomes reports in a throwaway writer session, checked
//! against the repository's issues (through `gh`, else the public API), and is filed as the
//! user with their `gh`: a new issue, or a comment on the one it repeats. A stand-in `gh`
//! answers here; nothing reaches GitHub.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{Flow, Options, Reply, Script, Turn};
use crate::feedback::{FEEDBACK_REPO, FeedbackFiled, FeedbackFiling};
use crate::manager::ci::Gh;
use crate::work::FeedbackKind;

/// Marks the report writer's turns, and the duplicate check's.
const WRITER: &str = "You turn a note a user wrote about Brigadier";
const JUDGE: &str = "You decide whether new reports about Brigadier repeat";

/// A stand-in `gh`: logs each call (and what it read on stdin) and answers from files.
const GH: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$*" >> "$D/calls"
n=$(wc -l < "$D/calls" | tr -d ' ')
case "$1 $2" in
  "auth status") exit "$(cat "$D/auth.exit" 2>/dev/null || echo 0)" ;;
  "api user") cat "$D/login" ;;
  "issue list") cat "$D/issues.json" ;;
  "issue view") cat "$D/view-$3.json" ;;
  "issue create")
    cat > "$D/stdin.$n"
    for a in "$@"; do
      if [ "$a" = "--label" ] && [ -f "$D/label-refused" ]; then
        echo "could not add label: 'bug' not found" >&2; exit 1
      fi
      case "$a" in *FAIL*) echo "HTTP 502: Bad gateway" >&2; exit 1 ;; esac
    done
    echo "https://github.com/syncra-studio/brigadier-ai/issues/$((500 + n))" ;;
  "issue comment")
    cat > "$D/stdin.$n"
    echo "https://github.com/syncra-studio/brigadier-ai/issues/$3#issuecomment-1" ;;
  *) echo "unknown call: $*" >&2; exit 1 ;;
esac
"#;

/// A stand-in `curl` that prints `api.json` for any page, or fails without one.
const CURL: &str = r#"#!/bin/sh
D="$(dirname "$0")"
echo "$*" >> "$D/curl-calls"
[ -f "$D/api.json" ] || exit 22
case "$*" in
  *page=1*) cat "$D/api.json" ;;
  *issues/7*) echo '{"body":"Make it dark."}' ;;
  *) echo "[]" ;;
esac
"#;

struct Stubs {
    dir: PathBuf,
}

impl Stubs {
    fn new(flow: &Flow) -> Self {
        let dir = flow.repo.parent().unwrap().join("feedback-stubs");
        std::fs::create_dir_all(&dir).unwrap();
        for (name, script) in [("gh", GH), ("curl", CURL)] {
            let path = dir.join(name);
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        *flow.manager.gh_override.lock().unwrap() = Some(Gh {
            program: dir.join("gh"),
            vars: vec![("PATH".into(), "/usr/bin:/bin".into())],
        });
        *flow.manager.curl_override.lock().unwrap() = Some(dir.join("curl"));
        Self { dir }
    }

    fn file(&self, name: &str, body: &str) -> &Self {
        std::fs::write(self.dir.join(name), body).unwrap();
        self
    }

    fn signed_out(&self) -> &Self {
        self.file("auth.exit", "1")
    }

    fn lines(&self, name: &str) -> Vec<String> {
        read(&self.dir.join(name))
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// What `gh` read on its stdin for call `n` (1-based, counting every call).
    fn stdin(&self, n: usize) -> String {
        read(&self.dir.join(format!("stdin.{n}")))
    }
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

type Seen = Arc<Mutex<Vec<String>>>;

/// A writer that answers `write` to the report turn and `judge` to the duplicate check, and
/// keeps what each was given.
fn writer(seen: &Seen, write: &'static str, judge: &'static str) -> Script {
    let seen = seen.clone();
    Arc::new(move |turn: Turn| {
        let seen = seen.clone();
        Box::pin(async move {
            if turn.prompt.contains(WRITER) {
                seen.lock().unwrap().push(format!("write: {}", turn.input));
                return Reply::text(write);
            }
            if turn.prompt.contains(JUDGE) {
                seen.lock().unwrap().push(format!("judge: {}", turn.input));
                return Reply::text(judge);
            }
            Reply::text("Hello.")
        })
    })
}

const TWO_REPORTS: &str = r#"{"reports":[
 {"kind":"bug","title":"Stop leaves the tab spinning","area":"Sessions, workers, landing or review",
  "steps":"Not given; see What happened.","expected":"It stops.","actual":"The tab keeps spinning after Stop.",
  "candidates":[12, 7]},
 {"kind":"idea","title":"Pin a chat","problem":"Chats get lost in the list.","today":"Not given.",
  "wanted":"A pin on a chat.","candidates":[]}]}"#;

const ISSUES: &str = r#"[
 {"number":12,"title":"Stop doesn't stop the spinner","state":"OPEN","url":"https://github.com/syncra-studio/brigadier-ai/issues/12","labels":[{"name":"bug"}]},
 {"number":7,"title":"Dark mode","state":"CLOSED","url":"https://github.com/syncra-studio/brigadier-ai/issues/7"}]"#;

/// Signed in: the issues come through `gh` (open and closed, on Brigadier's repository), the
/// likely ones are read in full, and the check marks the bug as #12's (open, a bug); closed #7
/// it called the same too is only linked.
#[tokio::test]
async fn a_note_becomes_reports_and_a_repeat_points_at_its_issue() {
    let seen = Seen::default();
    let flow = Flow::start(
        "feedback-write-up",
        Options::default(),
        writer(
            &seen,
            TWO_REPORTS,
            r#"{"matches":[{"report":0,"issue":12,"same":true},
                           {"report":0,"issue":7,"issueKind":"bug","same":true}]}"#,
        ),
    )
    .await;
    let stubs = Stubs::new(&flow);
    stubs
        .file("issues.json", ISSUES)
        .file("view-12.json", r#"{"body":"The spinner keeps going."}"#)
        .file("view-7.json", r#"{"body":"Make it dark."}"#);
    let write_up = flow
        .manager
        .write_up_feedback("The tab spins after Stop. Also let me pin chats.")
        .await
        .unwrap();
    assert!(write_up.searched);
    assert_eq!(write_up.reports.len(), 2);
    let bug = &write_up.reports[0];
    assert_eq!(bug.kind, FeedbackKind::Bug);
    let duplicate = bug.duplicate.as_ref().expect("a repeat of #12");
    assert_eq!(
        (duplicate.number, duplicate.open, duplicate.comment.as_str()),
        (
            12,
            true,
            "Seen again.\n\nWhat happened: The tab keeps spinning after Stop.\nExpected: It stops."
        )
    );
    assert_eq!(bug.related, [7]);
    assert_eq!(write_up.reports[1].kind, FeedbackKind::Idea);
    assert_eq!(write_up.reports[1].duplicate, None);

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert!(seen[0].contains("The tab spins after Stop."), "{}", seen[0]);
    assert!(seen[0].contains("#12 open: Stop doesn't stop the spinner"));
    assert!(seen[0].contains("#7 closed: Dark mode"));
    assert!(
        seen[1].contains("issue #12 (open, bug)") && seen[1].contains("The spinner keeps going."),
        "{}",
        seen[1]
    );
    assert!(seen[1].contains("issue #7 (closed, kind not marked)"));
    let calls = stubs.lines("calls");
    assert!(
        calls.iter().any(|call| call
            == &format!("issue list --repo {FEEDBACK_REPO} --state all --limit 1000 --json number,title,state,url,labels")),
        "{calls:?}"
    );
    assert!(
        calls
            .iter()
            .any(|call| call == &format!("issue view 12 --repo {FEEDBACK_REPO} --json body"))
    );
    assert!(stubs.lines("curl-calls").is_empty());
    flow.stop().await;
}

/// Signed out: the issues come from the public API (its pull requests left out); unsure, the
/// check marks nothing and every report is filed new.
#[tokio::test]
async fn signed_out_the_public_list_is_searched() {
    let seen = Seen::default();
    let flow = Flow::start(
        "feedback-public",
        Options::default(),
        writer(&seen, TWO_REPORTS, r#"{"matches":[]}"#),
    )
    .await;
    let stubs = Stubs::new(&flow);
    stubs.signed_out().file(
        "api.json",
        r#"[{"number":7,"title":"Dark mode","state":"closed","html_url":"https://github.com/x/7","body":"Make it dark."},
            {"number":9,"title":"A pull request","state":"open","html_url":"https://github.com/x/9","pull_request":{}}]"#,
    );
    let write_up = flow.manager.write_up_feedback("It spins.").await.unwrap();
    assert!(write_up.searched);
    assert!(
        write_up
            .reports
            .iter()
            .all(|report| report.duplicate.is_none())
    );
    let seen = seen.lock().unwrap().clone();
    assert!(seen[0].contains("#7 closed: Dark mode"), "{}", seen[0]);
    assert!(!seen[0].contains("A pull request"));
    assert!(seen[1].starts_with("judge: ") && seen[1].contains("Make it dark."));
    let curl = stubs.lines("curl-calls");
    assert!(curl[0].contains(&format!(
        "https://api.github.com/repos/{FEEDBACK_REPO}/issues?state=all"
    )));
    flow.stop().await;
}

/// With no list at all, the writer is told so, gives no candidates and nothing is checked.
#[tokio::test]
async fn with_no_list_the_reports_go_unsearched() {
    let seen = Seen::default();
    let flow = Flow::start(
        "feedback-unsearched",
        Options::default(),
        writer(&seen, TWO_REPORTS, "unused"),
    )
    .await;
    Stubs::new(&flow).signed_out();
    let write_up = flow.manager.write_up_feedback("It spins.").await.unwrap();
    assert!(!write_up.searched);
    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen.len(), 1, "no duplicate check: {seen:?}");
    assert!(seen[0].contains("couldn't be read"));
    flow.stop().await;
}

/// No writer can run (every model at its limit): it fails, and the app opens one form.
#[tokio::test]
async fn with_no_writer_the_write_up_fails() {
    let flow = Flow::start(
        "feedback-no-writer",
        Options::default(),
        Arc::new(|_turn: Turn| Box::pin(async { Reply::limited() })),
    )
    .await;
    Stubs::new(&flow).signed_out();
    let err = flow
        .manager
        .write_up_feedback("It spins.")
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("no model can write the report now"),
        "{err}"
    );
    flow.stop().await;
}

fn new_issue(kind: FeedbackKind, title: &str, body: &str) -> FeedbackFiling {
    FeedbackFiling {
        kind,
        title: title.into(),
        body: body.into(),
        comment_on: None,
    }
}

/// Filed as the user on Brigadier's repository: each item on its own, a repeat as a comment,
/// the exact body on stdin, and a failure kept to its own item.
#[tokio::test]
async fn reports_are_filed_one_by_one_and_a_failure_stays_its_own() {
    let flow = Flow::start(
        "feedback-file",
        Options::default(),
        writer(&Seen::default(), TWO_REPORTS, ""),
    )
    .await;
    let stubs = Stubs::new(&flow);
    let items = [
        new_issue(
            FeedbackKind::Bug,
            "[Bug]: Stop leaves the tab spinning",
            "### Area\n\nChats\n",
        ),
        FeedbackFiling {
            comment_on: Some(12),
            ..new_issue(
                FeedbackKind::Bug,
                "",
                "Also reported from Brigadier: seen again.",
            )
        },
        new_issue(FeedbackKind::Idea, "[Proposal]: FAIL", "x"),
        new_issue(
            FeedbackKind::Idea,
            "[Proposal]: Pin a chat",
            "### The problem\n\nLost chats\n",
        ),
    ];
    let results = flow.manager.file_feedback(&items).await.expect("signed in");
    let calls = stubs.lines("calls");
    // Call 1 is the sign-in check.
    assert_eq!(
        results,
        vec![
            FeedbackFiled::Filed {
                number: 502,
                url: format!("https://github.com/{FEEDBACK_REPO}/issues/502")
            },
            FeedbackFiled::Commented {
                number: 12,
                url: format!("https://github.com/{FEEDBACK_REPO}/issues/12#issuecomment-1")
            },
            FeedbackFiled::Failed {
                reason: "GitHub didn't take it: HTTP 502: Bad gateway".into()
            },
            FeedbackFiled::Filed {
                number: 505,
                url: format!("https://github.com/{FEEDBACK_REPO}/issues/505")
            },
        ],
        "{calls:?}"
    );
    assert_eq!(
        calls[1],
        format!(
            "issue create --repo {FEEDBACK_REPO} --title [Bug]: Stop leaves the tab spinning --body-file - --label bug"
        )
    );
    assert_eq!(
        calls[2],
        format!("issue comment 12 --repo {FEEDBACK_REPO} --body-file -")
    );
    assert!(calls[4].ends_with("--label proposal"), "{}", calls[4]);
    assert_eq!(stubs.stdin(2), "### Area\n\nChats\n");
    assert_eq!(stubs.stdin(3), "Also reported from Brigadier: seen again.");
    assert_eq!(stubs.stdin(5), "### The problem\n\nLost chats\n");
    // Sending again sends only what failed.
    let retried = flow.manager.file_feedback(&items[2..3]).await.unwrap();
    assert!(matches!(retried[0], FeedbackFiled::Failed { .. }));
    assert_eq!(stubs.lines("calls").len(), calls.len() + 2);
    flow.stop().await;
}

/// GitHub refusing the label files it again without one; signed out, nothing is filed.
#[tokio::test]
async fn a_refused_label_is_dropped_and_signed_out_nothing_is_filed() {
    let flow = Flow::start(
        "feedback-label",
        Options::default(),
        writer(&Seen::default(), TWO_REPORTS, ""),
    )
    .await;
    let stubs = Stubs::new(&flow);
    stubs.file("label-refused", "");
    let items = [new_issue(FeedbackKind::Bug, "[Bug]: Spins", "body")];
    let results = flow.manager.file_feedback(&items).await.unwrap();
    assert!(
        matches!(results[0], FeedbackFiled::Filed { .. }),
        "{results:?}"
    );
    let calls = stubs.lines("calls");
    assert_eq!(calls.len(), 3, "{calls:?}");
    assert!(calls[1].ends_with("--label bug"));
    assert_eq!(
        calls[2],
        format!("issue create --repo {FEEDBACK_REPO} --title [Bug]: Spins --body-file -")
    );
    assert_eq!(stubs.stdin(3), "body");

    stubs.signed_out();
    assert_eq!(flow.manager.file_feedback(&items).await, None);
    assert_eq!(stubs.lines("calls").len(), 4, "only the sign-in check");
    flow.stop().await;
}

/// Who it posts as: the login `gh` names when signed in, nothing when it can't say it, and no
/// account at all when signed out.
#[tokio::test]
async fn the_account_is_who_gh_is_signed_in_as() {
    let seen = Seen::default();
    let flow = Flow::start(
        "feedback-account",
        Options::default(),
        writer(&seen, "", ""),
    )
    .await;
    let stubs = Stubs::new(&flow);
    stubs.file("login", "octocat\n");
    assert_eq!(
        flow.manager.feedback_account().await,
        Some(Some("octocat".into()))
    );
    stubs.file("login", "");
    assert_eq!(flow.manager.feedback_account().await, Some(None));
    stubs.signed_out();
    assert_eq!(flow.manager.feedback_account().await, None);
}
