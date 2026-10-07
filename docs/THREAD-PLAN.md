# Thread plan: Brigadier's main session as a delegator-style thread

> Status: plan, 2026-10-07, from the thread-orchestrator grill (Q1–Q14). Base: main `7cff0db9`.
> It is built by a new `/delegator` run (§4), after Codex review and the user's approval.
> File:line references are to `7cff0db9`. Paths: `M/` = `crates/core/src/manager/`, `P/` = `crates/providers/src/`.

## 1. Target and success test

Today the main session is a pure orchestrator. It is a CLI session with no tools of its own, run by a fixed pipeline: lead → outline → automatic reviewer → verifier → review worker → land.

The target is one long-lived CLI session that works like a person running `/delegator` in cmux:
- It reads, runs and previews things itself. It makes tiny edits itself and hands everything else to warm workers that start with a context pack.
- It decides when an outline, a verifier or a deeper review is worth it.
- Every landed change gets one non-blocking, one-shot review from the other vendor.

**Success test (Q3)** is an A/B against a plain cmux `/delegator` session on two real requests:
- **T1:** the "Sidebar hide mode" request, from base `59a60afb`.
  Request text (verbatim): "For our Brigadier, I'll need you to make the sidebar toggle have a chevron with an option (persisted) for users to completely hide the sidebar so that not even the strip is shown, so our sidebar would have 2 modes (strip and fully closed), a chevron like in this screenshot [Image #2]". Image #2 is copied to the 20261007-1057 run's `evidence/sidebar-chevron-reference.png`.
- **T2:** this run's "thread shows no sign it is working" request, from base `53c5cd32`.

Brigadier must win on all of these:
- **Faster:** time to verified landing and time to the final answer.
- **Cheaper:** tokens per provider, both raw and without cache reads.
- **Same verified result:** the same work done, and an independent evaluator passes the same checks in both arms.
- **Never "I can't"** where the cmux session could do it, for example running the dev app.
- **A live line at every busy moment.** No stretch over 5 s with only "Thinking".

The baseline today: T1 took 16 m 33 s in Brigadier. An estimated 6–9 min in `/delegator`.

## What this supersedes in docs/PLAN.md

- §1, the sentence "The orchestrator never does work itself".
- §2:
  - principle 1, "The orchestrator only talks";
  - principle 4, an automatic verifier per outlined phase.
- §5 "Permission level": the default becomes Full access (Q6).
- §6:
  - Phase 3 deliverable "Orchestrator runtime… no built-in tools", its MCP tool list, and "A phase lands once, after its verifier";
  - Phase 6 "One verifier per phase".
- §9:
  - row Q2 (pure orchestrator);
  - row Q27's default;
  - the 2026-10-05 "Delegator-style flow" row: the parts that make outline review, verifier and the lead's `request_review` fixed steps.
- §10:
  - 10.1 rows on the conductor;
  - 10.6 (already superseded);
  - 10.15, replaced by §3 phase 6 here.

Everything else in PLAN.md stands. That includes §7's rules: no caps, lossless, no settings, judged per completed task.

## 2. Per grill decision: delete / keep / change / add

**Q1: the main thread is a real CLI session with tools; lean start; voice rules.**
- **Change** the thread's spec, `M/conversation.rs:1573-1603`:
  - `access: Access::ReadOnly` (`:1585`) → from the session's permission level (Q6).
  - `ToolSet::None` (`:1588`) → new `ToolSet::Thread`.
  - **Keep** the cwd as `data_dir/orch/<id>` (`:1481-1486`): the thread's own scratch, registered by `prepare_owned_dir` (`:1731-1745`) as `ScratchDir` + `ProcessesIn`.
  - **Add an "effective workspace"**: the session checkout, or the run worktree during an overnight run (Q10). It reaches the CLI as `--add-dir <workspace>` (Claude) or a writable root plus per-command workdir (Codex), and it is named in the prompt.
  - The workspace is **never** registered under the thread's owner. So hibernation's `end_processes` (`M/lifecycle.rs:313`) and the scratch sweep can't reach previews or anything else that runs there.
  - A stable cwd keeps `--resume` working: Claude finds sessions by cwd. When the workspace changes (a merge to a new path, run start or end), the CLI restarts between turns with `--resume` and the new `--add-dir`, and gets a `[workspace]` note.
  - The repo's CLAUDE.md/AGENTS.md reach the thread through `instructions::` (as for workers, `M/workers.rs:1211`), not through CLI auto-discovery.
- **Add** `ToolSet::Thread` (`P/model.rs:791-804`):
  - Claude: `--tools "Read,Grep,Glob,Bash,Edit,Write,WebSearch,WebFetch"`. No Agent/Task, no Monitor. Add it next to `:224-231`.
  - Codex: default tools, keeping `RESTRICTED_FEATURES` for multi-agent (`P/codex/mod.rs:75-84`).
- **Delete:**
  - the thread's use of `orchestrator_lockdown()` (`P/codex/mod.rs:106-131`; called at `M/conversation.rs:1685, 3015`);
  - the blanket denial "this session only talks" (`M/conversation.rs:2385-2395`). Approvals then follow the permission level, as for workers (`M/workers.rs:1967, 2190`, `M/cards.rs:79-92`).
