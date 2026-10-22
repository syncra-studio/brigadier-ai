//! A phase settles done only with evidence for every criterion (BUILD-PLAN E2, research A5).
//!
//! - **Every criterion of the frozen plan.** Each `p<phase>-c<n>` of the phase is matched, by
//!   its id or its words, against the phase's report (its verifier's final report when one
//!   exists, else its other workers' reports) and the thread's own `done_when` lines in
//!   `settle_step`. A criterion nobody names, marked [not met] or [not checked] anywhere, or
//!   [met] with no evidence that holds at the tip, keeps the phase from settling done.
//! - **Bound to the accepted tip.** Quoted lines count when the run's worktree, which is at the
//!   tip, still shows their words.

use std::path::Path;

use brigadier_brain::Source;

use super::super::SessionManager;
use super::super::evidence::{met, names_criterion, quoted_lines};
use crate::model::ConversationId;
use crate::overnight::{OvernightPhase, OvernightRun};
use crate::work::{Task, WorkerRole};

/// What a phase settled done rests on.
pub(crate) struct Accepted {
    /// Each criterion with the evidence that met it at the tip.
    pub evidence: Vec<String>,
    /// What is missing, conflicting, failed, unchecked or not bound to the tip: a fact the
    /// step settles with (#251), not a refusal.
    pub unmet: Vec<String>,
}

impl SessionManager {
    /// Whether phase `number` of `run` may settle done at `tip`, the thread's `own` lines
    /// added to its report's.
    pub(crate) async fn accept_phase(
        &self,
        id: &ConversationId,
        run: &OvernightRun,
        number: u32,
        tip: &str,
        own: &[String],
    ) -> Accepted {
        let Some(phase) = run.phases.iter().find(|phase| phase.number == number) else {
            return Accepted {
                evidence: Vec::new(),
                unmet: Vec::new(),
            };
        };
        let board = match self.core.board(id).await {
            Ok(board) => board,
            Err(err) => {
                return Accepted {
                    evidence: Vec::new(),
                    unmet: vec![format!("the board can't be read: {err}")],
                };
            }
        };
        let tasks = phase_tasks(&board, run, number);
        // A verifier's final report stands for the phase; else every report of its workers.
        let reports: Vec<(&Task, &crate::work::Report)> = match tasks
            .iter()
            .rev()
            .find(|task| task.role == Some(WorkerRole::Verifier) && task.report.is_some())
        {
            Some(verifier) => vec![(*verifier, verifier.report.as_ref().expect("reported"))],
            None => tasks
                .iter()
                .filter_map(|task| task.report.as_ref().map(|report| (*task, report)))
                .collect(),
        };
        let mut lines: Vec<(String, String)> = reports
            .iter()
            .flat_map(|(task, report)| {
                report
                    .done_when
                    .iter()
                    .map(move |line| (format!("task-{}'s report", task.number), line.clone()))
            })
            .collect();
        lines.extend(
            own.iter()
                .map(|line| ("your settle_step".to_owned(), line.clone())),
        );
        let workspace = run
            .workspace
            .as_ref()
            .map(|workspace| workspace.path.clone());
        // A quoted file counts only as it is in the tip's commit.
        let reader = workspace.as_ref().map(|path| {
            self.sources_for(id, Some(Path::new(path)))
                .at_commit(path.into(), tip.to_owned())
        });
        let mut evidence = Vec::new();
        let mut problems = Vec::new();
        for criterion in &phase.done_when {
            let named: Vec<&(String, String)> = lines
                .iter()
                .filter(|(_, line)| names(line, phase, &criterion.id, &criterion.text))
                .collect();
            if named.is_empty() {
                problems.push(format!(
                    "{} ({}): no report or settle_step line gives it",
                    criterion.id, criterion.text
                ));
                continue;
            }
            let (held, unmet): (Vec<_>, Vec<_>) =
                named.into_iter().partition(|(_, line)| met(line).is_some());
            if !unmet.is_empty() {
                let (whose, line) = unmet[0];
                problems.push(format!(
                    "{}: {} in {whose}{}",
                    criterion.id,
                    status_of(line),
                    if held.is_empty() {
                        String::new()
                    } else {
                        " while another line says [met]: they conflict".to_owned()
                    }
                ));
                continue;
            }
            let mut why_not = Vec::new();
            let mut found = None;
            'lines: for (_, line) in &held {
                let rest = met(line).unwrap_or_default();
                for (cite, quote) in quoted_lines(rest) {
                    let Some(reader) = &reader else {
                        why_not.push(format!("{cite} can't be read: the run has no checkout"));
                        continue;
                    };
                    let checked = reader.check(&Source {
                        cite: cite.clone(),
                        quote: quote.clone(),
                        blob: None,
                    });
                    match checked {
                        Some(checked) if checked.holds => {
                            found = Some(format!("{cite} shows `{quote}` at the tip"));
                            break 'lines;
                        }
                        Some(checked) => why_not.push(
                            checked
                                .why
                                .unwrap_or_else(|| format!("{cite} doesn't show it")),
                        ),
                        None => {}
                    }
                }
            }
            match found {
                Some(found) => evidence.push(format!("{}: {found}", criterion.id)),
                None => problems.push(format!(
                    "{}: [met] with no evidence that holds at the tip{}",
                    criterion.id,
                    if why_not.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", why_not.join("; "))
                    }
                )),
            }
        }
        Accepted {
            evidence,
            unmet: problems,
        }
    }
}

