//! A phase settles done only with evidence for every criterion (BUILD-PLAN E2, research A5).
//!
//! - **Every criterion of the frozen plan.** Each `p<phase>-c<n>` of the phase is matched, by
//!   its id or its words, against the phase's report (its verifier's final report when one
//!   exists, else its other workers' reports) and the thread's own `done_when` lines in
//!   `settle_step`. A criterion nobody names, marked [not met] or [not checked] anywhere, or
//!   [met] with no evidence that holds at the tip, keeps the phase from settling done.
//! - **Bound to the accepted tip.** A check counts when it passed on the tip's own tree (its
//!   whole input fingerprint starts with that tree, so nothing older can stand in for it), the
//!   session's full checks when they passed at the tip, and quoted lines when the run's
//!   worktree, which is at the tip, still shows their words.
//! - **The review.** With reviews on, the phase's reviews must have ended and their findings
//!   been recorded; with reviews off, the settlement says no review was run.

use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use brigadier_brain::Source;

use super::super::SessionManager;
use super::super::evidence::{met, names_criterion, output_ids, quoted_lines};
use crate::model::ConversationId;
use crate::overnight::{OvernightPhase, OvernightRun};
use crate::work::{FindingStatus, IntegratedCheckState, ReviewState, Task, WorkerRole};

/// How long settling waits for a phase's review still running before it says so.
const REVIEW_WAIT: Duration = Duration::from_secs(if cfg!(test) { 10 } else { 30 });

/// What a phase settled done rests on.
pub(crate) struct Accepted {
    /// Each criterion with the evidence that met it at the tip.
    pub evidence: Vec<String>,
    /// Its review, as the morning report says it.
    pub review: String,
    /// What is missing, conflicting, failed, unchecked or not bound to the tip: a fact the
    /// step settles with (#251), not a refusal.
    pub unmet: Vec<String>,
}

