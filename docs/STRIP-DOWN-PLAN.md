# Strip-down plan: every Brigadier session is a plain Claude Code or Codex CLI

> **Take, don't rebuild.** We take Claude Code's and Codex's own conversation, session files, subagents,
> background tasks, permission modes and memory files as they are. Brigadier rebuilds none of them. It adds only
> what a CLI can't do by itself: seeing and driving other sessions, previews, the browser, computer use, asking
> the user as a card, and searching the record. This plan only removes and re-wires.

> Status: plan, 2026-10-22, from the user's decisions G1–G15 (project memory `plain-cli-threads-1022`), the
> 10-22 diagnosis (run `20261022-0805-take-over-5-sessions`, `DIAGNOSIS.md`) and the long-sessions research
> (same run, `research/PROPOSAL.md`, `QUESTIONS.md`, `EVIDENCE.md`). The decisions are settled. Where one looks
> hard, §8 says so with evidence; nothing here changes one.
> Base: origin/main `ae56e9271`. Paths: `M/` = `crates/core/src/manager/`, `P/` = `crates/providers/src/`,
> `D/` = `apps/desktop/src/`. File:line references are to `ae56e9271`. Line counts are rough (`wc -l`, tests
> included unless said).

## 0. Why

On 10-22 all five sessions got stuck (DIAGNOSIS.md). None of the causes was the model. They were all engine
machinery wrapped around it:

- A merge gate (`run_checks` in Brigadier's sandbox) that could not pass for 11 hours. Four "merge anyway?" asks
  reached the user.
- A machine-wide build lease. It queued 2-second checks for 5–10 minutes, about 77 minutes in 95.
- Engine-started turns. In S4, 64 of 66 turns came from engine events, and 24 of them said only `[quiet]`. That
  was 22% of the session's tokens.
- Briefings in place of context. The R5 review findings vanished into a session that died in 387 ms.
- Cards computed from guesses: "Ready" while the thread asked a question, and a red ✗ on "Merged, pushed, CI
  passes".

The user's ask: Brigadier sessions should work exactly like plain Claude Code and Codex conversations.

## 1. The target, on one page

**A session is a plain CLI.** Each tab runs one Claude Code (`claude -p` stream-json) or Codex (`codex
app-server`) session, in the project's worktree or its Local checkout. The session does the work itself. It
runs checks (`tools/full-checks.sh`), reviews (`codex review`), `git merge`, `git push` and `gh` as plain
commands. They show as ordinary tool rows with their real output and exit code. Its subagents, background tasks,
permission prompts and memory files are the CLI's own.

**Brigadier pushes nothing (G1).** A thread receives only:
- the user's messages;
- results of things it started itself, which the CLI already delivers;
- one message per seam: "write your hand-off" (G12).

There are no status nags, no cross-session notices, no mid-task rule changes, no `[quiet]` and no `[next:]`.

**The UI reads the CLI and git, never the thread's tool calls (G2, G11).**

- **Thread.** The thread is the CLI's own session file: Claude JSONL, or the Codex rollout. While the CLI runs,
  the UI follows its live stream. Brigadier keeps no copy of the conversation. The UI shows the same items in
  the same order as the CLI. It may draw them more nicely, but it never shows different data, and it never
  hides or adds anything.
- **Work block.** A turn runs from the user's message to the CLI's turn-end event. Its steps fold into "Worked
  for Xm", timed from the CLI's own timestamps. The turn's last assistant text shows outside the fold. Stopped
  and failed come from the CLI's result event.
- **Long sessions.** One thread is the chain of its lineage's CLI session files.

**Brigadier MCP is pull-only (G3, G6).** The thread calls these tools when it wants to:
- **Read anything:** sessions, tabs, states, the workers it started.
- **Session tools equal to `dlg`:** start, send, wait for events of the sessions it started, read the last
  message or screen, close.
- **Previews, browser, computer use.**
- **Ask the user as a card.**
- **Brain search** over the project's records (G14).

Every checks, review, land and headline tool goes.

**Teams = the Delegator pattern, built in (G3).** A plain, editable skill plus the session tools. Workers are
ordinary sessions in the sidebar. Overnight is the same skill with a deadline. The engine has no lead, phase or
landing protocol.

**Permissions pass straight through (G5).** The composer offers Ask, Accept edits and Full access. These map to
Claude Code's and Codex's own modes. The CLI's own permission prompts show as cards. Brigadier runs no sandbox
of its own.

**Context card (G9, G10).** It has these rows:
- **Project and branch**, with +/− from git.
- **PR row:** `#N`, a state word and a colour, from `gh`. It has no actions.
- **Waiting on you:** only real pending CLI permission prompts, ask-user questions and MCP sign-ins of the
  sessions this one started. This session's own question shows only above the composer.
- **Workers:** title, state from their CLI events, +/− from their worktree's git, and "N done · M stopped".
- **Background:** the CLI's own background tasks. The label is the CLI's description, with elapsed time. These
  rows behave exactly like Claude Code's own.
- **Sources:** attachments first, then fetched URLs, then MCP servers used; + adds one.

No context-used number shows anywhere. A usage-limit "Resumes at" card is allowed; it reads the reset time from
the CLI's own error. The Job card is gone, overnight included (G7).

**Changes card (G8).** Brigadier takes a git snapshot of the worktree at each turn's start and end. The card
shows the exact diff between the two. Revert restores the start snapshot.

**Long sessions (G12).** The CLI never compacts.
- **Ledger.** The thread keeps `record/ledger.md` as it works. A standing instruction asks for it; nothing
  enforces it (Q3).
- **Nudge.** Past 200k tokens of hot context, Brigadier queues one mid-turn nudge, Delegator style. 200k is an
  env default, not a cap (Q1, Q2). The thread updates the ledger and writes a hand-off note.
- **Successor.** A fresh CLI session continues in the same tab, after a visible divider: "Fresh context ·
  hand-off note", and the note opens from it. Its bootstrap is the first lines of its first user message, also
  saved to disk (Q4). The successor may search every earlier file of the lineage (Q5).
- **Codex.** Codex's forced compaction at 90% is a known limit. The divider shows when it happens (Q0).

**Brain (G14).** The Brain is a pull-only search over the project's records:
- every session's ledger;
- the CLI transcripts;
- hand-off notes;
- "learned from reading" pointers.

Briefings, context packs, the Brain-first rule, the rebirth Brain budget, engine-written learned nodes and
enrichment jobs all go. The code index stays as an optional tool. The Personal Brain moves into the CLIs' own
memory files.

**Folders (G13, G15).** Each session gets one folder, `~/.brigadier/s/<short-id>/`:

| Subfolder | Holds | Archive | Delete |
|---|---|---|---|
| `record/` | ledger, hand-off notes, `lineage.json`, `seam.json` | keeps | removes |
| `work/` | the worktree | removes once merged | removes |
| `tmp/` | `TMPDIR` points here | removes | removes |
| `dev/` | dev servers and build output | removes | removes |
| `logs/` | logs | removes | removes |

`TMPDIR` points at `tmp/` as a convention, not a guard.
- **Archive** also stops the session's processes, and keeps its Brain entries and lineage transcripts.
- **Delete** removes everything of the session: its folder, its Brain entries, its lineage's CLI transcript
  files, its worktree, and its branch after the existing "not merged" ask.
- **Storage** shows the size of each folder.

Some things stay where they are: the CLIs' own session files, shared caches (`~/.cargo`, the pnpm store) and
cross-session locks.

**Local checkout stays** next to New worktree. New worktree remains the default (built 10-22, `2d72ce20f`).

## 2. What this supersedes

In `docs/PLAN.md`:

- **§1.** "The orchestrator never does work itself", and these Design-goal rows:
  - Context: "Brain + invisible rebirth"
  - Review: "risk-tiered, cross-vendor"
  - Security: "OS sandbox for workers"
  - Routing and Fallback, as far as they route tasks to workers
- **§2.** These principles:
  - 1, "The orchestrator only talks"
  - 2, "Brigadier owns the truth": the CLI's session file is now the truth
  - 4, a verifier per phase
  - 9, as far as it removes CLI session files: Archive keeps them now
- **§3.** The architecture box items Orchestrator runtime, Worker pool, Router, Review engine and "Sandbox".
  Also the line that Brigadier stores the canonical transcript.
- **§5.**
  - **Session:** "Brigadier's git engine performs merges"; workers' temporary worktrees.
  - **Lifecycle:** Archive's and Delete's lists, replaced by G15 (§1 above). Delete no longer keeps what the
    Brain learned. This reverses Q25.
  - **Permission level:** the three levels and their sandbox, replaced by G5.
  - **Cleanup ledger:** removing CLI transcripts on archive.
  - **Task, Worker, Artifact and Personal Brain.**
- **§6.**
  - **Phase 3:** the orchestrator runtime, its tool list, "A phase lands once…", the permission levels and
    worker cards.
  - **Phase 4:** rebirth, briefings, skeleton pass, idle-quota enrichment and Personal Brain.
  - **Phase 5:** mid-task cross-provider hand-off of a task.
  - **Phase 6:** verifier, verification pipeline as an engine step, freshness research scout.
- **§7.** The rebirth and hand-off paragraph. The rules stay: no caps, lossless, no settings, judged per
  completed task.
- **§9.** These rows:
  - Q2, Q5 (mid-task fallback of a task), Q6, Q8, Q13 (Personal Brain), Q15, Q18 (enrichment), Q25 (Delete
    keeps Brain), Q26 and Q27
  - the 2026-10-05 "Delegator-style flow" row
- **§10.** Overnight mode, all of it. Overnight is now the team skill with a deadline.

Other plans lose the parts G1–G15 change. Where they conflict, this plan wins:
- `THREAD-PLAN.md`: engine tools, reviews and the verifier.
- `THREAD-UX-PLAN.md` and `THREAD-PARITY-PLAN.md`: thread items from engine records, and the Job card.
- `SESSION-MESSAGING-PLAN.md`: peer tools become the session tools of S11.
- `WORKSPACES-PLAN.md`: auto-merge.
- `SESSION-FIXES-PLAN.md`: merge gates.
- `docs/research/long-sessions/BUILD-PLAN.md`: rebirth, briefings and the output store.

w03's "Job card only for plans and overnight runs" (`4b1ea4c2a`) is a stopgap until S14.

## 3. Deletion inventory

Each part below goes. The files, rough sizes and closing issues are listed with it. "Closes" means the open
GitHub issue becomes obsolete when that part goes; the step closes it with a note. The total is roughly
**55–65k lines of Rust plus about 4k of TypeScript**, about half of it tests. This is an estimate from the
inventory.

### 3.1 Engine pushes into the thread (G1) → step S1
- `[status due]`: `M/status.rs` (225 lines; text at :134, 30-minute timer at :45–66).
- `[quiet]`:
  - the constant at `M/prompts.rs:214`, and `late_event!` at :397–406;
  - in `M/conversation.rs`, the Quiet filter at :7069–7171, `waits_quietly` at :7277 and the deferral at
    :5536–5560 and :5761.
  - `M/quiet.rs` is unrelated (DB maintenance). It stays.
- Cross-session notices:
  - `M/project_sessions.rs` `tell_overlaps` at :486 ("[project] … also changed") and `tell_merged` at :553;
  - `hold_fact` at `M/conversation.rs:1224`.
- "From now on" rule revisions and `[settings]` notes:
  - in `M/prompts.rs`: `REVISIONS` at :800–1363 (about 560 lines) and `notes()` at :2115–2535;
  - in `M/conversation.rs`: `instruction_notes` at :6612 and `told_notes` at :6677;
  - in `M/workers.rs`: the worker revisions at :1854.
- `[next: …]` hints: `M/prompts.rs:410–420` and `M/conversation.rs:7123–7170`, plus `keep_guess` at :5287.
- Engine-event turns:
  - `deliver`, `kick` and `next_turn` at `M/conversation.rs:2704–3363` shrink to user messages, plus results of
    work the thread started (until S14) and the seam nudge.
  - `M/inbox_store.rs` (443) narrows the same way.
- Tests: `flow/status_tests`, `quiet_wake_tests`, `settings_toggles_tests`, `suggestion_tests`, and parts of
  `project_sessions_tests`, `owed_tests` and `restart_waits_tests`.
- Closes: #520 (with S2).

### 3.2 Check gates, the full-checks sandbox and the build lease (G4, G6) → step S2
- **Tools and checks code:**
  - Tools: `run_checks` (thread and worker) and `run_check`.
  - `M/integrated_checks.rs` (1,778): `full_check_access` at :783 and the checkout at :725.
  - `M/checks.rs` (3,939).
  - `M/test_counts.rs` (321): its one caller, `preview.rs:1158`, loses the count.
- **The full-checks sandbox test machinery** (added by `54eba0403`, #493):
  - `Nested`, `test_platform`'s wrapper and `skip_inside_a_sandbox`, at `M/flow/mod.rs:1233–1302`.
  - The skips at `M/preview.rs:3359`, `M/protection.rs:244/403`, `M/run.rs:753` and `M/workers.rs:6474`.
  - `no_terminal_here()` and its 9 uses in `crates/daemon/src/terminals.rs:868–1344`.
  - `nested_sandbox_refused` at `crates/sandbox/src/macos.rs:946`, and `confined()` (`crates/sandbox/src/lib.rs`,
    `macos.rs`), unless S4 still needs it.
  - The test `the_core_tests_pass_in_the_full_checks_sandbox` at `flow/integrated_checks_tests.rs:478–570`.
  - The CI job **"Core tests in the full checks' sandbox (macOS)"** (`checks-sandbox`, `.github/workflows/ci.yml`
    after the `rust-tests` job), and the header comment that names it.
- **The build lease:**
  - `crates/core/src/machine/builds.rs` (1,282; "One build at a time") and `machine/heavy.rs` (455).
  - The lease rows and held workers in `M/machine.rs` (234).
  - The lease calls in `checks.rs:1212–1250` and `preview.rs:123/899–923/1596–1693`.
  - The heat and memory guard in `machine/mod.rs` stays until S14 decides, because workers still use it.
- Tests: `integrated_checks_tests` (570), `checks_tests` (533), `commit_on_pass_tests` (293), the
  `run_checks` cases in `agent_tools_tests`, the lease rows in `background_tests`, and the inline tests of
  `builds.rs`, `heavy.rs` and `checks.rs`.
- Closes: #514, #515, #479. Re-check #382 and #296 after S14 (most flow tests named in them go).

### 3.3 Reviews, merge gates and "merge anyway" (G4, G6) → step S3
- **Review tools:** `request_review` (thread and worker), `record_findings` and `review_plan`.
  - `M/review_runs.rs` (2,713): the findings ledger, the "Review the work" setting and `auto_merge_on`.
  - `crates/review` (1,160), `M/fusion.rs` (290) and `M/unreviewed.rs` (243).
- **Merge gates:**
  - `merge_rule` at `M/landing.rs:2116–2164` ("needs run_checks passed"), plus `checks_waived` and the consent
    functions at :2170–2508.
  - `M/merge_consent.rs` (130), `M/review_hold.rs` (366), and `M/merge_record.rs` (396, which `pull_request.rs`
    also reads).
  - `MergeSession { anyway }` (`crates/daemon/src/server.rs:757`).
  - `MergeRefused`: `work.rs:1928`, `board.rs:214/613`, `model.rs:1495`, `sessions.rs:1532` and
    `ipc/src/protocol.rs:911`.
- **UI:** the "Merge anyway" rows in `D/app/conversation/Activities.tsx` and `backgroundRows.ts`.
- **Tests:**
  - `held_merge`, `merge_by_user`, `merge_queue`, `auto_merge`, `review_hold`, `review_tiers`, `review_switch`,
    `review_picks`, `findings`, `fusion`, `unreported` and `gate_fixes`;
  - `agent_tools_tests:947–1086`.
- Closes: #519, #518 for its card half. The CI half was fixed by `8bce22754`: check the last Windows Clippy runs
  and close.

### 3.4 Brigadier's sandbox and its permission levels (G5) → step S4
- Sandboxing of CLIs, runs and previews:
  - `providers/src/cli.rs:259–287` wraps every CLI in the "protect" profile, Full access included.
  - Seatbelt `confine`, `protect` and `confine_preview` at `crates/sandbox/src/macos.rs:614–920`, plus the
    matching parts of `linux.rs` and `windows.rs`.
  - `M/protection.rs` (573).
  - The separate confines in `M/run.rs:233/354`, `M/preview.rs:850` and `M/workers.rs:6498`.
- Brigadier's own reviewer for unsafe commands:
  - `M/escalation.rs` (408);
  - the Codex auto-review answering in `P/codex/mod.rs:1149–1288`.
- Tools: `run` and `run_unsandboxed` (`M/run.rs`, 838) and `suggest_full_access`.
- In the Claude `--settings`: the `sandbox` block, `disableBypassPermissionsMode`, `autoMode.hard_deny`, and the
  ask rule on `Bash(dangerouslyDisableSandbox:true)` (`P/claude/mod.rs:656–946`).
- UI: `D/.../SandboxLimitNotice.tsx` (86), and the "Approve for me" wording in `SetupPickers.tsx`.
- The rest of `crates/sandbox` stays: paths, processes, Keychain, login shell and machine.
- Closes: #521.

### 3.5 Phases, outlines, overnight and the Job card (G4, G7) → step S14a
- **Tools:** `plan_phases`, `propose_plan`, `approve_outline`, `start_verifier`, `submit_outline`,
  `propose_overnight`, `settle_step` and `end_run`.
- **Engine:**
  - `M/phases.rs` (1,160).
  - `M/overnight/*` (about 7.6k), `crates/core/src/overnight.rs` (628), and in the daemon
    `overnight_supervisor.rs`, `overnight_notifications.rs` and the overnight bits of `awake.rs` and `idle.rs`.
  - `M/decisions.rs` (1,822), `M/constraints.rs` (476) and `cards.rs` `decide_plan` (:670).
- **Job card, engine side:**
  - `set_headline` at `M/tools.rs:1112–1146`, and `HEADLINE_RULE` at `prompts.rs:465`;
  - `HeadlineSet` (`model.rs:2547`) and `SessionHeadline` (`work.rs:1906`).
- **Job card, UI** (about 1.9k lines):
  - `D/app/conversation/cards/JobCard.tsx`, `JobStack.tsx`, `liveJob.ts`, `boardJob.ts`, `runJob.ts` and
    `job.ts`;
  - `components/conversation/elements/job-progress.tsx` and `app/side-panel/JobTab.tsx`;
  - the plan cards in `cards/` (`PlanCard`, `PlanDocument`, `PlanFrame`, `PhaseWork`, `PhaseMore`, `DoneWhen`,
    `PlanRating` and friends).
- **Tests:** `plan_tests`, `phase_acceptance_tests`, `overnight_tests`, `engine_tests`, `constraints_tests`,
  `decision_index_tests`, `card_tests`, `stop_hook_tests`, and the headline cases in `agent_tools_tests`.
- WaitCard is replaced by the "Resumes at" card read from the CLI's error (S12).

### 3.6 Workers, routing and the landing engine (G3, G4) → steps S14b, S14c
- **Worker tools:** `delegate_task`, `message_worker`, `answer_worker`, `stop_worker`, `rerun_task`,
  `route_follow_up`, `list_tasks`, `read_report`, `read_artifact`, `store_output`, `ask_thread`, `note_progress`,
  `submit_report` and `attach_proof`.
- **Landing and CI tools:** `land_phase`, `finish_session`, `stop_merge`, `ci_status`, `watch_ci`, `clean_up` and
  `note_for_user`.
- **Session and suggestion tools:** `offer_session`, `suggest_setting`, and the peer tools that S11 replaces
  (`message_session`, `wait_for_replies` and `settle_wait`).
- **Worker engine:**
  - `M/workers.rs`, about 6k of its 7,033 lines. Kept and moved: `make_worktree` (:2578), `session_worktree_path`
    and `ensure_target` (:2642–2700), and the temp folders (:5812–5866), until S5 re-homes them.
  - `M/worker_handoff.rs` (994), `M/worker_resume.rs` (303), `M/outcomes.rs` (272) and `M/reruns.rs` (317).
  - `M/research.rs` (894), `M/context_pack.rs` (1,013) and `M/peer_answers.rs` (321).
  - The worker-silence half of `M/watchdog.rs`.
- **Routing:** `M/routing.rs`'s task routing (923), and `crates/router`'s decision code (`decide.rs` 3,868).
  - `ProviderKind` and `Registry` stay.
  - `crates/core/src/routing/*` stays (quota and model lists).
  - `M/fallback.rs` (946) goes as far as it reroutes a task; account switching stays.
- **Landing engine and CI watch:**
  - `M/landing.rs` (3,477, what S3 left).
  - The CI-watch half of `M/ci.rs` (2,358). The `gh` PR read in `M/pull_request.rs` stays and moves to S12's PR
    row.
  - `M/background_runs.rs` (1,003), unless S10 already removed it.
- **Prompts:** the worker prompts and report envelopes at `M/prompts.rs:2623–3440`, and the protocol half of
  `thread()` at :260–363.
- **Tests:** `resume`, `successor`, `worker_turns`, `watchdog`, the peer test files, `takeover` (worker
  hand-back), `thread_overflow` and `base_merge`; most of `flow/tests.rs` and `thread_tests.rs` are rewritten.
- Closes: #480, #481, #276, #271, #295, and #482 (workers are plain sessions with their own write access).
  Re-check #456, #45 and #34. They test scout and stand-in paths that no longer exist; close them if so.

### 3.7 Brigadier's copy of the conversation and its guessed states (G2, G11) → steps S8, S9
- **Thread items from the engine store:**
  - `M/thread_items.rs` (845) and the `orch:<id>` log written by `log_orchestrator` (`M/conversation.rs:6959`);
  - `M/export.rs` (874), the successor transcript;
  - `D/state/board.ts` fields that only these feed.
- **Request inference:**
  - `M/requests.rs` (1,293) and the worked and waited spans in `work.rs` (`moved_to` :2994);
  - `M/status.rs` if anything is left after S1.
- **The changes card's sources:**
  - `M/request_changes.rs` (1,693, the most guess-heavy file), `M/pending_changes.rs` (884),
    `M/shell_changes.rs` (634) and `M/undo.rs` (210);
  - the PreToolUse and PostToolUse capture hooks in `P/claude/mod.rs`.
  - Closes: #256.
- **The sidebar's engine half:** the `Board::activity` and `settled_state` use in `board.rs:348–420` and
  `project_sessions.rs:217`. Git ahead, merged and reverted stays.
- **Archive and Delete removing CLI transcripts:** `ClaudeSession`, `ClaudeProjectDir` and `CodexThread` ledger
  artifacts disposed on archive (`M/lifecycle.rs:1040–1075`). Under G11 and G15 these are the record, so
  Archive must keep them (S6).

### 3.8 Briefings, rebirth and Brain machinery (G12, G14) → steps S16, S17, S21
- **Fork-based rebirth and briefings:**
  - `M/rebirth.rs` (1,879): `HANDOFF_PROMPT` at :92, the 5k Brain budget at :62, and `briefing()` at :590.
  - The rebirth thresholds in `crates/core/src/knowledge.rs:175–406`, including the `BRIGADIER_REBIRTH_TOKENS`,
    `BRIGADIER_COLD_REBIRTH_TOKENS`, `BRIGADIER_HARD_LINE_TOKENS` and `BRIGADIER_BRIEFING_TOKENS` env vars.
  - `M/cold.rs` (407) and `M/conversation/overflow.rs` (412).
  - The recovery and stand-in reseed: `M/conversation.rs:3196–3210`, :5960–6110 and :6469–6530. There are no
    hidden restarts on quota errors any more. `switch_account`, which continues the same session, stays.
  - Tests: `thread_overflow` and `rebirth_words`, plus the rebirth cases.
- **Brain-first and learned nodes:**
  - the Brain-first prompt line (`prompts.rs:313`) and `M/brain_first.rs` (41);
  - in `M/brains.rs`: `learn_report` (:1494), `learn_user_decision` (:1797) and `keep_note_decisions` (:1866);
  - the `remember` tool (`brains.rs:861`), which the ledger replaces.
- **Enrichment jobs:** `M/brain_jobs.rs` skeleton and idle-quota enrichment (about 1,250 of 1,422 lines), and
  `record_job_nodes` (:712).
- **Personal Brain:**
  - `save_memory`, `memory_lines`, `preference_lines` and the preference notes;
  - `list_memories`, `edit_memory` and `forget_memory` (`brains.rs:2172–2275`);
  - `personal.sqlite`;
  - the forced `autoMemoryEnabled:false` (`P/claude/mod.rs:827`).
- **Thread prompt:**
  - `M/prompts.rs` `thread()` and its rule macros. The standing instructions (§6) replace them.
  - `M/instructions.rs` (572), which inlines CLAUDE.md and AGENTS.md that the CLIs already read themselves.

### 3.9 Open issues that stay
These are unrelated to the strip-down: #522 (two sessions in one Local checkout), #483, #478, #476, #474, #473,
#459, #453 and #86. #523 (first turn outside the worktree) is fixed by S5, not made obsolete. #517 depends on
the rule in §8, item 5.

## 4. Build steps

The rules for every step:
- **Land small, land often.** Each step lands on main on its own, with its own tests. The worker rebases,
  ff-merges and pushes.
- **No hidden fallback (G4).** Unfinished user-facing work stays behind an env flag. The old path is deleted in
  the step that turns the new one on.
- **The app keeps working after every step.**
- **"Done when" names real checks.** Full suites and the live smoke run once on the integrated tree, not per
  worker.
- **Who suits a step.** **Codex** = bounded and well specified, good for a Codex worker. **Opus** = needs
  judgement across the engine.
- **Parallel steps.** **∥** lists steps that touch disjoint files and can run at the same time. Steps without ∥
  run in order.

Five steps edit `M/tools.rs`, `crates/mcp-server/src/catalog.rs`, `M/prompts.rs`, `board.rs` and `model.rs`:
S1–S4, S11 and S14. They run one after another.

### Stage 1: stop pushing and gating (relief first; deletions only)

**S1. No engine pushes (G1).** Opus.
- **Scope:** delete everything in §3.1. A thread's turn now starts only for:
  - a user message;
  - a result of work it started itself (worker reports and session events, until S14 turns those into pulls);
  - later, the seam nudge.
- **Files:** `M/status.rs`, `M/project_sessions.rs:486–578`, `M/prompts.rs` (revisions, notes, `[quiet]`,
  `[next:]`), `M/conversation.rs` (deliver, quiet and next filters, notes), `M/inbox_store.rs`, `M/workers.rs:1854`.
- **Done when:**
  - A new flow test drives a thread through 2 hours of simulated engine events and gets **no** turn: a status
    timer, another session merging, a settings change and a rule revision.
  - A user message still starts a turn, and a worker report still does.
  - `git grep -n '\[status due\]\|\[quiet\]\|\[next:\|From now on'` finds nothing in non-test code.
  - `cargo test -p brigadier-core` passes for the touched modules.

**S2. Checks are plain commands; no lease, no sandbox gate (G6).** Opus.
- **Scope:** §3.2.
  - Remove the checks tools and the check gate in `merge_rule`. A merge no longer waits for a Brigadier verdict.
  - Remove the build lease, the `Nested` and skip test machinery, and the `checks-sandbox` CI job.
  - Thread prompt: "run `tools/full-checks.sh` yourself (in the background for long runs) and read its output".
- **Files:** `M/integrated_checks.rs`, `M/checks.rs`, `M/test_counts.rs`, `crates/core/src/machine/builds.rs`,
  `machine/heavy.rs`, `M/machine.rs`, the lease calls in `M/preview.rs`, `M/flow/mod.rs:1233–1302`, the skip
  sites in §3.2, `crates/daemon/src/terminals.rs`, `.github/workflows/ci.yml`, `M/landing.rs` (gate only),
  `M/tools.rs`, `catalog.rs`.
- **Done when:**
  - `catalog.rs` lists no `run_check(s)`.
  - A thread that runs `tools/full-checks.sh` through its own Bash sees the real exit code and output in a tool
    row (flow test with the fake CLI).
  - Two sessions' heavy commands start at once, without a lease row.
  - `ci.yml` has no `checks-sandbox` job.
  - The tests of the touched modules pass.
- **Closes:** #514, #515, #479.

**S3. Reviews and merges are plain commands (G6).** Opus.
- **Scope:** §3.3. Remove the review tools, the findings ledger, the merge-anyway path and `MergeRefused`.
  - The thread runs `codex review …` and `git merge` / `git push` itself.
  - Worker landing (`land_phase`) keeps working, without any gate, until S14.
- **Files:** `M/review_runs.rs`, `crates/review`, `M/fusion.rs`, `M/unreviewed.rs`, `M/merge_consent.rs`,
  `M/review_hold.rs`, `M/landing.rs` (consent and refusal), `board.rs`, `work.rs`, `model.rs`, `sessions.rs`,
  `ipc/src/protocol.rs`, `crates/daemon/src/server.rs`, `D/app/conversation/Activities.tsx`, `backgroundRows.ts`,
  `prompts.rs`, `tools.rs`, `catalog.rs`.
- **Done when:**
  - No "merge anyway" text is left in `crates/` or `apps/` (`git grep -i 'merge anyway'`, tests aside).
  - A flow test: the thread asks to merge in words, runs `git merge` itself, and the sidebar reads Merged from
    git.
  - `pnpm typecheck` and the app tests for the touched files pass.
- **Closes:** #519, #518 (card half).

**S4. Permissions pass straight through; no Brigadier sandbox (G5).** Opus.
- **Scope:** the composer picker becomes **Ask / Accept edits / Full access**:

  | Picker | Claude Code | Codex |
  |---|---|---|
  | Ask | `default` | `untrusted` + `workspace-write` |
  | Accept edits | `acceptEdits` | `on-request` + `workspace-write` |
  | Full access | `bypassPermissions` | `never` + `danger-full-access` |

  - Codex's own sandbox counts as the CLI's own setting and stays.
  - Saved levels migrate: `askForApproval` → Ask, `approveForMe` → Accept edits, `fullAccess` → Full access.
  - The CLI's own permission prompts keep showing as cards: `--permission-prompt-tool stdio` → `open_cli_card`
    at `M/cards.rs:144`, and Codex approval requests.
  - Delete §3.4.
- **Files:** `P/cli.rs`, `crates/sandbox/src/{macos,linux,windows}.rs` (sandbox parts only), `M/protection.rs`,
  `M/escalation.rs`, `M/run.rs`, `M/preview.rs` (its confine), `M/workers.rs` (`access_for`, `sandboxed`),
  `M/thread.rs` (`thread_access`), `P/claude/mod.rs` (`permission_mode` :637, settings), `P/codex/mod.rs`,
  `D/app/shell/SetupPickers.tsx`, `SandboxLimitNotice.tsx`, the IPC setup types.
- **Done when:**
  - Provider unit tests assert the exact flags for each of the three levels on both CLIs.
  - A flow test: under Ask, a CLI permission prompt becomes a card, and answering it continues the turn.
  - `git grep -n 'confine\|protect(' crates/core crates/providers` shows only `crates/sandbox` paths still used
    by previews' process groups, or nothing.
  - On a dev build, a session under Accept edits opens a pty: `tools/full-checks.sh` runs in it.
- **Closes:** #521.

### Stage 2: the UI reads the CLI and git

**S5. One folder per session (G13).** Opus.
- **Scope:** new sessions get `~/.brigadier/s/<short-id>/{record,work,tmp,dev,logs}` (short id: 8 characters,
  no spaces).
  - The worktree goes under `work/`, previews and dev servers under `dev/`, logs under `logs/`.
  - `TMPDIR` and `TMPPREFIX` point at `tmp/`. This replaces `session_temp_dir` at `M/workers.rs:5832`.
  - The thread starts in its worktree, never in `<data>/orch/<id>`.
  - `CLAUDE_CODE_TMPDIR` stays a short `/tmp/brigadier-<12hex>` (§8, item 3).
  - Existing sessions keep their old paths, which are recorded, until they are archived.
  - Storage shows size per session folder.
- **Files:** `crates/sandbox/src/paths.rs`, `M/mod.rs` (`owned_dir` :889), `M/workers.rs:2532–2700` and
  `:5812–5866`, `M/thread.rs:366–395`, `M/preview.rs:78/529`, `M/disk/mod.rs`, `D/` Storage page.
- **Done when:**
  - A flow test: a new session's CLI sees `TMPDIR` under its folder.
  - Its first turn runs in its worktree (#523).
  - A Unix socket made under `$TMPDIR` by `python3 -c 'import socket…bind'` binds on macOS.
  - Storage lists the folder with its size.
- **Closes:** #523.
- ∥ S9.

**S6. The CLI's files are the record.** Codex.
- **Scope:**
  - Archive and Delete stop removing the CLI transcript files of a live lineage. Delete still removes them
    (G15, S18).
  - Every CLI session start appends `{provider, session_id, file, started_at}` to `record/lineage.json`.
  - The ledger's `ClaudeSession`, `ClaudeProjectDir` and `CodexThread` artifacts move to "kept until Delete".
- **Files:** `M/lifecycle.rs:1040–1075`, `crates/core/src/ledger.rs`, `P/model.rs:1486`, the CLI start path in
  `M/conversation.rs`.
- **Done when:** a flow test archives a session and finds its CLI session file and `lineage.json` still there.
  Restore resumes it with `--resume`.
- ∥ S9.

**S7. Thread reader from the CLI's session file, behind a flag (G11).** Opus.
- **Scope:** a reader that turns a lineage's files into thread items:
  - Claude JSONL, including `tool-results/` and subagent sidechains;
  - Codex rollout.
  - While the CLI runs, the live stream extends the items. Item ids are file positions, so the stream and the
    file agree.
  - Turn = user message → the CLI's turn-end event. "Worked for Xm" comes from the CLI timestamps. Stopped and
    failed come from the result event.
  - It is shown when `BRIGADIER_CLI_THREAD=1`.
- **Files:** a new `P/claude/session_file.rs`, `P/codex/rollout.rs` and `crates/core/src/thread_view.rs`, plus
  IPC events. `D/` thread renderer reads the new items under the flag.
- **Done when:**
  - Fixture tests: real JSONL and rollout files from this Mac, scrubbed, give the same items in the same order
    as the CLI shows. That covers tool rows, subagents, images, a failed turn and a stopped turn.
  - Under the flag, a live dev session renders the same conversation as the CLI's own `--resume` view. This is
    checked by hand on two sessions.
  - Benchmarks are not part of this check.

**S8. Flip the thread; work block and sidebar from CLI events; delete Brigadier's copy.** Opus.
- **Scope:** the flag goes away and the new reader is the only path.
  - Sidebar words:
    - **Working** = a turn is open.
    - **Needs you** = an open CLI permission prompt, ask-user card or MCP sign-in.
    - **Ready, Merged, Reverted** = from git.
  - Delete §3.7's thread copy and request inference: `thread_items.rs`, the display use of `orch:<id>`,
    `export.rs`, `requests.rs`, the request spans in `work.rs`, and the engine half of `tabStates`.
- **Files:** as in §3.7, plus `D/state/tabStates.ts`, `activity.ts`, `requestTime.ts`, `WorkHeader.tsx`,
  `WorkFold.tsx`, `RequestBlock.tsx`, `blocks.ts`.
- **Done when:**
  - A shared test table (successor of `session_states.json`) covers each sidebar word from CLI events and git
    only.
  - The app tests pass.
  - A dev session shows the same items as `claude --resume`.
  - No code path writes `OrchestratorLogged` for display.

**S9. Changes card from git snapshots (G8).** Codex.
- **Scope:**
  - At each turn's start and end, snapshot the worktree as a git tree object. Use a temporary index with
    `git add -A` and `write-tree`, including untracked and excluding ignored, and keep a ref under
    `refs/brigadier/s/<id>/turn-<n>-{start,end}`.
  - The card is the exact diff between the two: files, +/− and the diff.
  - Revert restores the start snapshot, after a confirm. Files changed after the turn are named first.
  - Delete `request_changes.rs`, `pending_changes.rs`, `shell_changes.rs`, the capture hooks and `undo.rs`'s
    recorded-commit revert.
  - The card keeps its look (bug fix ≠ redesign).
- **Files:** a new `crates/git/src/snapshot.rs`, `M/` turn hooks in the CLI pump, `D/app/conversation/ChangesCard.tsx`,
  `ChangesFileRow.tsx`, `RevertConfirmRow.tsx`, `state/pendingChanges.ts`.
- **Done when:**
  - Unit tests on a temp repo: new, deleted, renamed, binary and untracked files, and a turn with no change.
  - A flow test: Revert restores the start tree exactly (`git diff` against the start ref is empty).
  - App tests pass.
- **Closes:** #256.
- ∥ S5, S6, S7.

**S10. Background = the CLI's own (G10).** Codex.
- **Scope:** stop setting `CLAUDE_CODE_DISABLE_BACKGROUND_TASKS=1` (`M/thread.rs:389`).
  - **Claude rows:** from stream-json `task_started`, `task_progress`, `task_notification` and
    `background_tasks_changed`. These are already parsed at `P/claude/parse.rs:884–903`.
    - Label = the CLI's description. Elapsed time from its timestamps. Latest output line from `output_file`.
    - Actions: open the output or transcript; stop.
    - Drop rules as Claude Code's own: a done subagent stays 30 s; failed and stopped tasks leave; shell rows
      clear about 5 min after exit.
  - **Codex rows:** its background terminals, as `/ps` lists them.
  - Delete `M/background_runs.rs`, the Brigadier-owned run rows in `Activities.tsx` and `backgroundRows.ts`, and
    `run`'s background mode.
- **Files:** `M/thread.rs`, `P/claude/parse.rs`, `P/codex/parse.rs`, `M/background_runs.rs`,
  `D/app/conversation/Activities.tsx`, `backgroundRows.ts`, `activity/BackgroundRunRow.tsx`.
- **Done when:**
  - Fixture test: a stream with a `run_in_background` Bash and a background subagent gives two rows that appear
    and drop on the documented timings.
  - A live dev session: `sleep 30 &` through Ctrl+B-style `run_in_background` shows a row and clears.
  - Stop ends the task.
- ∥ S9. It comes after S8 (shared item model).

### Stage 3: teams = the Delegator, built in

**S11. Session tools equal to `dlg` (G3, G6).** Opus.
- **Scope:** pull-only MCP tools on the thread:

  | Tool | What it does |
  |---|---|
  | `sessions` | List the project's sessions, with title, state from CLI events, branch, +/− and started-by. This is "read anything". |
  | `session_start` | Takes project, title, prompt, agent (claude or codex), model, effort, worktree or local, base branch and permission. It starts an ordinary session in the sidebar, marked "started by <this one>". |
  | `session_send` | Queued mid-turn, like `dlg send`. |
  | `session_wait` | Takes ids and a timeout. Returns events of sessions this one started: turn ended (with the last message), needs input, gone, usage limit, fresh context. |
  | `session_read` | The last message, the last N items or the screen. |
  | `session_close` | Closes the session. |

  - Build on the plain session start path (`M/project_sessions.rs:23/51`) and the raw-session runtime
    (`crates/core/src/runtime.rs:1028–1205`), not on `delegate_task`.
  - `peers.rs`, `peer_wait.rs` and `peer_records.rs` fold into this. `message_session`, `wait_for_replies`,
    `settle_wait` and `offer_session` go.
  - App tools stay: `app_state`, `read_terminal`, `open_in_app`, `terminal_run`, `show_session`, `close_tab`.
  - Model rules from the user's policy hold: never Fable, effort ≤ high.
- **Files:** a new `M/session_tools.rs`, `catalog.rs`, `tools.rs`, `M/peers.rs`, `M/peer_wait.rs`,
  `crates/core/src/sessions/peer_records.rs`, `D/` sidebar (started-by link).
- **Done when:**
  - Flow tests with the fake CLI: start two sessions, send one a message mid-turn, `session_wait` returns both
    turn ends with their last messages, and close removes them.
  - A live dev check: a thread starts a Codex session that writes a file and reports.

**S12. Context card per G9/G10.** Opus.
- **Scope:** the card from §1:
  - the branch row (git);
  - the PR row: `#N` + state word + colour from `gh pr view`, refreshed when the CLI's events show a
    `git push` or `gh pr` command; failed checks red;
  - Waiting on you (pending prompts of started sessions);
  - Workers (from S11; "N done · M stopped");
  - Background (S10);
  - Sources: attachments first, then URLs from the CLI's fetch and browser tool items, then MCP servers seen in
    its events; + adds one.
  - A "Resumes at" card from the CLI's usage-limit error.
  - Nothing reads `Board` engine records any more.
- **Files:** `D/app/conversation/SummaryContent.tsx`, `PinnedSummary.tsx`, `FloatingSummary.tsx`,
  `side-panel/WorkerSummary.tsx`, `SourcesTab.tsx`, `M/pull_request.rs` (read only; merge queue and CI watch
  go), `M/mod.rs:663` (`session_diff_stat`).
- **Done when:**
  - App tests per row with fixture events.
  - A flow test: after a `git push` item, the PR row refreshes.
  - A dev session with a started worker shows the worker's state and +/−.
  - No component under `SummaryContent` imports `boardJob` or `liveJob`.

**S13. The team skill (G3).** Opus.
- **Scope:** one plain, editable skill shipped with Brigadier: `crates/core/src/manager/team_skill/SKILL.md`,
  included like `computer_use_skill.md`.
  - It is adapted from `~/.claude/skills/delegator/SKILL.md`:
    - the `dlg` commands become the S11 tools;
    - cmux tabs become sessions;
    - "hand off at 300k" becomes the seam;
    - overnight is the same skill with a deadline;
    - workers run their own checks and push.
  - Install it into each Claude session's skills folder and Codex's skills folder (§8, item 7). The user
    invokes it by name; the thread may invoke it too.
  - It is behind `BRIGADIER_TEAM_SKILL=1` until the done-when passes, then on.
- **Files:** the skill file, `M/computer_use.rs`-style installer, `P/claude/mod.rs` (`--plugin-dir` / skill
  path), `P/codex/home.rs`.
- **Done when:**
  - A live run on a dev build: a two-worker change (one Claude, one Codex) lands on a test repo's main through
    the skill, each worker running its own checks.
  - The A/B against cmux `/delegator` on the same request is written as a report and gates nothing.

**S14. Delete the old engine.** Opus. These are three landable sub-steps, in this order, after S11–S13 are on.

- **S14a.** Phases, outlines, overnight, plan cards, the Job card and WaitCard (§3.5).
  - Done when `git grep -n 'JobCard\|set_headline\|propose_overnight\|plan_phases'` finds nothing.
  - Overnight is reachable only as the skill.
  - The app and core tests that remain pass.
- **S14b.** Workers, routing, context packs, worker hand-off and resume, research, outcomes and reruns (§3.6).
  - Done when `catalog.rs` lists only:
    - the S11 session tools;
    - the app tools (`app_state`, `read_terminal`, `open_in_app`, `terminal_run`, `show_session`, `close_tab`);
    - previews, browser, `ask_user`, `request_approval`, `offer_feedback`;
    - pages (`read_page`, `write_page`, `save_report`);
    - Brain search and `code_search`, `code_refs`, `project_map`.
  - `crates/router` keeps only the shared types.
- **S14c.** The landing engine, the CI-watch engine and what is left of `background_runs.rs` (§3.6).
  - Done when `M/landing.rs` is gone and the PR row still works.
- Each sub-step closes the §3.6 issues it obsoletes, and re-checks #382 and #296.

### Stage 4: long sessions (G12) and Brain (G14)

This stage can run in parallel with Stage 3 once S8 has landed. It touches `rebirth.rs`, `brains.rs`, `crates/brain`
and the seam code, not the session tools. The one shared file is `prompts.rs`, so S15 rebases after S14.

**S15. Standing instructions and the ledger (Q3, G12.read).** Opus.
- **Scope:** the text in §6 becomes one file, `crates/core/src/manager/standing_instructions.md`, identical for
  every session.
  - Claude receives it with `--append-system-prompt`; Codex receives it as developer instructions.
  - Per-session values arrive as env vars: `BRIGADIER_SESSION_DIR`, `BRIGADIER_LEDGER`.
  - It replaces the thread prompt (`M/prompts.rs` `thread()` and macros) and `M/instructions.rs`. The CLIs load
    CLAUDE.md and AGENTS.md themselves.
  - The ledger shows in Sources and opens from there.
- **Done when:**
  - A provider test asserts the exact appended text and env on both CLIs.
  - A byte test keeps the file under 2,500 bytes.
  - A dev session writes `record/ledger.md` when told a rule.

**S16. Brain = pull-only search over the records (G14, Q5).** Codex for the reader, Opus for the deletions.
- **Scope:** re-point the transcript index (`M/brains.rs:1218–1302` `sync_transcript_stream`, `crates/brain/src/transcript.rs`)
  from the event store to:
  - every session's lineage files, including `tool-results/`;
  - every ledger, `ledger-archive.md` and hand-off note in the project.
  - Entries of kind "Learned from reading" are indexed as pointers (file:line + finding) with a content hash.
    When the file changes, they read as stale.
  - Tools: `brain_search(query, scope?)` returns hits with session, position and a text window.
    `transcript_read(session, from, to)` returns a verbatim span.
  - Delete §3.8's Brain-first rule, learned-node writers, `remember`, `constraints.rs` (if S14 left it) and
    `brain_jobs.rs` enrichment.
  - The code index stays as optional tools.
- **Done when:**
  - Fixture tests: a phrase in a predecessor's JSONL, in a large tool output under `tool-results/`, and in a
    ledger is found.
  - A changed file marks its pointer stale.
  - `git grep -n 'learn_report\|enrichment\|Brain-first'` finds nothing.

**S17. Personal Brain → the CLIs' own memory.** Opus.
- **Scope:**
  - Stop forcing `autoMemoryEnabled:false`, so Claude Code's own memory works. Codex memory follows the user's
    own Codex config.
  - Export `personal.sqlite` once to `~/.brigadier/personal-preferences.md`.
  - Where it goes from there is the user's call (§8, item 6).
  - Delete §3.8's Personal Brain code and the Settings memories page.
  - Chats follow the same rule.
- **Done when:**
  - A test exports a fixture `personal.sqlite` exactly.
  - No prompt contains "The user's preferences".
  - A Chat in a dev build remembers a preference through Claude Code's own memory.

**S18. Archive and Delete per G15.** Codex.
- **Scope:**
  - **Archive** stops processes and removes `tmp/`, `logs/`, `dev/`, build output and a merged worktree. It keeps
    `record/`, the lineage transcripts and Brain entries, and is restorable.
  - **Delete** removes the session folder, its Brain entries, its lineage's CLI transcript files and the
    worktree. It removes the branch after the existing unmerged ask.
  - Update the §5 Lifecycle text in `docs/PLAN.md` to point here.
- **Files:** `M/lifecycle.rs`, `M/closing.rs`, `crates/core/src/ledger.rs`, `M/brains.rs` (`forget_transcript`
  → forget the session's entries), `M/disk/*`.
- **Done when:** flow tests list exactly what each action leaves on disk and in the Brain, for a session with
  two lineage generations.

**S19. Continuity battery v0 (Q7).** Codex. Can run in parallel with anything after S6.
- **Scope:** a local script, `tools/continuity-battery/`, with a text report. It covers the planted items and
  forced seams of `research/PROPOSAL.md` §2.6. It runs against today's rebirth, so S20 has a baseline.
- **Done when:** one run's report exists, item by item, for Claude.
- This is a report, never a gate.

**S20. Seam v1 behind `BRIGADIER_SEAM=1` (G12, Q0–Q2, Q4).** Opus.
- **Trigger:** when the hot context (from the CLI's usage events, as today) passes `BRIGADIER_SEAM_TOKENS`
  (default 200000), queue the one nudge mid-turn, the Delegator's way.
  - Wait while the CLI reports running background tasks or an open prompt.
  - One nudge per generation, recorded in `record/seam.json`.
- **Note and divider:**
  - The thread writes `record/handoff/<n>.md`.
  - Brigadier draws the divider "Fresh context · hand-off note" (openable) and closes the predecessor.
- **Successor:** starts on the next user message, with the bootstrap as the first lines of that message, also
  saved as `record/handoff/<n>-start.md`. The bootstrap holds:
  - the map;
  - the whole ledger;
  - the note;
  - a verbatim tail of about 15k tokens;
  - the successor rules of §6.
- **Edge cases:**
  - A user message during the note turn waits up to a minute, then goes to the successor, exactly once.
  - Death without a note (limit, crash, prompt too long) → the successor starts from the ledger and tail with
    the "check git first" line.
  - Daemon restart → `seam.json` decides; never two successors.
- **Probes on the side:**
  - Codex's opt-in 1M window, a 1-hour probe (Q0).
  - Whether a failure hook sees the full failing output.
- **Done when:**
  - Flow tests with a 60k debug threshold give two seams, the divider and note, exactly-once delivery, a death
    without a note, and a restart between note and successor.
  - The battery is run against it, and its report goes into the run's notes.

**S21. Seam on; delete rebirth (Q6).** Opus. It runs in the same run as S20, once the battery shows the seam no
worse than today's rebirth.
- **Scope:** the flag defaults on. Delete §3.8's rebirth, briefing, cold, overflow, threshold and stand-in
  reseed code.
  - A usage limit stops the turn and shows "Resumes at"; it never restarts quietly.
  - Open sessions switch at their next seam.
- **Done when:** `git grep -n 'rebirth\|briefing\|BRIGADIER_REBIRTH_TOKENS'` finds only docs, and the S20 tests
  pass with no flag.

**S22. Cold-cache rule.** Opus.
- **Scope:** when the user returns after the cache TTL and the context is ≥ 100k, start the successor from the
  ledger and tail instead of resuming. There is no idle nudge (Q2).
- **Done when:** a flow test with a short `BRIGADIER_CACHE_TTL_SECS` starts a successor, not a resume.

**S23. Battery nightly (Q7).** Codex.
- **Scope:**
  - The battery runs nightly on both providers, as a CI artifact and a Reports page.
  - A weekly sample is drawn from real lineages.
  - A regression becomes a GitHub issue, never a hold.
- **Done when:** the first nightly artifact exists.

**S24. Codex parity.** Codex.
- **Scope:**
  - The standing instructions and bootstrap on Codex.
  - The divider when Codex compacted, read from the rollout.
  - The 1M window if the S20 probe held, otherwise a documented limit.
  - No Codex hooks.
- **Done when:** a Codex dev session goes through a debug-threshold seam with its divider.

**S25. Tool-output trimming, A/B first (Q8).** Opus, later.
- **Scope:** an A/B on whole tasks of a lower `bashOutputMaxChars` and of the exact-read instructions. Switch on
  only with ≥5% measured saving on whole tasks and no quality loss.
- **Done when:** the A/B report exists. Turning it on is a separate one-line change.

### Order and parallel runs, in one view

```
S1 → S2 → S3 → S4 → S5 ─┬→ S6 → S7 → S8 → S10 → S11 → S12 → S13 → S14a → S14b → S14c
                        └→ S9 (∥ S5–S8)
after S8:  S15 → S16 → S17 → S18      (∥ Stage 3; S15 rebases after S14)
after S6:  S19 (∥ anything)
after S16 + S19:  S20 → S21 → S22 → S23 → S24 → S25
```

## 5. Benchmarks never gate

The continuity battery (S19, S23), the token benches and the A/B against cmux `/delegator` (S13) are reports.
They run nightly or before a release. A regression becomes a GitHub issue. No step's "done when" waits on a
number from them, except S21's switch-on, which compares the seam with today's rebirth. That is the user's Q6
decision, and the comparison is read by the person landing it, not by a gate in code.

## 6. Standing instructions (draft of `standing_instructions.md`)

```
This is a plain Claude Code / Codex session inside Brigadier. Work as you would anywhere: run checks, reviews, git and gh yourself.
Your session folder is $BRIGADIER_SESSION_DIR. Scratch files go in $TMPDIR (inside it), never in the repo.
Keep a ledger at $BRIGADIER_LEDGER as you work. Write an entry the moment something is settled, in the same step as your other tool calls.
Each entry says its kind and its source (which message, command or file). Kinds:
- Rule from the user: their exact words, which message, scope (this request / session / project); add "supersedes …", never delete.
- Decision: one line and the reason, marked "user decided" or "I decided".
- Exact identifier (hash, path:line, number with units, id) · Failed check (command, exact error line, fixed or not) · Ruled out (what, why) · Open promise.
- Learned from reading: file:line and a one-line finding, never a copy.
Never delete entries; move finished ones to ledger-archive.md beside it. A worker's claim is not a fact until you check it; say whose it is.
For big reading, use your own subagents and keep only conclusions with file:line. Read exact ranges.
When a message says a fresh session will continue: finish the step you are on, update the ledger, write the hand-off note where it says
(doing now, waiting for, next, "read A,B,C; next D,E", stale ledger entries), then stop. Don't start new work.
If this is your second hand-off in a row that is still reading, say so in the note and propose splitting the work.
If you are that fresh session: the ledger, note and last exchanges in your first message are authoritative. For anything else from
before, search the record (brain_search, transcript_read) and quote what you find; if it isn't there, say so rather than guess.
```

## 7. Order of risk: what proves the plan early

- S1–S4 are deletions with immediate user value: the 10-22 failure modes go away. They are low risk because
  they only remove things.
- S7 is the riskiest build step, because the CLI file formats are internal (§8, item 1). It runs behind a flag
  and is checked against real files before S8 flips it.
- S13 is the one that proves G3: a team run that lands real work.

## 8. Open risks

1. **The CLI session-file formats are internal.** Claude Code says its JSONL "is internal … and changes between
   versions" (EVIDENCE 2.4). Codex rollouts are not a public API either.
   - Mitigation: the reader lives in `P/` next to each adapter, with fixture tests from real files, and it is
     re-checked on every CLI version bump.
   - The live stream stays the source while a turn runs.
2. **Background tasks in `claude -p` stream-json.**
   - Start and end are exposed: `system` `task_started`, `task_progress`, `task_notification` (status
     completed, failed or stopped, with `output_file`) and `background_tasks_changed` (EVIDENCE 2.4). They are
     already parsed at `P/claude/parse.rs:884–903`.
   - Open: there is no control request that backgrounds a running tool. A user message with `priority: "now"`
     moves movable work to the background (v2.1.286+), which is the nearest Ctrl+B. Stopping one task needs a
     probe in S10.
   - For Codex, whether app-server exposes `/ps`-style background terminals needs the same probe. If not,
     Codex rows show only what its events carry.
3. **The macOS 104-byte Unix-socket path.**
   - `~/.brigadier/s/<8>/tmp/` is about 35–45 bytes for typical home paths, so a tool's own socket under it
     usually fits.
   - Claude Code's sandbox makes sockets under `<tmp>/claude-<uid>/…`, and Seatbelt drops socket allowances on
     long roots (`crates/sandbox/src/macos.rs:689–711`). So `CLAUDE_CODE_TMPDIR` stays a short `/tmp` path, a
     G13 exception like shared caches.
   - S5's done-when binds a socket to prove it.
4. **Codex hooks need manual trust by hash** (EVIDENCE 2.5). Nothing in this plan uses a Codex hook. The changes
   card uses git snapshots, the seam uses usage events and a queued message, and Background uses events.
5. **#517 and G9.** Under G9 "Needs you" means only real pending prompts. A thread that asks in plain text and
   ends its turn reads as Ready, like an idle plain CLI.
   - The case behind #517 (merge anyway / Full access) disappears with S2–S4. The thread can still raise a real
     card with `ask_user`.
   - Recommendation: close #517 with S8 as "by G9". If the user wants plain-text questions to count, that is a
     new decision.
6. **The Personal Brain's destination.** Writing into the user's own `~/.claude/CLAUDE.md` or
   `~/.codex/AGENTS.md` changes files Brigadier doesn't own.
   - S17 exports to `~/.brigadier/personal-preferences.md` and stops there.
   - Recommendation: the user copies what they want, or says "append it", and S17 does so. This is the only
     user ask in the plan.
7. **Skills on Codex.** S13 assumes Codex loads a skill from its skills folder. If the installed Codex can't,
   the team skill reaches Codex threads as a file the thread is told to read when asked. This is checked in S13.
8. **No Brigadier sandbox (G5) also removes the "protect" profile.** That profile kept every CLI, Full access
   included, from writing Brigadier's own data folder (`P/cli.rs:259–287`). After S4, a session can delete
   `brigadier.db` like any other file it can reach. That is accepted under G5 and the same as a plain CLI. S4's
   commit says so.
9. **Ledger path.** PROPOSAL.md put it at `~/.brigadier/s/<id>/ledger.md`. G15 later placed the kept record under
   `record/`. This plan uses `record/ledger.md` so Archive keeps it, which follows G15.
10. **Many seams in one long turn on Codex.** A Codex turn can compact before the 200k nudge is read (Q0). This
    is accepted and shown by the divider. The ledger, note and rollout survive it.
11. **Big deletions break tests far away.** S14's sub-steps touch shared types (`board.rs`, `model.rs`, IPC).
    Each sub-step runs `cargo test -p brigadier-core` for the touched modules and `pnpm typecheck`. The full suite
    runs once on the integrated tree, as the Delegator's rule says.
