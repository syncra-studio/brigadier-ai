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

/// What each of [`BUG_AREAS`] covers, for the writer: an area is picked only when the bug
/// plainly sits in it, else "Other".
const AREA_HINTS: [(&str, &str); 11] = [
    (
        "Sessions, workers, landing or review",
        "a session's conversation, its workers or jobs, landing or reviewing their changes",
    ),
    (
        "Chats",
        "the Chats (one-off conversations outside a project)",
    ),
    ("Overnight runs", "overnight runs"),
    (
        "Accounts, usage or routing",
        "Claude or Codex accounts, usage and limits, which account or model a task goes to",
    ),
    ("Computer use", "the agent driving apps on the computer"),
    ("Built-in browser", "the built-in browser panel"),
    (
        "Terminal, files or source",
        "the terminal, the file tree and editor, git and diffs",
    ),
    (
        "Dictation or read aloud",
        "dictating into the composer, reading replies aloud",
    ),
    (
        "Settings, onboarding or uninstall",
        "Settings, the first-launch screens, removing the app",
    ),
    ("Build from source", "building Brigadier from its source"),
    (
        "Other",
        "everything else, the app's frame too: the sidebar, tabs, windows, menus, switching projects, the look",
    ),
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
    /// The open issue of its kind it repeats, when the check was sure: a note goes there instead.
    pub duplicate: Option<FeedbackDuplicate>,
    /// Issues close to it that it doesn't repeat (closed, another kind, or not the same
    /// thing): a new issue names them as maybe related.
    pub related: Vec<u64>,
}

/// An existing issue a report repeats, and the note for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct FeedbackDuplicate {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub open: bool,
    /// The note for it: the report's own fields, so only what the user wrote.
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

/// Where signing in to GitHub for Send feedback starts: already signed in, no GitHub CLI to sign
/// in with, or waiting for the user to enter `code` at `url`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum GitHubSignIn {
    SignedIn,
    NoGh,
    Code { code: String, url: String },
}

/// The one-time code in a line `gh auth login --web` printed ("! One-time code (ABCD-1234)
/// copied to clipboard", older: "! First copy your one-time code: ABCD-1234").
pub(crate) fn device_code(line: &str) -> Option<String> {
    if !line.to_lowercase().contains("code") {
        return None;
    }
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .find(|word| {
            let bytes = word.as_bytes();
            bytes.len() == 9
                && bytes[4] == b'-'
                && word
                    .chars()
                    .filter(|c| *c != '-')
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
        .map(str::to_owned)
}

/// The page a line `gh auth login --web` printed sends the user to.
pub(crate) fn device_page(line: &str) -> Option<String> {
    line.split_whitespace()
        .find(|word| word.starts_with("https://"))
        .map(|word| word.trim_end_matches(['.', ',']).to_owned())
}

/// An issue of the repository as listed: enough to name it to the writer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct KnownIssue {
    pub number: u64,
    pub title: String,
    pub open: bool,
    pub url: String,
    /// Its form's kind, by its label or title prefix; `None` when it has neither.
    pub kind: Option<FeedbackKind>,
}

impl KnownIssue {
    /// An issue's kind by its labels (`bug`, `proposal`), else its form's title prefix.
    pub(crate) fn kind_of<'a>(
        title: &str,
        labels: impl IntoIterator<Item = &'a str>,
    ) -> Option<FeedbackKind> {
        for label in labels {
            match label.trim().to_ascii_lowercase().as_str() {
                "bug" => return Some(FeedbackKind::Bug),
                "proposal" => return Some(FeedbackKind::Idea),
                _ => {}
            }
        }
        let title = title.trim_start().to_ascii_lowercase();
        if title.starts_with("[bug]") {
            Some(FeedbackKind::Bug)
        } else if title.starts_with("[proposal]") {
            Some(FeedbackKind::Idea)
        } else {
            None
        }
    }
}

fn kind_word(kind: FeedbackKind) -> &'static str {
    match kind {
        FeedbackKind::Bug => "bug",
        FeedbackKind::Idea => "idea",
    }
}

