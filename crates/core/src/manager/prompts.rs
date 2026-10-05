//! The role instructions each CLI session starts with, and the envelopes the orchestrator
//! reads.

use crate::model::{Conversation, Environment, PermissionLevel, Project, Setup};
use crate::work::{
    ArtifactRef, ContextInjection, InjectionKind, Report, Task, TaskKind, Told, WorkerRole,
};

/// Logged on `orch:<id>` when a conversation's CLI files were removed: the next CLI session
/// starts over from the transcript instead of resuming.
pub(crate) const SESSION_RESET: &str = "brigadier: CLI session reset";
/// The orchestrator's whole reply when it has nothing to tell the user while work runs.
/// Brigadier never shows it.
pub(crate) const QUIET: &str = "[quiet]";

/// Today's date (UTC). Development builds take `BRIGADIER_FAKE_TODAY` instead when it is set,
/// so a check can start a session on another day.
pub(crate) fn today() -> String {
    #[cfg(debug_assertions)]
    if let Ok(day) = std::env::var("BRIGADIER_FAKE_TODAY")
        && !day.trim().is_empty()
    {
        return day.trim().to_owned();
    }
    date_of(crate::now_ms())
}

/// The UTC date of a time in ms since the epoch, as `YYYY-MM-DD`.
pub(crate) fn date_of(at_ms: i64) -> String {
    // Days since the epoch → a civil date (Howard Hinnant's algorithm).
    let days = at_ms.div_euclid(86_400_000);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// The orchestrator's role, with the user's preferences from the Personal Brain. `short`:
/// the user's Short replies setting.
pub(crate) fn orchestrator(
    conversation: &Conversation,
    project: Option<&Project>,
    preferences: &[String],
    run: Option<&crate::overnight::RunWorkspace>,
    short: bool,
) -> String {
    let repo = match &conversation.setup {
        Some(Setup::Session { repo, .. }) => repo.as_str(),
        _ => "(none)",
    };
    let (environment, permission) = setting_texts(conversation, run);
    let project = project.map_or("(no project)", |p| p.name.as_str());
    format!(
        r#"You are the orchestrator of a Brigadier session. Today is {today}.
Project: {project}. Repository: {repo}.
{environment}
Permission level: {permission}

You only talk. You cannot read files, run commands or edit anything, and you must never pretend you did. Workers do all the work in their own worktrees and report back. You run them as a lead engineer runs a team: you write the brief, answer their questions, judge their outlines and reports, and land finished work.

How to work (one loop per request):
- Understand what the user wants. If something only the user can decide is unclear, ask (in your reply, or with ask_user when a task must wait for the answer).
- Ask the Project Brain first (query_brain): it keeps what earlier scouts, research and reports found, the project's modules, stack, conventions, contracts and decisions, each with where it came from. Delegate a scout only when the Brain has no answer or marks it stale. Every report is kept in the Brain for next time.
- When the user settles something later work must respect (a decision, a convention, a contract), or states a preference, keep it with remember (personal: true for a preference that holds in every project). A rule the user sets for this session only is a decision; a convention is how the project always works, and is shared with its other sessions and exported to AGENTS.md. Do it silently. Outline go-aheads and ask_user answers are kept for you.
- search_transcript finds anything said earlier in this conversation, including what is no longer in view.
- 1. Brief. Give the work to one lead with delegate_task (kind implement). Its spec is the brief: the worker sees nothing of this conversation, so state the request in the user's words, the constraints and settled decisions, what "done" means and how to verify each part (typecheck, lint, build, tests, a runtime check), and code pointers (the files and symbols the Brain or a scout named) so it reads precisely instead of searching. A small request (one file, a copy change, a quick fix) goes straight to a lead with no phases. Use scout tasks to look around the repository and research tasks to check current docs; don't guess about code nobody has read.
- 2. Phases. Split a request into phases with plan_phases only when they are large and must run one after another; otherwise it is one phase. Each phase has one lead (delegate_task with phase N). Add a parallel worker (role parallel) only for a stream whose files no other running worker touches.
- 3. Outline. A lead whose work is multi-step or risky writes an outline first and waits: you get it with one advisory review from the other vendor. Check it against the brief, merge the findings you agree with into corrections, and call approve_outline. The brief wins any conflict. There are no review rounds and nothing is rejected.
- 4. Questions. A worker that asks ([question from task-N]) waits for you: answer at once with answer_worker, yourself. Take its recommendation when it fits the brief, else what the brief, the outline, the user's words or the Brain imply; never reopen a settled decision. Ask the user only what truly only they can decide. What only the user can do (a credential, an account, a paid signup, a push) the worker stubs and lists; it never waits on it. message_worker steers a running worker, or sends a reported one back with the specific gaps.
- 5. Reports. Read each report against the brief's "done when". Nothing checks it for you, and nothing blocks it: you judge it. Send it back with message_worker if something is missing.
- 6. Verify. When the lead of an outlined phase reports, Brigadier starts a fresh verifier on its work by itself: it gets one review of the whole phase from the other vendor, checks every "done when" for real, fixes and commits defects, and reports. A small request has no verifier: its lead asked for its own review before reporting.
- 7. Land with land_phase: the verifier's task once it reports (its commits hold the lead's), or the lead of a small request. Brigadier moves the commits onto the session's branch with no card and no further checks, and tells you if the branch moved (the worker runs a quick self-check and its work lands on its own) or if they conflict (delegate a merge task). What the verifier couldn't fix goes to a fix task (role fix, subject the verifier's task: it continues from that work, and landing the fix lands it) or, when only the user can settle it, to note_for_user (kind waiting).
- 8. Then start the next phase, or write the final answer.
- Tools return at once; never wait or poll. Reports, worker questions and outcomes arrive later as messages from Brigadier, in blocks like [report task-3 …] … [/report]. Only these and the user's messages reach you.
- The user's session summary lists what Brigadier decided on their behalf and what only they can do (each worker's needs_user). Add your own with note_for_user: a judgement call you made for them that they would want to know (kind decided, with why), or something only they can do (kind waiting), which stays listed until they mark it done; you hear when they do. Work that doesn't depend on it carries on meanwhile.
- Use read_report and read_artifact only when you need details a report left out; they cost context.
- Each worker has an outputs folder for files meant for you or the user (long findings, documents, generated images); they come back as artifacts, and the user saves them from the task card. Never tell a worker to write files to /tmp or anywhere else outside its worktree and scratch folder.
- Workers never push, publish, deploy or open pull requests on their own: they list such steps for the user, who starts them (with Brigadier's buttons, or by asking you in chat; then delegate exactly that, with no request_approval). Use request_approval only for spending money, using credentials or the keychain, or destroying something outside this session's own work.

How to talk to the user:
- The user sees every worker live next to your replies: its title, state, model, what it is doing and its report summary. Don't announce what you delegated, don't repeat a task's spec, and don't restate reports.
- Everything a user message sets in motion (your turns, the workers, their reports and landings) is one request, shown as one answer. Messages from Brigadier are not the user; each ends with what still runs for that request. While work for the request is still running, don't write to the user at all: reply with exactly {quiet} and nothing else, which Brigadier doesn't show (progress lines like "task-1 finished, waiting on task-2" are noise). This holds right after you delegate, too. Never write text before or between tool calls ("Let me…", "I'll delegate…"): call the tools, then reply {quiet} or your final answer. Write one short line only when something changed their plans.
- When the request's work is done, or the user must decide something, write one final answer: what was found or done, what was verified and how (as the workers and the verifier reported it), and what's next or the decision you need. What waits on the user shows as a short list under your answer by itself (from note_for_user and the workers' needs_user): don't repeat it. Don't repeat what you already told them.
- A message from Brigadier marked [for the user's earlier request: …] belongs to that earlier request; answer about it as such, briefly.
- A [follow-up …] block is a message the user sent while you work on their request; it waits in their queue until you sort it with route_follow_up, silently (the user sees where it goes). If it belongs to this work (a question about the same thing, a detail or a change for it), it joins it: it reaches you at once as the user's message, and your one final answer covers it too. If it is a request of its own, it waits and reaches you on its own once this work is done; don't act on it before.{voice}{orchestrator_voice}
- {AUTHORITY}{short}{preferences}"#,
        today = today(),
        quiet = QUIET,
        voice = VOICE,
        orchestrator_voice = ORCHESTRATOR_VOICE,
        short = if short {
            SHORT_REPLIES
        } else {
            SHORT_REPLIES_OFF_NOW
        },
        preferences = preference_lines(preferences),
    )
}

/// What the orchestrator's instructions say about where accepted work lands and its
/// permission level: the session's own, or an overnight run's while one is active.
pub(crate) fn setting_texts(
    conversation: &Conversation,
    run: Option<&crate::overnight::RunWorkspace>,
) -> (String, String) {
    let (environment, permission) = match &conversation.setup {
        Some(Setup::Session {
            environment,
            permission,
            ..
        }) => (environment, *permission),
        _ => (
            &Environment::LocalCheckout { branch: "?".into() },
            PermissionLevel::ApproveForMe,
        ),
    };
    let unsandboxed = permission == PermissionLevel::FullAccess;
    // An overnight run works on its own branch and approves for the user; its workers keep the
    // session's sandbox, or run without one under full access (PLAN §10.8).
    match run {
        Some(run) => (
            format!(
                "Overnight run: the user started an overnight run and is away. Landed work goes onto the run's own branch `{}` (from `{}`), never on the user's branch; only the user merges verified work, in the morning. Never call finish_session.",
                run.branch, run.base
            ),
            format!(
                "Approve for me, for this run only: Brigadier approves plans and changes on the user's behalf, {sandbox} Workers never do what only the user may do: pushing, publishing, deploying, spending, credentials, contacting anyone, and changes outside the run's branch. Nobody can answer questions or approvals before the morning: decide what the plan and the Rules settle (and note it with note_for_user, kind decided), and list what only the user can do (a key, an account, a push, a product choice the Rules leave open) with note_for_user, kind waiting, then carry on with everything that doesn't depend on it. Never ask the user, and never use request_approval.",
                sandbox = if unsandboxed {
                    "and workers run without the OS sandbox, as in the session."
                } else {
                    "and approves a worker's request to leave its sandbox."
                }
            ),
        ),
        None => (
            environment_text(environment),
            permission_text(permission).to_owned(),
        ),
    }
}

fn environment_text(environment: &Environment) -> String {
    match environment {
        Environment::LocalCheckout { branch } => format!(
            "Local checkout: landed work goes, as the workers' own commits, directly onto `{branch}` in the user's own checkout."
        ),
        Environment::NewWorktree { base, branch, .. } => format!(
            "New worktree: landed work goes, as the workers' own commits, onto the session branch `{branch}` (from `{base}`). When the work is done, call finish_session to merge it into `{base}`; the user approves that with one click."
        ),
    }
}

fn permission_text(permission: PermissionLevel) -> &'static str {
    match permission {
        PermissionLevel::AskForApproval => {
            "Ask for approval: the user gives each outline's go-ahead (approve_outline shows them a \"Start this plan?\" card), and workers ask them before anything outside their sandbox."
        }
        PermissionLevel::ApproveForMe => {
            "Approve for me: you give outlines their go-ahead on the user's behalf. Small tasks just go. Ask the user only what only they can answer (product choices, unclear requirements)."
        }
        PermissionLevel::FullAccess => {
            "Full access: like Approve for me, but workers run without the OS sandbox. Be careful."
        }
    }
}

/// How the orchestrator and workers write (PLAN.md §7): brief, plain and lossless.
const VOICE: &str = "

How to write:
- Answer first: the result, the decision, or yes or no. Then only what the reader needs to act on it. Most answers fit in a few lines; give full detail when the facts need it or the reader asks.
- Cut every sentence that adds no fact: no greetings, praise, apologies, filler, restating the question, recaps or closing offers. Say each fact once. From a log, quote only the decisive line.
- Write whole, plain sentences in the active voice, with their articles and verbs. Use plain words, not jargon. No arrows, symbols or dropped words in place of a sentence, and no emoji. Use a list only for three or more parallel items, and headings only in long answers.
- Lose nothing: keep every fact, number, path, name, command, error text, negation (\"not\", \"only\", \"except\") and condition exact. If you don't know something, say so once.
- For security, irreversible actions and steps whose order matters, write full, careful sentences.";

/// What the voice covers for the orchestrator.
const ORCHESTRATOR_VOICE: &str = "
- This covers your own prose: replies to the user and your notes (remember, plans, handoff notes). Task specs stay complete, and commit messages follow the project's style.
- When you write to the user, name a worker by its title, as the user sees it, never as task-N.";

/// The Short replies setting (on by default, PLAN.md §7): what the user reads stays a few
/// lines.
const SHORT_REPLIES: &str = "

Short replies (the user's setting):
- What you write for the user: the outcome in the first line, then at most about five short lines or bullets, unless they ask for more. No headings or bold labels.
- Never quote a reviewer's or verifier's text to the user: name what it found in a few words.
- At the end of an overnight phase, your reply is at most three lines: what changed, the outcome, and what waits on the user.
- This doesn't shorten plans, handoff notes, task specs, exact commands and error text, evidence the user needs to decide, or instructions about security and order: those stay complete.";

/// The Short replies setting, off.
const SHORT_REPLIES_OFF_NOW: &str = "

Short replies is off: the user wants fuller answers. Write by \"How to write\" alone.";

/// How the log names role instructions: a Chat's and those without Short replies (sessions
/// started before the setting existed too), and those with it.
pub(crate) const ROLE_INSTRUCTIONS: &str = "role instructions";
const ROLE_INSTRUCTIONS_SHORT: &str = "role instructions, short replies";
const SHORT_REPLIES_ON: &str = "short replies on";
const SHORT_REPLIES_OFF: &str = "short replies off";

/// The log label of an orchestrator's role instructions.
pub(crate) fn instructions_label(short: bool) -> &'static str {
    if short {
        ROLE_INSTRUCTIONS_SHORT
    } else {
        ROLE_INSTRUCTIONS
    }
}

/// The log label of a note that the user switched Short replies.
pub(crate) fn short_replies_label(short: bool) -> &'static str {
    if short {
        SHORT_REPLIES_ON
    } else {
        SHORT_REPLIES_OFF
    }
}

/// Whether instructions logged under `label` had Short replies on (none: not about it).
pub(crate) fn short_in_label(label: &str) -> Option<bool> {
    match label {
        ROLE_INSTRUCTIONS_SHORT | SHORT_REPLIES_ON => Some(true),
        ROLE_INSTRUCTIONS | SHORT_REPLIES_OFF => Some(false),
        _ => None,
    }
}

/// Tells a running orchestrator the user switched Short replies: its CLI keeps the
/// instructions it started with.
pub(crate) fn short_replies_note(short: bool) -> String {
    if short {
        format!(
            "[settings] The user turned Short replies on. From now on, follow these rules too; don't mention this note.{}",
            SHORT_REPLIES
                .trim_start_matches('\n')
                .trim_start_matches("Short replies (the user's setting):")
        )
    } else {
        "[settings] The user turned Short replies off. From now on, the \"Short replies\" rules no longer apply: write by \"How to write\" alone, with full detail where it helps. Don't mention this note.".to_owned()
    }
}

/// The instructions' contract: from version 1 on they say that Brigadier's notes replace what
/// they say about the note's subject. A session that started on an older one hears it once.
pub(crate) const CONTRACT: u32 = 1;

/// What an orchestrator's instructions say about Brigadier's notes (contract 1).
const AUTHORITY: &str = "Brigadier tells you when something these instructions say changes after they were written, in a note at the start of a message: [today] for today's date, [settings] for the user's settings (Short replies, the permission level, their preferences), [run] for an overnight run starting, changing or ending. Such a note replaces what these instructions say about it, from then on.";

/// The same for a Chat.
const CHAT_AUTHORITY: &str = "Brigadier tells you in a note at the start of a message when today's date ([today]) or what you know about the user ([settings]) changed after these instructions were written. Such a note replaces what these instructions say about it, from then on.";

const CONTRACT_LABEL: &str = "notes replace instructions";
const TODAY_LABEL: &str = "today";
const PERMISSION_LABEL: &str = "permission level";
const RUN_LABEL: &str = "overnight run";
const RUN_OVER_LABEL: &str = "overnight run over";
const PREFERENCES_LABEL: &str = "preferences";

/// The parts of a session's instructions that can change while its CLI session lives on, as
/// they are now.
pub(crate) struct Current {
    pub chat: bool,
    pub today: String,
    pub short_replies: bool,
    pub permission: PermissionLevel,
    /// Where accepted work lands and the permission level, as the instructions say them
    /// without a run.
    pub plain: (String, String),
    /// While an overnight run is active: what the instructions say about it, and the
    /// restrictions Brigadier enforces for it.
    pub run: Option<(String, String, String)>,
    pub preferences: Vec<String>,
}

impl Current {
    /// A session's: `run` is the active run's branch and the restrictions it enforces.
    pub(crate) fn session(
        conversation: &Conversation,
        run: Option<(&crate::overnight::RunWorkspace, String)>,
        short_replies: bool,
        preferences: Vec<String>,
    ) -> Self {
        let permission = match &conversation.setup {
            Some(Setup::Session { permission, .. }) => *permission,
            _ => PermissionLevel::ApproveForMe,
        };
        let run = run.map(|(workspace, restrictions)| {
            let (environment, permission) = setting_texts(conversation, Some(workspace));
            (environment, permission, restrictions)
        });
        Self {
            chat: false,
            today: today(),
            short_replies,
            permission,
            plain: setting_texts(conversation, None),
            run,
            preferences,
        }
    }

    /// A Chat's: only the date and the user's memories can change.
    pub(crate) fn chat(memories: Vec<String>) -> Self {
        Self {
            chat: true,
            today: today(),
            short_replies: false,
            permission: PermissionLevel::ApproveForMe,
            plain: (String::new(), String::new()),
            run: None,
            preferences: memories,
        }
    }

    fn run_fingerprint(&self) -> String {
        self.run.as_ref().map_or_else(String::new, |(a, b, c)| {
            fingerprint(&[a.as_str(), b.as_str(), c.as_str()])
        })
    }

    /// What role instructions written now tell a new CLI session.
    pub(crate) fn told(&self) -> Told {
        let session = !self.chat;
        Told {
            contract: Some(CONTRACT),
            today: Some(self.today.clone()),
            short_replies: session.then_some(self.short_replies),
            permission: session.then_some(self.permission),
            run: session.then(|| self.run_fingerprint()),
            preferences: Some(preferences_fingerprint(&self.preferences)),
        }
    }
}

/// A short, stable fingerprint of some texts.
fn fingerprint(parts: &[&str]) -> String {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part.as_bytes());
        hasher.update(&[0]);
    }
    hasher.finalize().to_hex()[..16].to_owned()
}

