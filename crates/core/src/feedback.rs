//! Send feedback (#471): what the user wrote, turned into reports in the shape of Brigadier's
//! issue forms (`.github/ISSUE_TEMPLATE`), one per bug or idea, each checked against the
//! repository's existing issues. A throwaway writer session judges (the split, the kind, the
//! fields, which issue it repeats); this plumbs: the prompts, reading its reply, and the
//! types the app files with (`manager/feedback.rs` runs it all).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::work::FeedbackKind;
use crate::{Error, Result};

/// Brigadier's repository, `owner/name`: where feedback is filed (`gh --repo`).
pub const FEEDBACK_REPO: &str = "syncra-studio/brigadier-ai";

/// The bug form's Area options, as `bug_report.yml` lists them.
pub const BUG_AREAS: [&str; 11] = [
    "Sessions, workers, landing or review",
    "Chats",
    "Overnight runs",
    "Accounts, usage or routing",
    "Computer use",
    "Built-in browser",
    "Terminal, files or source",
    "Dictation or read aloud",
    "Settings, onboarding or uninstall",
    "Build from source",
    "Other",
];

/// The most reports one note becomes.
const MAX_REPORTS: usize = 10;
/// The most issues one report is checked against, by their text.
const MAX_CANDIDATES: usize = 3;
/// A title is cut past this many characters.
const MAX_TITLE: usize = 90;
/// A field is cut past this many characters.
const MAX_FIELD: usize = 4000;
/// How much of an existing issue's text the duplicate check reads.
const ISSUE_BODY_CHARS: usize = 2000;
/// How much of the user's note the writer reads.
pub(crate) const NOTE_CHARS: usize = 8000;

/// One report the writer made of the user's note, in its form's fields; the other kind's
/// fields stay empty. Written in the user's words: what they didn't say reads "Not given".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackReport {
    pub kind: FeedbackKind,
    /// One line, without the form's "[Bug]: " prefix.
    pub title: String,
    /// A bug's: one of [`BUG_AREAS`].
    pub area: String,
    pub steps: String,
    pub expected: String,
    /// What happened.
    pub actual: String,
    /// An idea's: what they're trying to do, and what gets in the way.
    pub problem: String,
    /// What they do today.
    pub today: String,
    /// What should happen instead.
    pub wanted: String,
    /// The existing issue it repeats, when the check was sure: a note goes there instead.
    pub duplicate: Option<FeedbackDuplicate>,
}

/// An existing issue a report repeats, and the note for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackDuplicate {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub open: bool,
    /// What this report adds to it, a few plain lines.
    pub comment: String,
}

/// The note as reports, and whether the existing issues could be searched for duplicates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackWriteUp {
    pub reports: Vec<FeedbackReport>,
    pub searched: bool,
}

/// One thing to file as the user: a new issue, or a comment on issue `comment_on`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackFiling {
    pub kind: FeedbackKind,
    /// The issue's title, with its form's prefix; unused for a comment.
    pub title: String,
    /// The issue's body, or the comment.
    pub body: String,
    pub comment_on: Option<u64>,
}

/// What became of one [`FeedbackFiling`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "outcome", rename_all = "camelCase")]
pub enum FeedbackFiled {
    /// A new issue.
    Filed { number: u64, url: String },
    /// A comment on an existing issue; `url` is the comment's.
    Commented { number: u64, url: String },
    /// Not filed, in plain words; sending again retries it.
    Failed { reason: String },
}

/// An issue of the repository as listed: enough to name it to the writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnownIssue {
    pub number: u64,
    pub title: String,
    pub open: bool,
    pub url: String,
}

