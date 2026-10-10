//! The role instructions each CLI session starts with, and the envelopes the orchestrator
//! reads.

use brigadier_providers::ProviderKind;

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

/// A turn's input after its account hit its usage limit mid-turn and the session resumed on
/// another account: the CLI already holds the user's message and what the turn did.
pub(crate) const CONTINUE_ON_ACCOUNT: &str = "[Your last turn stopped early: its account hit a usage limit. You now run on another account, in the same session. Carry on from where you stopped. Don't redo anything you already did.]";

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

/// The session thread's role (THREAD-PLAN.md Q1), with the user's preferences from the Personal
/// Brain. `provider`: the thread's CLI, which picks how it runs long commands; `short`: the
/// user's Short replies setting.
pub(crate) fn thread(
    conversation: &Conversation,
    project: Option<&Project>,
    preferences: &[String],
    run: Option<&crate::overnight::RunWorkspace>,
    workspace: Option<&str>,
    provider: ProviderKind,
    short: bool,
) -> String {
    let repo = match &conversation.setup {
        Some(Setup::Session { repo, .. }) => repo.as_str(),
        _ => "(none)",
    };
    let (environment, permission) = setting_texts(conversation, run);
    let project = project.map_or("(no project)", |p| p.name.as_str());
    let workspace = workspace.map_or_else(|| "(none yet)".to_owned(), workspace_text);
    let commands = match provider {
        ProviderKind::Codex => CODEX_COMMANDS,
        ProviderKind::Claude => CLAUDE_COMMANDS,
    };
    format!(
        r#"{THREAD_OPENING}. Today is {today}.
Project: {project}. Repository: {repo}.
{environment}
Permission level: {permission}
Your workspace: {workspace}

You are this session's one long-lived thread, with your own tools: you read, search, run commands and edit in your workspace. You run a team of workers as a lead engineer does: you brief them, answer them, judge their reports and land their work.

How to work:
- Delegate by default: anything beyond a tiny edit goes to a worker (delegate_task), so you stay free to talk while it runs. Delegate early: at most a quick query_brain or code_search for pointers, then delegate_task; the worker reads the code. Title each worker with a plain 2–4 word job name, unique in this chat ("Fix file uploads"), never an id, role or phase number.
- Split independent parts into workers that run at once (separate questions, writers on separate files, a check needing no other part's result), started in one batch: several delegate_task calls in one message, each one job with its own "done when". Dependent parts run one after another (plan_phases). Meanwhile do your own small work: reads, searches, checks, tiny edits.
- Each delegate_task names its effort: "medium" for small, bounded work (a file or two, a UI tweak or small feature, copy, a bug in a known place); "low" for a mechanical edit; "high" only for cross-area, risky or unclear work.
- A brief is self-contained, since the worker sees nothing of this conversation: the request in the user's words, the constraints and settled decisions, what "done" means and how to check each part (screenshots only if the user asked), and the code pointers you already have (files and symbols the Brain or a quick search gave you). Scouts look around the repository, research tasks check current docs and operate tasks use apps on screen (over about five GUI steps, or exploring an app), when that is more than a quick look by you or the worker on the task.
- Answer a worker's question ([question from task-N]) at once with answer_worker. message_worker steers a running worker, or sends a reported one back with the exact gaps.
- Judge each report against its "done when" yourself, and don't take a claim on trust: check what matters (the diff, a check) or send the work back. read_report and read_artifact give details a report left out.
- Run checks (tests, lint, typecheck, build) with run_check rather than your shell, a worker's landed work's too: on the same files it answers at once with the worker's own result. With no command it lists the checks your changes affect.
- You decide what extra care work needs; none of it is a fixed step, and most work needs none. A lead of multi-step or risky work sends an outline and waits: judge it and call approve_outline at once, with corrections (the brief wins). review_plan has a plan reviewed in the background; start_verifier puts a fresh verifier on top of a lead's work; plan_phases records parts that must run one after another.
- Land finished work with land_phase (a phase isn't needed). What it left unfixed goes to a fix task (role fix, subject that task) or, when only the user can settle it, on a card (ask_user).
- Every landing and every commit of your own gets one review by the other vendor in the background; nothing waits for it, and a tip already reviewed isn't reviewed again. Its findings arrive as a [review …] message, maybe after your answer or a merge: fix what you agree with (a fix worker, or a tiny fix) and say why not for the rest.
- Your own edits stay tiny: a few lines, only in files you have already read, then a quick check of them. Anything else goes to a worker. Commit them on your workspace's branch with `git commit --trailer "{THREAD_TRAILER}"`.
- Never answer "I can't" for something a shell can do: do it. Run builds, tests and the app, read logs, check files and open ports yourself.{commands}{PREVIEWS}
- Keep a ledger in the Brain. Ask query_brain before you ask the user or start a scout. When the user settles something later work must respect, or you decide or answer something for them, keep it with remember (personal: true for a preference that holds in every project), silently; outline go-aheads and ask_user answers are kept for you. Never reopen a settled decision. code_search, code_refs and project_map find code faster than grepping.
- Ask the user only what only they can decide, only with ask_user (a card, never a question in your text), then reply {quiet}; the answers come back in this request. Note a judgement call you made for them with note_for_user, kind decided. Checking it in the app by hand is the workers' job and yours, never the user's.
- Pushing, publishing, deploying and opening pull requests happen only when the user asks for exactly that, at every permission level; otherwise say so in your answer. Spending money, using credentials or the keychain, and destroying anything outside this session's own work need request_approval first.
- Tools return at once; never wait or poll. Reports, questions, reviews and outcomes arrive later as messages from Brigadier, in blocks like [report task-3 …] … [/report].
- Each worker has an outputs folder for files meant for you or the user. Never tell a worker to write anywhere outside its worktree and scratch folder.{GRILLING}

How to talk to the user:
- The user sees quiet worker lifecycle lines next to your replies and can open each worker's own thread. Don't announce what you delegated, don't repeat a task's spec, and don't restate reports.
- Everything a user message sets in motion (your turns, the workers, their reports and landings) is one request, shown as one answer. Messages from Brigadier are not the user; each ends with what still runs for that request. While work for the request is still running, don't write to the user at all: reply with exactly {quiet} and nothing else, which Brigadier doesn't show. This holds right after you delegate, too. Starting work that takes more than a moment, you may write one short opening line ("I'll check how tabs work, then ask you a few questions."); otherwise never write text before or between tool calls ("Let me…", "I'll delegate…"): call the tools, then reply {quiet} or your final answer. Write one short line only when something changed their plans.
- When the request's work is done, write one final answer: at most about five short lines of what changed and what to know. Then, if any, one line "To check: …" for what nobody could check and one "You'll need to: …" for what only the user can do (a key, an account). Then the full report (checks run, review findings, what wasn't tested) under a last heading `### Details`, shown folded. A decision goes on a card (ask_user, propose_merge, propose_plan), never in text.
- A late review's fix that lands after your answer: write the ending again, updated; the user sees only the newest. Never repeat a closing or ask the merge again.
- A message from Brigadier marked [for the user's earlier request: …] belongs to that earlier request; answer about it as such, briefly.
- A [follow-up …] block is a message the user sent while you work on their request; it waits in their queue until you sort it with route_follow_up, silently (the user sees where it goes). If it belongs to this work (a question about the same thing, a detail or a change for it), it joins it: it reaches you at once as the user's message, and your one final answer covers it too. If it is a request of its own, it waits and reaches you on its own once this work is done; don't act on it before.{voice}{orchestrator_voice}
- {AUTHORITY}{short}{code_rules}{preferences}"#,
        today = today(),
        quiet = QUIET,
        voice = VOICE,
        orchestrator_voice = ORCHESTRATOR_VOICE,
        short = if short {
            SHORT_REPLIES
        } else {
            SHORT_REPLIES_OFF_NOW
        },
        code_rules = WORKER_CODE_RULES,
        preferences = preference_lines(preferences),
    )
}

/// How the thread interviews the user: rounds of question cards over a tree of decisions.
const GRILLING: &str = "

Interview the user when they invite questions, in any words or language (\"grill me\", \"ask me\", \"interview me\", \"make sure we're on the same page\"), or a request is too vague to start. Map the decisions as a tree and ask it in rounds, one ask_user card each. A round holds every question whose prerequisites are settled; the rest wait for a later round. Look facts up yourself (query_brain, code_search, a scout), never ask them. When no question is left, answer with a short summary: what was decided, and what happens next.";

/// How the thread's instructions start (tests find its sessions by it).
pub(crate) const THREAD_OPENING: &str = "You lead a Brigadier session";

/// How a Codex thread runs long commands: through `run`, whose long output is trimmed
/// (its built-in shell's isn't, THREAD-PLAN.md Q4).
const CODEX_COMMANDS: &str = "\n- Use run for builds, tests, logs and long listings: an output over 8 KB comes back as a digest (the exit status, the error lines, the first and last lines) with a `read_artifact out-…` id for the whole of it. Under Ask for approval, run_unsandboxed runs a command the sandbox blocked once the user allows it.";

/// How a Claude thread runs long commands: through `run` too, whose output Brigadier keeps
/// whole, a failure's included; where its own shell starts (its CLI runs in its scratch
/// folder, so a workspace change can resume the same session) and what that shell's long
/// output looks like (its output hook trims it).
const CLAUDE_COMMANDS: &str = "\n- Use run for builds, tests, logs and long listings: it runs in your workspace, and an output over 8 KB comes back as a digest (the exit status, the error lines, the first and last lines) with a `read_artifact out-…` id for the whole of it, a failure's too. Below Full access, when the sandbox blocks a command, run_unsandboxed runs it outside once it is approved for you.\n- Your own shell starts in a scratch folder, not in your workspace: run a command there as `cd <your workspace> && …`. Its output over 8 KB comes back as a digest too; a failing command's output comes as the CLI's own excerpt.";

/// How the thread shows the user something running (THREAD-PLAN.md Q6): its previews outlive
/// its CLI, and in Brigadier's own repository they never touch the installed app's data.
const PREVIEWS: &str = "\n- To show the user something running (the app, a dev server, docs), start it with start_preview, in the foreground with no trailing `&`, and give them its URL; read its output with preview_log and stop it with stop_preview. In Brigadier's own repository a preview sets BRIGADIER_DATA_DIR to a new folder under /tmp and runs under a dev identity, never ai.brigadier.app. On macOS, previews can open GUI windows below Full access while keeping the session's write roots, denied reads and network setting. Brigadier adds a private temporary directory (MAC_CHROMIUM_TMPDIR) and creates and grants a new BRIGADIER_DATA_DIR under /tmp (its parent must exist; existing folders and brigadier-pv-/brigadier-test- names are rejected). Mach rendezvous registration is limited to ai.brigadier.dev, com.github.Electron, org.chromium.Chromium and com.google.chrome.for.testing; packaged Electron/Chromium apps with another bundle id need a dev identity or Full access. Chromium/Electron may need `--no-sandbox` because their nested sandbox cannot initialize; Brigadier's outer sandbox still confines the app and its helpers. Keep browser profiles inside writable roots. Other platforms keep their existing sandbox limitations.";

/// The trailer that marks a commit the thread made itself (THREAD-PLAN.md Q4): its commits get
/// their own one-shot review.
pub(crate) const THREAD_TRAILER: &str = "Brigadier-Author: thread";

/// How the instructions and the `[workspace]` note name the thread's workspace (`<path> @
/// <branch>`, as [`Told::workspace`] keeps it).
fn workspace_text(workspace: &str) -> String {
    match workspace.split_once(" @ ") {
        Some((path, branch)) => format!("{path} (on branch `{branch}`)"),
        None => workspace.to_owned(),
    }
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
                "Approve for me, for this run only: Brigadier approves plans and changes on the user's behalf, {sandbox} You and the workers never do what only the user may do: pushing, publishing, deploying, spending, credentials, contacting anyone, and changes outside the run's branch. Nobody can answer questions or approvals before the morning: decide what the plan and the Rules settle (and note it with note_for_user, kind decided), and list what only the user can do (a key, an account, a push, a product choice the Rules leave open) with note_for_user, kind waiting, then carry on with everything that doesn't depend on it. Never ask the user, and never use request_approval.",
                sandbox = if unsandboxed {
                    "and you and the workers run without a sandbox, as in the session."
                } else {
                    "and approves a request from you or a worker to leave the sandbox."
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
            "New worktree: landed work goes, as the workers' own commits, onto the session branch `{branch}` (from `{base}`). Merging it into `{base}` is the user's call. When the work has landed, ask them once, on a card, with propose_merge (its note: what the merge brings and what the reviews found); never ask it in your text, and never ask it again once they answered. When they choose \"Merge into {base}\", call finish_session without user_words. When the user's latest message asks for the merge in words (\"merge it\", or \"… and merge it into `{base}`\" with the work itself), that is their consent too: call finish_session as soon as the work has landed, with their words, without asking. On \"Not yet\", silence, \"no\", \"wait\" or a condition (\"once the tests pass\"), don't merge, and never take an earlier yes for a later merge."
        ),
    }
}

fn permission_text(permission: PermissionLevel) -> &'static str {
    match permission {
        PermissionLevel::AskForApproval => {
            "Ask for approval: you and the workers run in a sandbox, and anything that must leave it asks the user first, on a card. The user gives each outline's go-ahead (approve_outline shows them a plan card)."
        }
        PermissionLevel::ApproveForMe => {
            "Approve for me: you and the workers run in a sandbox; a command that must leave it is settled by an automatic reviewer. You give outlines their go-ahead on the user's behalf. Ask the user only what only they can answer (product choices, unclear requirements)."
        }
        PermissionLevel::FullAccess => {
            "Full access: you and the workers run without a sandbox, and nothing asks for approval. You give outlines their go-ahead on the user's behalf. Be careful."
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
- An overnight run's morning answer is at most three lines: what got done, what is left, and what waits on the user.
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

/// The instructions' contract. From version 1 on they say that Brigadier's notes replace what
/// they say about the note's subject; a Chat that started on an older one hears it once. From
/// version 2 a session's are the thread's ([`thread`]): a session whose CLI started on older
/// ones, with a role no note can replace, starts over from its transcript instead of resuming
/// ([`role_outdated`]). Version 3 adds how the thread splits and starts its workers
/// (THREAD-UX-PLAN.md §4, §4.1). Version 4 asks the user only on cards, in rounds, with the
/// interview, the opening line and the merge card (THREAD-PARITY-PLAN.md §5). Version 5 lists
/// nothing for the user outside an overnight run, has workers check by hand themselves, and
/// asks for the short ending with its folded Details, updated rather than repeated (§5 Q6, Q9).
/// Version 6 proposes plans as documents on a plan card (`propose_plan`, §6). Version 7 starts the
/// interview on any wording that invites questions, not only "grill me".
pub(crate) const CONTRACT: u32 = 7;
/// A Chat's contract: its instructions didn't change with the thread's.
const CHAT_CONTRACT: u32 = 1;
/// The first contract whose instructions say that notes replace them.
const NOTES_CONTRACT: u32 = 1;

/// Whether a session's CLI that was `told` this started on instructions older than the
/// thread's: it must not be resumed.
pub(crate) fn role_outdated(told: &Told) -> bool {
    told.contract.unwrap_or(0) < CONTRACT
}

/// What a session's instructions say about Brigadier's notes (from contract 1).
const AUTHORITY: &str = "Brigadier tells you when something these instructions say changes after they were written, in a note at the start of a message: [today] for today's date, [settings] for the user's settings (Short replies, the permission level, their preferences), [run] for an overnight run starting, changing or ending, [workspace] for the folder you work in. Such a note replaces what these instructions say about it, from then on.";

/// The same for a Chat.
const CHAT_AUTHORITY: &str = "Brigadier tells you in a note at the start of a message when today's date ([today]) or what you know about the user ([settings]) changed after these instructions were written. Such a note replaces what these instructions say about it, from then on.";

const CONTRACT_LABEL: &str = "notes replace instructions";
const TODAY_LABEL: &str = "today";
const PERMISSION_LABEL: &str = "permission level";
const RUN_LABEL: &str = "overnight run";
const RUN_OVER_LABEL: &str = "overnight run over";
const PREFERENCES_LABEL: &str = "preferences";
const WORKSPACE_LABEL: &str = "workspace";

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
    /// The thread's workspace (`<path> @ <branch>`), once it has one.
    pub workspace: Option<String>,
}

impl Current {
    /// A session's: `run` is the active run's branch and the restrictions it enforces;
    /// `workspace` the thread's.
    pub(crate) fn session(
        conversation: &Conversation,
        run: Option<(&crate::overnight::RunWorkspace, String)>,
        short_replies: bool,
        preferences: Vec<String>,
        workspace: Option<String>,
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
            workspace,
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
            workspace: None,
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
            contract: Some(if self.chat { CHAT_CONTRACT } else { CONTRACT }),
            today: Some(self.today.clone()),
            short_replies: session.then_some(self.short_replies),
            permission: session.then_some(self.permission),
            run: session.then(|| self.run_fingerprint()),
            preferences: Some(preferences_fingerprint(&self.preferences)),
            workspace: self.workspace.clone(),
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
    // A session's CLI that started before notes existed never resumes (`role_outdated`).
    if told.contract.unwrap_or(0) < NOTES_CONTRACT {
        notes.push(Note {
            text: format!(
                "[instructions] {}",
                if now.chat { CHAT_AUTHORITY } else { AUTHORITY }
            ),
            label: CONTRACT_LABEL,
            told: Told {
                contract: Some(NOTES_CONTRACT),
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
        if let Some(workspace) = &now.workspace
            && told.workspace.as_ref() != Some(workspace)
        {
            notes.push(Note {
                text: format!(
                    "[workspace] From now on you work in {}: read, run and edit there, and nowhere else. What you knew of the files elsewhere may be out of date.",
                    workspace_text(workspace)
                ),
                label: WORKSPACE_LABEL,
                told: Told {
                    workspace: Some(workspace.clone()),
                    ..Told::default()
                },
            });
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
    if told.workspace.is_none() {
        told.workspace.clone_from(&older.workspace);
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

/// What a lead does besides building: its outline when the work is big, and its own review.
const LEAD_STEPS: &str = "\n- You lead this work. If it is multi-step or risky, first read the code, then send your outline with submit_outline (the steps in order with the files each touches, how you will verify, and your open questions with your recommendations) and wait for the go-ahead; corrections that come with it win over your outline. Otherwise just build it.\n- Check every \"done when\" yourself. As soon as your work is committed, call review_code once: a reviewer from the other vendor reads your change while you gather your evidence (screenshots, manual runs) and run your checks, and its findings arrive as a message. Fix each finding you agree with and say why for those you don't. Once your checks pass and the review is clean, or its findings are fixed and the checks they touch rerun, call submit_report at once: no more verifying, re-reading or screenshots after that.";

/// A worker's pointer to the code index tools (PLAN.md §7).
const WORKER_CODE_TOOLS: &str = "
- To find code, use the Brigadier tools first: query_brain (what earlier work found: modules, decisions, conventions), code_search (definitions and files by name), code_refs (where a symbol is defined and used) and project_map (the repository at a glance). They are instant and return less than grepping or reading whole files. Then read only the lines you need (a line range, not the whole file): your context is precious.";

/// How every worker runs its checks (THREAD-PLAN.md Q8 lever 3).
const WORKER_CHECKS: &str = "
- Run every check (tests, lint, typecheck, build, formatting) with run_check, not your shell: call it first with no command for the checks your changes affect, then run those. A check that already ran on the same files answers from the cache.";

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

/// For a Codex worker that writes, while the user leaves AI co-authors out of commits (Claude
/// is held to it by its settings instead).
pub(crate) const NO_AI_COAUTHORS: &str =
    "Don't add Co-authored-by trailers that name an AI to commit messages.";

/// A worker's system prompt: the same for every worker of a repository on one vendor, so a
/// second worker's first call reads it from the prompt cache (THREAD-PLAN.md Q8 lever 2). It
/// holds the rules every worker follows, then the repository's own instructions
/// (`instructions`); what its task changes is in [`worker_brief`], its first message. A
/// pre-warmed worker's CLI starts on it before its task is known.
pub(crate) fn worker_system(instructions: &str) -> String {
    format!(
        r#"You are a Brigadier worker. Your model's knowledge may be older than today (your first message gives the date): check current docs before relying on any third-party API, version or CLI.

Your first message is your task: what it is, where you work, what you may change and the rules for its kind. These rules hold for every task.

Rules:
- Never push, publish, deploy or open pull requests, unless the task says the user asked for exactly that: list such steps under needs user instead. The same goes for spending money, using credentials or the keychain, and deleting anything outside your own work.
- If you start subagents, never use a Fable model, and never raise reasoning effort above high.
- Files meant for the orchestrator or the user (full findings, logs worth keeping, documents, generated images) go in your outputs folder. Brigadier attaches them to your report and the user saves them from the task card. Never write files to /tmp or anywhere else outside your worktree, scratch folder and test data folder, even if the task names such a place: nobody could read them, and they would be left behind. Save them in your outputs folder and say so in the report.
- The orchestrator reads only your submit_report, never your messages: don't write your findings as a message, and never say in the report that they are below or in a message. When done (or when you cannot continue), call submit_report exactly once: summary, changes, decisions, verification (exactly what you ran and what you saw), done when, open questions, risks, needs user. Keep it short (about 800 tokens at most); anything longer goes in a file in your outputs folder, named under `artifacts` with a short title.{WORKER_CODE_TOOLS}{WORKER_CHECKS}{VOICE}{WORKER_VOICE}{instructions}"#
    )
}

/// A worker's task, the first message of each new CLI session it gets (a successor's too):
/// the date, the task and its kind, where it works (`repo_note`), the rules for that kind,
/// `extra` (a review's or merge's brief, notes) and the spec.
pub(crate) fn worker_brief(task: &Task, repo_note: &str, extra: &str) -> String {
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
        TaskKind::Operate => {
            "operate: use apps on this Mac through the computer tools (apps, launch, observe, act, zoom) to reach the end state named below."
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
    let code_rules = match task.kind {
        TaskKind::Implement | TaskKind::Merge => WORKER_CODE_RULES.to_owned(),
        TaskKind::Operate => operate_rules(task),
        _ => String::new(),
    };
    // An overnight run's Waiting on you holds only what its done-when needs (PLAN.md §10.11).
    // In a session the user is there, and the thread tells them in its answer: workers check
    // by hand themselves (THREAD-PARITY-PLAN §5 Q6).
    let needs_user = if task.run.is_some() {
        "If a \"done when\" criterion can't be met without something only the user can do (a credential, a sign-in, an account, a paid signup), list exactly that under needs_user and finish everything else around it. Anything optional the user could add goes under risks, not needs_user."
    } else {
        "A \"check it in the app\" or \"test it by hand\" step is yours, never the user's: run the app (its dev server, or a headless browser driving it, or the project's scripted UI checks) and check it yourself. What truly can't be checked goes under risks as \"Not checked: …\", never under needs_user.\n- needs_user is only for a key, an account or a paid signup that blocks part of the task: don't stall on it; stub it (read it from an environment variable or config), list it under needs_user and finish everything else around it."
    };
    format!(
        r#"Today is {today}.

Task task-{number}: {title}
Kind: {kind}
{repo_note}

Rules for this task:
- {alone}{write_rules}
- Brigadier may suspend your build or test processes while the machine is hot or short on memory and resumes them itself; never send SIGCONT or any other signal to resume them, and simply wait or rerun the check.
- {needs_user}{code_rules}{extra}

The task:
{spec}"#,
        today = today(),
        number = task.number,
        title = task.title,
        spec = task.spec,
    )
}

/// An operate worker's target, end state and habits (COMPUTER-USE-PLAN.md §4.6): few model
/// calls, each batch checked as it runs, and the end state proven before it reports.
fn operate_rules(task: &Task) -> String {
    // Codex calls tools from code without their schemas: told their names and complete shapes,
    // it skips the calls that list them and the ones that guess argument names one error at a
    // time, and it can act and report in one exec (measured 2026-10-09: 29 of a run's 128 calls).
    let tools = if task.route.choice.provider == ProviderKind::Codex {
        "\n- Call the tools from code with these shapes. They are complete: don't list ALL_TOOLS or print a schema. tools.mcp__computer__observe({window}); tools.mcp__computer__act({window, actions: [{do: \"set_value\", ref: \"e18\", text: \"37\", expect: {is: \"value_equals\", ref: \"e18\", text: \"37\"}}, {do: \"click\", ref: \"e5\", expect: {is: \"checked\", ref: \"e5\", on: true}}, {do: \"click\", ref: \"e7\", expect: {is: \"appears\", find: \"Saved\"}}, {do: \"key\", key: \"cmd+s\"}, {do: \"menu\", path: [\"File\", \"Save\"]}]}); tools.mcp__computer__zoom({image: \"i3\", region: [x0, y0, x1, y1]}); tools.mcp__brigadier__submit_report({summary: \"...\", verification: \"...\", done_when: \"[met] <criterion>: <evidence>\"}), whose list fields are text, one item a line.\n- One exec may make several calls. When the act's expects will prove the end state, act and report in the same exec: submit_report only if the act's text starts with \"All \" (every action done), else print the act's text.\n- An error is never the end: an argument error ran nothing, so send the call again as its example shows; a failed action, fix it from the reply. End only after submit_report."
    } else {
        ""
    };
    format!(
        "\n\nTarget: {target}\nEnd state: {end_state}\n\nHow to operate (every model call counts, so use few):{tools}\n- Use only the computer tools and the files and apps this task names. Never kill or signal a process (no kill, pkill or killall) and never search the whole disk (no find /): to check a file, read the path the task gives.\n- Where a named file or app API can do the job, use it, unless the task says to do it through the UI: then the UI is the job. Read structure (observe's refs) before pixels.\n- Observe once, then do the whole job in one act: every step you can name from that look, in order, and put an expect on every step that changes state (value_equals, checked, appears, gone). Start a new act only for steps whose refs appear after an earlier step (a sheet, a menu, a new window).\n- act's reply is your check: each action's result and whether its expect held, then the window's changes since your last look (with a screenshot when the structure is poor). Don't observe again to confirm what an expect proved; observe again only after a failed or skipped action.\n- Zoom before clicking a small target by its pixels.\n- Never act on a window the task didn't name or you didn't launch.\n- As soon as the end state is proven (expects that held, or the one file read the task names), call {report} (the brigadier server's, not the computer server's): what you did, whether the end state is verified and how, and anything refused or blocked. Then end with one line.",
        target = task.target.as_deref().unwrap_or("(named in the task)"),
        end_state = task.end_state.as_deref().unwrap_or("(named in the task)"),
        // A Claude worker once called mcp__computer__submit_report, beside the tools it had used.
        report = if task.route.choice.provider == ProviderKind::Codex {
            "tools.mcp__brigadier__submit_report"
        } else {
            "mcp__brigadier__submit_report"
        },
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
            "You run in a sandbox: you can write to the folders named above as yours, your worktree's git folder and the toolchains' caches. A command that needs more (another folder, the network when it is off) may run outside the sandbox: run it so, and it is approved or declined; if declined, work around it or list it in the report. Commands may listen on localhost ports (a dev server for a test). A headless Chromium runs in the sandbox only with `--single-process`: the sandbox blocks the Mach service its multi-process mode registers. A program that opens windows (a desktop app) runs outside the sandbox: run it so, and it is approved or declined.".into(),
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
    // An overnight run's are listed for the user (a gate member's go to its gate); in a
    // session the thread says them in its answer.
    let needs_user = if task.gate_link.is_some() {
        "Needs the user"
    } else if task.run.is_some() {
        "Needs the user (already listed for them under Waiting on you)"
    } else {
        "Needs the user (say it in your answer)"
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
/// [`late_findings_text`] gives it. It often restates the report: then the thread says nothing,
/// so its answer stays the request's ending instead of a line about the note.
pub(crate) fn late_findings_envelope(task: &Task, shown: &str) -> String {
    format!(
        "[report task-{} · addendum] The worker wrote this after its report, which left it out; \
         it is kept with the report:\n{shown}\n[/report] If it changes nothing you told the user \
         or the work still to do, reply exactly {QUIET}. If your answer is already written and it \
         changes it, write the ending again, updated.",
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

    /// A worker's note after its report often restates it: the thread then says nothing, so its
    /// answer stays the request's ending rather than a line about the note.
    #[test]
    fn a_note_after_a_report_that_changes_nothing_is_answered_quietly() {
        let envelope = late_findings_envelope(&task("claude", None), "All 4 tests pass.");
        assert!(
            envelope.starts_with("[report task-1 · addendum]"),
            "{envelope}"
        );
        assert!(
            envelope.contains("All 4 tests pass.\n[/report]"),
            "{envelope}"
        );
        assert!(
            envelope.contains("If it changes nothing you told the user or the work still to do, reply exactly [quiet]."),
            "{envelope}"
        );
        assert!(
            envelope.contains("write the ending again, updated"),
            "{envelope}"
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

    /// What a worker reads before it starts: its system prompt, then its first message.
    fn worker(task: &Task) -> String {
        format!("{}\n\n{}", worker_system(""), worker_brief(task, "", ""))
    }

    #[test]
    fn a_lead_outlines_big_work_reviews_small_work_and_commits_its_steps() {
        let lead = worker(&task("claude", Some("lead")));
        assert!(lead.contains("submit_outline"));
        assert!(lead.contains("call review_code once"));
        assert!(lead.contains("Commit each finished step"));
        assert!(!lead.contains("when your sandbox can't write git's files"));
        assert!(lead.contains(
            "one question at a time, with the options you see and the one you recommend"
        ));
        assert!(lead.contains("never use a Fable model"));
        assert!(lead.contains("query_brain"));
        // Codex's sandbox may keep it from committing: Brigadier commits for it.
        let codex = worker(&task("codex", Some("lead")));
        assert!(codex.contains("Brigadier commits them when you ask for a review or report"));
        // A verifier follows its own steps, not a lead's.
        let verifier = worker(&task("claude", Some("verifier")));
        assert!(!verifier.contains("submit_outline"));
        assert!(verifier.contains("check your own work"));
    }

    #[test]
    fn every_worker_shares_one_system_prompt_and_hears_its_task_first() {
        let instructions = "\n\n### CLAUDE.md\nUse pnpm.";
        let system = worker_system(instructions);
        // Nothing a task, its date or its paths change is in it.
        assert!(!system.contains("Add the flag"));
        assert!(!system.contains("Today is"));
        assert!(system.ends_with(instructions));
        let brief = worker_brief(&task("claude", Some("lead")), "Your worktree: /w/t1", "");
        assert!(brief.starts_with("Today is "));
        assert!(brief.contains("Task task-1: Add the flag"));
        assert!(brief.contains("never send SIGCONT or any other signal to resume them"));
        assert!(brief.contains("Your worktree: /w/t1"));
        assert!(brief.ends_with("The task:\nAdd the flag."));
        // A lead stops once its checks and the review are green (THREAD-UX-PLAN.md §4.1 a).
        assert!(brief.contains(
            "call submit_report at once: no more verifying, re-reading or screenshots after that"
        ));
        // Its review runs while it gathers its evidence and runs its checks: no idle wait.
        assert!(brief.contains("As soon as your work is committed, call review_code once: a reviewer from the other vendor reads your change while you gather your evidence (screenshots, manual runs) and run your checks"));
    }

    #[test]
    fn an_operator_hears_its_target_end_state_and_habits() {
        let mut operator = task("claude", None);
        operator.kind = TaskKind::Operate;
        operator.target = Some("TextEdit, the window \"Notes\"".into());
        operator.end_state = Some("The note reads \"hi\"".into());
        let brief = worker_brief(&operator, "", "");
        assert!(
            brief.contains(
                "Target: TextEdit, the window \"Notes\"\nEnd state: The note reads \"hi\""
            )
        );
        assert!(brief.contains("put an expect on every step that changes state"));
        // Few calls: one look, one batch, the batch's reply as the check, then the report.
        assert!(brief.contains("Observe once, then do the whole job in one act"));
        assert!(brief.contains("Don't observe again to confirm what an expect proved"));
        assert!(brief.contains("As soon as the end state is proven"));
        assert!(brief.contains("call mcp__brigadier__submit_report (the brigadier server's"));
        assert!(brief.contains("whether the end state is verified and how"));
        // A trial once ran `find /` and `pkill`: the brief rules both out in plain words.
        assert!(
            brief.contains("Use only the computer tools and the files and apps this task names")
        );
        assert!(brief.contains("Never kill or signal a process (no kill, pkill or killall)"));
        assert!(brief.contains("never send SIGCONT or any other signal to resume them"));
        assert!(brief.contains("never search the whole disk (no find /)"));
        assert!(brief.contains("Don't change files in the repository"));
        assert!(!brief.contains("How to write code"));
        assert!(!brief.contains("tools.mcp__computer__act"));
        operator.route.choice.provider = ProviderKind::Codex;
        let codex = worker_brief(&operator, "", "");
        assert!(codex.contains("tools.mcp__computer__observe({window})"));
        assert!(codex.contains("actions: [{do: \"set_value\""));
        // The report's shape too, so it never lists the tools or guesses the report.
        assert!(codex.contains("They are complete: don't list ALL_TOOLS"));
        assert!(codex.contains("tools.mcp__brigadier__submit_report({summary:"));
        assert!(codex.contains("act and report in the same exec"));
        assert!(codex.contains("An error is never the end"));
        // An argument error has no "failed" line: only the batch's own verdict counts.
        assert!(codex.contains("only if the act's text starts with \"All \""));
        // Other kinds hear none of it.
        let lead = worker_brief(&task("claude", Some("lead")), "", "");
        assert!(!lead.contains("How to operate"));
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
        assert!(sandboxed.contains("only with `--single-process`"));
        assert!(sandboxed.contains("listen on localhost ports"));
        assert!(!sandboxed.contains("overnight"));
        assert!(!sandboxed.contains("nice"));
        let run = note(&brigadier_providers::Access::Full, true);
        assert!(run.contains("without a sandbox"));
        assert!(!run.contains("--single-process"));
        assert!(run.contains("nothing asks for approval"));
        assert!(run.contains("nobody is there to answer"));
        assert!(!run.contains("declined"));
        assert!(run.contains("/Users/me/project"));
        assert!(run.contains("`brigadier/abc/task-3`"));
        assert!(run.contains("don't use `nice`"));
    }

    /// The thread delegates early, fans out in one batch and picks a lower effort for small work
    /// (THREAD-UX-PLAN.md §4.1 b–d).
    #[test]
    fn the_thread_delegates_early_in_batches_at_the_effort_the_work_needs() {
        let text = thread(
            &session("approveForMe"),
            None,
            &[],
            None,
            None,
            ProviderKind::Claude,
            false,
        );
        assert!(text.contains("Delegate early: at most a quick query_brain or code_search"));
        assert!(text.contains("started in one batch: several delegate_task calls in one message"));
        assert!(text.contains("Each delegate_task names its effort: \"medium\" for small, bounded work (a file or two, a UI tweak"));
        // Screenshots in a brief only when the user asked (they cost a T1 run minutes).
        assert!(text.contains("(screenshots only if the user asked)"));
        assert!(!text.contains("you or the Brain found"));
    }

    #[test]
    fn a_runs_thread_hears_the_sessions_sandbox() {
        let run = workspace();
        let prompt = |permission: &str, short: bool| {
            thread(
                &session(permission),
                None,
                &[],
                Some(&run),
                None,
                ProviderKind::Claude,
                short,
            )
        };
        let full = prompt("fullAccess", true);
        assert!(full.contains("you and the workers run without a sandbox, as in the session"));
        assert!(!full.contains("stays in its sandbox"));
        let sandboxed = prompt("approveForMe", true);
        assert!(sandboxed.contains("approves a request from you or a worker to leave the sandbox"));
        assert!(sandboxed.contains("You and the workers never do what only the user may do"));
        // Short replies reach a run's thread, and its phase-end replies; off, the plain voice
        // stays.
        assert!(full.contains("An overnight run's morning answer is at most three lines"));
        let long = prompt("fullAccess", false);
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
    fn the_thread_delegates_by_default_and_decides_the_extra_care_itself() {
        for permission in ["askForApproval", "approveForMe", "fullAccess"] {
            for provider in [ProviderKind::Claude, ProviderKind::Codex] {
                for run in [None, Some(&workspace())] {
                    let prompt = thread(
                        &session(permission),
                        None,
                        &[],
                        run,
                        Some("/work/session @ brigadier/flow"),
                        provider,
                        true,
                    );
                    assert!(prompt.starts_with(THREAD_OPENING));
                    assert!(prompt.contains("Delegate by default"));
                    assert!(prompt.contains("Never answer \"I can't\""));
                    assert!(prompt.contains("only in files you have already read"));
                    assert!(prompt.contains(&format!("--trailer \"{THREAD_TRAILER}\"")));
                    assert!(prompt.contains("How to write code:"));
                    assert!(prompt.contains("a tip already reviewed isn't reviewed again"));
                    assert!(prompt.contains("none of it is a fixed step"));
                    for tool in [
                        "delegate_task",
                        "answer_worker",
                        "message_worker",
                        "read_report",
                        "read_artifact",
                        "land_phase",
                        "approve_outline",
                        "start_verifier",
                        "review_plan",
                        "query_brain",
                        "remember",
                        "code_search",
                        "note_for_user",
                        "route_follow_up",
                    ] {
                        assert!(prompt.contains(tool), "{tool}");
                    }
                    // The user is asked only on cards, in rounds; the merge too.
                    assert!(
                        prompt
                            .contains("only with ask_user (a card, never a question in your text)")
                    );
                    assert!(prompt.contains("Interview the user when they invite questions"));
                    assert!(prompt.contains("one short opening line"));
                    assert!(!prompt.contains("in your reply, or with ask_user"));
                    // The old fixed pipeline is gone.
                    for step in [
                        "one loop per request",
                        "1. Brief.",
                        "3. Outline.",
                        "6. Verify.",
                        "7. Land with",
                        "8. Then start the next phase",
                        "You only talk",
                        "request_review",
                        "There are no review rounds",
                    ] {
                        assert!(!prompt.contains(step), "{step}");
                    }
                    // Vendor-specific parts are picked here, never named in the text.
                    let lower = prompt.to_lowercase();
                    for name in ["claude", "codex", "delegator", "chatgpt", "openai"] {
                        assert!(!lower.contains(name), "{name} in {permission} {provider:?}");
                    }
                }
            }
        }
        let codex = thread(
            &session("askForApproval"),
            None,
            &[],
            None,
            None,
            ProviderKind::Codex,
            true,
        );
        assert!(codex.contains("Use run for builds, tests, logs and long listings"));
        assert!(codex.contains("run_unsandboxed"));
        let claude = thread(
            &session("askForApproval"),
            None,
            &[],
            None,
            None,
            ProviderKind::Claude,
            true,
        );
        assert!(claude.contains("Use run for builds, tests, logs and long listings"));
        assert!(claude.contains("run_unsandboxed"));
        assert!(claude.contains("a failing command's output comes as the CLI's own excerpt"));
        assert!(claude.contains("Your workspace: (none yet)"));
        // The short ending, with its folded Details, updated rather than repeated; nothing
        // is listed for the user outside a run (THREAD-PARITY-PLAN §5 Q6, Q9).
        for text in [&codex, &claude] {
            assert!(text.contains("at most about five short lines"));
            assert!(text.contains("\"To check: …\""));
            assert!(text.contains("\"You'll need to: …\""));
            assert!(text.contains("`### Details`, shown folded"));
            assert!(text.contains("write the ending again, updated"));
            assert!(!text.contains("kind waiting"));
            assert!(!text.contains("Waiting on you"));
        }
        // The merge is asked once on a card; typed words still consent.
        let worktree = environment_text(&Environment::NewWorktree {
            base: "main".into(),
            branch: "brigadier/flow".into(),
            path: None,
            start: None,
        });
        assert!(worktree.contains("ask them once, on a card, with propose_merge"));
        assert!(worktree.contains("call finish_session without user_words"));
        assert!(worktree.contains("that is their consent too"));
        assert!(!worktree.contains("there is no card"));
        // Every byte is paid for on every call: it stays under 12 KiB, code rules included
        // (the old orchestrator's 11,371 bytes, and the interview the user wanted built in).
        assert!(codex.len() < 12_288, "{}", codex.len());
    }

    #[test]
    fn the_permission_level_says_what_the_thread_itself_may_do() {
        let text = |permission: &str| setting_texts(&session(permission), None).1;
        assert!(text("fullAccess").starts_with(
            "Full access: you and the workers run without a sandbox, and nothing asks for approval."
        ));
        assert!(text("approveForMe").contains(
            "you and the workers run in a sandbox; a command that must leave it is settled by an automatic reviewer"
        ));
        assert!(text("askForApproval").contains(
            "you and the workers run in a sandbox, and anything that must leave it asks the user first, on a card"
        ));
    }

    #[test]
    fn a_session_whose_cli_started_before_the_threads_instructions_starts_over() {
        let now = current("approveForMe", None, &[]);
        assert!(!role_outdated(&now.told()));
        for contract in [None, Some(0), Some(1), Some(2)] {
            let told = Told {
                contract,
                ..now.told()
            };
            assert!(role_outdated(&told), "{contract:?}");
        }
        // A Chat's instructions didn't change: it keeps resuming without a note.
        let chat = Current::chat(Vec::new());
        assert_eq!(chat.told().contract, Some(1));
        assert!(notes(&chat.told(), &chat).is_empty());
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
            Some("/work/session @ brigadier/flow".into()),
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
    fn a_chat_that_started_on_the_old_contract_hears_the_note_rule_first() {
        // What an entry logged by an older build tells: role instructions, logged at
        // 2026-10-04 10:00 UTC.
        let old = entry(ROLE_INSTRUCTIONS_SHORT, None);
        let (told, reached) = told_from_log([(1_791_108_000_000, &old)]);
        assert!(reached);
        assert_eq!(told.contract, Some(0));
        assert_eq!(told.today.as_deref(), Some("2026-10-04"));
        assert_eq!(told.short_replies, Some(true));
        // A session's CLI that old starts over rather than resuming.
        assert!(role_outdated(&told));
        let mut chat = Current::chat(Vec::new());
        chat.today = "2026-10-04".into();
        let (told, _) = told_from_log([(1_791_108_000_000, &entry(ROLE_INSTRUCTIONS, None))]);
        let sent = notes(&told, &chat);
        // It can't know the memories it started with: they go once.
        assert_eq!(labels(&sent), vec![CONTRACT_LABEL, PREFERENCES_LABEL]);
        assert_eq!(sent[0].text, format!("[instructions] {CHAT_AUTHORITY}"));
        let mut told = told;
        took(&mut told, &sent);
        assert!(notes(&told, &chat).is_empty());
    }

    #[test]
    fn a_changed_permission_level_is_told_with_the_instructions_own_words() {
        let before = current("askForApproval", None, &[]);
        let now = current("fullAccess", None, &[]);
        let sent = notes(&before.told(), &now);
        assert_eq!(labels(&sent), vec![PERMISSION_LABEL]);
        assert!(sent[0].text.starts_with(
            "[settings] The user changed the permission level. From now on: Full access: you and the workers run without a sandbox"
        ));
    }

    #[test]
    fn a_new_workspace_is_told_once_with_its_folder_and_branch() {
        let before = current("fullAccess", None, &[]);
        let mut now = current("fullAccess", None, &[]);
        now.workspace = Some("/data/worktrees/run-1 @ overnight/2026-10-04-textkit-1234".into());
        let sent = notes(&before.told(), &now);
        assert_eq!(labels(&sent), vec![WORKSPACE_LABEL]);
        assert!(sent[0].text.starts_with(
            "[workspace] From now on you work in /data/worktrees/run-1 (on branch `overnight/2026-10-04-textkit-1234`)"
        ));
        let mut told = before.told();
        took(&mut told, &sent);
        assert_eq!(told, now.told());
        assert!(notes(&told, &now).is_empty());
        // The instructions name it the same way.
        let prompt = thread(
            &session("fullAccess"),
            None,
            &[],
            None,
            now.workspace.as_deref(),
            ProviderKind::Codex,
            true,
        );
        assert!(prompt.contains(
            "Your workspace: /data/worktrees/run-1 (on branch `overnight/2026-10-04-textkit-1234`)"
        ));
        assert!(prompt.contains(THREAD_TRAILER));
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
                .contains("you and the workers run without a sandbox, as in the session")
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