/// The user's preferences' fingerprint, as [`Told`] keeps it.
pub(crate) fn preferences_fingerprint(preferences: &[String]) -> String {
    fingerprint(&preferences.iter().map(String::as_str).collect::<Vec<_>>())
}

/// A note that tells a session what changed in its instructions, its log label, and what it
/// tells.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Note {
    pub text: String,
    pub label: &'static str,
    pub told: Told,
}

/// The notes a session needs before its next turn: what changed since it was last `told`
/// (unknown counts as changed, except a run it can't have heard of), the contract first.
pub(crate) fn notes(told: &Told, now: &Current) -> Vec<Note> {
    let mut notes = Vec::new();
    if told.contract.unwrap_or(0) < CONTRACT {
        notes.push(Note {
            text: format!(
                "[instructions] {}",
                if now.chat { CHAT_AUTHORITY } else { AUTHORITY }
            ),
            label: CONTRACT_LABEL,
            told: Told {
                contract: Some(CONTRACT),
                ..Told::default()
            },
        });
    }
    if told.today.as_deref() != Some(now.today.as_str()) {
        notes.push(Note {
            text: format!("[today] It's now {}.", now.today),
            label: TODAY_LABEL,
            told: Told {
                today: Some(now.today.clone()),
                ..Told::default()
            },
        });
    }
    if !now.chat {
        if told.short_replies != Some(now.short_replies) {
            notes.push(Note {
                text: short_replies_note(now.short_replies),
                label: short_replies_label(now.short_replies),
                told: Told {
                    short_replies: Some(now.short_replies),
                    ..Told::default()
                },
            });
        }
        let run = now.run_fingerprint();
        let told_run = told.run.as_deref();
        match &now.run {
            Some((environment, permission, restrictions)) if told_run != Some(run.as_str()) => {
                notes.push(Note {
                    text: format!(
                        "[run] From now on, for this overnight run (this replaces what your instructions say about where accepted work lands and the permission level):\n{environment}\nPermission level: {permission}\n{restrictions}"
                    ),
                    label: RUN_LABEL,
                    told: Told {
                        run: Some(run),
                        ..Told::default()
                    },
                });
            }
            // A run's instructions are the session's while it runs.
            Some(_) => {}
            None if told_run.is_some_and(|run| !run.is_empty()) => {
                let (environment, permission) = &now.plain;
                notes.push(Note {
                    text: format!(
                        "[run] The overnight run is over: what your instructions or earlier notes say about it no longer applies. From now on:\n{environment}\nPermission level: {permission}"
                    ),
                    label: RUN_OVER_LABEL,
                    told: Told {
                        run: Some(String::new()),
                        permission: Some(now.permission),
                        ..Told::default()
                    },
                });
            }
            None if told.permission != Some(now.permission) => {
                notes.push(Note {
                    text: format!(
                        "[settings] The user changed the permission level. From now on: {}",
                        now.plain.1
                    ),
                    label: PERMISSION_LABEL,
                    told: Told {
                        permission: Some(now.permission),
                        ..Told::default()
                    },
                });
            }
            None => {}
        }
    }
    let preferences = preferences_fingerprint(&now.preferences);
    if told.preferences.as_deref() != Some(preferences.as_str()) {
        let what = if now.chat {
            "what you know about the user from earlier conversations"
        } else {
            "the user's preferences"
        };
        let text = if now.preferences.is_empty() {
            format!(
                "[settings] There is nothing saved now as {what}: the list in your instructions no longer applies."
            )
        } else {
            let mut text = format!(
                "[settings] This is {what} now, in place of the list in your instructions (follow it):"
            );
            for preference in &now.preferences {
                text.push_str("\n- ");
                text.push_str(preference);
            }
            text
        };
        notes.push(Note {
            text,
            label: PREFERENCES_LABEL,
            told: Told {
                preferences: Some(preferences),
                ..Told::default()
            },
        });
    }
    notes
}

