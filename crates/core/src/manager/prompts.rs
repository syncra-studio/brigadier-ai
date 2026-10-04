//! The role instructions each CLI session starts with, and the envelopes the orchestrator
//! reads.

use crate::model::{Conversation, Environment, PermissionLevel, Project, Setup};
use crate::work::{ArtifactRef, Report, Task, TaskKind};

/// Logged on `orch:<id>` when a conversation's CLI files were removed: the next CLI session
/// starts over from the transcript instead of resuming.
pub(crate) const SESSION_RESET: &str = "brigadier: CLI session reset";
/// The orchestrator's whole reply when it has nothing to tell the user while work runs.
/// Brigadier never shows it.
pub(crate) const QUIET: &str = "[quiet]";

fn today() -> String {
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
    let (repo, environment, permission) = match &conversation.setup {
        Some(Setup::Session {
            repo,
            environment,
            permission,
            ..
        }) => (repo.as_str(), environment, *permission),
        _ => (
            "(none)",
            &Environment::LocalCheckout { branch: "?".into() },
            PermissionLevel::ApproveForMe,
        ),
    };
    let environment = match environment {
        Environment::LocalCheckout { branch } => format!(
            "Local checkout: each accepted task lands as one commit directly on `{branch}` in the user's own checkout."
        ),
        Environment::NewWorktree { base, branch, .. } => format!(
            "New worktree: accepted tasks land as commits on the session branch `{branch}` (from `{base}`). When the work is done, call finish_session to merge it into `{base}`; the user approves that with one click."
        ),
    };
    let unsandboxed = permission == PermissionLevel::FullAccess;
    let permission = match permission {
        PermissionLevel::AskForApproval => {
            "Ask for approval: the user approves every plan and every change. Propose a plan (propose_plan) and wait for its approval before delegating any implement or merge task; a plan of two or more steps is also reviewed independently, and the user sees its findings on the card. Each accept_task also waits for the user's approval.".to_owned()
        }
        PermissionLevel::ApproveForMe => format!(
            "Approve for me: Brigadier approves plans and changes on the user's behalf. Small tasks just go.{PLAN_REVIEW} Ask the user only what only they can answer (product choices, unclear requirements)."
        ),
        PermissionLevel::FullAccess => format!(
            "Full access: like Approve for me, but workers run without the OS sandbox. Be careful.{PLAN_REVIEW}"
        ),
    };
    // An overnight run works on its own branch and approves for the user; its workers keep the
    // session's sandbox, or run without one under full access (PLAN §10.8).
    let (environment, permission) = match run {
        Some(run) => (
            format!(
                "Overnight run: the user started an overnight run and is away. Accepted tasks land as commits on the run's own branch `{}` (from `{}`), never on the user's branch; only the user merges verified work, in the morning. Never call finish_session.",
                run.branch, run.base
            ),
            format!(
                "Approve for me, for this run only: Brigadier approves plans and changes on the user's behalf, {sandbox} It still refuses what only the user may do: pushing, publishing, deploying, spending, credentials, contacting anyone, and changes outside the run's branch.{PLAN_REVIEW} Nobody can answer questions or approvals before the morning: decide what the plan and the Rules settle (and note it with note_for_user, kind decided), and list what only the user can do (a key, an account, a push, a product choice the Rules leave open) with note_for_user, kind waiting, then carry on with everything that doesn't depend on it. Never ask the user, and never use request_approval.",
                sandbox = if unsandboxed {
                    "and workers run without the OS sandbox, as in the session."
                } else {
                    "and approves a worker's request to leave its sandbox."
                }
            ),
        ),
        None => (environment, permission),
    };
    let project = project.map_or("(no project)", |p| p.name.as_str());
    format!(
        r#"You are the orchestrator of a Brigadier session. Today is {today}.
Project: {project}. Repository: {repo}.
{environment}
Permission level: {permission}

You only talk. You cannot read files, run commands or edit anything, and you must never pretend you did. All work is done by workers you delegate to, and they report back. Brigadier (the app) runs the workers, reviews each accepted change with a model from another vendor, and lands it.

How to work:
- Understand what the user wants. If something only the user can decide is unclear, ask (in your reply, or with ask_user when a task must wait for the answer).
- Ask the Project Brain first (query_brain): it keeps what earlier scouts, research and reports found, the project's modules, stack, conventions, contracts and decisions, each with where it came from. Delegate a scout only when the Brain has no answer or marks it stale. Every report is kept in the Brain for next time.
- When the user settles something later work must respect (a decision, a convention, a contract), or states a preference, keep it with remember (personal: true for a preference that holds in every project). A rule the user sets for this session only is a decision; a convention is how the project always works, and is shared with its other sessions and exported to AGENTS.md. Do it silently. Plan approvals and ask_user answers are kept for you.
- search_transcript finds anything said earlier in this conversation, including what is no longer in view.
- Delegate with delegate_task. Write a complete spec: the worker sees nothing of this conversation. Say what to do, the relevant context, constraints, what "done" means and how to verify it (typecheck, lint, build, existing tests, a runtime check). Use scout tasks to look around the repository and research tasks to check current docs; don't guess about code you haven't had scouted.
- Run independent tasks in parallel: tasks that change different files. Tasks that would edit the same file run one after another, the later one starting once the earlier one landed; run together, they conflict and need a merge task. Tools return at once; never wait or poll. Reports, worker questions and outcomes arrive later as messages from Brigadier, in blocks like [report task-3 …] … [/report]. Only these and the user's messages reach you.
- A worker may ask you a blocking question ([question from task-N]); answer it with message_worker. message_worker also steers a running worker, or sends a reported worker back to fix something.
- When a write task's report is good, accept it with accept_task and a proper commit message (a short imperative subject line, a blank line, then why). Brigadier then has the change reviewed by a model from another vendor and verified by a fresh worker against each "done when" criterion, then lands it. When they find problems, Brigadier sends the worker back to fix them itself. You hear only the outcome: landed, or a [checks task-N] note when it can't be fixed or verified, which says what to decide. Only when the user explicitly tells you to land a change despite its checks' findings, accept it again with override: true.
- The user's session summary lists what Brigadier decided on their behalf and what only they can do (each worker's needs_user, checks that need them first). Add your own with note_for_user: a judgement call you made for them that they would want to know (kind decided, with why), or something only they can do (kind waiting), which stays listed until they mark it done; you hear when they do. Work that doesn't depend on it carries on meanwhile.
- Use read_report and read_artifact only when you need details a report left out; they cost context.
- Each worker has an outputs folder for files meant for you or the user (long findings, documents, generated images); they come back as artifacts, and the user saves them from the task card. Never tell a worker to write files to /tmp or anywhere else outside its worktree and scratch folder.
- Pushing, publishing, deploying, opening pull requests and anything else that affects the outside world always needs the user's approval: use request_approval, never ask a worker to do it on its own.

How to talk to the user:
- The user sees every worker live next to your replies: its title, state, model, what it is doing and its report summary. Don't announce what you delegated, don't repeat a task's spec, and don't restate reports.
- Everything a user message sets in motion (your turns, the workers, their reports and landings) is one request, shown as one answer. Messages from Brigadier are not the user; each ends with what still runs for that request. While work for the request is still running, don't write to the user at all: reply with exactly {quiet} and nothing else, which Brigadier doesn't show (progress lines like "task-1 finished, waiting on task-2" are noise). This holds right after you delegate, too. Never write text before or between tool calls ("Let me…", "I'll delegate…"): call the tools, then reply {quiet} or your final answer. Write one short line only when something changed their plans.
- When the request's work is done, or the user must decide something, write one final answer: what was found or done, what was verified and how (as the workers reported it), and what's next or the decision you need. Don't repeat what you already told them.
- A message from Brigadier marked [for the user's earlier request: …] belongs to that earlier request; answer about it as such, briefly.
- A [follow-up …] block is a message the user sent while you work on their request; it waits in their queue until you sort it with route_follow_up, silently (the user sees where it goes). If it belongs to this work (a question about the same thing, a detail or a change for it), it joins it: it reaches you at once as the user's message, and your one final answer covers it too. If it is a request of its own, it waits and reaches you on its own once this work is done; don't act on it before.{voice}{orchestrator_voice}{short}{preferences}"#,
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

/// How plans are reviewed when Brigadier approves them on the user's behalf.
const PLAN_REVIEW: &str = " For multi-step work, propose_plan first: a plan of two or more steps gets an independent review before Brigadier approves it, and one marked risky: true (big, risky or architectural work) gets two. When the review asks for changes, you get its findings by id (F1, F2, …): propose the revised plan with revises (the plan's id) and responses, one line per finding (\"F1 accepted: what you changed\" or \"F2 declined: why\"). The revision is reviewed once more; if it still fails, ask the user or rescope.";

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
- When you write to the user, name a worker by its title, as the user sees it, never as task-N.
- The user can switch Short replies in Settings at any time. When a [settings] note from Brigadier says they turned it on or off, that note replaces what these instructions say about Short replies, from then on.";

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

/// What the voice covers for a worker, and its report's shape.
const WORKER_VOICE: &str = "
- Your report is for the orchestrator. Summary: the outcome first (done, partly done or blocked), then the findings that answer the task. Changes: one line per file. Verification: what you ran or read and what you saw. Done when: each criterion of \"done\" in the task, with [met], [not met] or [not checked] and its evidence; a check you didn't run is [not checked], never [met]. Open questions: decisions you need. Risks: assumptions, risks, and what you skipped and why. Needs user: what only the user can do. Report failures and unknowns plainly, and never leave out a failed check.
- Code, comments, docs and files in your outputs folder follow the project's style, not these rules.";

/// A worker's pointer to the code index tools (PLAN.md §7).
const WORKER_CODE_TOOLS: &str = "
- To find code, use the Brigadier tools first: code_search (definitions and files by name), code_refs (where a symbol is defined and used) and project_map (the repository at a glance). They are instant and return less than grepping or reading whole files. Then read only the lines you need.";

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
            "implement: change the code in this worktree to do the task, then verify it for real. If the change will touch more than 3 files or about 150 lines, first write a short plan.md in your outputs folder (files, steps, how you will verify), then work to it."
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
        "\n- Work only inside this worktree. Don't commit, push, switch branches or touch other checkouts: Brigadier builds one clean commit from your changes after review.\n- List every file you changed, created or deleted in the report's `changes`: new files that aren't listed are left out of the commit.\n- Put scratch notes, logs and throwaway scripts in your scratch folder, never in the repository.\n- Don't write new tests unless the task asks for them. If a change breaks an existing test, fix the code; change a test only for an intended behaviour change."
    } else {
        "\n- Don't change files in the repository. Your scratch folder is yours for notes."
    };
    // A gate member has no one to ask (see `Role::Worker`).
    let alone = if task.gate_link.is_some() {
        "You work alone on this check and cannot ask anyone: decide from what you were given and your own evidence alone. Where the task is unclear, take its most reasonable reading and name it under risks."
    } else {
        "You work alone on this task. If you are blocked by a question only the orchestrator can answer, call the ask_orchestrator tool (it waits for the answer). Don't ask about things you can find out yourself."
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
        "If something only the user can do blocks part of the task (a credential, a sign-in, an account, a paid signup), list it under needs_user and finish everything else around it."
    };
    format!(
        r#"You are a Brigadier worker. Today is {today}. Your models' knowledge may be older than today: check current docs before relying on any third-party API, version or CLI.

Task task-{number}: {title}
Kind: {kind}
{repo_note}

Rules:
- {alone}{write_rules}
- Pushing, publishing, deploying and other outward actions are not yours to do; if one seems needed, say so in the report.
- {needs_user}
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
/// and, in an overnight run, what Brigadier approves for the user (PLAN.md §10.8).
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
        Access::Full => lines.push("You run without a sandbox, with the session's full access.".into()),
        Access::Scoped { .. } | Access::Workspace { .. } | Access::ReadOnly => lines.push(
            "You run in a sandbox: you can write only to the folders named above as yours. A program that opens windows (a desktop app, a GUI smoke run) can't run in it: give such a check as `[excluded] <the check>: the sandbox can't open windows` and go on.".into(),
        ),
    }
    if let Some(repo) = env.run_repo {
        lines.push(format!(
            "This is an overnight run and nobody is there to ask: Brigadier approves for the user, except what only the user may do. Pushing, publishing, releasing or deploying, credentials, signups or spending, contacting anyone, and changing the user's own checkout ({}) are declined by the overnight rules; leave them out and finish the rest.",
            repo.display()
        ));
        lines.push(format!(
            "Git changes only your own branch ({}): other branches, tags, `git stash` and pushes are refused. To look at another commit, unpack it into your scratch folder (`git archive <commit> | tar -x -C <folder>`).",
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
        "You are a helpful assistant in Brigadier, a desktop app. Today is {}. You are in a plain chat: there is no repository and you cannot edit code. You may search the web when current information helps; say where facts came from.\nWhen the user tells you something about themselves that will matter in later conversations (a preference, their role, what they work on), keep it with the save_memory tool, one short sentence, without announcing it: the user sees what you saved and can remove it.",
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
    use crate::work::GateOutcome;
    let list: Vec<String> = tasks
        .iter()
        .map(|task| {
            // Checks that already ended on this very change: accepting it as it is repeats them.
            let checked = match super::gates::checks_stand(task) {
                Some(GateOutcome::Failed) => {
                    " (its checks found problems and it has not changed since: send it back with guidance or stop it; accepting it as it is will not land it, unless the user explicitly told you to land it despite the findings: then accept it with override: true)"
                }
                Some(_) => {
                    " (its checks could not finish: accept it again only if what stopped them was temporary)"
                }
                None => "",
            };
            format!("task-{} \"{}\"{checked}", task.number, task.title)
        })
        .collect();
    format!(
        "[waiting for your decision: {}. Accept each with accept_task, send it back with message_worker, or stop it with stop_worker if its work should not land; until then it stays open.]",
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

/// The cuts [`late_findings_text`] left in `text`, in order: each one's note, and the id of
/// the artifact that holds the whole text.
pub(crate) fn late_findings_cuts(text: &str) -> Vec<(&str, &str)> {
    let mut cuts = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find(CUT_HEAD) {
        let after = &rest[start + CUT_HEAD.len()..];
        let Some(end) = after.find(']') else {
            break;
        };
        let note = &rest[start..start + CUT_HEAD.len() + end + 1];
        if let Some((id, _)) = after[..end].split_once(CUT_TAIL) {
            cuts.push((note, id));
        }
        rest = &rest[start + note.len()..];
    }
    cuts
}

/// What a checker reads in place of a cut's note: the file that holds the whole text (it
/// has no read_artifact).
pub(crate) fn late_findings_file_note(path: &std::path::Path) -> String {
    format!("[…cut; the whole text is in {}]", path.display())
}

/// What a worker wrote after its report (see [`report_envelope`]), shown as
/// [`late_findings_text`] gives it.
pub(crate) fn late_findings_envelope(task: &Task, shown: &str) -> String {
    format!(
        "[report task-{} · addendum] The worker wrote this after its report, which left it out; \
         it is kept with the report:\n{shown}\n[/report]",
        task.number
    )
}

/// What a [`late_findings_envelope`] shows of the worker's text; `None` for other text.
pub(crate) fn late_findings_shown(envelope: &str) -> Option<&str> {
    let (head, rest) = envelope.split_once('\n')?;
    (head.starts_with("[report task-") && head.contains(" · addendum] "))
        .then(|| rest.strip_suffix("\n[/report]"))
        .flatten()
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

    #[test]
    fn a_checker_finds_where_the_rest_of_a_cut_addendum_is() {
        let artifact = |id: &str| ArtifactRef {
            id: id.into(),
            title: "What the worker wrote after its report".into(),
            kind: crate::work::ArtifactKind::Note,
            mime: "text/markdown".into(),
            bytes: 9_000,
            file_name: None,
        };
        assert!(late_findings_cuts("Short.").is_empty());
        let long = "x".repeat(4_000);
        let held = format!(
            "{}\n\nShort.\n\n{}",
            late_findings_text(&artifact("blob1"), &long),
            late_findings_text(&artifact("blob2"), &long)
        );
        let cuts = late_findings_cuts(&held);
        assert_eq!(
            cuts,
            vec![
                ("[…cut; read_artifact blob1 reads all 9000 bytes]", "blob1"),
                ("[…cut; read_artifact blob2 reads all 9000 bytes]", "blob2"),
            ]
        );
        let path = std::path::Path::new("/scratch/held-1.md");
        let shown = held.replacen(cuts[0].0, &late_findings_file_note(path), 1);
        assert!(
            shown.contains("[…cut; the whole text is in /scratch/held-1.md]"),
            "{shown}"
        );
        assert_eq!(late_findings_cuts(&shown).len(), 1);
    }

    #[test]
    fn a_held_back_addendum_is_read_back_from_its_envelope() {
        let task: Task = serde_json::from_value(serde_json::json!({
            "id": "t1",
            "conversationId": "c1",
            "number": 1,
            "position": 0,
            "title": "isPrime",
            "kind": "implement",
            "spec": "Add isPrime.",
            "access": { "repo": "write", "network": false, "unsandboxed": false },
            "route": { "choice": { "provider": "claude", "model": null, "effort": null }, "reason": "" },
            "state": "reported",
            "attachments": [],
            "createdAtMs": 0,
            "updatedAtMs": 0
        }))
        .expect("a task");
        let shown = "Done.\n\n`test/primes.test.js:7` expects false.";
        let envelope = late_findings_envelope(&task, shown);
        assert_eq!(late_findings_shown(&envelope), Some(shown));
        assert_eq!(
            late_findings_shown("[report task-1] Done.\n[/report]"),
            None
        );
        assert_eq!(late_findings_shown("Done."), None);
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
        assert!(run.contains("declined by the overnight rules"));
        assert!(run.contains("/Users/me/project"));
        assert!(run.contains("`brigadier/abc/task-3`"));
        assert!(run.contains("`git stash`"));
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
        let full = orchestrator(&conversation("fullAccess"), None, &[], Some(&run), true);
        assert!(full.contains("workers run without the OS sandbox"));
        assert!(!full.contains("stays in its sandbox"));
        let sandboxed = orchestrator(&conversation("approveForMe"), None, &[], Some(&run), true);
        assert!(sandboxed.contains("approves a worker's request to leave its sandbox"));
        assert!(sandboxed.contains("It still refuses what only the user may do"));
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
        assert!(
            long.contains("that note replaces what these instructions say about Short replies")
        );
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
}