/// The run's tasks of phase `number`, oldest first.
fn phase_tasks<'a>(
    board: &'a crate::board::Board,
    run: &OvernightRun,
    number: u32,
) -> Vec<&'a Task> {
    let mut tasks: Vec<&Task> = board
        .tasks
        .values()
        .filter(|task| {
            task.phase == Some(number)
                && task
                    .run
                    .as_ref()
                    .is_some_and(|context| context.run_id == run.id)
        })
        .collect();
    tasks.sort_by_key(|task| task.number);
    tasks
}

/// A "done when" line names this criterion: by its id, or by its own words (the line names no
/// other criterion's id).
fn names(line: &str, phase: &OvernightPhase, id: &str, text: &str) -> bool {
    if names_criterion(line, id) {
        return true;
    }
    if phase
        .done_when
        .iter()
        .any(|other| other.id != id && names_criterion(line, &other.id))
    {
        return false;
    }
    let words = |text: &str| {
        text.split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .trim_end_matches('.')
            .to_lowercase()
    };
    let text = words(text);
    !text.is_empty() && words(line).contains(&text)
}

/// A line's status, as the refusal names it.
fn status_of(line: &str) -> &'static str {
    let lower = line.trim_start().to_lowercase();
    if lower.contains("[not met]") {
        "[not met]"
    } else if lower.contains("[not checked]") {
        "[not checked]"
    } else {
        "no status"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overnight::OvernightPhase;

    #[test]
    fn a_line_names_a_criterion_by_its_id_or_its_words() {
        let phase = OvernightPhase::new(
            2,
            "Files",
            "Make them.",
            &["p2.txt exists.".into(), "The docs mention it.".into()],
            &[],
        );
        assert!(names(
            "[met] p2-c1: out-1",
            &phase,
            "p2-c1",
            "p2.txt exists."
        ));
        assert!(names(
            "[met] P2.txt exists: p2.txt:1 `x`",
            &phase,
            "p2-c1",
            "p2.txt exists."
        ));
        // Another criterion's id: not this one, whatever its words.
        assert!(!names(
            "[met] p2-c2 p2.txt exists",
            &phase,
            "p2-c1",
            "p2.txt exists."
        ));
        assert!(!names(
            "[met] p2-c10: out-1",
            &phase,
            "p2-c1",
            "p2.txt exists."
        ));
    }

    #[test]
    fn a_status_is_named() {
        assert_eq!(status_of("[not met] p2-c1"), "[not met]");
        assert_eq!(status_of("[Not checked] p2-c1"), "[not checked]");
        assert_eq!(status_of("p2-c1 looked fine"), "no status");
    }
}
