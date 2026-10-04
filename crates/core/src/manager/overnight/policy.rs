//! What an overnight run may do on the user's behalf (PLAN.md §10.8), enforced in code.
//!
//! A run keeps its session's access (full access runs its tasks unsandboxed) and approves for
//! the user: plans are decided for them and leaving the sandbox is approved. What only the
//! user may do is refused at once and listed in the report's "What got in the way": outward
//! actions (the approval route and the command gate), changes to the user's own checkout and
//! to branches the run doesn't own (the approval route and the git guard), and landing despite
//! failed checks. Run tasks don't get credentials: the project's secret files aren't copied
//! in, credential variables are taken out of the worker's environment, and a sandboxed
//! worker can't read their usual locations.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::{ConversationId, OvernightRunId};
use crate::overnight::{ObstacleKind, OvernightRun, RunRole, RunTaskContext, RunWorkspace};
use crate::work::Task;

/// A session's active run, as task code needs it without reading the board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveRun {
    pub id: OvernightRunId,
    pub segment: u32,
    pub generation: u32,
    pub rules_hash: String,
    pub workspace: Option<RunWorkspace>,
    /// "max N workers": its tasks executing at once.
    pub max_workers: Option<u32>,
    /// The phase being worked on now (`phase-0` while Phase 0 writes the plan).
    pub phase_id: Option<String>,
    /// Phase 0: only the plan is written, nothing changes yet.
    pub planning: bool,
    /// Ending (Stop, the deadline, a block): no new work starts.
    pub winding_down: bool,
    /// When wind-down starts, for a deadline run (wall clock, ms).
    pub wind_down_at_ms: Option<i64>,
}

/// The active run of each session that has one. Kept in step with every recorded run, and
/// rebuilt from the boards at startup.
#[derive(Default)]
pub(crate) struct ActiveRuns(std::sync::Mutex<HashMap<ConversationId, ActiveRun>>);

impl ActiveRuns {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ConversationId, ActiveRun>> {
        self.0.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn get(&self, id: &ConversationId) -> Option<ActiveRun> {
        self.lock().get(id).cloned()
    }

    /// Every session's active run.
    pub fn all(&self) -> Vec<(ConversationId, ActiveRun)> {
        self.lock()
            .iter()
            .map(|(id, run)| (id.clone(), run.clone()))
            .collect()
    }

    /// Takes in a run as just recorded: an active one is the session's run, one that ended
    /// no longer is.
    pub fn note(&self, run: &OvernightRun) {
        let mut active = self.lock();
        if run.state.is_active() {
            active.insert(
                run.conversation_id.clone(),
                ActiveRun {
                    id: run.id.clone(),
                    segment: run.segment,
                    generation: run.generation,
                    rules_hash: rules_hash(&run.rules),
                    workspace: run.workspace.clone(),
                    max_workers: run.directives.max_workers,
                    phase_id: current_phase(run),
                    planning: run.state == crate::overnight::OvernightState::Planning,
                    winding_down: matches!(
                        run.state,
                        crate::overnight::OvernightState::WindingDown
                            | crate::overnight::OvernightState::Reporting
                    ),
                    wind_down_at_ms: run.wind_down_at_ms,
                },
            );
        } else if active
            .get(&run.conversation_id)
            .is_some_and(|now| now.id == run.id)
        {
            active.remove(&run.conversation_id);
        }
    }
}

/// The phase a run works on now: Phase 0 while it plans, else the one running or checked.
fn current_phase(run: &OvernightRun) -> Option<String> {
    if run.state == crate::overnight::OvernightState::Planning {
        return Some(PLANNING_PHASE.into());
    }
    run.phases
        .iter()
        .find(|phase| {
            matches!(
                phase.state,
                crate::overnight::PhaseState::Running | crate::overnight::PhaseState::Checking
            )
        })
        .map(|phase| phase.id.clone())
}

/// Phase 0's id.
pub(crate) const PLANNING_PHASE: &str = "phase-0";

impl ActiveRun {
    /// The context a task made now gets: a check of another task's change inherits that
    /// task's run (or none: checks of work from before the run stay the session's), anything
    /// else works for the run's current phase.
    pub fn context_for(active: Option<&Self>, subject: Option<&Task>) -> Option<RunTaskContext> {
        match subject {
            Some(subject) => subject.run.clone().map(|run| RunTaskContext {
                role: RunRole::Check,
                candidate: None,
                ..run
            }),
            None => active.map(|active| active.context(RunRole::Worker)),
        }
    }

