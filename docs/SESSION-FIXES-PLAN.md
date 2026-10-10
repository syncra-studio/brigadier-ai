# Session fixes plan: merges, worker deaths, resumes, tokens and the thread's habits

> Status: plan, 2026-10-11, from the 2026-10-10 grill of three real sessions (Q1–Q11) and the audit behind it.
> Branch `session-fixes-plan` from `main` at `7d8898ff`. File:line references are to `7d8898ff`.
> Next: Codex plan review → the user's approval → a `/delegator` build run, phase by phase (§7, §8).
> Paths: `M/` = `crates/core/src/manager/`, `P/` = `crates/providers/src/`, `C/` = `apps/desktop/src/app/conversation/`.
> Sessions are named as in the grill. Times are the user's (EEST), log times are UTC (3 h earlier):
> - **A**: `01a120a0…9a2b00c9`, the session-tabs work merged into `main` (15:09–17:49).
> - **B**: `01a125be…9f9d192a`, Settings > Configuration, merge auto-resolve and the preview chip (15:17–22:46).
> - **C**: `01a12648…a25518cf`, five computer-use fixes (17:47–19:52).

## 1. What this fixes, and the rule behind it

In three sessions the user hit the same kinds of trouble:
- Work went missing at merges.
- Workers were killed and then started again from zero.
- The thread was reborn when nothing called for it.
- Tokens burned while the Brain sat unused.
- The thread repeated its answers, misreported review counts and went quiet for long stretches.

The grill settled ten decisions. The audit found eight plain bugs. This plan maps each decision and each bug to its
code and a phase step, and says how it is verified.

**The principle (from Q2 and Q7).** Judgement belongs to the LLM thread and its workers: merges, a finding's status,
whether to watch CI, what to say. The engine does plumbing, hard rules (consent, permissions, ownership) and cheap
checks only. Where today's engine judges (it builds merge commits from files, it sums review counts), this plan moves
the judgement to the LLM and leaves the engine a check it can do in code.

## 2. What this supersedes

- **THREAD-PLAN §2 Q12, "Review status" bullet** (`Review: N findings` for the work since the last merge) and its
  copy in `C/reviewStatus.ts:30-43`: replaced by Q7 here. The line shows open findings only, with the status the
  thread recorded, and never a running total.
- **THREAD-PLAN §2 Q12, "Findings … The thread fixes them (a fix worker or a tiny fix …)"**: narrowed by Q6.1. One
  review per change; all its findings go back to the same, resumed worker in one batch; fixes get no new review,
  except one targeted re-check for a high-severity finding.
- **THREAD-PLAN §5 decision 6 / THREAD-PARITY §5 Q6 (merge card)**: kept, with two changes from Q3:
  - The card is offered only when no worker is writing.
  - Consent given while workers write is held, and landings no longer make it stale. Only the user's later words
    do ("wait", "no", a new ask, as `merge_consent::takes_back` already reads them).
- **THREAD-PARITY §8.2 / §5 Q6, "write tasks are still open … Land or stop them first"** (`M/landing.rs:1419`):
  replaced by Q3's hold.
- **THREAD-PARITY §5 Q9, "A late review: … the thread writes the ending again, updated"** (`M/prompts.rs:107`,
  `:1025`): replaced by bug 4's rule. A late event changes the answer only when it changes what the user must know,
  and then as one short update line, never a rewritten ending.
- **THREAD-PARITY §2.8, "Long-running processes live in the side panel, not above the composer"**: refined by Q8.
  Live work shows in one slot above the composer. Finished work moves to the side panel.
- **THREAD-PLAN §2 Q4/Q8 "warm workers"**: the shared worker home (`M/workers.rs:1314-1324`) stays for the prompt
  cache, but it is no longer swept when one worker ends (bug 1).
- **PLAN.md §6 phase 5, mid-task fallback**, for crashes only: a worker whose CLI dies now resumes its own session
  (Q4). The vendor switches only for quota, rate limits or an outage.
- **The rebirth threshold** `min(150k, 75% of the window)` (`crates/core/src/knowledge.rs:292`, from PLAN.md §6
  phase 4): now 400k on a window that holds it (Q5).
- **The landing's merge handling** (`M/landing.rs:209-330`, `:829-1035`; the 2026-10-10 "auto-resolve" work): Brigadier
  no longer merges for the worker or builds the merge commit from files (Q2).

Everything else in THREAD-PLAN and THREAD-PARITY-PLAN stands.

## 3. The decisions (Q1–Q10)

### Q2: merges are done by the LLM worker with real git; the engine checks
**Today.**
- A merge task starts from a merge that Brigadier made outside the sandbox, with conflict markers left in the files:
  `merge_start` (`M/landing.rs:893-948`) and `base_merge_start` (`:954-975`).
- The worker is told "Run no git command that changes anything … Brigadier builds the commit from the files"
  (`:883`, `:1033`).
- Landing then rebuilds the commit from the worktree's files (`move_commits`, squash branch, `:259-330`). The litter
  guard runs over them (`keep_paths` at `:284`, `:293`), and the second parent is set only when `base_merge` is
  present (`:210-213`, `:310-318`).
- In B, `main`'s 18 added files were new to the session's base and named in no report, so the guard left them out. In
  A, the commit landed with one parent and 116 of `main`'s files missing.

**Change.**
1. **The merge worker merges.** A merge task gets a worktree on a branch from the session tip (or the task's work).
   The brief says: run `git merge <base>` yourself, resolve every conflict keeping both sides' intent, build and test,
   then `git commit` normally. The report's `deleted` field lists any file you removed on purpose, with why.
   - Rewrite `merge_brief` and `base_merge_brief` for this.
   - Delete the pre-merge in `merge_start`, `base_merge_start` and `merge_for_resolution` / `merge_branch_for_resolution`
     in `crates/git`, once nothing calls them.
2. **The engine checks; it doesn't build.** A new `check_merge(worktree, tip, ours, theirs, report)` in
   `M/landing.rs` runs before a merge task lands:
   - **No conflict markers** in the tree: reuse `conflict_marker` (`:1848`) over the files the merge touched.
   - **Both parents present:** the tip is a merge commit whose parents include `ours` (the target's tip at the
     start, or a descendant of it) and `theirs` (the base tip it merged).
   - **Nothing vanished:** every file that either side added or changed since their merge base exists in the tip.
     The exceptions are a file the other side deleted, or one the report's `deleted` lists with a reason.
   - **Fail:** nothing lands. The worker is resumed (Q4) with the exact list: "Missing from the merge: a.ts (from
     main), b.rs (from the session). Restore them or list them under deleted with why."
3. **No litter guard on merges.** `move_commits` handles a `TaskKind::Merge` landing as a fast-forward of the target
   to the checked merge commit. `keep_paths` stays for normal task commits.
4. **The consent check before merging into the base stays in the engine** (`finish_session`, `merge_consent`,
   `M/landing.rs:1378-1720`). When a session merge stops at conflicts (`:1497-1517`), the thread is told to delegate
   a merge task. That worker merges the base into the session branch as above. `finish_session` then merges on the
   held consent (Q3).
5. **Sandbox.** A merge worker below Full access needs write access to the repository's git directory (the common
   `.git`: `objects`, `refs`, `worktrees/<name>`). Add that root for `TaskKind::Merge` only, in `access_for`
   (`M/workers.rs:4321`).