/// Fills what `told` doesn't know yet from what an older log entry told.
pub(crate) fn fill_told(told: &mut Told, older: &Told) {
    told.contract = told.contract.or(older.contract);
    if told.today.is_none() {
        told.today.clone_from(&older.today);
    }
    told.short_replies = told.short_replies.or(older.short_replies);
    told.permission = told.permission.or(older.permission);
    if told.run.is_none() {
        told.run.clone_from(&older.run);
    }
    if told.preferences.is_none() {
        told.preferences.clone_from(&older.preferences);
    }
}

/// What the current CLI session was last told, from its log's instruction entries (logged at
/// `at_ms`), newest first. Entries from before told was recorded say less: role instructions
/// their Short replies setting, contract 0 and the day they were logged; a Short replies note
/// its setting. Returns it and whether the session's role instructions were reached.
pub(crate) fn told_from_log<'a>(
    entries: impl IntoIterator<Item = (i64, &'a ContextInjection)>,
) -> (Told, bool) {
    let mut told = Told::default();
    for (at_ms, entry) in entries {
        if entry.kind != InjectionKind::Instructions {
            continue;
        }
        let role = entry.label.starts_with(ROLE_INSTRUCTIONS);
        match &entry.told {
            Some(said) => fill_told(&mut told, said),
            None => fill_told(
                &mut told,
                &Told {
                    contract: role.then_some(0),
                    today: role.then(|| date_of(at_ms)),
                    short_replies: short_in_label(&entry.label),
                    ..Told::default()
                },
            ),
        }
        // Older entries belong to an earlier CLI session.
        if role {
            return (told, true);
        }
    }
    (told, false)
}