    /// A task of the run's current phase in `role`.
    pub fn context(&self, role: RunRole) -> RunTaskContext {
        RunTaskContext {
            run_id: self.id.clone(),
            segment: self.segment,
            phase_id: self.phase_id.clone(),
            generation: self.generation,
            role,
            rules_hash: self.rules_hash.clone(),
            candidate: None,
        }
    }
}

/// A short, stable hash of the run's Rules, so a task's briefing can be matched to them.
pub(crate) fn rules_hash(rules: &str) -> String {
    blake3::hash(rules.as_bytes()).to_hex()[..16].to_owned()
}

/// Where credentials usually live under `home`: unreadable for run tasks.
pub(crate) fn credential_paths(home: &Path) -> Vec<PathBuf> {
    [
        ".ssh",
        ".gnupg",
        ".config/gh",
        ".config/hub",
        ".config/gcloud",
        ".config/op",
        ".config/doctl",
        ".git-credentials",
        ".config/git/credentials",
        ".netrc",
        ".npmrc",
        ".yarnrc.yml",
        ".pypirc",
        ".gem/credentials",
        ".cargo/credentials",
        ".cargo/credentials.toml",
        ".aws",
        ".azure",
        ".kube",
        ".docker/config.json",
        ".terraform.d/credentials.tfrc.json",
        ".fly",
        ".vercel",
        ".netlify",
        ".wrangler",
        ".railway",
        "Library/Keychains",
        "Library/Application Support/gh",
    ]
    .iter()
    .map(|rest| home.join(rest))
    .collect()
}

/// Environment variables taken out of a run worker's environment: agent sockets, askpass
/// helpers and tokens, which commands could use to act as the user. The CLI's own sign-in
/// stays (it can't work without it).
pub(crate) fn scrubbed_env(names: impl IntoIterator<Item = String>) -> Vec<String> {
    const KEEP: &[&str] = &[
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "OPENAI_API_KEY",
        "CODEX_API_KEY",
        "BRIGADIER_GATE",
    ];
    const EXACT: &[&str] = &[
        "SSH_AUTH_SOCK",
        "SSH_ASKPASS",
        "GIT_ASKPASS",
        "SUDO_ASKPASS",
        "GH_TOKEN",
        "GITHUB_TOKEN",
        "GH_ENTERPRISE_TOKEN",
        "GITHUB_ENTERPRISE_TOKEN",
        "GITLAB_TOKEN",
        "NPM_TOKEN",
        "NODE_AUTH_TOKEN",
        "CARGO_REGISTRY_TOKEN",
        "AWS_ACCESS_KEY_ID",
        "AWS_SECRET_ACCESS_KEY",
        "AWS_SESSION_TOKEN",
        "AWS_PROFILE",
        "GOOGLE_APPLICATION_CREDENTIALS",
        "AZURE_CLIENT_SECRET",
        "DOCKER_AUTH_CONFIG",
        "KUBECONFIG",
        "HOMEBREW_GITHUB_API_TOKEN",
        "VERCEL_TOKEN",
        "NETLIFY_AUTH_TOKEN",
        "FLY_API_TOKEN",
        "CLOUDFLARE_API_TOKEN",
        "RAILWAY_TOKEN",
        "OP_SESSION",
    ];
    const SUFFIXES: &[&str] = &[
        "_TOKEN",
        "_SECRET",
        "_SECRET_KEY",
        "_PASSWORD",
        "_API_KEY",
        "_ACCESS_KEY",
        "_PRIVATE_KEY",
        "_CREDENTIALS",
        "_ASKPASS",
    ];
    let mut scrubbed: Vec<String> = EXACT.iter().map(|name| (*name).to_owned()).collect();
    for name in names {
        let upper = name.to_ascii_uppercase();
        if KEEP.contains(&upper.as_str()) || scrubbed.contains(&name) {
            continue;
        }
        if SUFFIXES.iter().any(|suffix| upper.ends_with(suffix)) || upper.starts_with("OP_SESSION")
        {
            scrubbed.push(name);
        }
    }
    scrubbed
}

/// Environment a run worker gets on top: git never prompts for or looks up credentials, no
/// credential helper (Keychain included) runs on its behalf, and `guard` (the git guard's
/// configuration, the run repository's git folder and the worker's own refs) applies the git
/// guard there.
pub(crate) fn run_env(guard: Option<(&Path, &Path, &[String])>) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = [
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GCM_INTERACTIVE", "never"),
        ("GIT_CONFIG_KEY_0", "credential.helper"),
        ("GIT_CONFIG_VALUE_0", ""),
    ]
    .into_iter()
    .map(|(name, value)| (name.to_owned(), value.to_owned()))
    .collect();
    let mut count = 1;
    if let Some((config, common_dir, owned)) = guard {
        let (more, total) = super::git_guard::env(config, common_dir, owned, count);
        env.extend(more);
        count = total;
    }
    env.push(("GIT_CONFIG_COUNT".to_owned(), count.to_string()));
    env
}