/// Why a phase can't settle done yet.
pub(crate) enum NotAccepted {
    /// Its review hasn't ended, or its findings aren't recorded.
    Review(String),
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
    ) -> std::result::Result<Accepted, NotAccepted> {
        let review = self.phase_review(id, run, number).await?;
        let Some(phase) = run.phases.iter().find(|phase| phase.number == number) else {
            return Ok(Accepted {
                evidence: Vec::new(),
                review,
                unmet: Vec::new(),
            });
        };
        let board = match self.core.board(id).await {
            Ok(board) => board,
            Err(err) => {
                return Ok(Accepted {
                    evidence: Vec::new(),
                    review,
                    unmet: vec![format!("the board can't be read: {err}")],
                });
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
        let tree = match &workspace {
            Some(path) => tree_of(Path::new(path), tip),
            None => None,
        };
        let checks = self.check_runs(id).await;
        let full: HashSet<String> = board
            .integrated_checks
            .values()
            .filter(|check| {
                check.tip == tip && matches!(check.state, IntegratedCheckState::Passed { .. })
            })
            .map(|check| check.id.clone())
            .collect();
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
                for alias in output_ids(rest) {
                    match checks.get(&alias) {
                        Some(run)
                            if run.status == "exit 0" && run.tree.is_some() && run.tree == tree =>
                        {
                            found = Some(format!(
                                "{alias} (`{}`) passed on the tip's tree",
                                run.command
                            ));
                            break 'lines;
                        }
                        Some(run) if run.status != "exit 0" => {
                            why_not.push(format!("{alias} ended {}", run.status));
                        }
                        Some(_) => {
                            why_not.push(format!("{alias} ran on another tree than the tip's"))
                        }
                        None => why_not.push(format!("{alias} is no check of this session")),
                    }
                }
                for word in rest.split(|c: char| !(c.is_ascii_alphanumeric() || c == '-')) {
                    if word.starts_with("check-") && full.contains(word) {
                        found = Some(format!(
                            "{word}, the session's full checks, passed at the tip"
                        ));
                        break 'lines;
                    }
                }
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
        Ok(Accepted {
            evidence,
            review,
            unmet: problems,
        })
    }

    /// A review ended: when settling its run's phase waited for it and nothing of the phase is
    /// reviewed any more, the thread hears it may settle now.
    pub(crate) async fn settle_wait_over(&self, review: &crate::work::ReviewRun) {
        let Some(task_id) = &review.task_id else {
            return;
        };
        let Ok(board) = self.core.board(&review.conversation_id).await else {
            return;
        };
        let Some((run_id, number)) = board
            .tasks
            .get(task_id)
            .and_then(|task| Some((task.run.as_ref()?.run_id.clone(), task.phase?)))
        else {
            return;
        };
        let waited = self
            .overnight
            .settle_waits
            .lock()
            .is_ok_and(|waits| waits.contains(&(run_id.clone(), number)));
        let Some(run) = board.runs.get(&run_id).filter(|_| waited) else {
            return;
        };
        let tasks: HashSet<_> = phase_tasks(&board, run, number)
            .into_iter()
            .map(|task| task.id.clone())
            .collect();
        let running = board.reviews.values().any(|other| {
            other.state == ReviewState::Running
                && other
                    .task_id
                    .as_ref()
                    .is_some_and(|task| tasks.contains(task))
        });
        if running {
            return;
        }
        if let Ok(mut waits) = self.overnight.settle_waits.lock() {
            waits.remove(&(run_id, number));
        }
        self.tell_thread(
            run,
            "phase review ended",
            format!("[run] Phase {number}'s review has ended: settle the phase now with settle_step (record its findings first, if it had any)."),
        )
        .await;
    }

    /// The phase's review as it stands: with reviews on, it waits a little for one still
    /// running, and refuses one whose findings aren't recorded.
    async fn phase_review(
        &self,
        id: &ConversationId,
        run: &OvernightRun,
        number: u32,
    ) -> std::result::Result<String, NotAccepted> {
        if !self.reviews_on() {
            return Ok("No review was run: reviews are off.".to_owned());
        }
        let started = std::time::Instant::now();
        loop {
            let Ok(board) = self.core.board(id).await else {
                return Err(NotAccepted::Review("the board can't be read".into()));
            };
            let tasks: HashSet<_> = phase_tasks(&board, run, number)
                .into_iter()
                .map(|task| task.id.clone())
                .collect();
            let reviews: Vec<_> = board
                .reviews
                .values()
                .filter(|review| {
                    review
                        .task_id
                        .as_ref()
                        .is_some_and(|task| tasks.contains(task))
                })
                .collect();
            if reviews
                .iter()
                .any(|review| review.state == ReviewState::Running)
            {
                if started.elapsed() < REVIEW_WAIT {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }
                return Err(NotAccepted::Review(format!(
                    "Phase {number}'s review still runs, and a phase is done only once it has ended and its findings are recorded. You hear when it ends; settle the phase then."
                )));
            }
            let open: Vec<&str> = reviews
                .iter()
                .flat_map(|review| &review.items)
                .filter(|item| item.status == FindingStatus::New)
                .map(|item| item.id.as_str())
                .collect();
            if !open.is_empty() {
                return Err(NotAccepted::Review(format!(
                    "Phase {number}'s review findings {} aren't recorded yet: record each with record_findings (fixed, and how it was checked, or why not), then settle the phase.",
                    open.join(", ")
                )));
            }
            let said: Vec<String> = reviews
                .iter()
                .map(|review| match &review.state {
                    ReviewState::Clean => format!("R{} found nothing", review.number),
                    ReviewState::Findings { count } => {
                        format!("R{}'s {count} findings are recorded", review.number)
                    }
                    ReviewState::Failed { reason } => {
                        format!("R{} could not run ({reason})", review.number)
                    }
                    ReviewState::Running => format!("R{} runs", review.number),
                })
                .collect();
            return Ok(if said.is_empty() {
                "No review was asked for.".to_owned()
            } else {
                format!("Review settled: {}.", said.join("; "))
            });
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

/// The git tree of commit `tip` in the repository at `dir`.
fn tree_of(dir: &Path, tip: &str) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", &format!("{tip}^{{tree}}")])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
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