/// What the voice covers for a worker, and its report's shape.
const WORKER_VOICE: &str = "
- Your report is for the orchestrator. Summary: the outcome first (done, partly done or blocked), then the findings that answer the task. Changes: one line per file. Verification: what you ran or read and what you saw. Done when: each criterion of \"done\" in the task, with [met], [not met] or [not checked] and its evidence; a check you didn't run is [not checked], never [met]. Open questions: decisions you need. Risks: assumptions, risks, and what you skipped and why. Needs user: what only the user can do. Report failures and unknowns plainly, and never leave out a failed check.
- Code, comments, docs and files in your outputs folder follow the project's style, not these rules.";

/// What a lead does besides building: its outline when the work is big, its own review when
/// it isn't.
const LEAD_STEPS: &str = "\n- You lead this work. If it is multi-step or risky, first read the code, then send your outline with submit_outline (the steps in order with the files each touches, how you will verify, and your open questions with your recommendations) and wait for the go-ahead; corrections that come with it win over your outline. Otherwise just build it.\n- If you sent no outline, call request_review once when your work is committed, before you report: a reviewer from the other vendor reads your change. Fix each finding you agree with and say why for those you don't. With an outline, a verifier checks your phase after you report instead.";

/// A worker's pointer to the code index tools (PLAN.md §7).
const WORKER_CODE_TOOLS: &str = "
- To find code, use the Brigadier tools first: query_brain (what earlier work found: modules, decisions, conventions), code_search (definitions and files by name), code_refs (where a symbol is defined and used) and project_map (the repository at a glance). They are instant and return less than grepping or reading whole files. Then read only the lines you need (a line range, not the whole file): your context is precious.";

/// How implement and merge workers write code (PLAN.md §7; after ponytail's rules, see
/// THIRD_PARTY_NOTICES.md).
const WORKER_CODE_RULES: &str = "

How to write code:
- First read the task and the code it touches, and trace the real flow. The smallest change in the wrong place is a second bug.
- Reuse what the repository already has (a helper, type or pattern), then the standard library, then a dependency already installed. Add a dependency only when the task needs it.
- Write the least code that does the job: no abstraction, option, layer or scaffolding the task didn't ask for. Prefer deleting to adding and plain code to clever code. Between two options of the same size, take the one that is correct on edge cases.
- Fix bugs at their root. When you change a shared function, type or contract, find all its callers and keep them correct.
- Never simplify away validation, error handling, security checks or anything the task asks for.";

/// The user's preferences as an instructions section (empty without any).
fn preference_lines(preferences: &[String]) -> String {
    if preferences.is_empty() {
        return String::new();
    }
    let mut text =
        "\n\nThe user's preferences (kept in their Personal Brain; follow them):".to_owned();
    for preference in preferences {
        text.push_str("\n- ");
        text.push_str(preference);
    }
    text
}