- **Change** the prompt. `prompts::orchestrator` (`M/prompts.rs:47-106`) is replaced by `prompts::thread`:
  - Taken from `/delegator` SKILL.md §2–§5: delegate by default, answer workers, judge reports, keep a ledger in the Brain, never reopen settled decisions.
  - The tiny-edit rule (Q4) and preview use.
  - "Never answer 'I can't' for something a shell can do; do it."
  - Keep `VOICE`/`ORCHESTRATOR_VOICE`/`SHORT_REPLIES` (`:176-203`). The thread's own edits follow `WORKER_CODE_RULES` (`:553-560`).
  - Remove the 8-step pipeline loop (`:74-81`).
  - No vendor or skill names in prompt text.
- **Keep (lean start):**
  - `--strict-mcp-config` and `--setting-sources project` (`P/claude/mod.rs:203-211`): no user MCP or skills.
  - The only MCP server is Brigadier's (`M/conversation.rs:1708-1727`), with the worker-control, Brain and code-index tools (Q2).
- **Keep:** the `[quiet]` filter (`M/conversation.rs:3772`) and `UNANSWERED_GRACE` (`:215-226`), because reports still arrive as turns.

**Q2: keep the product, make the pipeline optional.**
- **Keep the worker-control interface** (`crates/mcp-server/src/catalog.rs:210-282`) as the thread's async tools:
  - `delegate_task` (routed, watchable, either vendor), `message_worker`, `answer_worker`, `stop_worker`, `read_report`, `read_artifact`, `list_tasks`, `ask_user`, `note_for_user`, `request_approval`;
  - `land_phase`, renamed in the UI to "land", with no phase required; `finish_session` (the merge card);
  - `query_brain`, `remember`, `search_transcript`.
- **Add for the thread:**
  - `code_search`, `code_refs`, `project_map` (today worker-only, `catalog.rs:311-313`);
  - `review_plan` (non-blocking, Q8 lever 5);
  - `start_verifier` (optional, thread decides). It wraps `start_verifier` (`M/phases.rs:752-802`), whose brief (`:772-775`) is rewritten in phase 1. Instead of the deleted `request_review`, the verifier calls a new non-blocking worker tool `review_code`. That tool starts the Q12 one-shot review of base..HEAD and returns at once; the findings arrive as a message while the verifier checks the done-whens, and it triages them before reporting. A landing whose (base, tip) already has a `ReviewRun` doesn't start a second one;
  - `start_preview` / `stop_preview` / `preview_log` (Q6);
  - `run`, for Codex only (Q4).
- **Add for workers:** `review_code`, which is non-blocking and replaces the deleted `request_review` (`catalog.rs:297`). Its findings arrive as a message to the worker.
- **Keep `approve_outline` and the phase records until phase 5.** The thread uses them only when it chooses an outline or phases. They hold state that a plain message doesn't:
  - `go_ahead` (`M/phases.rs:609-638`) clears the outline block, keeps the corrections with the lead and sets `Building`;
  - `held_by_plan_mode` (`:641-655`) reads that stage;
  - under Ask, `approve_outline` opens the user's card (`:498-561`, `outline_decided` `:564`).

  The record consumers that stay are:
  - `submit_outline` (`:270-367`, which makes a plan if none exists);
  - `start_verifier` (sets `Verifying`);
  - `land_phase` (`M/landing.rs:76-125`, sets `Landing`);
  - the conductor's `propose_phases` → `record_phases` (`M/overnight/conductor.rs:874`, `M/phases.rs:95`);
  - the rebirth `phase_briefing` (`M/rebirth.rs:637`);
  - the plan card (`PlanCardView.tsx:163`, `PinnedSummary.tsx`).

  Phase 1 deletes only the `OutlineReview` stage (`crates/core/src/work.rs:859-875`; outlines go straight to `AwaitingGoAhead`) and the "after one review" wording in `go_ahead`. Phase 5 decides what is left, once the conductor is gone.
- **Keep unchanged** (each on its own grill line):
  - watchable worker cards (`apps/desktop/src/app/conversation/cards/TaskCardView.tsx:90`, `WorkerThread.tsx`);
  - quota vendor fallback (`M/fallback.rs`, `M/conversation.rs:2679-2700`);
  - rebirth (`M/rebirth.rs`, `crates/core/src/knowledge.rs:256-290`) and cold rebirth (`M/cold.rs`);
  - the session branch, worktree and merge click (`M/landing.rs:1025-1177`);
  - the three access levels (`crates/core/src/model.rs:128-138`);
  - overnight (Q10), side chat, dictation, usage.
- **Router:** the thread model stays the user's pick (`model.rs:263-268`, `apps/desktop/src/lib/setup.ts:208-218`). Workers are routed (Q7).

**Q3: success = A/B.** See §3: a baseline in phase 1, a measurable check in every phase, the final A/B in phase 6. The live line, the workers row and the "Worked for" timer already shipped on main (`apps/desktop/src/app/conversation/liveStatus.ts:67-84`, `ThreadStatus.tsx`, `blocks.ts:174-189`). They only need to stay correct under the new engine.