/// The writer's instructions: the note becomes reports, with candidates for the duplicate check.
pub(crate) fn write_prompt() -> String {
    let areas = BUG_AREAS
        .iter()
        .map(|area| format!("\"{area}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "You turn a note a user wrote about Brigadier (a desktop app that runs Claude Code and \
Codex sessions) into issue reports for its developers. Reply with JSON only, no code fences: \
{{\"reports\":[{{\"kind\":\"bug\",\"title\":\"\",\"area\":\"\",\"steps\":\"\",\"expected\":\"\",\
\"actual\":\"\",\"problem\":\"\",\"today\":\"\",\"wanted\":\"\",\"candidates\":[]}}]}}\n\
Rules:\n\
- One report per separate bug or idea in the note; most notes hold one. Never put two problems \
in one report, never split one problem in two.\n\
- kind: \"bug\" for something Brigadier does wrong, \"idea\" for something missing or a change \
in how it behaves.\n\
- title: one plain line naming it, at most 70 characters, no prefix.\n\
- A bug fills area (exactly one of {areas}), steps (to reproduce), expected and actual (what \
happened). An idea fills problem (what they're trying to do and what gets in the way), today \
(what they do now) and wanted (what should happen instead). Leave the other kind's fields \"\".\n\
- Use only what the note says, in plain words close to the user's own. Never invent steps, \
versions, errors or details. What the note doesn't say reads \"Not given.\"; steps it doesn't \
give read \"Not given; see What happened.\"\n\
- Leave out secrets, emails, tokens and file paths.\n\
- candidates: the numbers of up to 3 existing issues listed below that may be about the same \
thing, closest first; [] when none come close."
    )
}

/// The writer's turn: the note, and the repository's issues when they could be read.
pub(crate) fn write_turn(note: &str, issues: Option<&[KnownIssue]>) -> String {
    let note: String = note.trim().chars().take(NOTE_CHARS).collect();
    let mut text = format!("The note:\n<<<\n{note}\n>>>\n\n");
    match issues {
        Some([]) => text.push_str("Brigadier has no issues yet."),
        Some(issues) => {
            text.push_str("Brigadier's existing issues (number, state, title):");
            for issue in issues {
                let state = if issue.open { "open" } else { "closed" };
                text.push_str(&format!("\n#{} {state}: {}", issue.number, issue.title));
            }
        }
        None => text.push_str("The existing issues couldn't be read: give no candidates."),
    }
    text
}

/// The duplicate check's instructions.
pub(crate) const JUDGE_PROMPT: &str = "You decide whether new reports about Brigadier repeat \
an existing GitHub issue. Reply with JSON only, no code fences: \
{\"duplicates\":[{\"report\":0,\"issue\":123,\"comment\":\"\"}]}\n\
Rules:\n\
- A report repeats an issue only when the issue is clearly about the same problem or wish, not \
merely the same part of the app. When unsure, leave the report out: it is filed as a new issue.\n\
- A closed issue counts too.\n\
- comment: a short note for that issue in plain words: that it was seen again, and anything \
the report adds (what happened, when). Leave out secrets, emails, tokens and file paths.\n\
- Reports that repeat nothing are left out.";

/// The duplicate check's turn: each report with the issues it may repeat, their text read.
pub(crate) fn judge_turn(
    reports: &[(FeedbackReport, Vec<u64>)],
    bodies: &[(KnownIssue, String)],
) -> String {
    let mut text = String::new();
    for (index, (report, candidates)) in reports.iter().enumerate() {
        if candidates.is_empty() {
            continue;
        }
        text.push_str(&format!(
            "Report {index} ({:?}): {}\n",
            report.kind, report.title
        ));
        for (label, field) in [
            ("Steps", &report.steps),
            ("Expected", &report.expected),
            ("What happened", &report.actual),
            ("The problem", &report.problem),
            ("Wanted", &report.wanted),
        ] {
            if !field.is_empty() {
                text.push_str(&format!("{label}: {field}\n"));
            }
        }
        text.push_str("May repeat:\n");
        for number in candidates {
            if let Some((issue, body)) = bodies.iter().find(|(issue, _)| issue.number == *number) {
                let state = if issue.open { "open" } else { "closed" };
                let body: String = body.chars().take(ISSUE_BODY_CHARS).collect();
                text.push_str(&format!(
                    "--- issue #{} ({state}): {}\n{body}\n---\n",
                    issue.number, issue.title
                ));
            }
        }
        text.push('\n');
    }
    text
}

/// The JSON object in a reply: code fences and words around it dropped.
fn json_in(reply: &str) -> Option<serde_json::Value> {
    let start = reply.find('{')?;
    let end = reply.rfind('}')?;
    serde_json::from_str(reply.get(start..=end)?).ok()
}

fn cut(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let kept: String = text.chars().take(max - 1).collect();
    format!("{}…", kept.trim_end())
}

/// The writer's reply as reports, each with the issue numbers it may repeat (only listed ones,
/// at most [`MAX_CANDIDATES`]). A bug's area outside the form's list reads "Other"; the other
/// kind's fields are emptied. A reply with no report is an error.
pub(crate) fn reports_of(
    reply: &str,
    issues: &[KnownIssue],
) -> Result<Vec<(FeedbackReport, Vec<u64>)>> {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        reports: Vec<Raw>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Raw {
        kind: String,
        title: String,
        area: String,
        steps: String,
        expected: String,
        actual: String,
        problem: String,
        today: String,
        wanted: String,
        candidates: Vec<serde_json::Value>,
    }
    let unreadable = || Error::Provider("the report writer's reply couldn't be read".into());
    let reply: Reply = json_in(reply)
        .and_then(|value| serde_json::from_value(value).ok())
        .ok_or_else(unreadable)?;
    let reports: Vec<_> = reply
        .reports
        .into_iter()
        .filter(|raw| !raw.title.trim().is_empty())
        .take(MAX_REPORTS)
        .map(|raw| {
            let kind = if raw.kind.trim().eq_ignore_ascii_case("idea") {
                FeedbackKind::Idea
            } else {
                FeedbackKind::Bug
            };
            let field = |text: &str, of: FeedbackKind| {
                if kind == of {
                    cut(text, MAX_FIELD)
                } else {
                    String::new()
                }
            };
            let area = match kind {
                FeedbackKind::Bug => BUG_AREAS
                    .iter()
                    .find(|area| area.eq_ignore_ascii_case(raw.area.trim()))
                    .unwrap_or(&"Other")
                    .to_string(),
                FeedbackKind::Idea => String::new(),
            };
            let mut candidates: Vec<u64> = Vec::new();
            for number in raw.candidates.iter().filter_map(|value| {
                value
                    .as_u64()
                    .or_else(|| value.as_str()?.trim_start_matches('#').parse().ok())
            }) {
                if issues.iter().any(|issue| issue.number == number)
                    && !candidates.contains(&number)
                    && candidates.len() < MAX_CANDIDATES
                {
                    candidates.push(number);
                }
            }
            let report = FeedbackReport {
                kind,
                title: cut(&raw.title.replace(['\n', '\r'], " "), MAX_TITLE),
                area,
                steps: field(&raw.steps, FeedbackKind::Bug),
                expected: field(&raw.expected, FeedbackKind::Bug),
                actual: field(&raw.actual, FeedbackKind::Bug),
                problem: field(&raw.problem, FeedbackKind::Idea),
                today: field(&raw.today, FeedbackKind::Idea),
                wanted: field(&raw.wanted, FeedbackKind::Idea),
                duplicate: None,
            };
            (report, candidates)
        })
        .collect();
    if reports.is_empty() {
        return Err(Error::Provider("the report writer made no report".into()));
    }
    Ok(reports)
}

/// Marks the reports the duplicate check's `reply` says repeat an issue: only an issue that
/// report was checked against, once per report. An unreadable reply marks none, so every
/// report is filed new.
pub(crate) fn mark_duplicates(
    reports: &mut [(FeedbackReport, Vec<u64>)],
    reply: &str,
    bodies: &[(KnownIssue, String)],
) {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        duplicates: Vec<Raw>,
    }
    #[derive(Deserialize)]
    struct Raw {
        report: usize,
        issue: u64,
        #[serde(default)]
        comment: String,
    }
    let Some(reply) = json_in(reply).and_then(|value| serde_json::from_value::<Reply>(value).ok())
    else {
        tracing::warn!("the duplicate check's reply couldn't be read; filing every report new");
        return;
    };
    for raw in reply.duplicates {
        let Some((report, candidates)) = reports.get_mut(raw.report) else {
            continue;
        };
        if report.duplicate.is_some() || !candidates.contains(&raw.issue) {
            continue;
        }
        let Some((issue, _)) = bodies.iter().find(|(issue, _)| issue.number == raw.issue) else {
            continue;
        };
        let comment = cut(&raw.comment, MAX_FIELD);
        report.duplicate = Some(FeedbackDuplicate {
            number: issue.number,
            title: issue.title.clone(),
            url: issue.url.clone(),
            open: issue.open,
            comment: if comment.is_empty() {
                "Seen again.".to_owned()
            } else {
                comment
            },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(number: u64, title: &str) -> KnownIssue {
        KnownIssue {
            number,
            title: title.into(),
            open: number.is_multiple_of(2),
            url: format!("https://github.com/{FEEDBACK_REPO}/issues/{number}"),
        }
    }

    #[test]
    fn the_areas_are_the_bug_forms_own() {
        let form = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../.github/ISSUE_TEMPLATE/bug_report.yml"
        ))
        .unwrap();
        let after = &form[form.find("id: area").unwrap()..];
        let options = &after[after.find("options:").unwrap()..after.find("validations:").unwrap()];
        let listed: Vec<&str> = options
            .lines()
            .filter_map(|line| line.trim().strip_prefix("- "))
            .collect();
        assert_eq!(listed, BUG_AREAS);
        for area in BUG_AREAS {
            assert!(write_prompt().contains(&format!("\"{area}\"")), "{area}");
        }
    }

    #[test]
    fn a_note_of_a_bug_and_an_idea_becomes_two_reports_each_in_its_forms_fields() {
        let reply = r##"Here you go:
```json
{"reports":[
 {"kind":"bug","title":"Stop leaves the tab spinning","area":"sessions, workers, landing or review",
  "steps":"Not given; see What happened.","expected":"It stops","actual":"It spins",
  "problem":"stray","today":"","wanted":"","candidates":[12, "#13", 999, 12]},
 {"kind":"idea","title":"Pin a chat","area":"Chats","steps":"x","expected":"","actual":"",
  "problem":"Chats get lost","today":"Not given.","wanted":"A pin","candidates":[]}
]}
```"##;
        let reports = reports_of(reply, &[issue(12, "a"), issue(13, "b")]).unwrap();
        assert_eq!(reports.len(), 2);
        let (bug, candidates) = &reports[0];
        assert_eq!(bug.kind, FeedbackKind::Bug);
        assert_eq!(bug.area, "Sessions, workers, landing or review");
        assert_eq!(bug.steps, "Not given; see What happened.");
        assert_eq!(bug.problem, "", "a bug keeps no idea fields");
        assert_eq!(candidates, &[12, 13], "only listed issues, once each");
        let (idea, candidates) = &reports[1];
        assert_eq!(idea.kind, FeedbackKind::Idea);
        assert_eq!((idea.area.as_str(), idea.steps.as_str()), ("", ""));
        assert_eq!(idea.wanted, "A pin");
        assert!(candidates.is_empty());
    }

    #[test]
    fn an_unknown_area_reads_other_and_a_long_title_is_cut() {
        let title = "word ".repeat(40);
        let reply = format!(
            r#"{{"reports":[{{"kind":"bug","title":"{title}","area":"Somewhere","actual":"x"}}]}}"#
        );
        let (report, _) = &reports_of(&reply, &[]).unwrap()[0];
        assert_eq!(report.area, "Other");
        assert!(report.title.chars().count() <= MAX_TITLE);
        assert!(report.title.ends_with('…'));
    }

    #[test]
    fn a_reply_with_no_report_or_no_json_is_an_error() {
        assert!(reports_of("Sorry, I can't.", &[]).is_err());
        assert!(reports_of(r#"{"reports":[]}"#, &[]).is_err());
        assert!(reports_of(r#"{"reports":[{"kind":"bug","title":"  "}]}"#, &[]).is_err());
    }

    #[test]
    fn only_a_checked_candidate_becomes_a_duplicate() {
        let reply = r#"{"reports":[
          {"kind":"bug","title":"A","actual":"x","candidates":[12]},
          {"kind":"idea","title":"B","problem":"y","candidates":[13]}]}"#;
        let issues = [issue(12, "Spinning tab"), issue(13, "Pins")];
        let mut reports = reports_of(reply, &issues).unwrap();
        let bodies: Vec<_> = issues
            .iter()
            .map(|issue| (issue.clone(), "text".to_owned()))
            .collect();
        mark_duplicates(
            &mut reports,
            r#"{"duplicates":[{"report":0,"issue":13,"comment":"not its candidate"},
                              {"report":0,"issue":12,"comment":"Seen on 0.2 too."},
                              {"report":5,"issue":12}]}"#,
            &bodies,
        );
        let duplicate = reports[0].0.duplicate.as_ref().unwrap();
        assert_eq!(duplicate.number, 12);
        assert_eq!(duplicate.comment, "Seen on 0.2 too.");
        assert!(duplicate.open);
        assert_eq!(reports[1].0.duplicate, None);
        // An unreadable check marks nothing.
        let mut fresh = reports_of(reply, &issues).unwrap();
        mark_duplicates(&mut fresh, "no", &bodies);
        assert!(fresh.iter().all(|(report, _)| report.duplicate.is_none()));
    }

    #[test]
    fn the_writers_turn_lists_the_issues_or_says_they_couldnt_be_read() {
        let listed = write_turn("It spins", Some(&[issue(12, "Spinning tab")]));
        assert!(listed.contains("It spins") && listed.contains("#12 open: Spinning tab"));
        assert!(write_turn("x", None).contains("couldn't be read"));
    }
}