/// A worker's role and task.
pub(crate) fn worker(task: &Task, repo_note: &str, instructions: &str, extra: &str) -> String {
    let kind = match task.kind {
        TaskKind::Scout => {
            "scout: look around the repository and answer the question. Change nothing."
        }
        TaskKind::Research => {
            "research: check current official docs, changelogs and sources on the web and answer the question. Change nothing in the repository."
        }
        TaskKind::Implement => {
            "implement: change the code in this worktree to do the task, then verify it for real."
        }
        TaskKind::Review => {
            "review: review the change described below against the task and the repository's conventions. Look for bugs, missing verification, stray files and slop. Change nothing."
        }
        TaskKind::Merge => {
            "merge: resolve the conflicts described below in this worktree, keeping both sides' intent, then verify."
        }
        TaskKind::Verify => {
            "verify: prove each \"done when\" criterion of the task with your own evidence, run the checks the task's brief asks for on this worktree, and report exactly what passed and failed. Set submit_report's checks: noChecks only when the project has none you could run. Fix nothing."
        }
    };
    let write_rules = if task.kind.writes() {
        let commits = if task.route.choice.provider == brigadier_providers::ProviderKind::Codex {
            "Commit each finished step with a short plain message if you can; when your sandbox can't write git's files, leave the changes: Brigadier commits them when you ask for a review or report."
        } else {
            "Commit each finished step with a short plain message."
        };
        format!(
            "\n- Work only inside this worktree, on its branch. {commits} Don't switch branches or touch other checkouts. What you leave uncommitted is committed for you when your work lands.\n- List every file you changed, created or deleted in the report's `changes`: new files that aren't listed are left out when your work lands.\n- Put scratch notes, logs and throwaway scripts in your scratch folder, never in the repository.\n- Don't write new tests unless the task asks for them. If a change breaks an existing test, fix the code; change a test only for an intended behaviour change.\n- Before you report, check your own work: format, lint, build, and the tests of what you touched.{role}",
            role = match task.role {
                Some(WorkerRole::Lead) | None if task.kind == TaskKind::Implement => LEAD_STEPS,
                _ => "",
            }
        )
    } else {
        "\n- Don't change files in the repository. Your scratch folder is yours for notes."
            .to_owned()
    };
    // A gate member has no one to ask (see `Role::Worker`).
    let alone = if task.gate_link.is_some() {
        "You work alone on this check and cannot ask anyone: decide from what you were given and your own evidence alone. Where the task is unclear, take its most reasonable reading and name it under risks."
    } else {
        "You report to the orchestrator, who speaks for the user: treat its answers as the user's. Keep going on your own for anything the task, the project's docs and the Project Brain (query_brain) settle. When a question truly blocks you, call ask_orchestrator: one question at a time, with the options you see and the one you recommend. It waits for the answer."
    };
    let mut practices = String::new();
    if task.kind != TaskKind::Research {
        practices.push_str(WORKER_CODE_TOOLS);
    }
    if matches!(task.kind, TaskKind::Implement | TaskKind::Merge) {
        practices.push_str(WORKER_CODE_RULES);
    }
    // An overnight run's Waiting on you holds only what its done-when needs (PLAN.md §10.11).
    let needs_user = if task.run.is_some() {
        "If a \"done when\" criterion can't be met without something only the user can do (a credential, a sign-in, an account, a paid signup), list exactly that under needs_user and finish everything else around it. Anything optional the user could add goes under risks, not needs_user."
    } else {
        "If something only the user can do blocks part of the task (a credential, a sign-in, an account, a paid signup), don't stall on it: stub it (read it from an environment variable or config), list it under needs_user and finish everything else around it."
    };
    format!(
        r#"You are a Brigadier worker. Today is {today}. Your models' knowledge may be older than today: check current docs before relying on any third-party API, version or CLI.

Task task-{number}: {title}
Kind: {kind}
{repo_note}

Rules:
- {alone}{write_rules}
- Never push, publish, deploy or open pull requests, unless the task says the user asked for exactly that: list such steps under needs user instead. The same goes for spending money, using credentials or the keychain, and deleting anything outside your own work.
- {needs_user}
- If you start subagents, never use a Fable model, and never raise reasoning effort above high.
- Files meant for the orchestrator or the user (full findings, logs worth keeping, documents, generated images) go in your outputs folder. Brigadier attaches them to your report and the user saves them from the task card. Never write files to /tmp or anywhere else outside your worktree, scratch folder and test data folder, even if the task names such a place: nobody could read them, and they would be left behind. Save them in your outputs folder and say so in the report.
- The orchestrator reads only your submit_report, never your messages: don't write your findings as a message, and never say in the report that they are below or in a message. When done (or when you cannot continue), call submit_report exactly once: summary, changes, decisions, verification (exactly what you ran and what you saw), done when, open questions, risks, needs user. Keep it short (about 800 tokens at most); anything longer goes in a file in your outputs folder, named under `artifacts` with a short title.{practices}{VOICE}{WORKER_VOICE}{instructions}{extra}

The task:
{spec}"#,
        today = today(),
        number = task.number,
        title = task.title,
        spec = task.spec,
    )
}

/// What a worker is told about where and how it runs: what was prepared, what it may write,
/// and, in an overnight run, that nobody is there to ask (PLAN.md §10.8).
pub(crate) struct WorkerEnvironment<'a> {
    pub access: &'a brigadier_providers::Access,
    /// Folders copied in from the user's checkout.
    pub warmed: &'a [String],
    pub test_dir: &'a std::path::Path,
    /// The user's own checkout, for a run task.
    pub run_repo: Option<&'a std::path::Path>,
    /// The worker's own branch.
    pub branch: Option<&'a str>,
    /// It runs at low OS priority.
    pub low_priority: bool,
}

pub(crate) fn environment(env: &WorkerEnvironment<'_>) -> String {
    use brigadier_providers::Access;
    let mut lines: Vec<String> = Vec::new();
    if !env.warmed.is_empty() {
        lines.push(format!(
            "Dependencies are already installed: {} {} copied in from the user's checkout. Don't reinstall them.",
            env.warmed
                .iter()
                .map(|folder| format!("`{folder}`"))
                .collect::<Vec<_>>()
                .join(", "),
            if env.warmed.len() == 1 { "was" } else { "were" }
        ));
    }
    lines.push(format!(
        "Your test data folder, for whatever a test or a smoke run writes outside the checkout (an app's data folder, a test database): {}. Never use an app's real data folder.",
        env.test_dir.display()
    ));
    match env.access {
        Access::Full => lines.push("You run without a sandbox, with the session's full access: nothing asks for approval.".into()),
        Access::Scoped { .. } | Access::Workspace { .. } | Access::ReadOnly => lines.push(
            "You run in a sandbox: you can write to the folders named above as yours, your worktree's git folder and the toolchains' caches. A command that needs more (another folder, the network when it is off) may run outside the sandbox: run it so, and it is approved or declined; if declined, work around it or list it in the report. A program that opens windows (a desktop app, a GUI smoke run) can't run in the sandbox: give such a check as `[excluded] <the check>: the sandbox can't open windows` and go on.".into(),
        ),
    }
    if let Some(repo) = env.run_repo {
        lines.push(format!(
            "This is an overnight run and nobody is there to answer before the morning. Leave out what only the user may do (pushing, publishing, releasing or deploying, credentials, signups or spending, contacting anyone) and don't change the user's own checkout ({}); list such steps in the report and finish the rest.",
            repo.display()
        ));
        lines.push(format!(
            "Change only your own branch ({}): leave other branches and tags alone. To look at another commit, unpack it into your scratch folder (`git archive <commit> | tar -x -C <folder>`).",
            env.branch.map_or_else(|| "you have none".to_owned(), |branch| format!("`{branch}`"))
        ));
    }
    if env.low_priority {
        lines.push("You already run at low priority: don't use `nice`.".into());
    }
    format!("How this task runs:\n- {}", lines.join("\n- "))
}

/// A Chat's role.
pub(crate) fn chat(memories: &[String]) -> String {
    let mut text = format!(
        "You are a helpful assistant in Brigadier, a desktop app. Today is {}. You are in a plain chat: there is no repository and you cannot edit code. You may search the web when current information helps; say where facts came from.\nWhen the user tells you something about themselves that will matter in later conversations (a preference, their role, what they work on), keep it with the save_memory tool, one short sentence, without announcing it: the user sees what you saved and can remove it.\n{CHAT_AUTHORITY}",
        today()
    );
    if !memories.is_empty() {
        text.push_str(
            "\n\nWhat you know about the user from earlier conversations (their memories):",
        );
        for memory in memories {
            text.push_str("\n- ");
            text.push_str(memory);
        }
    }
    text
}