/// The writer's instructions: the note becomes reports, with candidates for the duplicate check.
pub(crate) fn write_prompt() -> String {
    let areas = AREA_HINTS
        .iter()
        .map(|(area, covers)| format!("  \"{area}\": {covers}"))
        .collect::<Vec<_>>()
        .join("\n");
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
- A bug fills area, steps (to reproduce), expected and actual (what happened). area is exactly \
one of the bug form's options below, read literally: the one the bug plainly sits in, by where \
it shows, not what it might touch. When none fits plainly, \"Other\".\n{areas}\n\
- An idea fills problem (what they're trying to do and what gets in the way), today \
(what they do now) and wanted (what should happen instead). Leave the other kind's fields \"\".\n\
- Use only what the note says, in plain words close to the user's own. Never invent steps, \
versions, errors or details, and never take any from the issue list. What the note doesn't say reads \"Not given.\"; steps it doesn't \
give read \"Not given; see What happened.\"\n\
- Leave out secrets, emails, tokens and file paths.\n\
- candidates: the numbers of up to 3 existing issues listed below (open or closed) that may be \
about the same thing, closest first; [] when none come close."
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

/// The duplicate check's instructions. It only judges; which match becomes a comment is
/// [`mark_duplicates`]'s rule (an open issue of the report's own kind).
pub(crate) const JUDGE_PROMPT: &str = "You decide whether new reports about Brigadier repeat \
an existing GitHub issue. Reply with JSON only, no code fences: \
{\"matches\":[{\"report\":0,\"issue\":123,\"issueKind\":\"bug\",\"same\":false}]}\n\
Rules:\n\
- List a report with an issue only when the issue is close to it: the same problem or wish, \
or one very like it. Same part of the app alone is not close. Reports with no close issue are \
left out.\n\
- same: true only when the issue describes the very same specific thing: the same behaviour \
going wrong in the same way, or the same wish. A broader, narrower or neighbouring problem \
is not the same (a stop that fails during setup is not every stop failing). When unsure, \
false: the report is filed as a new issue that links this one.\n\
- A bug is never the same as an idea, nor an idea as a bug.\n\
- issueKind: \"bug\" when the issue reports something Brigadier does wrong, \"idea\" when it \
asks for something new or a change.";

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
            "Report {index} ({}): {}\n",
            kind_word(report.kind),
            report.title
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
                let kind = issue.kind.map_or("kind not marked", kind_word);
                text.push_str(&format!(
                    "--- issue #{} ({state}, {kind}): {}\n{body}\n---\n",
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
                related: Vec::new(),
            };
            (report, candidates)
        })
        .collect();
    if reports.is_empty() {
        return Err(Error::Provider("the report writer made no report".into()));
    }
    Ok(reports)
}

/// Most issues a new one names as maybe related.
const MAX_RELATED: usize = 3;