**Q4: the thread mostly delegates; tiny edits itself; tool output trimmed losslessly.**
- The tiny-edit rule lives in `prompts::thread`: a few lines, in files it has already read, with a quick check.
- Thread commits carry the trailer `Brigadier-Author: thread`, using the git crate's `crates/git/src/trailers.rs`. They land on the branch of the effective workspace: the session branch, or the run branch overnight. So they get the Q12 review like any landed change.
- **The digest has a byte budget of 4,096 bytes**, built in this order and cut at line boundaries:
  1. a header (≤ 200 B) with the exit status and `[full output: read_artifact out-<id>, N lines, M bytes]`;
  2. lines matching error/warning/FAIL/panic, up to 2,048 B, then `[+K more matching lines in the full output]`;
  3. the head and tail lines, alternately, until the budget is used, then `[… L lines omitted …]`.

  The full output is always stored in the blob store first.
- **Claude trimming:** a `PostToolUse` command hook (a new `brigadierd hook post-tool-use` subcommand) is passed through Brigadier's own `--settings`. That flag still applies under `--setting-sources project`.
  - It runs for `Bash` only. File reads are what the model asked for, and §7 found trimming them lossy.
  - It applies to **successful** results over 8 KB. It returns `hookSpecificOutput.updatedToolOutput` with **the same object shape as that tool's `tool_response`**, with only `stdout`/`stderr` replaced by the digest.
  - **Failed tool calls are not trimmed.** They fire `PostToolUseFailure`, which can only add `additionalContext` and can't replace the output (hooks docs). Brigadier still stores their output and adds no context.
  - Phase 2 step 1 checks live on the installed CLI:
    - the exact Bash `tool_response` fields;
    - whether a non-zero exit is a "failure" (memory from 2.1.283 says PostToolUse doesn't fire for failing commands);
    - that a malformed replacement is ignored, so the hook is tested against the real shape.
- **Codex trimming:** Codex hooks stay disabled (`P/codex/mod.rs:97-104`). The Codex thread gets a Brigadier `run` MCP tool (command, workdir, timeout).
  - It runs under the thread's access level through the sandbox crate.
  - It returns the same digest for **every** result over 8 KB, failures included, because Brigadier owns that call.
  - The prompt asks Codex to use `run` for builds, tests, logs and long listings.
  - Exactly these Codex calls are trimmed: `run` results. Its built-in shell and file tools are not; that is a known gap (§5).
- The orchestrator's MCP timeout of 120 s (`M/conversation.rs:63`) is raised for `run`, to 30 min.

**Q5: workers headless; "Open in terminal" resumes the worker's own session.**
- **Keep:** headless stream-json workers (`M/workers.rs:1095-1322`).
- **Add** an "Open in terminal" button on the worker card (`Agents.tsx:67`, `TaskCardView.tsx:90`). The daemon:
  1. waits for the turn to end (or interrupts it);
  2. closes the headless CLI and marks the task `TakenOver`;
  3. opens a PTY (`crates/daemon/src/terminals.rs:84-96`, `TerminalTab.tsx:401`) in the task worktree, running the worker's own command with the same flags: `claude --resume <native id> --settings … --mcp-config …`, or `codex resume <thread id> -c …`;
  4. when the terminal exits, resumes headless supervision (`M/workers.rs:1344-1376`) and asks for a report.
- Retention: the task's worktree and CLI session files are kept while it is taken over. `dispose_task` (`M/workers.rs:3387-3434`) waits for the terminal. Stop or archive closes the PTY first.
- **A feasibility spike ends phase 2** (correction 8). It checks both vendors: resume a stream-json / app-server session interactively; no concurrent writers; supervision is handed back; the resumed headless session sees the terminal turns.

**Q6: Full access default; sandbox fixed for the other levels; previews; only a worker's own leftovers killed.**
- **Change** `Settings.default_permission` (`model.rs:527`, default at `:597`) to `FullAccess`. It is remembered per project as before (`sessions.rs:1848`).
- **Thread and workers both follow the conversation's level** (correction 5). The thread gets the same mapping as workers (`M/workers.rs:1416-1458`, `P/claude/mod.rs:421-442`, `P/codex/mod.rs:1056-1116`):
  - Full → `bypassPermissions` / `never` + `danger-full-access`;
  - Approve for me → `auto` / `on-request` with the auto-reviewer, writable roots = session checkout + scratch + `toolchain_roots` (`M/workers.rs:3678-3719`);
  - Ask → `acceptEdits` + cards.
- **Fix the sandbox** under Approve for me and Ask:
  - Chromium (`MachPortRendezvous` denied) and `git commit` (`index.lock` denied, even though the git common dir is a writable root, `M/workers.rs:1439-1444`) must work for both vendors with no escalation.
  - First find the cause: Codex's own `.git` protection is suspected, and Claude's sandbox needs the needed mach lookups.
  - Remove the "GUI checks can't run" line (`M/prompts.rs:699`).
- **Add previews** (correction 4). A new `M/preview.rs` holds daemon-owned processes started by `start_preview{command, env}` in the effective workspace:
  - They are spawned in their own process group with ledger owner `preview:<conversation>`. They are not under any CLI's `Tree` (`P/process.rs:83-150`), so provider teardown (`:116-150`) doesn't reach them.
  - The workspace is not a `ProcessesIn` dir of the thread's owner (Q1), so hibernation's `end_processes` (`M/lifecycle.rs:313`), the `end_in_dir` sweep (`crates/core/src/ledger.rs:489-521`), rebirth and vendor fallback don't reach them either.
  - The `session:<id>` owner does record its worktree. Its disposal (merge, archive) stops previews first, by design.
  - A preview belongs to one workspace. When the workspace changes (run start or end), its previews stop.
  - Logs go to the blob store.
  - They are stopped by `stop_preview`, stop, archive, delete, merge (Q9) and daemon quit.
  - For Brigadier itself, the preview recipe uses a dev identity and a scratch `BRIGADIER_DATA_DIR`. It never uses `ai.brigadier.app`.
  - UI: a "Running · Stop" chip in the thread header.
- **Keep** the kill-at-task-end sweep (`crates/core/src/ledger.rs:489-521`, `M/workers.rs:1561-1578`). It only touches the worker's own worktree and scratch, which is what Q6 wants. Previews don't live there.

**Q7: router role floors.**
- **Change** `default_floor` (`crates/router/src/decide.rs:161-170`) and the role hook in `create_task_as` (`M/workers.rs:817-819`):
  - Lead, Fix, Verifier, Merge and Reviewer roles, and any code-writing or review kind, get floor `Frontier` and effort `high`.
  - Scout, summaries and other chores get `Light` and effort `low`.
- **Change** `category_effort` (`decide.rs:195-203`): Implement goes from medium to high.
- **Keep:** the router still picks freely above the floor (`decide.rs:466-622`), and Fable stays excluded (`decide.rs:1019-1022`, `router/src/merge.rs:128`).

**Q8: the speed levers.**
- **Lever 1, context pack.**
  - Phase 2 records what the thread read: `Read` paths and line ranges and `Grep` hits, taken from the thread's tool events on `orch:<id>`.
  - Phase 3 writes `<scratch>/context.md` for each worker: the index digest (`crates/index/src/lib.rs:686`) scoped to the brief's areas, plus those reads. It is hooked at `M/workers.rs:1213-1230`.
- **Lever 2, lean shared prefix.** The worker system prompt becomes a fixed per-vendor prefix (`M/prompts.rs:582-670` reordered, with the static parts first). The task brief moves to the first user message.
- **Lever 3, checks.** Today there is no result cache (`crates/core/src/work.rs:325,340`).
  - Add a check cache keyed by (tree hash including uncommitted changes, command, relevant env) → exit, digest, log blob. Workers run checks through a `run_check` tool.
  - "Only affected checks": a per-project path → check map, learned into the Brain.
  - A shared per-project `CARGO_TARGET_DIR` for check builds. The build lease already serializes them (`crates/core/src/machine/builds.rs`).
- **Lever 4: one-shot cross-vendor reviews** (Q12).
- **Lever 5: non-blocking plan review.** `review_plan` runs a one-shot review in the background. The findings reach the thread as a message, and the worker is never parked on it.
- **Lever 6: pre-warmed worker.** When a user message arrives, the daemon prepares one task worktree from the session tip (`M/warm.rs:156`; per-task today, no pool) and one idle CLI for the default implement route. It is used by the next `delegate_task`. It is dropped when the tip moves or after 10 min.
- **Lever 7: cheap models for chores.** Router floors (Q7).
- **Lever 8: metrics.** Per-step time and tokens:
  - `TurnUsage` (`crates/core/src/routing/store.rs:85-96`) gets `duration_ms` and the step kind (thread turn, worker, review, preview).
  - Store the Claude `total_cost_usd` already parsed (`P/claude/parse.rs:654-664`; Codex has none).
  - A per-request summary goes in the Inspector.
  - `tools/ab/`, ported from the 10-05 phaseE tools, runs the A/B automatically.

**Q9: after merge, the session worktree and branch are removed; the next message starts fresh.**
- Today a merge (`M/landing.rs:1122-1145`) leaves both. They only go on archive (`M/lifecycle.rs:496-525`).
- **Change:** after the merge lands, stop previews, dispose `session:<id>`, and delete the merged `brigadier/…` branch, reusing `lifecycle.rs:496-525`.
- On the next message, `ensure_target` (`M/workers.rs:1640-1747`) recreates the worktree **at the same path** on a new branch from the base tip. The thread's `--add-dir` stays valid with no CLI restart (its cwd is its own scratch, Q1).
- Reviews still running are not affected (Q12).

**Q10: overnight is the same thread plus a deadline.**
- **Delete** the conductor's phase loop:
  - `M/overnight/conductor.rs`: `advance_run`, `hand_to_lead`, `nudge_lead`, `pick_next`, `phase_done`, `propose_phases`;
  - the per-phase CLI restart `lead_phase` (`M/conversation.rs:2873-2877`);
  - the phase gating in `M/overnight/policy.rs`.
- **Keep:**
  - the directives parser (`M/overnight/directives.rs`) and `Deadline`/`Directives` (`crates/core/src/overnight.rs:398-458`);
  - `wind_down.rs`, `admission.rs` (worker cap), `workspace.rs` (run branch);
  - `awake.rs` and `overnight_supervisor.rs`;
  - `report.rs`, re-pointed at requests, tasks, reviews and usage instead of phase records.
- **Change:** a run is a `[run]` note to the same thread (the mechanism at `M/prompts.rs:241-255, 399-406`) carrying the plan, the deadline and the directives. The thread works through it with the normal tools and writes the morning answer. `propose_overnight` stays as the Start card.
- **Workspace switch at run start and end** (`M/overnight/workspace.rs` makes the run worktree):
  - The thread's effective workspace (Q1) becomes the run worktree: between turns the CLI restarts with `--resume` and `--add-dir <run worktree>`, so the native session continues, and a `[workspace]` note names the switch.
  - Thread reads, checks, `run`, previews and tiny-edit commits all use the run worktree and land on the run branch.
  - The session checkout is out of the writable roots for the run (run isolation, PLAN §10.5).
  - At run end it switches back the same way. The run's previews stop at each switch.

**Q11: the build path.** This doc → Codex review → the user's approval → a `/delegator` run in a worktree (§4), each phase checked against the A/B. After that, Brigadier builds itself.

**Q12: a review floor in code** (correction 3).
- **Delete:**
  - the full-worker review: `run_review` (`M/phases.rs:416-474`), `review_outline` (`:371-412`) and the reviewer task brief (`M/landing.rs:669-713`);
  - the blocking `request_review` (`M/phases.rs:660-721`; tool `catalog.rs:297`, `crates/core/src/tools.rs:610`);
  - `needs_verifier` and the automatic verifier (`M/phases.rs:725-746`, `M/workers.rs:2518-2546`, note `:2591-2601`).
- Lower `WORKER_TOOL_TIMEOUT_SECS` (24 h, `M/workers.rs:71`) to `QUESTION_TIMEOUT` (1 h).
- **Add** a one-shot runner in `crates/review/src/lib.rs` (empty today):
  - **Codex:** `codex exec review --base <base> -o <file> -c model_reasoning_effort="high"`. No custom prompt with `--base`; a focused review names the range in the prompt instead (as `dlg review`, `~/.claude/skills/delegator/scripts/dlg:420-433`).
  - **Claude:** started through the existing provider adapter (`P/claude/mod.rs:197-279`, `settings()` `:450-571`) with `Access::ReadOnly`. That gives `-p --output-format stream-json`, `--tools "Read,Grep,Glob,Bash"`, the read-only sandbox (`:520-525`), `--strict-mcp-config` and `--setting-sources project`. The diff range goes in the prompt.
    - The permission mode is `dontAsk`, plus `--allowedTools "Read Grep Glob Bash(git diff:*) Bash(git log:*) Bash(git show:*)"`. Anything not allowed is denied and never prompts.
    - Today `permission_mode()` maps ReadOnly to `"default"` (`P/claude/mod.rs:428`). That value is not among 2.1.292's listed choices (`acceptEdits, auto, bypassPermissions, manual, dontAsk, plan`), although `claude -p --permission-mode default --version` parsed without error and `bogus` was rejected.
    - Phase 1 moves ReadOnly to `dontAsk` for every read-only session and checks a review launch live.
- **Each `ReviewRun`** records conversation, `base`, `tip`, author vendor and reviewer model. It is triggered after every landing: `landed` (`M/landing.rs:595-666`) and thread commits.
  - It runs in a detached worktree at `tip` (`git worktree add --detach`) owned by `review:<id>`. So landing disposal (`:638`), merge and the next session branch don't remove it.
  - The reviewer is always the other vendor from the author.
- **Findings** reach the thread as a `[review …]` message, even after an early merge. The thread fixes them (a fix worker or a tiny fix on the current session branch) or notes why not.
- **Merge card** (`ActionCards.tsx:359-366`, `cards/ApprovalCardView.tsx:130-135`): "Review running…", then "Review: clean" or "N findings". Merge is allowed before the review finishes.
- The thread may still add a verifier or a deeper review for big or risky work.

**Q13: no edit guard; metrics to tune the prompt.** Per session, measure:
- thread self-edit lines, from git, via the `Brigadier-Author: thread` trailer;
- thread context growth per request, from `turn_usage` on `orch:`.

Both show in the Inspector. Nothing blocks edits.

**Q14: old-engine chats are deleted, with no compat.**
- On the first start of the new engine, a store marker `engine: thread-1` triggers deletion of every `kind == Session` conversation created before it, through the normal delete path (`M/lifecycle.rs:627, 711-783`). That purges `conversation:`, `orch:`, `task:` and `draft:` streams, blobs, `routing.sqlite` rows and Brigadier's branches.
- Plain Chats are kept: their engine doesn't change.
- Then delete the dead types and shims, each only once nothing reads it:
  - in phase 2: `legacy.rs` (`crates/core/src/legacy.rs:8-19`) and `Gate`/`GateMember` (`crates/core/src/work.rs:438`);
  - in phase 1: the `OutlineReview` stage (`work.rs:859-875`);
  - in phase 5, after the conductor is gone: whatever phase records and stages no consumer listed under Q2 still needs.

## 3. Phases

Shared work comes first, so parallel streams don't edit the same contracts (correction 6). Phases 1 and 2 are sequential. After phase 2, only the streams listed in "Parallel" may run at once, and they are integrated one after another.

**Measuring (all phases).** Port the 10-05 phaseE tools (`~/.claude/delegator/runs/20261005-0049-takeover-3-sessions/msgs/phaseE/tools/`: `startd.sh`, `send.py`, `recorder.py`, `timeline.py`, `tokens.py`, `brig_manifest.py`, `dlg_manifest.py`, `replay`) to `tools/ab/`.
- A Brigadier arm is a dev `brigadierd` with its own `BRIGADIER_DATA_DIR`, driven over IPC, in a throwaway clone at the task's base.
- **Time:** from submit to verified landing, and to the final answer.
- **Tokens:** per provider, from that data dir's `routing.sqlite` `turn_usage` (Brigadier); from `~/.claude/projects/<clone-slug>/*.jsonl` and `~/.codex/sessions` rollouts per a session manifest (`/delegator`). Input, cached input, cache writes and output are kept separate. Unknown is recorded as unknown, never as 0.
- **"Only Thinking":** the gaps come from replaying recorded events through the app's row code.
- The installed app and its data are never used.

### Phase 1: Baseline, then cut the fixed pipeline (biggest speed win)

**Scope:**
1. **Baselines first.** Freeze T1 and T2: request text, base, done-when and check commands. Run each once in cmux `/delegator` and once in current Brigadier (`7cff0db9`). Save `docs/evidence/<date>-thread-baseline.md`.
2. Q12 one-shot review runner and `ReviewRun` (§2 Q12), with the merge-card review status and findings → thread.
3. Delete the automatic outline reviewer, the automatic verifier and `request_review`. In the same phase, migrate everything that depends on them (correction 1):
   - orchestrator prompt steps 3, 6 and 7 (`M/prompts.rs:76, 79, 80`);
   - `LEAD_STEPS` (`:545`, which drops `request_review`);
   - the verifier brief (`M/phases.rs:772-775`): `request_review` → the new non-blocking `review_code`;
   - the `OutlineReview` stage and `go_ahead`'s "after one review" wording (`M/phases.rs:631-636`). `approve_outline`/`go_ahead`/`held_by_plan_mode` and the Ask card stay;
   - `submit_outline` (`M/phases.rs:270-367`): outside plan mode it now delivers the outline to the orchestrator at once, like the plan-mode branch at `:345-358`, and starts a background `review_plan`. The worker waits only for the go-ahead, which the orchestrator gives right away. No worker can hang on a removed reviewer.
   - the overnight conductor's "land the verifier" texts (`M/overnight/conductor.rs:279, 494, 631-673, 769, 1214-1218`);
   - `land_phase`'s verifier refusal (`M/landing.rs:91-96`) now applies only to a verifier the orchestrator started.
4. Q7 role floors and effort.
5. Q6 Full access default and the sandbox fixes (Chromium, git commit).

**Done when:**
- T1 in Brigadier lands verified in ≤ 8 min, down from 16.5. Tokens ≤ 0.6× the Brigadier baseline. Measured with `tools/ab`.
- 0 automatically started verifiers or review workers. Every landing has a `ReviewRun` whose findings reach the thread, including one merged before the review finished.
- Under Approve for me, `pnpm test` (Chromium) and `git commit` succeed for a Claude worker and a Codex worker with 0 escalations.
- An overnight smoke (2 small phases) still completes on the old conductor with the new texts.
- An optional verifier, started by the orchestrator, calls `review_code`, gets the findings as a message, triages them and reports. No `request_review` is left (`rg request_review crates apps` is empty).
- A Claude one-shot review of a Codex-authored landing launches under `dontAsk` and returns findings.
- `tools/full-checks.sh` passes.

### Phase 2: The thread gets tools (shared contracts)

**Scope, in this order:**
1. Live-check the CLI contracts (§5): the Bash `tool_response` shape; whether a non-zero exit fires `PostToolUseFailure`; and `--resume` with a changed `--add-dir` (session continues, cache read).
2. `ToolSet::Thread`, the thread spec under all three levels, and the worker-control tool set (§2 Q2).
3. Lossless trimming: the Claude hook and the Codex `run` tool.
4. `prompts::thread`.
5. Thread read tracking, for phase 3's context pack.
6. Previews (`M/preview.rs`, the tools and the UI chip).
7. Live-line words for thread tool steps (`OrchestratorSteps.tsx`, `toolWords.ts`, `liveStatus.ts:67`).
8. Q13 metrics.
9. Q14 deletion of old sessions, and of `legacy.rs` and `Gate`/`GateMember`. Phase records stay until phase 5 (§2 Q2).
10. End with the "Open in terminal" feasibility spike for both vendors, written to `docs/evidence/`.

**Done when:**
- T1 lands verified in ≤ 7 min.
- "Run the app so I can see it" makes the thread start a preview: 0 "I can't" over 5 scripted asks (run the app, show logs, run tests, check a file, open a port).
- The thread runs Read/Bash/Edit under Full access, Approve for me (sandboxed, escalation via the auto-reviewer) and Ask (card shown), for both vendors.
- A preview survives the thread's hibernation, a forced rebirth (`BRIGADIER_REBIRTH_TOKENS`) and a forced vendor fallback, and is gone after stop, archive and merge.
- **Trimming:**
  - A successful Claude `Bash` call with 50 KB of output (e.g. a passing `cargo test` log) reaches the model as ≤ 4,096 bytes, with the exit status, the `read_artifact` reference and the matching lines up to their budget (overflow count shown).
  - `read_artifact` returns the full 50 KB byte-identical.
  - A **failing** Claude `Bash` call reaches the model untrimmed, and its full output is still stored.
  - A failing Codex `run` with 50 KB of output reaches it as ≤ 4,096 bytes, error lines first.
- Old sessions are gone after the first start and plain Chats are kept.
- The preview checks run with the preview's cwd inside the session worktree, and after a hibernation `ps` still shows it. Both provider teardown and the ledger's `end_processes`/`end_in_dir` were exercised.
- `tools/full-checks.sh` passes.

### Phase 3: Workers start warm and lean

**Scope:**
- the context pack (lever 1);
- the shared, cache-friendly worker prefix (lever 2);
- the pre-warmed worktree and CLI (lever 6);
- the check cache, affected checks and shared target dir (lever 3).

**Done when:**
- From `delegate_task` to the worker's first event takes ≤ 2 s, down from 5–7 s.
- A second worker's first call reads ≥ 50% of its prompt from cache.
- In T1, no check command runs twice on the same tree (cache hits are logged).
- T1 lands verified in ≤ 6 min. Tokens ≤ the phase 2 run.

### Phase 4: Lifecycle, terminal takeover, live line under the new engine

**Scope:**
- Q9 merge cleanup with a same-path fresh branch;
- "Open in terminal" for both vendors, built on the spike;
- keep the live line, the workers row and "Worked for" correct for thread tool steps, previews and background reviews;
- the per-request time/token summary in the Inspector.

**Done when:**
- After merge, the session worktree and branch are gone. The next message gets a fresh branch from the base tip, and the thread resumes (not reborn).
- "Open in terminal" round trip for a Claude worker and a Codex worker: the session's earlier turns are visible, an edit made there shows up in the headless report, and no two writers run at once.
- The longest stretch with only "Thinking" in T1 is ≤ 5 s.

### Phase 5: Overnight on the same thread

**Scope:**
- Q10: delete the conductor loop and re-point the report.
- The workspace switch at run start and end.
- Then delete the phase records and types nothing reads any more (`record_phases`, `PhaseStage`, plan-card fields), or keep the ones `approve_outline` still needs.

**Done when:**
- A 2-phase plan with a 20-min deadline runs on the normal thread. It winds down at the deadline, writes the morning report (commits, reviews, waiting items, usage) and keeps the machine awake during the run.
- No code from `conductor.rs`'s phase loop remains.
- During the run, a thread tiny edit lands on the run branch and a thread preview runs from the run worktree. The session branch is unchanged. After the run, the same native thread session continues (same session id) in the session checkout.
- Stop and restart mid-run recover.

### Phase 6: A/B against cmux /delegator

**Scope:** T1 and T2, both arms:
- fresh clones at the frozen base, warmed the same way;
- run one after the other, `/delegator` first, each on a fresh Claude 5-hour window;
- Brigadier on its normal defaults (Full access, router floors);
- `/delegator` with its usual Opus 5.5 high.

An independent evaluator runs every done-when command on both arms' results. Write `docs/evidence/<date>-thread-ab.md`.

**Done when**, for both tasks:
- Brigadier is faster to verified landing and to the final answer;
- Brigadier uses fewer tokens per provider, both raw and without cache reads;
- both arms pass the same checks with the same work done;
- 0 "I can't";
- no only-"Thinking" stretch over 5 s.

If a task is inconclusive, rerun it once with the arm order swapped. Missing evidence is not a win.

**Parallel** (only these):
- After phase 2: phase 3 (`M/warm.rs`, `M/workers.rs` spawn path, `M/machine*`, `prompts::worker`) alongside phase 5 (`M/overnight/*`, `crates/daemon/src/awake.rs`). These files are disjoint.
- Phase 4 follows phase 3, because it touches `dispose_task` in `M/workers.rs`.
- Phase 6 comes last.

## 4. Brief template for the build run

> `/delegator` goal. Build docs/THREAD-PLAN.md, phase **N**: "<title>". Read docs/THREAD-PLAN.md in full, plus docs/PLAN.md §7 and the sections it supersedes. Scope and done-when: exactly phase N's text in §3. Send an outline first and wait for "Go ahead." Report every done-when with the command you ran and the numbers you saw (`tools/ab` for time and tokens).
>
> **Rules**
> - Integration branch `thread-build`, created once from main at the run's start in worktree `../brigadier-ai-thread`. Each phase works in a worktree `../brigadier-ai-thread-<phase>` on a branch from the **verified tip of `thread-build`**. After its verifier passes, the Delegator fast-forwards `thread-build` to it (rebasing first if needed).
> - The parallel streams after phase 2 (phase 3 and phase 5) both start from the same verified phase-2 tip of `thread-build`. They are integrated one after another: the second rebases onto the first and is re-verified.
> - Never edit, build or commit in the main checkout.
> - No push, no merge into main, no PR, no release, until the user says so.
> - Follow project memory (`~/.claude/projects/-Users-stephen-Development-brigadier-ai/memory/`). Settled decisions (THREAD-PLAN §2, the grill Q1–Q14) are not reopened. Something that looks impossible goes in the report with a recommended fix.
> - Never touch `/Applications/Brigadier.app`, its daemon or `~/Library/Application Support/Brigadier`. Test with a dev identity and a scratch `BRIGADIER_DATA_DIR`. Never smoke-test the installed bundle on a temp dir.
> - No backups (no `.bak`, no `backup/*` branches, no app copies). Clean up your own scratch, test identities and processes. Kill only PIDs you started; never `pkill`/`killall` by name.
> - Models: never Fable; Opus 5.5 by default; Sonnet 5 only for mechanical chores; Codex for reviews and bounded tasks; effort never above `high`, no ultracode.
> - Never name the reference app or its vendor in the repo (see memory). No user-facing quality knobs: ship the best behaviour on.
> - Check every third-party CLI flag or API against the installed version's `--help` or current docs, and cite it.
> - Before you report: `tools/full-checks.sh`, `cargo fmt`, `clippy`, `gen-ts`, `pnpm` checks on the exact tree.

## 5. Open risks and the user's decisions

**Risks** (each with the recommended handling):
1. **Claude hook shape.** The docs say `updatedToolOutput` replaces a built-in tool's result and must match its shape. Memory (CLI 2.1.283) says Bash's shape was `{stdout, stderr, interrupted, isImage}`. Failures go to `PostToolUseFailure`. Phase 2 step 1 checks this live on 2.1.292. If replacement fails, keep failures untrimmed.
2. **Codex built-in shell output isn't trimmed** (hooks are off). Use `run`, and enable Codex hooks only after a measured test (PLAN §7).
3. **The thread's context grows faster with tools.** Rebirth fires sooner. Track it with the Q13 metrics. Its briefing must list recently read files.
4. **Beating `/delegator` on time.** Brigadier adds worktree warm-up and landing, which `/delegator` doesn't have. The pre-warm (lever 6) and the check cache must pay for them.
5. **Shared `CARGO_TARGET_DIR`** across worktrees can churn fingerprints. Measure it in phase 3. Fall back to per-worktree CoW-warmed targets.
6. **Open in terminal on Codex.** `codex resume <id>` on a thread made by app-server is unproven. That's what the spike is for. If it fails for either vendor, phase 4's takeover part stops. A revised takeover design (for example a PTY-hosted worker from the start) is written, Codex-reviewed and approved before it's built. Transcript viewing is not a substitute: that would be a scope change to Q5, and only the user can make it.
7. **ToS.** Only the user's own unmodified `claude` binary is used. `claude --bg`/agents are API-key-only and not used.
8. **T1's exact request text** is in §1 (copied from the app); its image is in the run's evidence folder.

**The user decided (2026-10-07):**
1. Full access is the default for every project the user adds, remembered per project. Yes.
2. Q14: delete everything from the old engine, sessions and plain Chats alike. No migration.
3. T2 (the thread-indicator request, base `53c5cd32`) is the second A/B task. Yes.
4. The "deciding" follow-up routing (PLAN §9 Q30) stays as it is; the user likes it. Revisit only if phase 2 shows a reason.
5. Baselines at the start of phase 1, a T1 run per phase, the full A/B in phase 6. Yes.

## Checked third-party contracts (2026-10-07)

- **`claude --help`, Claude Code 2.1.292:**
  - `--tools` (`""` = none, or a list such as `"Bash,Edit,Read"`);
  - `--disallowedTools`, `--settings <file-or-json>`, `--setting-sources`, `--strict-mcp-config`, `--mcp-config`;
  - `--permission-mode` (acceptEdits, auto, bypassPermissions, manual, dontAsk, plan);
  - `--input-format/--output-format stream-json`, `--include-partial-messages`, `--replay-user-messages`;
  - `-r/--resume`, `--fork-session`, `--session-id`, `--system-prompt-snapshot`, `--append-system-prompt`, `--add-dir`, `--effort`;
  - `--bg` (API key users only);
  - `--permission-mode bogus` is rejected with the list of choices; `-p --permission-mode default --version` parsed without error.
- **Hooks docs** (code.claude.com/docs/en/hooks, PostToolUse decision control): `hookSpecificOutput.updatedToolOutput` replaces a tool's result; `updatedMCPToolOutput` does the same for MCP; it fires after success only, and failures fire `PostToolUseFailure`, whose only output field is `additionalContext` (it can't replace output); the input carries `tool_name`, `tool_input`, `tool_use_id`, `tool_response`.
- **`codex --help`, `codex exec --help`, `codex exec review --help`, `codex resume --help`, `codex app-server --help`, codex-cli 0.160.1:**
  - `exec`: `-s read-only|workspace-write|danger-full-access`, `--approve-for-me`, `--dangerously-bypass-approvals-and-sandbox`, `--ephemeral`, `--json`, `-o`, `--output-schema`, `-c key=value`;
  - `exec review`: `--base`, `--commit`, `--uncommitted`, `[PROMPT]`;
  - `resume [SESSION_ID] [PROMPT]`;
  - `app-server` with `-c`, `--enable/--disable`.