/// A report as the orchestrator reads it.
pub(crate) fn report_envelope(task: &Task, report: &Report, route: &str) -> String {
    let mut text = format!(
        "[report task-{} · {:?} · \"{}\" · {}]\n{}",
        task.number, task.kind, task.title, route, report.summary
    );
    let list = |title: &str, items: &[String], text: &mut String| {
        if !items.is_empty() {
            text.push_str(&format!("\n{title}:"));
            for item in items {
                text.push_str(&format!("\n- {item}"));
            }
        }
    };
    list("Changes", &report.changes, &mut text);
    list("Decisions", &report.decisions, &mut text);
    list("Verification", &report.verification, &mut text);
    list("Done when", &report.done_when, &mut text);
    list("Open questions", &report.open_questions, &mut text);
    list("Risks", &report.risks, &mut text);
    // A worker's are listed for the user (a gate member's go to its gate).
    let needs_user = if task.gate_link.is_none() {
        "Needs the user (already listed for them under Waiting on you)"
    } else {
        "Needs the user"
    };
    list(needs_user, &report.needs_user, &mut text);
    if let Some(verdict) = report.verdict {
        text.push_str(&format!("\nVerdict: {verdict:?}"));
    }
    if !report.artifacts.is_empty() {
        text.push_str("\nArtifacts:");
        for artifact in &report.artifacts {
            text.push_str(&format!(
                "\n- {} ({:?}, {} bytes): {}",
                artifact.id, artifact.kind, artifact.bytes, artifact.title
            ));
        }
    }
    if !task.outputs.is_empty() {
        text.push_str(
            "\nOutputs (the user saves them from the task card; read_artifact reads them):",
        );
        for output in &task.outputs {
            text.push_str(&format!(
                "\n- {} ({}, {} bytes): {}",
                output.id, output.mime, output.bytes, output.title
            ));
        }
    }
    text.push_str("\n[/report]");
    text
}

/// The reported write tasks still waiting for the orchestrator's decision (see
/// `SessionManager::undecided`).
pub(crate) fn undecided_note(tasks: &[Task]) -> String {
    let list: Vec<String> = tasks
        .iter()
        .map(|task| format!("task-{} \"{}\"", task.number, task.title))
        .collect();
    format!(
        "[waiting for your decision: {}. Land each with land_phase, send it back with message_worker, or stop it with stop_worker if its work should not land; until then it stays open.]",
        list.join(", ")
    )
}

/// What a worker wrote after its report, as the orchestrator and later checks read it: in
/// full when it fits the report's size, else its first part and the artifact that holds it
/// all.
pub(crate) fn late_findings_text(artifact: &ArtifactRef, text: &str) -> String {
    let limit = super::workers::REPORT_MAX_BYTES;
    if text.len() > limit {
        let mut end = limit;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        format!(
            "{}\n{CUT_HEAD}{}{CUT_TAIL}{} bytes]",
            &text[..end],
            artifact.id,
            artifact.bytes
        )
    } else {
        text.to_owned()
    }
}

/// How [`late_findings_text`] says where the rest is: `{CUT_HEAD}<id>{CUT_TAIL}<n> bytes]`.
const CUT_HEAD: &str = "[…cut; read_artifact ";
const CUT_TAIL: &str = " reads all ";

/// What a worker wrote after its report (see [`report_envelope`]), shown as
/// [`late_findings_text`] gives it.
pub(crate) fn late_findings_envelope(task: &Task, shown: &str) -> String {
    format!(
        "[report task-{} · addendum] The worker wrote this after its report, which left it out; \
         it is kept with the report:\n{shown}\n[/report]",
        task.number
    )
}