**Verify.**
- Rust tests in `M/flow/` (`base_merge_tests.rs` is rewritten):
  - Main adds 18 files and the session adds 3, with a conflict in one: after the merge task, the landed tip has two
    parents and all 21 files.
  - A worker that leaves a marker is refused, nothing lands, and the worker hears the file.
  - A worker that deletes a base file without listing it is refused; listed with a reason, it lands.
  - A normal task's litter (a stray `debug.log`) is still left out.
- Live, dev build: replay B's situation (a session branch behind a `main` that added files and touched the same
  sidebar file). `finish_session` hits conflicts, the thread delegates the merge, the merge lands, and the user's
  consent merges it. `git diff --stat main^1 main` shows no missing file.

### Q3: the merge is offered only when no one writes; a "Merge" said early waits
**Today.**
- `propose_merge` (`M/landing.rs:1238`) opens the card whatever runs.
- `finish_session` refuses while write tasks are open (`:1410-1421`), so the thread asks again.
- Consent held across a conflict goes stale when other work lands (`Prepared::Moved`, `:1446-1460`, `:1487-1491`).

**Change.**
1. `propose_merge` refuses while any write task is not final: "Workers are still writing (task-3, task-5): offer the
   merge once they land."
2. `finish_session` with valid consent while writers run no longer errors. It records `MergePending { asked_in,
   at_ms, branch, base }` on the conversation (stored, so it survives a restart) and returns `[merge held] Merging
   after 2 workers finish.`
3. **When the last write task is final** (landed, done, stopped or failed), the engine checks the pending merge:
   - If a write task failed or was stopped, or the thread recorded a high-severity finding as open (Q7), nothing
     merges. The thread gets `[merge stopped] task-5 failed: …` and tells the user why and asks.
   - Otherwise the thread gets `[merge due] The user's merge is due: call finish_session.` and merges on the held
     consent. The engine wakes the thread; the merge stays the thread's call.
4. **Consent goes stale only from the user's words.** `held_consent` and `fresh_consent` drop the `Prepared::Moved`
   refusal for work that landed from this session's own tasks after the consent. `merge_consent::takes_back`, the
   queue check and "Not yet" still refuse.
5. A **Cancel** on the pill (§Q8 slot) clears `MergePending` and tells the thread "[merge cancelled by the user]".

**Verify.**
- Rust tests:
  - `propose_merge` is refused while a writer runs, and accepted after it lands.
  - "Merge it" while 2 writers run → `MergePending`; both land → `[merge due]` → `finish_session` merges with no new
    question.
  - "Wait" written meanwhile → refused.
  - A writer fails → `[merge stopped]` and no merge.
  - The pending merge survives a daemon restart.
- Live, dev build: say "merge when done" with two workers running. The pill reads "Merging after 2 workers finish",
  then the merge lands without a second question.

### Q4: a worker that dies resumes itself
**Today.**
- A worker whose CLI exits before reporting is handed to another model from zero, with handoff files
  (`M/workers.rs:2370-2385` → `fallback::hand_off`, `M/fallback.rs:100-200`, up to `ERROR_HANDOFFS = 3`, `:52`).
- A daemon restart stops every running task: "Brigadier quit while this task was running."
  (`M/lifecycle.rs:158-164`).

**Change.**
1. **Crash → resume.** An exit with no report and no `take_cutoff` (exit null, a signal, a broken pipe, or the CLI's
   own crash) starts the same CLI session again:
   - `Origin::Resume { native_id }`, the same model and account, the same worktree.
   - The first input: "[Brigadier] You were interrupted (your process ended). Check what was running (its output
     may be lost), then continue the task."
   - `launch_admitted` already supports resuming (`M/workers.rs:1505-1525`).
2. **Daemon restart → resume.** `recover` resumes running write and read tasks the same way, instead of stopping them
   (`M/lifecycle.rs:158-164`). Tasks taken over in a terminal, and quota waits, keep their paths.
3. **Vendor switch only for quota, rate limits, auth or an outage:** `AttemptEnd::Limit`, `ErrorKind::Auth`, and
   `Overloaded/Server/Network` after the transient retry (`fallback::verdict`, `M/fallback.rs:66-90`). Never for
   `ErrorKind::Process`.
4. **Twice in a row → stop and say what kills it.** A task that dies again within its next resumed attempt (no
   report between) stops with `blocked_reason` set to the two exits (code or signal, the last stderr line) and the
   thread is told `[task-N stopped: died twice] …`. The thread tells the user.
5. **Check results stay cached per tree.** `check_results` already keys by tree and command. Bug 8 makes the command
   key canonical, so a resumed worker's re-run of a passed check answers from the cache.
6. **Its row** reads "Interrupted — resuming" while the resumed CLI starts (`C/workerPresentation.ts` state words).

**Verify.**
- Rust tests:
  - A fake CLI that exits mid-turn is resumed with the same `native_id`, model and worktree, and gets the
    interrupted note.
  - A second exit with no report in between stops the task with both exits named.
  - A usage-limit end still hands on.
  - After a simulated restart, a running task resumes instead of stopping.
- Live, dev build: `kill -9` one Claude worker's CLI PID (one we started). Its row reads "Interrupted — resuming",
  and the same session id continues (`orchestrator.logged` / worker `sessionStarted` with the same `nativeId`).
  `run_check` on the already-passed check is `cached: true`.

### Q5: no spurious rebirths; rebirth at 400k; decisions kept whole
**Today.**
- A permission change closes the thread's CLI between turns (`retire_moved_thread`, `M/thread.rs:240-267`). So
  does an idle close.
- The next turn looks up the session to resume with `last_native_id` (`M/conversation.rs:4033-4075`). It reads only
  the newest 200 `orchestrator.logged` events (`:4046`), and a busy session logs more than that between starts. The
  lookup then finds nothing, so `reseed` is set (`:1891`) and the turn starts from a Recovery briefing
  (`:1584-1592`).
- That is exactly A's 15:50 rebirth (trigger `recovery` at 124k of a 1M window, right after the switch to Full access)
  and B's 22:33 one (bugs 2 and 3).
- The rebirth threshold is `min(150k, 75% window)` (`crates/core/src/knowledge.rs:292-293`), even on a 1M window.
- The briefing caps decision bodies at `DECISIONS_BUDGET` (`M/rebirth.rs:54`, `:659`). Over the cap it cuts the
  Brain digest first, then the transcript tail (`:476-497`).

**Change.**
1. Bugs 2 and 3 (§4): the resume lookup finds the last session however far back it is, and a permission change or
   idle close resumes with a short note.
2. **Threshold:** prepare at 400k, swap at 450k, when the window holds that (`window ≥ 500k`). Otherwise keep the
   current shares. Codex keeps its lower shares (it compacts at 90% on its own). `BRIGADIER_REBIRTH_TOKENS` still
   overrides in debug builds.
3. **Briefing:**
   - The decisions part is exempt from every cut: all bodies in full, with `DECISIONS_BUDGET` removed.
   - Over the cap, the transcript excerpts (`recent`) shrink first, down to `MIN_EXCHANGES_OVERFLOW`. Then the Brain
     digest shrinks.
   - The handoff note's "Decisions not kept yet" is already kept as nodes (`:276-283`).

**Verify.**
- Rust tests:
  - `rebirth_thresholds(Claude, Some(1_000_000))` = (400k, 450k), and (150k, 180k) on a 200k window.
  - A briefing over the cap keeps every decision body and drops exchanges first.
  - (Bugs 2 and 3 have their own tests.)
- Live: a session on a 1M window passes 150k with no rebirth (contextSize events, no `rebirth` entry).