impl super::super::SessionManager {
    /// A run worker's approval request, under [`ApprovalMode::Unattended`]: approved for the
    /// user unless it is on the never-list (an outward action, a change to the user's own
    /// checkout), which is declined at once and listed in the report's "What got in the way".
    /// Answered from what the worker started with, before anything is recorded: a run worker
    /// asks for nearly every command.
    pub(crate) async fn route_unattended(
        &self,
        live: &std::sync::Arc<super::super::workers::TaskLive>,
        cli: &std::sync::Arc<super::super::conversation::Cli>,
        request: brigadier_providers::ApprovalRequest,
        run: super::super::workers::RunApprovals,
    ) {
        use brigadier_providers::policy::{self, ApprovalMode, Route};
        use brigadier_providers::{ApprovalDecision, Decider, ProviderEvent};
        let asked = std::time::Instant::now();
        // The route ignores the access under Unattended: what the session allows is approved.
        let declined = if policy::route(
            &request,
            &brigadier_providers::Access::Full,
            ApprovalMode::Unattended,
        ) == Route::Deny
        {
            Some(Declined::Outward)
        } else if policy::touches_protected(&request, &run.protected) {
            Some(Declined::Checkout)
        } else {
            None
        };
        let decision = match declined {
            None => ApprovalDecision::Allow,
            Some(Declined::Outward) => ApprovalDecision::Deny {
                message: "Declined by the overnight rules: this run never acts outside this machine for the user (push, publish, release, deploy, remote changes). Leave it out and finish the rest; if your done-when can't be met without it, say so under needs_user.".into(),
            },
            Some(Declined::Checkout) => ApprovalDecision::Deny {
                message: "Declined by the overnight rules: this run never changes the user's own checkout. Work in your worktree (and your scratch folder) only.".into(),
            },
        };
        let answered = cli
            .session
            .answer(request.id.clone(), decision.clone())
            .await;
        tracing::debug!(
            task = %live.id,
            micros = asked.elapsed().as_micros() as u64,
            allowed = declined.is_none(),
            "answered a run worker's approval"
        );
        if let Err(err) = answered {
            tracing::warn!(task = %live.id, error = %err, "could not answer an approval");
            return;
        }
        // What was allowed leaves no trace beyond the command itself.
        let Some(declined) = declined else {
            return;
        };
        let what = request
            .command
            .as_deref()
            .map(policy::unwrapped_command)
            .unwrap_or_else(|| {
                request
                    .paths
                    .first()
                    .cloned()
                    .unwrap_or(request.tool.clone())
            });
        self.record_worker_event(
            &live.id,
            ProviderEvent::ApprovalRequested {
                request: request.clone(),
            },
        )
        .await;
        self.record_worker_resolution(&live.id, request.id, decision, Decider::Policy)
            .await;
        let text = declined.line(&what);
        self.note_obstacle(
            &live.conversation_id,
            &run.run.run_id,
            ObstacleKind::Declined,
            &text,
            Some(run.number),
        )
        .await;
    }

    /// The command gate during an overnight run: an outward command is refused at once and
    /// listed in the report. `None` when the session has no active run.
    pub(crate) async fn unattended_outward(
        &self,
        conversation_id: &ConversationId,
        task_id: Option<&crate::model::TaskId>,
        argv: &[String],
    ) -> Option<String> {
        let task = match task_id {
            Some(id) => self.task_by_id(conversation_id, id).await.ok(),
            None => None,
        };
        let run_id = match task.as_ref().and_then(|task| task.run.as_ref()) {
            Some(run) => run.run_id.clone(),
            None => self.overnight.active.get(conversation_id)?.id,
        };
        let text = Declined::Outward.line(&argv.join(" "));
        self.note_obstacle(
            conversation_id,
            &run_id,
            ObstacleKind::Declined,
            &text,
            task.as_ref().map(|task| task.number),
        )
        .await;
        Some(
            "declined by the overnight rules: this run never acts outside this machine for the user. Leave it out and finish the rest; if your done-when can't be met without it, say so under needs_user.".into(),
        )
    }

    /// Lists an obstacle on an active run, or counts it again.
    pub(crate) async fn note_obstacle(
        &self,
        conversation_id: &ConversationId,
        run_id: &OvernightRunId,
        kind: ObstacleKind,
        text: &str,
        task: Option<u32>,
    ) {
        let _held = self.overnight.changes.lock().await;
        let Ok(board) = self.core.board(conversation_id).await else {
            return;
        };
        let Some(mut run) = board.runs.get(run_id).cloned() else {
            return;
        };
        if !run.state.is_active() {
            return;
        }
        run.note_obstacle(kind, text, task, crate::now_ms());
        if let Err(err) = self.record_run(&run).await {
            tracing::warn!(run = %run_id, error = %err, "could not list what got in the way");
        }
    }
}

/// Why a run worker's request was declined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Declined {
    Outward,
    Checkout,
}

impl Declined {
    /// The report's line, the same for every time it happens: the command's first words.
    fn line(self, what: &str) -> String {
        let words: Vec<&str> = what
            .split_whitespace()
            .filter(|word| !word.contains('='))
            .take(2)
            .collect();
        let short = one_line(&words.join(" "));
        match self {
            Declined::Outward => format!(
                "`{short}` was declined by the overnight rules: it acts outside this machine"
            ),
            Declined::Checkout => format!(
                "`{short}` was declined by the overnight rules: it changes your own checkout"
            ),
        }
    }
}

/// A command shortened to one line for the user's list.
fn one_line(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(160) {
        Some((at, _)) => format!("{}…", &line[..at]),
        None => line,
    }
}