/// Marks what the duplicate check's `reply` matched, among the issues each report was checked
/// against. A report repeats (and becomes a comment on) an issue only when the check says it
/// is the same thing, the issue is open, and it is of the report's kind: by its label or title
/// prefix, else as the check read it. Any other match is named as maybe related. An unreadable
/// reply marks none, so every report is filed new.
pub(crate) fn mark_duplicates(
    reports: &mut [(FeedbackReport, Vec<u64>)],
    reply: &str,
    bodies: &[(KnownIssue, String)],
) {
    #[derive(Deserialize)]
    struct Reply {
        #[serde(default)]
        matches: Vec<Raw>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Raw {
        report: usize,
        issue: u64,
        #[serde(default)]
        issue_kind: String,
        #[serde(default)]
        same: bool,
    }
    let Some(reply) = json_in(reply).and_then(|value| serde_json::from_value::<Reply>(value).ok())
    else {
        tracing::warn!("the duplicate check's reply couldn't be read; filing every report new");
        return;
    };
    for raw in reply.matches {
        let Some((report, candidates)) = reports.get_mut(raw.report) else {
            continue;
        };
        if !candidates.contains(&raw.issue) {
            continue;
        }
        let Some((issue, _)) = bodies.iter().find(|(issue, _)| issue.number == raw.issue) else {
            continue;
        };
        let read_kind = match raw.issue_kind.trim().to_ascii_lowercase().as_str() {
            "bug" => Some(FeedbackKind::Bug),
            "idea" | "proposal" => Some(FeedbackKind::Idea),
            _ => None,
        };
        let same_kind = issue.kind.or(read_kind) == Some(report.kind);
        if raw.same && issue.open && same_kind && report.duplicate.is_none() {
            report.related.retain(|number| *number != issue.number);
            report.duplicate = Some(FeedbackDuplicate {
                number: issue.number,
                title: issue.title.clone(),
                url: issue.url.clone(),
                open: issue.open,
                comment: repeat_note(report),
            });
        } else if report.duplicate.as_ref().map(|d| d.number) != Some(issue.number)
            && !report.related.contains(&issue.number)
            && report.related.len() < MAX_RELATED
        {
            report.related.push(issue.number);
        }
    }
}

/// What a repeat says on its issue: that it was seen again, then the report's own fields
/// (what the user wrote, nothing of the issue's), leaving out what the note didn't give.
pub(crate) fn repeat_note(report: &FeedbackReport) -> String {
    let (head, fields) = match report.kind {
        FeedbackKind::Bug => (
            "Seen again.",
            [
                ("What happened", &report.actual),
                ("Expected", &report.expected),
                ("Steps to reproduce", &report.steps),
            ],
        ),
        FeedbackKind::Idea => (
            "Asked for again.",
            [
                ("The problem", &report.problem),
                ("What they do today", &report.today),
                ("What should happen instead", &report.wanted),
            ],
        ),
    };
    let lines: Vec<String> = fields
        .iter()
        .map(|(label, value)| (label, value.trim()))
        .filter(|(_, value)| !value.is_empty() && !value.starts_with("Not given"))
        .map(|(label, value)| format!("{label}: {value}"))
        .collect();
    if lines.is_empty() {
        return head.to_owned();
    }
    format!("{head}\n\n{}", lines.join("\n"))
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
            kind: None,
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
        let hinted: Vec<&str> = AREA_HINTS.iter().map(|(area, _)| *area).collect();
        assert_eq!(
            hinted, BUG_AREAS,
            "every option has its hint, in the form's order"
        );
        for area in BUG_AREAS {
            assert!(write_prompt().contains(&format!("\"{area}\"")), "{area}");
        }
        // The app's frame, which no option names, is the form's catch-all.
        assert!(AREA_HINTS[10].1.contains("sidebar"));
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

    type Checked = Vec<(FeedbackReport, Vec<u64>)>;

    /// A bug (report 0) and an idea (report 1), each checked against #12 (open), #13 (closed)
    /// and #14 (open, an idea by its label).
    fn checked() -> (Checked, Vec<(KnownIssue, String)>) {
        let reply = r#"{"reports":[
          {"kind":"bug","title":"A","actual":"Stop does nothing.","expected":"Not given.",
           "steps":"Not given; see What happened.","candidates":[12,13,14]},
          {"kind":"idea","title":"B","problem":"Diffs are bright.","today":"Not given.",
           "wanted":"A dark diff.","candidates":[12,13,14]}]}"#;
        let mut idea = issue(14, "[Proposal]: Dark diffs");
        idea.kind = KnownIssue::kind_of(&idea.title, ["area: diff"]);
        let issues = [issue(12, "Spinning tab"), issue(13, "Stop in setup"), idea];
        let reports = reports_of(reply, &issues).unwrap();
        let bodies = issues
            .iter()
            .map(|issue| (issue.clone(), "text".to_owned()))
            .collect();
        (reports, bodies)
    }

    #[test]
    fn an_open_issue_of_its_kind_the_check_calls_the_same_takes_a_comment() {
        let (mut reports, bodies) = checked();
        mark_duplicates(
            &mut reports,
            r#"{"matches":[{"report":0,"issue":12,"issueKind":"bug","same":true},
                           {"report":0,"issue":99,"issueKind":"bug","same":true},
                           {"report":5,"issue":12,"same":true}]}"#,
            &bodies,
        );
        let duplicate = reports[0].0.duplicate.as_ref().unwrap();
        assert_eq!((duplicate.number, duplicate.open), (12, true));
        assert!(reports[0].0.related.is_empty(), "not a candidate: no link");
        assert_eq!(reports[1].0.duplicate, None);
        // An unreadable check marks nothing.
        let (mut fresh, bodies) = checked();
        mark_duplicates(&mut fresh, "no", &bodies);
        assert!(fresh.iter().all(|(report, _)| report.duplicate.is_none()));
    }

    #[test]
    fn a_closed_issue_is_never_commented_on_only_linked() {
        let (mut reports, bodies) = checked();
        mark_duplicates(
            &mut reports,
            r#"{"matches":[{"report":0,"issue":13,"issueKind":"bug","same":true}]}"#,
            &bodies,
        );
        assert_eq!(reports[0].0.duplicate, None, "a new issue");
        assert_eq!(reports[0].0.related, [13]);
    }

    #[test]
    fn a_bug_never_folds_into_an_idea_nor_an_idea_into_a_bug() {
        let (mut reports, bodies) = checked();
        mark_duplicates(
            &mut reports,
            // #14 is an idea by its title's prefix, whatever the check read; #12 is unmarked,
            // so the check's reading of it counts.
            r#"{"matches":[{"report":0,"issue":14,"issueKind":"bug","same":true},
                           {"report":1,"issue":12,"issueKind":"bug","same":true},
                           {"report":1,"issue":12,"issueKind":"bug","same":true}]}"#,
            &bodies,
        );
        assert_eq!(reports[0].0.duplicate, None);
        assert_eq!(reports[0].0.related, [14]);
        assert_eq!(reports[1].0.duplicate, None);
        assert_eq!(reports[1].0.related, [12], "named once");
        // An unmarked issue the check didn't read a kind for takes no comment either.
        let (mut reports, bodies) = checked();
        mark_duplicates(
            &mut reports,
            r#"{"matches":[{"report":0,"issue":12,"same":true},
                           {"report":1,"issue":14,"issueKind":"bug","same":true}]}"#,
            &bodies,
        );
        assert_eq!(reports[0].0.duplicate, None);
        assert_eq!(reports[0].0.related, [12]);
        assert_eq!(reports[1].0.duplicate.as_ref().map(|d| d.number), Some(14));
    }

    #[test]
    fn a_close_match_that_isnt_the_same_thing_is_linked() {
        let (mut reports, bodies) = checked();
        mark_duplicates(
            &mut reports,
            r#"{"matches":[{"report":0,"issue":12,"issueKind":"bug","same":false},
                           {"report":0,"issue":13,"issueKind":"bug"}]}"#,
            &bodies,
        );
        assert_eq!(reports[0].0.duplicate, None);
        assert_eq!(reports[0].0.related, [12, 13]);
    }

    #[test]
    fn the_repeat_note_is_the_reports_own_words_and_nothing_of_the_issue() {
        let (mut reports, bodies) = checked();
        mark_duplicates(
            &mut reports,
            r#"{"matches":[{"report":0,"issue":12,"issueKind":"bug","same":true,
                            "comment":"while setup is still running"},
                           {"report":1,"issue":14,"same":true}]}"#,
            &bodies,
        );
        assert_eq!(
            reports[0].0.duplicate.as_ref().unwrap().comment,
            "Seen again.\n\nWhat happened: Stop does nothing.",
            "what the note didn't give is left out; the check's words never get in"
        );
        assert_eq!(
            reports[1].0.duplicate.as_ref().unwrap().comment,
            "Asked for again.\n\nThe problem: Diffs are bright.\nWhat should happen instead: A dark diff."
        );
    }

    #[test]
    fn an_issues_kind_is_its_label_else_its_forms_prefix() {
        assert_eq!(
            KnownIssue::kind_of("Stop", ["bug", "area: x"]),
            Some(FeedbackKind::Bug)
        );
        assert_eq!(
            KnownIssue::kind_of("[Bug]: x", ["proposal"]),
            Some(FeedbackKind::Idea)
        );
        assert_eq!(
            KnownIssue::kind_of("[Proposal]: x", []),
            Some(FeedbackKind::Idea)
        );
        assert_eq!(
            KnownIssue::kind_of("[bug] x", ["nightly"]),
            Some(FeedbackKind::Bug)
        );
        assert_eq!(
            KnownIssue::kind_of("The diff has no colour", ["area: x"]),
            None
        );
    }

    #[test]
    fn the_one_time_code_and_its_page_are_read_from_ghs_lines() {
        assert_eq!(
            device_code("! One-time code (7E28-DD0E) copied to clipboard").as_deref(),
            Some("7E28-DD0E")
        );
        assert_eq!(
            device_code("! First copy your one-time code: ABCD-1234").as_deref(),
            Some("ABCD-1234")
        );
        assert_eq!(
            device_code("Press Enter to open github.com in your browser..."),
            None
        );
        assert_eq!(
            device_code("code: ab12-cd34"),
            None,
            "GitHub's codes are upper case"
        );
        assert_eq!(
            device_page(
                "Open this URL to continue in your web browser: https://github.com/login/device"
            )
            .as_deref(),
            Some("https://github.com/login/device")
        );
        assert_eq!(
            device_page("! One-time code (7E28-DD0E) copied to clipboard"),
            None
        );
    }

    #[test]
    fn the_writers_turn_lists_the_issues_or_says_they_couldnt_be_read() {
        let listed = write_turn("It spins", Some(&[issue(12, "Spinning tab")]));
        assert!(listed.contains("It spins") && listed.contains("#12 open: Spinning tab"));
        assert!(write_turn("x", None).contains("couldn't be read"));
    }
}