### Q6: fewer worker tokens, Brain first
**Today (B's numbers).**
- Workers start at 20–28k (system prompt, Brigadier's MCP tools, a ~6k brief).
- Tokens ≈ calls × average context: 50–272 calls at 40–160k.
- `query_brain` ran 0–2 times per task; `code_search` 49 times in all; Codex read code one shell command at a time
  (816 `zsh`, 464 `cd`); `run_check` ran 448 times.
- One review → fix → review chain was 11 follow-up tasks, about 60M tokens: over a quarter of B's worker tokens.
- The lean start exists (`--strict-mcp-config`, `--setting-sources project`, `LEAN_DENIED_TOOLS` in
  `P/claude/mod.rs:67`).

**Change.**
1. **No review-fix chains.**
   - Each landed change gets one review (`review_landing`, `M/review_runs.rs:127`).
   - Its findings go back as one batch to the same worker, resumed (Q4's resume, in a fresh worktree on its task
     branch rebased on the session tip), not to new fix tasks.
   - A fix task's landing (`subject` set, role fix) starts no review, except one targeted re-check
     (`review_code` scoped to the fix's range and the finding) when the thread marked the finding high severity.
   - What is left goes to the user as a list in the answer.
   - The thread's prompt (`M/prompts.rs:94`, "Every landing … gets one review …") and `LEAD_STEPS` (`:678`, the
     lead's own `review_code`) change to match. A lead that already ran `review_code` on its range gets no second
     landing review: the range rule in `review_runs.rs` exists and is kept.
2. **Tests: focused while working, the full suite once.**
   - `WORKER_CHECKS` (`M/prompts.rs:685`): while working, run only the affected checks (`run_check` with no command
     lists them). Run the full suite once, right before `submit_report`.
   - Results stay cached per tree (bug 8 makes the cache hit).
3. **Context pack plus the index over one-by-one reads.**
   - The pack (`M/context_pack.rs`) gains a Brain section (below).
   - `WORKER_CODE_TOOLS` (`:681`) says: use `code_search` / `code_refs` before shell reads.
   - A Codex worker gets the same in its brief, plus "read with one `sed -n a,bp` per file, never `cd` then `cat`".
4. **Worker handoff at 200k:** `WORKER_HANDOFF_MAX_TOKENS` 300k → 200k (`crates/core/src/knowledge.rs:222`).
5. **The article's settings** for workers and the thread, in `settings()` (`P/claude/mod.rs:550`), each checked
   against `claude --help` / the settings docs of the installed version before use:
   - `disableBundledSkills` (or `skillOverrides` off);
   - `disableClaudeAiConnectors`, `disableArtifact`, `disableRemoteControl`;
   - for workers, deny `AskUserQuestion`, `EnterPlanMode` and `ExitPlanMode` (added to `LEAN_DENIED_TOOLS`).
   - The thread's built-in tools are already only `THREAD_TOOLS` (`:74`). Its questions use Brigadier's `ask_user`,
     and workers ask through `ask_orchestrator`, so nothing user-facing changes.
6. **Brain first** (the user's emphasis: the Brain is the main token saver):
   - **The brief carries the Brain's facts that match the task.** At `delegate_task`, the engine runs `query_brain`
     on the task's title, spec and named files. The top matches (decisions, conventions, module notes; at most 3k
     tokens) go into the context pack as `[What the Brain knows about this]`, each with its node id.
   - **Workers query the Brain before reading code.** The worker prompt orders it: `query_brain` → `code_search` /
     `code_refs` → read the lines. A worker whose first 10 tool calls include no `query_brain` gets one steer: "Ask
     the Brain first: query_brain …".
   - **Workers write back.** `submit_report` gets a `learned` field (what the next worker in this area should know).
     The engine keeps each item through the existing `learn_report` (`M/brains.rs:1060`) with the task as source.
     The report envelope shows "Brain: +3 facts".
   - **Measured and shown.** Per worker: Brain queries, Brain facts given in the brief, facts written back, and
     tokens. These come from the worker's events and `turn_usage`, are stored on the task, and show on the worker's
     row in the side panel ("Brain 3↓ 2↑") and in the Inspector. A worker with 0 queries and 0 write-backs on a
     task over 50 calls is marked "low Brain use" in the Inspector: a gap to fix.
7. **Measurement (done-when of phase 3).**
   - A fixed task, chosen at phase start and frozen: one implement task in a clone at a fixed base, e.g. B's
     "Settings > Configuration" request at its base.
   - Run it before and after, on a dev daemon with its own data dir, Claude workers at the same model and effort.
   - `tools/ab/worker_tokens.py` is a new tool, ported from the session's `workers.py`/`outs.py` analysis (gone from
     `/tmp`; rebuilt from `routing.sqlite` `turn_usage` and the worker event streams). It prints per worker:
     - calls, average context and start context;
     - tokens: input, cached, cache writes, output;
     - Brain queries, facts in and facts out;
     - `run_check` calls and cache hits.

**Verify.**
- Rust tests:
  - A fix task's landing starts no review. A high-severity finding gets one scoped re-check.
  - Findings go to the resumed worker as one message.
  - The pack contains the Brain section when the Brain has matches.
  - `learned` items become Brain nodes.
  - The worker settings JSON contains the new keys and the denied tools.
- **Measured:**
  - Worker tokens on the fixed task ≤ 0.6× before, raw and without cache reads.
  - Every worker makes ≥ 1 `query_brain`, and the brief carries ≥ 1 Brain fact.
  - 0 follow-up review-fix tasks beyond one batch.
  - `run_check` calls ≤ half of before, with the cache hits logged.

### Q7: the review line shows open findings, as the thread recorded them
**Today.**
- `reviewStatus` sums `count` over every review since the last merge (`C/reviewStatus.ts:35-39`). So does the
  engine's `merged_reviews` (`M/landing.rs:1892-1937`).
- A review stores only a count (`ReviewState::Findings { count }`). So the line said "49" and "5", and the thread
  repeated those numbers.

**Change.**
1. **Findings become items.** When a review ends, the engine splits its text into findings (Codex's
   `- [P1] title — path:line` lines; Claude's review prompt asks for the same shape). Each gets an id (`R3.2`), a
   severity, a title and a location, stored on the `ReviewRun`. Splitting is plumbing. If the text can't be split,
   the review is one finding titled "See the review".
2. **The thread decides each finding's status** with a new tool, `record_findings { items: [{ id, status:
   fixed|open|dismissed, task?, why? }] }`. The thread prompt says: after a review's message, record each finding
   (fixed in task-N, open, or dismissed and why), in one call.
3. **The engine only stores and shows.** A finding the thread hasn't recorded yet shows as "new". The line counts
   what the thread recorded and never adds across merges or reviews:
   - before a merge: "Review running…", "Review: clean" or "N open";
   - after: "Merged · review clean" or "Merged · N open (not fixed)".
   - Clicking it lists each finding: "fixed in task-7", "open", "dismissed: …".
4. `merged_reviews` goes. `finish_session`'s result instead lists the findings the thread recorded as open, in the
   thread's words.

**Verify.**
- Rust tests:
  - A Codex review text with 3 findings stores 3 items.
  - `record_findings` sets statuses.
  - The status text after a merge counts only that merge's open items.
  - Nothing sums across two merges.
- `reviewStatus.test.ts`: 2 reviews (3 + 2 findings, 4 fixed, 1 dismissed) read "Review: clean"; 1 open reads
  "1 open".
- Live: B's fixture (49 findings over several reviews) shows the thread-recorded open count, not 49.

### Q1: after a push, the thread asks to watch CI
**Today.** Nothing watches CI. B pushed `97c298d5`, its CI went red (`ThreadScrollRender.test.ts:60`), and nobody
noticed.

**Change.**
1. **The thread asks.** After a push (the thread's own `git push`, which it knows it ran), and only if the repository
   has CI, the thread asks once with `ask_user`: "Watch CI for 97c298d5?" with Watch / No. A repository has CI when
   `.github/workflows/*.yml` exists or checks are reported for the pushed commit. Instructions live in `M/prompts.rs`
   next to the push rule (`:99`).
2. **A new tool, `watch_ci { sha, branch }`** (plumbing):
   - It polls `gh run list --commit <sha> --json databaseId,status,conclusion,name,url` every 30 s, with `gh` from the
     user's PATH.
   - While waiting, it shows a "Checking CI…" pill with the run names, live.
   - At the end it delivers `[ci 97c298d5 · failed · 2 of 5 jobs]` with each failing job's name and the key lines of
     `gh run view <id> --log-failed` (trimmed, the full log as an artifact).
   - With no `gh`, or `gh` not logged in, the tool says so and the thread tells the user.
3. **On red,** the thread fixes the failure (a worker, or a tiny fix), pushes the fix to the same branch and watches
   again, as decided. The user's yes to watching covers re-pushing fixes for that run's failures, and nothing else.
   On green it reports the result.

**Verify.**
- Rust tests with a fake `gh` on PATH: running → success delivers `[ci … passed]`; a failure delivers the job and its
  log lines; no `gh` returns the error text.
- Live: in a scratch GitHub repo of the user's (or the brigadier-ai repo only when the user pushes anyway), push a
  commit with a failing test. The pill shows, `[ci … failed]` reaches the thread, and the thread proposes the fix.

### Q8: the strip shows live work only; one slot above the composer
**Today.**
- `WorkersStrip` (`C/activity/WorkersStrip.tsx`, model in `C/activity/stripView.ts:29-49`) shows live workers and
  those finished since the latest request started.
- The phases pill lives in `C/ComposerCapsule.tsx:71-200`.
- There is no CI or merge pill.

**Change.**
1. The strip shows live workers only: "2 workers working · 1 waiting".
2. It hides once the request's answer that reported those workers is posted. That is the answer message with
   `requestId` equal to the workers' request, made after their last finish. The thread's answer decides, not a
   timer.
3. Finished workers stay in the right sidebar's Workers tab (`apps/desktop/src/state/rightSidebar.ts:5`), headed
   "Workers · N done".
4. **One shared slot** above the composer (`C/Composer.tsx:149`, a new `ComposerStatusSlot`) holds the strip, the
   "Checking CI…" pill (Q1) and the "Merging after N workers finish" pill (Q3), in the phases pill's style (`Pill`,
   `C/ComposerCapsule.tsx:89`). They stack, newest last, at most three rows.

**Verify.**
- `stripView.test.ts`:
  - A finished worker drops out once the request's answer is posted, not before.
  - A live worker always shows.
- Screenshot checks in the fixture gallery: the slot with the strip, the CI pill and the merge pill together; Compact
  and Normal; a narrow window.

### Q9: no silences, failures in the same turn
**Today.**
- The thread may go quiet for a long time: B's review-fix chain ran 17:31–19:58.
- A dev-app start failed silently in A. Preview exits are never delivered to the thread (`M/preview.rs` has no
  `deliver`).
- The "no answer" reminder fires only when nothing runs (`M/conversation.rs:2700-2725`).

**Change.**
1. **Failures of what the thread started** are reported in the same turn: what failed, the key error line, and what
   it does about it. This covers a preview, a dev app, a build, or a check it awaited.
   - A preview that exits on its own now delivers `[preview ended: exit 1] <last error line>` to the thread.
   - The thread's prompt says: never claim "compiling" or "running" without checking.
2. **30 minutes of nothing new in chat** while a request runs → the engine delivers `[status due] 30 min with nothing
   new for the user`. The thread writes one status line: what runs, why, roughly what's left.
   - After that, at most one every 30 min, and only when something changed since the last line (the engine passes
     what changed: landings, new workers, failures).
   - A timer per request in `ConvLive`, cleared by any visible message.
3. **Unrequested follow-up work** (e.g. starting review fixes): the thread first writes one sentence ("Starting 3
   fixes from the review; about 20 min."), which the user can veto by writing. Instruction only.
4. **"Working for …" ticks live** while the request runs, including while only workers run
   (`C/RequestBlock.tsx:145`). This is checked, and fixed if a stretch shows a frozen time.

**Verify.**
- Rust tests:
  - A preview whose process exits 1 delivers the envelope with its last stderr line.
  - With a fake clock, 30 min of no visible message delivers one `[status due]`, a second only after 30 more minutes
    and a change.
- Live: a request with a 40-minute worker shows one status line at about 30 min. A preview started with a broken
  command is reported in the same turn.

### Q10: a mockup first for visible UI changes, opt-in
**Today.** No mockups. The user sees UI only when it's built.

**Change.**
1. **The thread offers.** For a visible UI change that isn't a small fix, the thread offers a mockup with
   `ask_user`: "Want a mockup first?" with Yes / Skip. It's not a setting, and it costs no tokens unless the user says
   yes.
2. **On yes,** a worker builds a static preview of the real screen: the real components, fake data, in the fixture
   gallery (`apps/desktop/src/fixtures`, the headless-Chromium capture scripts). It reports screenshots and a
   preview URL.
3. **The surface** is a `mockup` tab of the right sidebar, opened by a chip in the thread (§5).
4. The user reacts to the picture. The thread asks only about what the picture can't show (behaviour, edge cases).
   Real work starts after the user's yes.
5. The final check compares the live app with the mockup: a side-by-side screenshot in the report.

**Verify.** Live, dev build:
- "Add a compact density toggle to the sidebar" → the thread offers a mockup; Skip → no mockup task.
- Yes → the mockup shows in the chosen surface, then the work, then the side-by-side.
- "Fix the typo in Settings" → no offer.

## 4. The bugs

### Bug 1: a Claude task landing kills the other Claude workers (root cause found)
**Root cause.**
1. Every Claude worker of a session runs with its cwd in one shared folder, `worker-home/<conversation>`
   (`M/workers.rs:1314-1324`, `worker_home` `:1789-1795`). This is deliberate, for a shared prompt cache.
2. The spec still says `owned_cwd: true` (`:1484`). The Claude adapter turns that into
   `owned_dir: Some(cwd)` (`P/claude/mod.rs:1010`): "this folder is this CLI's alone".
3. When one worker's CLI ends (`close_cli` from `dispose_task` at landing, `M/workers.rs:3950-3955`), its reaper
   calls `Tree::end_all`. That kills every process whose cwd is in `owned_dir` (`P/process.rs:116-140`, via
   `in_dir`, `crates/sandbox/src/macos.rs:125-132`).
4. That set includes every other Claude worker CLI of the session: they are SIGKILLed (exit null).
5. Their MCP pipes drop. The daemon logs "failed to send pending response during drain: Broken pipe", their checks
   end "stopped: the thread's session ended" (`M/run.rs:437`), and `fallback` hands each to Codex from zero.

**Evidence.**
- C at 18:35:57 (local): task-3's `land_phase`. At 15:35:58.680Z its CLI exits, and in the same millisecond three
  other Claude CLIs' six MCP streams end.
- The ledger's cleanup for task-3 is logged after the kills, so `ledger.dispose` is not the killer.
- B at 17:18:41 and 18:34:09 shows the same pattern. Codex workers survive: their cwd is elsewhere.
- It came from two commits together: `aee8d84c` (the folder sweep when a CLI ends) and `b765186b` (the shared home).

**Ruled out.**
- The ledger's `end_in_dir`: it runs later, and its folders are per task.
- `CLAUDE_CODE_TMPDIR`: one per session.
- Process groups: each CLI leads its own (`crates/sandbox/src/unix.rs:65-67`, `process_group(0)`).
- `run_command`'s `kill_group`, `end_takeover`, `computer.end_worker`: all per task.

**Fix.**
- A Claude worker in the shared home gets no `owned_dir`. Split "may sweep this folder" from `owned_cwd` with a new
  `SessionSpec::sweep_dir`, set to the task's own scratch folder.
- The tracker still ends the worker's own tree and its recorded group leaders, and the ledger still sweeps the
  per-task folders, so nothing leaks.
- Correct the `Options::owned_dir` doc comment.

**Verify.**
- Providers test: two `process::spawn` children with the same cwd, the first with `sweep_dir` = its own folder. Ending
  the first leaves the second running.
- Core test: two Claude write tasks of one conversation share the cwd, and neither spec sweeps it.
- Live, dev build: three Claude implement workers each run `run_check "sleep 120"`. One lands, and the log shows no
  "handing the task on … from claude", no worker `exited` with code null, and no "the thread's session ended".

### Bug 2: the resume lookup reads only the newest 200 events
**Root cause.** `last_native_id` reads one page of 200 `orchestrator.logged` events, newest first
(`M/conversation.rs:4038-4050`), and returns `None` when the `SessionStarted` is older. A busy thread logs hundreds
of events per hour, so the turn is reseeded and reborn (Recovery, `:1584-1592`). It is used at `:1308`, `:1879` and
`M/cold.rs:175`.

**Fix.**
- Page backwards (`StreamPage { before }`) until a `SessionStarted` of that provider, a `SESSION_RESET` notice or the
  stream's start.
- Cache the answer in `ConvLive` (set on `SessionStarted`, cleared on reset), so a turn normally reads no events.

**Verify.**
- Rust test: 1,000 logged events after the `SessionStarted` → the turn resumes with that id and no rebirth record.
- Live: B-sized session (fixture replay), close the CLI, send a message → `Origin::Resume`.

### Bug 3: a permission change rebirths the thread
**Root cause.**
- The change retires the idle CLI correctly (`M/thread.rs:264-266`, "the thread starts again for its new workspace
  or level").
- The restart's resume then fails on bug 2. A at 15:50:19: CLI exit 0, then a Recovery rebirth at 124k.

**Fix.**
- Bug 2's fix.
- The resumed turn gets the existing permission note (`prompts::notes`, tested by
  `a_changed_permission_level_is_told_with_the_instructions_own_words`) as its short note.
- A restart that can't resume (files gone) still reborns, as today.

**Verify.**
- Rust test: Approve for me → Full access between turns resumes the same `native_id` with the note.
- Live: switch a busy session to Full access → no `rebirth` entry, same session id.

### Bug 4: near-identical answers repeated (root cause found)
**Root cause: two instructions and two wake-ups.**
1. `M/prompts.rs:107` (from THREAD-PARITY §5 Q9) tells the thread: "A late review's fix that lands after your
   answer: write the ending again, updated". `late_findings_envelope` (`:1020-1028`) says the same.
   - In B's Configuration request, each review-fix landing woke the thread with `[for the user's earlier request: …]`
     (`M/requests.rs:243`), and it rewrote the full ending: 17:44, 18:15, 18:29 and 18:33.
   - These were all the same `requestId`. The UI shows only the last reply as the answer, but they read as four
     answers, because another request's messages interleaved between them.
2. B's two final answers 14 s apart (22:46:18 and :32): a `[review of your commits …]` arrived 1 s after the first
   answer, and the rule made the thread write a second, full ending.

**Fix.**
- Replace the rule with: "A late event (review, landing, addendum) changes your answer only if it changes what the
  user must know. Then write one short update line ('Update: the review's two fixes landed; nothing open.'), never
  a new ending. Otherwise reply {quiet}."
- `late_findings_envelope` and the review envelope say the same.
- With Q6.1 (no review-fix chains) and Q7 (findings recorded, not narrated), most of these wake-ups go away.
- UI: an update line after an answer shows as a small "Update" row under that answer, not as a new answer block.

**Verify.**
- Prompt tests assert the new rule and that "write the ending again" is gone.
- Live replay of B's fixture: one ending, followed by short update rows.

### Bug 5: "worktree and branch removed" though they were there
**What happened.**
- `finish_session` did remove them at 17:30 (`release_merged_session`, `M/lifecycle.rs:596-658`).
- At 17:45:51 the user's next message re-created the session worktree at the same path, and the branch
  `brigadier/9a2b00c9/session` from `main` at `59fdc017`. That is by design (THREAD-PLAN Q9; `ensure_target`,
  `M/workers.rs:2036`; a pre-warm followed at 17:45:58).
- The thread wasn't told. At 17:46 it found "both still there despite the earlier removal notice", and deleted the
  fresh worktree and branch with `git worktree remove` / `git branch -d`.
- So the claim was true. The fault is that the thread didn't know, and then destroyed its own live workspace.

**Fix.**
- When a merged session's workspace is re-created, the thread's next turn gets a `[workspace]` note ("A fresh branch
  `brigadier/9a2b00c9/session` from `main` at 59fdc017 was made for this message, in the same folder."). This is the
  mechanism THREAD-PLAN Q1 uses for workspace switches.
- `finish_session`'s text says the next message makes a fresh one there.
- Bug 6's guard refuses deleting the live session's own worktree or branch.

**Verify.**
- Rust test: after a merge, the next message re-creates the worktree, and the thread's turn input contains the
  `[workspace]` note.
- Bug 6's guard test covers the deletion.

### Bug 6: cleanup deleted another session's branch and the thread's own temp folder (root cause found)
**Root cause.**
- After pushing, B's thread (Full access) ran its own shell cleanup at 22:46 (`orch:01a125be…` seq 138498). It ran
  `git branch -D` on `brigadier/a25518cf/task-1-fix-shared-browser-ownership` (C's live task branch), and
  `rm -rf /private/tmp/brigadier-test-*` and `brigadier-442cf93845dd`.
- `brigadier-<12 hex>` under `/tmp` is a Claude session's `CLAUDE_CODE_TMPDIR` (`P/claude/files.rs:31-32`, `:162-171`),
  here the thread's own. `brigadier-test-*` are every task's test data folders (`M/workers.rs:4244-4247`), other
  sessions' included.
- Nothing stops this. The only guard is the prompt line "destroying anything outside this session's own work needs
  request_approval first" (`M/prompts.rs:99`), and the thread judged these to be its own leftovers.
- Brigadier already removes everything a session owns through the cleanup ledger, so the thread's manual cleanup is
  never needed.

**Fix.**
1. **A `clean_up` tool** (plumbing) for the thread:
   - It lists and removes only what the ledger records under this conversation's owners (`session:`, `task:`,
     `review:`, `workers:`, `orch:` of this id) and that is no longer in use: merged task branches
     `brigadier/<own short id>/…`, finished tasks' worktrees and test folders.
   - It returns what it removed and what it kept, and why.
   - The prompt says: "Brigadier cleans up after you; use clean_up if the user asks. Never delete branches,
     worktrees or temp folders by hand."
2. **A hard guard on the thread's shell** (a hard rule, so engine code):
   - A Claude `PreToolUse` hook on `Bash` (the existing `brigadierd hook`, `crates/daemon/src/hook.rs`, gains
     `pre-tool-use`) and the same check inside the `run` tool for Codex.
   - It refuses a command that would delete:
     - a `brigadier/<short>/…` branch of another conversation, or this session's live branch (`git branch -d/-D`,
       `git push --delete`, `git update-ref -d`);
     - a worktree the ledger records for another owner, or this session's live one (`git worktree remove`, `rm`);
     - any path the ledger records for another owner, or a live `ClaudeTempDir` / test data folder (`rm`, `trash`,
       `find -delete`, by a path match after expanding globs in the daemon).
   - The refusal says why, and that `clean_up` exists.
   - It is a best-effort parse, documented as a backstop: the ledger ownership rule is the real protection.
3. Codex's built-in shell isn't hooked (hooks are off, THREAD-PLAN §5 risk 2). Its thread runs commands through `run`,
   which gets the check.

**Verify.**
- Rust tests:
  - `clean_up` removes this session's merged task branch and keeps another conversation's.
  - The hook refuses `git branch -D brigadier/<other>/task-1`, `rm -rf /tmp/brigadier-test-*` (it matches a live
    task's folder) and `rm -rf /tmp/brigadier-<live temp>`, and allows `rm -rf /tmp/my-scratch`.
- Live, dev build: ask the thread to "clean up all branches and tmp" with another session's task running. Nothing of
  the other session is touched.

### Bug 7: "No such tool available" from short tool names
**Root cause.**
- The thread's prompt names Brigadier's tools by their short names: `delegate_task`, `query_brain`, `run` and the rest
  (`M/prompts.rs:85-99`, `:141`).
- Claude exposes them as `mcp__brigadier__<name>`. Sometimes the model calls the short name, which doesn't exist:
  - C's first 5 calls at 17:47:52–17:48:25 (`query_brain`, 4× `delegate_task`), while `mcp__brigadier__run` worked in
    the same turn;
  - B at 19:52:09.
- Then it corrects itself ("the tool names need the mcp__brigadier__ prefix").
- It's not a load-order problem: the prefixed call succeeds right away, with no tool search in between.

**Fix.**
- For Claude, the thread's prompt names each Brigadier tool by its callable name (`mcp__brigadier__delegate_task`),
  through one helper `tool(name, provider)`. Codex keeps its own naming.
- One line states the rule.
- This changes the instructions, so `CONTRACT` 7 → 8 (`M/prompts.rs:330`) in the same commit as phase 4's other
  instruction changes.

**Verify.**
- Prompt test: the Claude thread prompt has no bare `delegate_task(` / "delegate_task," outside the prefixed form.
- Live: 10 fresh Claude threads with a delegate-first request → 0 "No such tool available" in `orchestrator.logged`.

### Bug 8: the test environment: paused tests and 256 open files
**Root cause.**
1. **The build lease pauses commands.** A heavy command that starts while another holds the lease is "stopped where
   it stands" with SIGSTOP (`crates/core/src/machine/builds.rs:5-11`). At critical heat the newest running build is
   paused (`:17-19`). A test binary paused mid-run fails its timing assertions: the "SIGSTOPped machine tests" in the
   test-leftovers notes.
2. **No file limit is raised anywhere.** No `setrlimit` exists in the tree. The daemon inherits launchd's soft
   `RLIMIT_NOFILE` of 256, and every CLI and check inherits it from there. Workers rediscover `ulimit -n 8192` and
   prefix it to their commands, together with their own `TMPDIR=…`.
   - Those prefixes make every check command different, so the per-tree check cache never hits: `"cached":false`
     throughout B's log.

**Fix.**
1. The daemon raises its soft `RLIMIT_NOFILE` to `min(hard, 65536)` at start (`crates/daemon/src/main.rs:110`), so
   every child inherits it.
2. Test commands are never SIGSTOPped mid-run:
   - A check started through `run_check` waits for the lease **before** it is spawned (queued in the daemon,
     "waiting for another build" on its row), and is never paused once running.
   - The heat pause skips a process tree that runs a test binary (`is_heavy` classification gains "test").
   - A CLI's own shell builds keep today's behaviour.
3. `run_check` sets `TMPDIR` to the task's test folder itself and strips a leading `ulimit -n N;` and `TMPDIR=… ` /
   `export TMPDIR=…;` from the command before keying the cache (`M/checks.rs`). The worker prompt says not to add
   them.

**Verify.**
- Rust tests:
  - The daemon's spawned child reports `ulimit -n` ≥ 8192.
  - Two `run_check` calls with and without the `ulimit`/`TMPDIR` prefix on the same tree → the second is `cached`.
  - A check queued behind the lease is never sent SIGSTOP (a fake platform records signals).
- Live: two workers run `cargo test` at once. The second waits, then runs, with no `Too many open files` and no
  timing failure.

## 5. Q10: where the mockup shows

Both options were studied on 2026-10-11: the current assistant-ui docs and registry, and the reference app's bundle
(code only, extracted to a scratch folder and deleted).

**(a) assistant-ui's canvas-split** (`elements/canvas-split`, registry item `elements-canvas-split`, MIT).
- **What it is:** one vendored file (`canvas-split.tsx`, about 4.6 KB), installed with
  `npx assistant-ui@latest add elements-canvas-split`. It depends on `elements-surfaces` and `lucide-react`.
- **How it works:** a narrow chat column beside a document column. The document has a header (title, version,
  saved/editing, Copy, Close) and streamed text lines. It is fed by a tool UI registered `by_name`, reading
  `useToolArgsStatus`. assistant-ui says it has no dedicated canvas or artifact primitive.
- **Limits:** the root is `max-w-3xl` and `h-80`, and it stacks below 36rem. It has no tabs, zoom, comparison or
  markup.
- **Fit:** works on our `@assistant-ui/react` 0.15.22 with no upgrade (npm latest: 0.15.27). Our `elements/surfaces.ts`
  has its four surface styles. `lucide-react` is not a dependency and would be swapped for our icons.
- **What it lacks:** a screen built from our real components needs the window's width, and its chat column would
  duplicate our thread. We would keep perhaps a third of it.

**(b) The reference app.**
- **Every preview is a side-panel tab,** from one tab registry: open or reveal, with an icon, a title, `split` or
  `full-width` presentation, and restore after restart.
- **The artifact tab** has a header with zoom ("Zoom to fit", percent), Open / Open in, View source, "Refresh for latest"
  and a "Couldn't load this preview" state. It links back to the turn that made the file.
- **It isn't proposed by a special tool.** The agent produces an output; the thread's outputs list or a chip opens it
  in the panel.
- **The user reacts on the picture:** "Ask for change" marks a region, notes are batched and sent with ⌘↩, then "See
  latest changes / Accept all / Undo".
- **For several images,** a "Canvas view" shows them side by side, with comments across images: the pattern our
  mockup-vs-live comparison needs.

**Recommendation: (b), with two borrowings from (a).** Show the mockup as a new `mockup` tab of the right sidebar we
already have (`apps/desktop/src/state/rightSidebar.ts:5`, today `files | source | workers | running | plan`), opened
by a chip in the thread the way `C/PreviewChip.tsx` opens Running.
1. A mockup made of real components needs real room: split and full-width, which canvas-split's fixed text card can't
   give.
2. The plumbing exists: sidebar tabs, a chip that opens one, per-session tab memory, the plan document's tab. A
   second chat-plus-canvas layout would compete with the thread (and with "one obvious control per job").
3. It maps the decided flow directly:
   - **offer:** an `ask_user` card;
   - **build:** a chip;
   - **react:** notes on the picture, sent as one message;
   - **compare:** mockup and live screenshot side by side, as the reference app's canvas view does.
4. From canvas-split, borrow the header shape (title, version, building/done, Close) and the tool-UI pattern
   (`by_name` + `useToolArgsStatus`), which work on 0.15.22. No new dependency.
5. One surface in our own tokens: nothing vendored to keep in sync.

**Mockups stay opt-in:** offered as a card and built only on yes. If the user wants to see options first, a mockup of
the surface itself is offered the same way: built only on yes.

## 6. Phases

The order puts the riskiest engine fixes first. Each phase lands as small commits on its own branch from the
integration branch `session-fixes` (§7). Each "done when" also includes:
- `tools/full-checks.sh` passing on the integrated tree;
- one review by the other vendor;
- for UI phases, the native dev-app checks of THREAD-PARITY §7.

**Where each decision and bug is built:**

| Item | Phase, step |
|---|---|
| Bug 1 (Claude workers killed on landing) | 1.1 |
| Bug 8 (test environment) | 1.2 |
| Q4 (worker resume) | 1.3 (row label too) |
| Q2 (merges by the LLM, engine checks) | 1.4 |
| Bug 2 (resume lookup) | 2.1 |
| Bug 3 (permission change rebirth) | 2.2 |
| Q5 (rebirth at 400k, briefing) | 2.3 |
| Q6 (tokens, Brain-first, measured) | 3.1–3.7 |
| Q7 (review status by the thread) | 4.1 daemon, 5.4 UI |
| Q3 (merge consent hold) | 4.2 daemon, 5.1 pill |
| Q1 (CI watch) | 4.3 daemon, 5.1 pill |
| Q9 (no silences) | 4.4 daemon, 5.6 UI |
| Bug 4 (duplicate answers) | 4.5, 5.5 UI |
| Bug 7 (short tool names) | 4.6 |
| Bug 5 (removed-but-there worktree) | 4.7 |
| Bug 6 (cleanup safety) | 4.8 |
| Q8 (live-only strip, one slot) | 5.1–5.3 |
| Q10 (opt-in mockups) | 6.1–6.4 |

### Phase 1: engine safety (worker kills, merges, worker resume, the test environment)
**Scope**, commit-sized steps:
1. Bug 1: `sweep_dir` in `SessionSpec`, unset for the shared home; doc comment; providers and core tests.
2. Bug 8: the daemon's `RLIMIT_NOFILE`; `run_check` queued before spawn, never paused mid-run; canonical check keys
   with the `TMPDIR`/`ulimit` handling; worker prompt line.
3. Q4: crash → resume (same session, model, worktree, the interrupted note); daemon restart → resume; "died twice"
   stop; vendor switch only for limits, auth or an outage; the "Interrupted — resuming" row.
4. Q2: the merge brief (real git), git-dir access for merge tasks, `check_merge`, merge landing as a fast-forward
   with no litter guard. Then delete the pre-merge code and its tests.

**Why this order inside the phase.**
- Bug 1 first: every live check after it needs workers that survive a landing.
- Bug 8 before Q4: the resume relies on cached checks.
- Q2 last: it is the largest change, and it uses Q4's resume for "the worker is told what's missing".

**Done when:**
- Bug 1's three checks pass; the live run shows 0 Claude workers killed by a landing.
- Bug 8: `ulimit -n` ≥ 8192 in a worker's `run_check`; a second identical check with a different prefix is cached;
  0 SIGSTOP to a running check (signal log).
- Q4's tests pass; live `kill -9` of a worker CLI resumes the same `nativeId`; a second kill stops the task with both
  exits named.
- Q2's tests pass. Live replay of B's merge lands a two-parent commit with all of `main`'s files (`git diff --stat`
  shows none missing), and the thread doesn't ask the user about conflicts.

**Risks.**
- Resuming a Claude session whose process was killed mid-tool-call: the transcript may end on an unanswered tool use.
  Check live on the installed CLI version. If `--resume` refuses it, start a fresh session of the same model with the
  handoff files, which still beats switching vendor.
- A merge worker with git-dir access below Full access can rewrite refs. Limit its writable roots to the common
  git dir and its own worktree, and let the engine check refuse anything but a merge commit on its own branch.
- The heavy-command classifier may miss a test runner. The check-level queue covers `run_check`, and shell runs
  stay as today.

### Phase 2: thread rebirth and resume (Q5, bugs 2 and 3)
**Scope:**
1. Bug 2: paged lookup and the `ConvLive` cache.
2. Bug 3: permission change and idle close resume, with the note.
3. Q5: the 400k/450k thresholds on big windows; decisions whole; excerpts cut first.

**Done when:**
- Bug 2's and bug 3's tests pass.
- Live: a 1M-window session passes 150k and keeps its session id (0 rebirths). A permission switch resumes.
- The rebirth tests show every decision body kept at the cap.

**Risks.**
- A 400k context costs more per turn than a reborn 30k one, and a cold-cache resume writes the whole context again.
  `cold.rs`'s cache-expired rebirth stays as it is (§8 asks the user to confirm).
- Resuming after a CLI upgrade may fail. The existing Recovery path still catches a resume that fails.

### Phase 3: token economy and Brain-first (Q6), measured
**Scope:**
1. The fixed task and the "before" run with `tools/ab/worker_tokens.py` (the tool comes first).
2. Q6.1: one review per change, findings to the resumed worker, no review of fixes except the targeted re-check.
3. Q6.2 and Q6.3: prompt changes for checks and code reading; the Codex reading line.
4. Q6.4: worker handoff at 200k.
5. Q6.5: the article settings, each checked against the installed CLI's docs.
6. Q6.6: Brain facts in the pack, the "Brain first" steer, `learned` write-back, per-worker Brain metrics on the row
   and in the Inspector.
7. The "after" run, written to `docs/evidence/<date>-session-fixes-tokens.md`.

**Done when:** Q6's "Measured" criteria hold on the fixed task: worker tokens ≤ 0.6× before; ≥ 1 Brain query per
worker; ≥ 1 Brain fact in each brief; 0 extra review-fix tasks; `run_check` calls halved. The Brain counters show on
the worker rows.

**Risks.**
- The Brain may have little about a fresh area; the measurement must use an area the Brain has seen (B's).
- An unknown settings key may be ignored silently or break the start. Each key is checked against the installed
  version and dropped if it isn't listed.
- Resuming the original worker for a review batch needs its session files. If they are gone, a fresh worker of the
  same model gets the findings plus its handoff files.

### Phase 4: the thread's behaviour (Q1, Q3, Q7, Q9, bugs 4–7)
**Scope:**
1. Q7 daemon: findings as items, `record_findings`, statuses stored; `merged_reviews` removed.
2. Q3 daemon: `propose_merge` waits for writers, `MergePending`, `[merge due]` / `[merge stopped]`, consent stale
   only by words.
3. Q1 daemon: `watch_ci` with a `gh` poller, `[ci …]` messages, the ask-after-push instruction.
4. Q9 daemon: preview-exit messages, the 30-minute `[status due]`, the instructions.
5. Bug 4: the update-line rule; envelopes changed.
6. Bug 7: callable tool names for Claude.
7. Bug 5: the `[workspace]` note when a merged session's worktree is re-created.
8. Bug 6: `clean_up`, the `PreToolUse` guard and the `run` check.
9. One `CONTRACT` bump (7 → 8) for all the instruction changes, in the last commit of the phase.
10. The red CI on `97c298d5` (`ThreadScrollRender.test.ts:60`): check it on the current `main` first. Fix it here if
    it's still red.

**Done when:**
- All Rust tests of Q1, Q3, Q7, Q9 and bugs 4–7 pass.
- **Live, dev build:**
  - "merge when done" with 2 writers merges after they land, with no second question;
  - a push in a repo with CI asks to watch, and a red run reaches the thread;
  - a 40-minute request shows one status line;
  - the "clean up everything" request leaves a parallel session intact;
  - 10 fresh threads show 0 "No such tool";
  - B's fixture shows one ending plus updates.
- **`CONTRACT` 8:** Claude and Codex sessions on 7 start over once and 8 resumes. This extends the existing contract
  test.

**Risks.**
- Splitting review text into findings depends on the reviewer's format. Codex's `[P1]` lines are stable. Claude's
  review prompt asks for the same shape, and anything unparsed is one finding.
- `gh` may be missing or logged out. The tool says so; it never blocks.
- The shell guard is a best-effort parse: it can't see through `eval` or scripts. The ledger rule and `clean_up` are
  the real protection.
- **The contract bump reseeds every live session once:** one rebirth per session, the kind the user dislikes. It is
  one bump for the whole phase, said in the build report.

### Phase 5: the composer slot and the review line (Q1/Q3/Q8 UI, Q7 UI, Q9 UI)
**Scope:**
1. `ComposerStatusSlot` with the live-only strip (Q8), the CI pill (Q1) and the merge pill with Cancel (Q3).
2. The strip hides on the answer.
3. "Workers · N done" in the side panel.
4. Q7's review line and its finding list.
5. Bug 4's "Update" rows.
6. Q9: "Working for" ticks live.

**Done when:**
- `stripView.test.ts`, `reviewStatus.test.ts` and `blocks.test.ts` cover the rules above.
- Fixture-gallery screenshots (1728 × 1024, DPR 2, dark) of: the slot with all three; the review line before and after
  a merge; the finding list; an update row.
- Native dev-app checks in Compact, Normal and a narrow window.

**Risks.** Three stacked rows crowd a small window. Cap at three rows, and let the slot collapse to one line with a
count when the window is narrow.

### Phase 6: opt-in mockup previews (Q10)
**Scope:**
1. The thread's offer (instruction plus an `ask_user` card shape).
2. The mockup task kind or brief (real components, fake data, fixture page, screenshots).
3. The surface from §5.
4. The final side-by-side check in the report.

**Done when:** the three live checks of Q10 pass, with screenshots of the mockup in its surface and of the final
side-by-side.

**Risks.** A mockup of a deep screen may need state the fixtures don't have. The worker builds a fixture board for it
(as `thread-t1-2026-10-08.events.json` was), and a screen it can't fake is said in the offer.

### Why this order
- Phase 1 holds the bugs that lose work: missing files and killed workers. Every later phase's live checks need
  workers that survive and merges that keep files.
- Phase 2 next: rebirths cost the thread its context during every later live check.
- Phase 3 before phase 4: phase 4's review flow (Q7) builds on phase 3's single-review rule (Q6.1). Measuring tokens
  before the behaviour changes keeps the numbers clean.
- The UI comes after the daemon it shows.
- Mockups come last: they are new, opt-in, and depend on nothing else here.
- One change from the brief's suggested order: bug 8 (test environment) moves from "thread behaviour" into phase 1.
  Flaky tests and cache misses undermine every later done-when, and Q4's cached checks depend on it.

## 7. Rules for the build run

- **Worktrees only.**
  - Integration branch `session-fixes`, made once from `main` in `../brigadier-ai-session-fixes-build`.
  - Each phase works in `../brigadier-ai-sf-<phase>` on a branch from the verified tip of `session-fixes`.
  - Never edit, build or commit in the main checkout.
- **No push, no merge into `main`, no PR,** until the user says so.
- `CARGO_INCREMENTAL=0` for every build. Keep **≥ 100 GB free** (`df -h` before each build). Remove a phase's worktree
  and its `target/` when it is integrated.
- **Never `pkill`/`killall` by name.** Kill only PIDs you started (saved `$!` or a pidfile).
- **Never touch `/Applications/Brigadier.app`,** its daemon, or `~/Library/Application Support/Brigadier`.
  - Live checks use a dev build with a dev identity and a scratch `BRIGADIER_DATA_DIR`, built with the focus-off
    config override.
  - Never smoke-test the installed bundle on a temp dir.
  - The event log is read only with `?mode=ro`.
- **No backups** (no `.bak`, no `backup/*` branches, no app copies). Clean up your own scratch folders, test
  identities and processes.
- **No source-app names.** Never name the reference app or its vendor in code, docs or commits: say "the reference
  app". No marks of its vendor.
- **Models:**
  - Subagents one at a time, with narrow briefs; replace one at about 150k context.
  - Never Fable. Opus 5.5 by default, Sonnet 5 only for mechanical chores, Codex for reviews and bounded tasks.
  - Effort never above `high`; no ultracode.
- **Settled decisions** (the grill's Q1–Q10, THREAD-PLAN, THREAD-PARITY-PLAN, as superseded in §2) are not reopened.
  Something that looks impossible goes in the report with a recommended fix.
- **Third-party contracts.** Check every third-party CLI flag, settings key or API against the installed version's
  `--help` or current docs, and cite it in the commit or report. This covers `claude`, `codex`, `gh` and
  assistant-ui.
- **Before each report:** `tools/full-checks.sh`, `cargo fmt`, `clippy`, `gen-ts`, and the `pnpm` checks, on the
  exact tree.
- **One phase at a time:** commit, `report.md` for that phase only, status `done`, stop. A fresh verifier checks it
  before the next phase.

## 8. Open questions for the user

Only what the grill didn't settle:
1. **Cold-cache rebirth** (`M/cold.rs`: after an hour idle, a big thread is reborn from a checkpoint instead of
   resuming at the cache-write price). With rebirth at 400k, resuming a cold 300k context writes it all again.
   - Recommendation: keep it. It saves tokens, and the briefing now keeps every decision.
   - Q5's "idle CLI close resumes" covers the CLI closing while the cache is still warm.
2. **CI watch on a repository whose CI isn't GitHub Actions** (or with no `gh`). Recommendation: the thread says it
   can't watch that CI and gives the link if it has one. Nothing more for now.
3. **The one `CONTRACT` bump in phase 4** reseeds every open session once. Recommendation: accept it, landed with the
   build so it happens once.

## 9. Not in this plan

From the audit backlog, left out on purpose:
- **"Full access thread still said push needs go-ahead" (A).** By design: pushing happens only when the user asks
  for exactly that, at every level (`M/prompts.rs:99`). Q1 changes only what follows a push.
- **The Q&A habit "as you recommend" (7 of 9 answers).** The grill took no decision; the cards already mark the
  recommended option.
- **The dev-app start that failed silently (A)** is covered by Q9's rule on failures, not as a bug of its own.