/// Asks the orchestrator to sort a follow-up the user sent while `request` works.
pub(crate) fn follow_up(
    id: &str,
    request: &str,
    text: &str,
    attachments: &[crate::work::AttachmentRef],
) -> String {
    let mut block = format!(
        "[follow-up {id}] The user sent this while you work on their request \"{request}\":\n{text}"
    );
    if !attachments.is_empty() {
        let names: Vec<&str> = attachments.iter().map(|a| a.name.as_str()).collect();
        block.push_str(&format!("\n(attached: {})", names.join(", ")));
    }
    block.push_str(&format!(
        "\n[/follow-up] Sort it silently now: call route_follow_up (follow_up \"{id}\") with joins \
         true if it belongs to this work, false if it is a request of its own. The user sees \
         where it goes, so write nothing about the choice, before or after the call, and don't \
         answer it here. If nothing else is needed now, reply with exactly {QUIET} and nothing \
         else."
    ));
    block
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_a_worker_wrote_after_its_report_is_cut_to_a_reports_size() {
        let artifact = ArtifactRef {
            id: "blob1".into(),
            title: "What the worker wrote after its report".into(),
            kind: crate::work::ArtifactKind::Note,
            mime: "text/markdown".into(),
            bytes: 9_000,
            file_name: None,
        };
        assert_eq!(late_findings_text(&artifact, "Short."), "Short.");
        let long = "é".repeat(4_500);
        let shown = late_findings_text(&artifact, &long);
        assert!(shown.len() < long.len());
        assert!(
            shown.ends_with("[…cut; read_artifact blob1 reads all 9000 bytes]"),
            "{shown}"
        );
    }

    fn task(provider: &str, role: Option<&str>) -> Task {
        serde_json::from_value(serde_json::json!({
            "id": "t1",
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "Add the flag",
            "kind": "implement",
            "role": role,
            "spec": "Add the flag.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": provider, "model": null, "effort": null }, "reason": "" },
            "state": "running",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task")
    }

    #[test]
    fn a_lead_outlines_big_work_reviews_small_work_and_commits_its_steps() {
        let lead = worker(&task("claude", Some("lead")), "", "", "");
        assert!(lead.contains("submit_outline"));
        assert!(lead.contains("call request_review once"));
        assert!(lead.contains("Commit each finished step"));
        assert!(!lead.contains("when your sandbox can't write git's files"));
        assert!(lead.contains(
            "one question at a time, with the options you see and the one you recommend"
        ));
        assert!(lead.contains("never use a Fable model"));
        assert!(lead.contains("query_brain"));
        // Codex's sandbox may keep it from committing: Brigadier commits for it.
        let codex = worker(&task("codex", Some("lead")), "", "", "");
        assert!(codex.contains("Brigadier commits them when you ask for a review or report"));
        // A verifier follows its own steps, not a lead's.
        let verifier = worker(&task("claude", Some("verifier")), "", "", "");
        assert!(!verifier.contains("submit_outline"));
        assert!(verifier.contains("check your own work"));
    }
}

#[cfg(test)]
mod environment_tests {
    use std::path::Path;

    use super::*;

    fn note(access: &brigadier_providers::Access, run: bool) -> String {
        environment(&WorkerEnvironment {
            access,
            warmed: &[
                "node_modules".to_owned(),
                "apps/desktop/node_modules".to_owned(),
            ],
            test_dir: Path::new("/tmp/brigadier-test-12345678"),
            run_repo: run.then_some(Path::new("/Users/me/project")),
            branch: Some("brigadier/abc/task-3"),
            low_priority: run,
        })
    }

    #[test]
    fn a_worker_hears_what_was_prepared_and_what_it_may_do() {
        let sandbox = brigadier_providers::Access::Scoped {
            write_cwd: true,
            writable_roots: Vec::new(),
            network: true,
            deny_read: Vec::new(),
            unix_sockets: Vec::new(),
        };
        let sandboxed = note(&sandbox, false);
        assert!(sandboxed.contains("`node_modules`, `apps/desktop/node_modules` were copied in"));
        assert!(sandboxed.contains("Don't reinstall them"));
        assert!(sandboxed.contains("/tmp/brigadier-test-12345678"));
        assert!(sandboxed.contains("can't open windows"));
        assert!(!sandboxed.contains("overnight"));
        assert!(!sandboxed.contains("nice"));
        let run = note(&brigadier_providers::Access::Full, true);
        assert!(run.contains("without a sandbox"));
        assert!(!run.contains("can't open windows"));
        assert!(run.contains("nothing asks for approval"));
        assert!(run.contains("nobody is there to answer"));
        assert!(!run.contains("declined"));
        assert!(run.contains("/Users/me/project"));
        assert!(run.contains("`brigadier/abc/task-3`"));
        assert!(run.contains("don't use `nice`"));
    }

    #[test]
    fn a_runs_orchestrator_hears_the_sessions_sandbox() {
        let conversation = |permission: &str| -> Conversation {
            serde_json::from_value(serde_json::json!({
                "id": "01a106c3-1fb7-7593-a441-486b39799405",
                "kind": "session",
                "projectId": null,
                "title": "textkit",
                "pinnedAtMs": null,
                "createdAtMs": 0,
                "updatedAtMs": 0,
                "setup": {
                    "type": "session",
                    "repo": "/tmp/textkit",
                    "environment": { "type": "localCheckout", "branch": "main" },
                    "permission": permission,
                    "orchestrator": { "provider": "claude", "model": "opus", "effort": "high" },
                    "workersSeeUncommitted": null,
                    "planMode": false
                }
            }))
            .unwrap()
        };
        let run = crate::overnight::RunWorkspace {
            base: "main".into(),
            base_commit: "abc".into(),
            branch: "overnight/2026-10-04-textkit-1234".into(),
            path: "/tmp/run".into(),
        };
        for permission in ["askForApproval", "approveForMe", "fullAccess"] {
            for workspace in [None, Some(&run)] {
                let prompt = orchestrator(&conversation(permission), None, &[], workspace, true);
                assert!(prompt.contains(
                    "plan_phases only when they are large and must run one after another"
                ));
                assert!(prompt.contains("one advisory review from the other vendor"));
                assert!(prompt.contains("There are no review rounds"));
                assert!(prompt.contains("answer at once with answer_worker"));
                assert!(prompt.contains("Land with land_phase"));
                assert!(!prompt.contains("revises"));
                assert!(!prompt.contains("propose_plan"));
                assert!(!prompt.contains("accept_task"));
                assert!(!prompt.contains("delegate the next step right away"));
            }
        }
        let full = orchestrator(&conversation("fullAccess"), None, &[], Some(&run), true);
        assert!(full.contains("workers run without the OS sandbox"));
        assert!(!full.contains("stays in its sandbox"));
        let sandboxed = orchestrator(&conversation("approveForMe"), None, &[], Some(&run), true);
        assert!(sandboxed.contains("approves a worker's request to leave its sandbox"));
        assert!(sandboxed.contains("Workers never do what only the user may do"));
        // Short replies reach a run's lead, and its phase-end replies; off, the plain voice
        // stays.
        assert!(
            full.contains("At the end of an overnight phase, your reply is at most three lines")
        );
        let long = orchestrator(&conversation("fullAccess"), None, &[], Some(&run), false);
        assert!(!long.contains("Short replies (the user's setting)"));
        assert!(long.contains("How to write:"));
        assert!(long.contains("name a worker by its title"));
        assert!(long.contains("Short replies is off"));
        assert!(!full.contains("Short replies is off"));
        assert!(long.contains(AUTHORITY));
        assert!(full.contains(AUTHORITY));
        assert!(chat(&[]).contains(CHAT_AUTHORITY));
    }

    #[test]
    fn a_running_orchestrator_hears_short_replies_switched_and_the_log_says_which() {
        // Sessions started before the setting have plain "role instructions": without it.
        assert_eq!(short_in_label("role instructions"), Some(false));
        for short in [true, false] {
            assert_eq!(short_in_label(instructions_label(short)), Some(short));
            assert_eq!(short_in_label(short_replies_label(short)), Some(short));
        }
        assert_eq!(short_in_label("transcript so far"), None);
        let on = short_replies_note(true);
        assert!(on.starts_with("[settings] The user turned Short replies on."));
        assert!(on.contains("\n- What you write for the user: the outcome in the first line"));
        assert!(on.contains("at most three lines"));
        assert!(!on.contains("Short replies (the user's setting)"));
        assert!(short_replies_note(false).contains("no longer apply"));
    }

    fn session(permission: &str) -> Conversation {
        serde_json::from_value(serde_json::json!({
            "id": "01a106c3-1fb7-7593-a441-486b39799405",
            "kind": "session",
            "projectId": null,
            "title": "textkit",
            "pinnedAtMs": null,
            "createdAtMs": 0,
            "updatedAtMs": 0,
            "setup": {
                "type": "session",
                "repo": "/tmp/textkit",
                "environment": { "type": "localCheckout", "branch": "main" },
                "permission": permission,
                "orchestrator": { "provider": "claude", "model": "opus", "effort": "high" },
                "workersSeeUncommitted": null,
                "planMode": false
            }
        }))
        .unwrap()
    }

    fn workspace() -> crate::overnight::RunWorkspace {
        crate::overnight::RunWorkspace {
            base: "main".into(),
            base_commit: "abc".into(),
            branch: "overnight/2026-10-04-textkit-1234".into(),
            path: "/tmp/run".into(),
        }
    }

    fn current(permission: &str, run: Option<&str>, preferences: &[&str]) -> Current {
        let workspace = workspace();
        let mut current = Current::session(
            &session(permission),
            run.map(|restrictions| (&workspace, restrictions.to_owned())),
            true,
            preferences.iter().map(|p| (*p).to_owned()).collect(),
        );
        current.today = "2026-10-04".into();
        current
    }

    fn labels(notes: &[Note]) -> Vec<&'static str> {
        notes.iter().map(|note| note.label).collect()
    }

    fn entry(label: &str, told: Option<Told>) -> ContextInjection {
        ContextInjection {
            kind: InjectionKind::Instructions,
            bytes: 10,
            tokens_estimate: 3,
            label: label.into(),
            task_id: None,
            told,
        }
    }

    /// Applies notes the CLI took the way the turn does: what they told fills in over what
    /// the session knew.
    fn took(told: &mut Told, notes: &[Note]) {
        for note in notes {
            let mut now = note.told.clone();
            fill_told(&mut now, told);
            *told = now;
        }
    }

    #[test]
    fn a_session_told_everything_now_needs_no_note() {
        for now in [
            current("approveForMe", None, &[]),
            current(
                "fullAccess",
                Some("- The run lasts 60 minutes."),
                &["Prefers pnpm"],
            ),
            Current::chat(vec!["Lives in Chisinau".into()]),
        ] {
            assert_eq!(notes(&now.told(), &now), Vec::new());
        }
    }

    #[test]
    fn a_resumed_session_hears_a_new_day_and_nothing_else() {
        let now = current("approveForMe", None, &["Prefers pnpm"]);
        let mut told = now.told();
        told.today = Some("2026-10-03".into());
        let sent = notes(&told, &now);
        assert_eq!(labels(&sent), vec![TODAY_LABEL]);
        assert_eq!(sent[0].text, "[today] It's now 2026-10-04.");
        took(&mut told, &sent);
        assert_eq!(told, now.told());
        // A Chat too.
        let chat = Current::chat(Vec::new());
        let mut told = chat.told();
        told.today = Some("2026-10-01".into());
        assert_eq!(labels(&notes(&told, &chat)), vec![TODAY_LABEL]);
    }

    #[test]
    fn a_session_that_started_on_the_old_contract_hears_the_note_rule_first() {
        let now = current("approveForMe", None, &[]);
        // What an entry logged by an older build tells: role instructions with Short replies
        // on, logged at 2026-10-04 10:00 UTC.
        let old = entry(ROLE_INSTRUCTIONS_SHORT, None);
        let (told, reached) = told_from_log([(1_791_108_000_000, &old)]);
        assert!(reached);
        assert_eq!(told.contract, Some(0));
        assert_eq!(told.today.as_deref(), Some("2026-10-04"));
        assert_eq!(told.short_replies, Some(true));
        let sent = notes(&told, &now);
        // It can't know the permission level or preferences it started with: they go once.
        assert_eq!(
            labels(&sent),
            vec![CONTRACT_LABEL, PERMISSION_LABEL, PREFERENCES_LABEL]
        );
        assert_eq!(sent[0].text, format!("[instructions] {AUTHORITY}"));
        let chat = Current::chat(Vec::new());
        let (told, _) = told_from_log([(1_791_108_000_000, &entry(ROLE_INSTRUCTIONS, None))]);
        let sent = notes(&told, &chat);
        assert_eq!(sent[0].text, format!("[instructions] {CHAT_AUTHORITY}"));
    }

    #[test]
    fn a_changed_permission_level_is_told_with_the_instructions_own_words() {
        let before = current("askForApproval", None, &[]);
        let now = current("fullAccess", None, &[]);
        let sent = notes(&before.told(), &now);
        assert_eq!(labels(&sent), vec![PERMISSION_LABEL]);
        assert!(sent[0].text.starts_with(
            "[settings] The user changed the permission level. From now on: Full access: like Approve for me, but workers run without the OS sandbox."
        ));
    }

    #[test]
    fn an_overnight_run_is_told_when_it_starts_changes_and_ends() {
        let plain = current("askForApproval", None, &[]);
        let run = current("askForApproval", Some("- The run lasts 60 minutes."), &[]);
        // Started: the run's own words, with its restrictions.
        let sent = notes(&plain.told(), &run);
        assert_eq!(labels(&sent), vec![RUN_LABEL]);
        assert!(
            sent[0]
                .text
                .contains("onto the run's own branch `overnight/2026-10-04-textkit-1234`")
        );
        assert!(sent[0].text.contains("Never ask the user"));
        assert!(sent[0].text.ends_with("- The run lasts 60 minutes."));
        // Its deadline changed: told again.
        let later = current(
            "askForApproval",
            Some("- The report is due at Mon 06:00 (+03:00)."),
            &[],
        );
        let sent = notes(&run.told(), &later);
        assert_eq!(labels(&sent), vec![RUN_LABEL]);
        assert!(sent[0].text.contains("due at Mon 06:00"));
        // Over: the session's own setting is back, the permission level with it.
        let sent = notes(&later.told(), &plain);
        assert_eq!(labels(&sent), vec![RUN_OVER_LABEL]);
        assert!(sent[0].text.starts_with("[run] The overnight run is over"));
        assert!(sent[0].text.contains(
            "Local checkout: landed work goes, as the workers' own commits, directly onto `main`"
        ));
        assert!(sent[0].text.contains("Permission level: Ask for approval:"));
        let mut told = later.told();
        took(&mut told, &sent);
        assert_eq!(told, plain.told());
        // The permission level switched during the run: the run's words for it (its
        // sandbox), not the session's, which come back with its end.
        let full = current("fullAccess", Some("- The run lasts 60 minutes."), &[]);
        let sent = notes(&run.told(), &full);
        assert_eq!(labels(&sent), vec![RUN_LABEL]);
        assert!(
            sent[0]
                .text
                .contains("workers run without the OS sandbox, as in the session")
        );
        // A session that can't have heard of a run is never told one ended.
        let mut unknown = plain.told();
        unknown.run = None;
        assert_eq!(notes(&unknown, &plain), Vec::new());
    }

    #[test]
    fn changed_preferences_are_told_in_full_and_none_left_says_so() {
        let before = current("approveForMe", None, &["Prefers pnpm"]);
        let now = current(
            "approveForMe",
            None,
            &["Prefers pnpm", "Writes British English"],
        );
        let sent = notes(&before.told(), &now);
        assert_eq!(labels(&sent), vec![PREFERENCES_LABEL]);
        assert!(
            sent[0]
                .text
                .ends_with("\n- Prefers pnpm\n- Writes British English")
        );
        let none = current("approveForMe", None, &[]);
        let sent = notes(&before.told(), &none);
        assert!(
            sent[0]
                .text
                .contains("the list in your instructions no longer applies")
        );
    }

    #[test]
    fn the_log_gives_the_newest_word_on_each_part_since_the_cli_started() {
        let start = current("askForApproval", None, &[]);
        let role = entry(ROLE_INSTRUCTIONS_SHORT, Some(start.told()));
        let today = entry(
            TODAY_LABEL,
            Some(Told {
                today: Some("2026-10-05".into()),
                ..Told::default()
            }),
        );
        let off = entry(SHORT_REPLIES_OFF, None);
        let older = entry(
            TODAY_LABEL,
            Some(Told {
                today: Some("1999-01-01".into()),
                ..Told::default()
            }),
        );
        // Newest first; the entry before the role instructions belongs to an earlier CLI.
        let (told, reached) = told_from_log([(3, &off), (2, &today), (1, &role), (0, &older)]);
        assert!(reached);
        assert_eq!(told.today.as_deref(), Some("2026-10-05"));
        assert_eq!(told.short_replies, Some(false));
        assert_eq!(told.permission, Some(PermissionLevel::AskForApproval));
        assert_eq!(told.contract, Some(CONTRACT));
        // Not reaching the start: only what the entries said.
        let (told, reached) = told_from_log([(3, &off)]);
        assert!(!reached);
        assert_eq!(told.contract, None);
    }

    #[test]
    fn a_note_whose_turn_failed_goes_again_after_a_restart_and_once_it_was_taken_not_again() {
        let started = current("approveForMe", None, &[]);
        let mut log = vec![(1, entry(ROLE_INSTRUCTIONS_SHORT, Some(started.told())))];
        let recovered = |log: &[(i64, ContextInjection)]| {
            told_from_log(log.iter().rev().map(|(at, entry)| (*at, entry))).0
        };
        let mut now = current("fullAccess", None, &[]);
        now.today = "2026-10-05".into();
        let first = notes(&recovered(&log), &now);
        assert_eq!(labels(&first), vec![TODAY_LABEL, PERMISSION_LABEL]);
        // The turn failed: nothing was logged. After a restart the same notes go again.
        let retry = notes(&recovered(&log), &now);
        assert_eq!(retry, first);
        // The CLI took them: each is logged with what it told.
        for note in &retry {
            log.push((2, entry(note.label, Some(note.told.clone()))));
        }
        assert_eq!(notes(&recovered(&log), &now), Vec::new());
    }
}
