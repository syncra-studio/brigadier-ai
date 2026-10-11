# Session fixes plan: merges, worker deaths, resumes, tokens and the thread's habits

> Status: plan, 2026-10-11, from the 2026-10-10 grill of three real sessions (Q1–Q12) and the audit behind it.
> Revised the same day after the Codex plan review (all 12 findings adopted, §10) and the user's answers (§8).
> Revised again after the second Codex plan review (all 8 findings adopted, §10.2), as the first commit of phase 1.
> Branch `session-fixes-plan` from `main` at `7d8898ff`. File:line references are to `7d8898ff`.
> Next: the user's approval → a `/delegator` build run, phase by phase (§6, §7).
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

The grill settled ten decisions. The audit found eight plain bugs. The user then added two asks: computer use
known to every role, and REA for study tasks. This plan maps each to its code and a phase step, and says how it is
verified.

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
- **The cold-cache rebirth floor** `COLD_REBIRTH_MIN_TOKENS` = 60k (`crates/core/src/knowledge.rs:204`): now 150k
  (Q5, the user's answer).
- **Disposal at landing** (`dispose_task`, `M/workers.rs:3950`; the branch deletion at `M/landing.rs:751-770`): a
  landed worker's CLI session and branch are kept until its review batch settles (Q6.1).
- **Full access as "no sandbox at all"** (`policy.rs`; PLAN §5): it now runs inside an allow-all sandbox that only
  takes back writes to other sessions' work and Brigadier's bookkeeping (bug 6). Writers below Full access lose
  write access to the repository's shared refs, except their own branch.
- **Contract bumps for instruction changes** (`CONTRACT`, `M/prompts.rs:319-330`): changes now reach open sessions
  as versioned notes, with no start-over (§6).
- **The landing's merge handling** (`M/landing.rs:209-330`, `:829-1035`; the 2026-10-10 "auto-resolve" work): Brigadier
  no longer merges for the worker or builds the merge commit from files (Q2).

Everything else in THREAD-PLAN and THREAD-PARITY-PLAN stands.

## 3. The decisions (Q1–Q10) and the two added asks

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
   - **As built:** a subject merge's worktree branch starts at the conflicting task's work (its WIP commit, with
     what its litter guard would leave out put back first: `Repo::work_without`) and merges the target in; a
     no-subject merge's branch starts at the session tip and merges the base in. The task records what it merges
     in (`TaskWorkspace.merge`: the commit and its name). The squash landing went with the pre-merge
     (`prepare_candidate`, `merge_parent`, `commit_merge`, `unresolved_base_merge`). The worker instructions
     changed for merges (commit with git, list `deleted`), with revision 3's note for open sessions.
2. **The engine checks; it doesn't build.** A new `check_merge(worktree, tip, ours, theirs, report)` in
   `M/landing.rs` runs before a merge task lands:
   - **No conflict markers** in the tree: reuse `conflict_marker` (`:1848`) over the files the merge touched.
   - **Both inputs merged:** HEAD's ancestry holds a merge commit whose parents include `ours` (the target's tip
     at the start, or a descendant of it) and `theirs` (the base tip it merged), and both `ours` and `theirs` are
     ancestors of HEAD. HEAD itself need not be that merge: an ordinary correction commit after it (a fix the tests
     asked for, a file restored after a refusal) is fine. The checks below run on HEAD's final tree.
   - **Nothing vanished.** Each path is classified against the merge base:
     - Added or changed by one side, untouched by the other: it must be in the tip, unless the report's `deleted`
       lists it with a reason.
     - **Contested** (one side changed it, the other deleted it, a modify/delete conflict): the tip may keep
       either version. If the file is gone, the report's `deleted` must list it with a reason, whichever side the
       worker followed.
     - **Uncontested deletion** (one side deleted it, the other left it as it was at the base): it may be absent
       with no reason. That is the ordinary result of a merge.
   - **Fail:** nothing lands. The worker is resumed (Q4) with the exact list: "Missing from the merge: a.ts (from
     main), b.rs (from the session). Restore them or list them under deleted with why." A contested file is named
     as such: "c.ts: main changed it, the session deleted it. Keep a version, or list it under deleted with why."
   - **As built:** `land_merge` (`M/landing.rs`) over `Repo::audit_merge` (`crates/git`). Markers are looked
     for in the files the merge holds as neither side had them, so a file taken whole from one side (a fixture
     with marker lines, say) never counts. Uncommitted work the report names refuses the landing too: only
     what the worker committed lands. The worker hears the refusal through `message_worker` and its task stays
     landing, so its next report lands on its own (the same path as a rebase's self-check). A second refusal
     in a row goes to the thread instead, so a worker that can't fix it doesn't loop.
3. **No litter guard on merges, and only the engine moves the target.** `move_commits` handles a
   `TaskKind::Merge` landing as a fast-forward of the target to the checked merge commit. The engine does this
   itself after `check_merge` passes (and, for the base, after the consent check). The worker commits only on its
   own branch. `keep_paths` stays for normal task commits.
   - **The move stays atomic.** The fast-forward keeps today's expected-tip check (`LandRequest::expected_tip`,
     `LandBlock::TipMoved`, `M/landing.rs:453-466`): it moves the target only if the target is still at the tip the
     check ran against.
   - **When the target moved** (another task landed, or the thread committed, while the merge worker ran), nothing
     lands. The same merge worker is resumed (Q4) against the new target: "`<target>` moved to <sha>; merge it in
     too, then build, test and submit again." `check_merge` and the affected checks then run again, with `ours` the
     new tip. No rebase of a merge, and no merge built by the engine.
   - **As built:** the consent held over from a session merge that stopped at conflicts covers the branch tip
     the no-subject merge task landed, when it landed on the tip the consent was held for (a fix committed on
     top of the merge included); work landed meanwhile still needs a fresh yes, as before.
4. **The consent check before merging into the base stays in the engine** (`finish_session`, `merge_consent`,
   `M/landing.rs:1378-1720`). When a session merge stops at conflicts (`:1497-1517`), the thread is told to delegate
   a merge task. That worker merges the base into the session branch as above. `finish_session` then merges on the
   held consent (Q3).
5. **Sandbox.** A merge worker is a writer, so it already gets the repository's commit roots below Full access
   (`commit_roots`, `M/workers.rs:4208`). Today those roots are the whole `refs`, `logs` and (for Codex)
   `packed-refs.lock`, so any writer can move `main` or delete another session's branch. Bug 6's protected refs
   (§4, phase 1) narrow them to the worker's own branch for every writer, the merge worker included, at every
   permission level. A merge needs nothing more: it reads the base and writes its own branch.

**Verify.**
- Rust tests in `M/flow/` (`base_merge_tests.rs` is rewritten):
  - Main adds 18 files and the session adds 3, with a conflict in one: after the merge task, the landed tip has two
    parents and all 21 files.
  - A worker that leaves a marker is refused, nothing lands, and the worker hears the file.
  - A refused merge repaired by a later ordinary commit (the missing file restored on top of the merge) lands: the
    merge is in HEAD's ancestry, both inputs are ancestors, and the final tree passes.
  - Concurrent landing: another task lands on the target while the merge worker runs. The fast-forward is refused
    by the expected-tip check, nothing of the other task is lost, the same merge worker is resumed against the new
    tip, and its second merge lands with both landings' files.
  - A worker that deletes a base file without listing it is refused; listed with a reason, it lands.
  - Modify/delete: `main` changed `c.ts` and the session deleted it. A merge that drops it without a reason is
    refused, naming it as contested; one that keeps `main`'s version lands; one that lists it with a reason lands.
  - An uncontested deletion (`main` deleted `d.ts`, the session never touched it) lands with no reason.
  - The merge worker's attempt to move `main` itself fails (bug 6's protected refs), and the engine's
    fast-forward is the only thing that moves it.
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
   at_ms, branch, base, condition }` on the conversation (stored, so it survives a restart; today's `merge_held` is
   in memory only, `M/conversation.rs:277`) and returns `[merge held] Merging after 2 workers finish.`
   - **"Merge when done" is valid consent for this.** `merge_consent::check` refuses any condition today
     ("when", "once", "after": `M/merge_consent.rs:22`, tested at `:418`). It gains one accepted condition: the
     running work finishing. "Merge when done", "merge once they finish", "merge it after the workers land", "merge
     when it's all in" are accepted, and stored as `condition: WorkersFinish`. The words must name the work ending
     (done, finish, land, complete, all in), with "it", "they", "the workers" or nothing as the subject.
   - Every other condition is still refused: "once the tests pass", "after you fix the docs", "if the review is
     clean". The thread asks for a plain yes once that is met, as today.
   - A `WorkersFinish` consent with no writer running merges at once, as a plain yes would.
3. **When the last write task is final** (landed, done, stopped or failed), the engine checks the pending merge:
   - **Stops:** a write task failed or was stopped, or any finding the thread recorded as open (Q7), whatever its
     severity. Nothing merges. The thread gets `[merge stopped] task-5 failed: …` or `[merge stopped] R3.2 is open:
     …`, and tells the user why and asks. Severity decides only whether a fix gets a targeted re-check (Q6.1).
   - **Waits:** a finding the thread hasn't recorded yet ("new"), or a review still running on the session's work.
     The pending merge stays, and the thread is told to record the findings first. `finish_session` refuses while
     any finding on the session's work is unrecorded.
   - **Due:** otherwise the thread gets `[merge due] The user's merge is due: call finish_session.` and merges on the
     held consent. The engine wakes the thread; the merge stays the thread's call.
4. **Consent goes stale only from the user's words.** `held_consent` and `fresh_consent` drop the `Prepared::Moved`
   refusal for work that landed from this session's own tasks after the consent. `merge_consent::takes_back`, the
   queue check and "Not yet" still refuse.
5. **Cancel revokes the consent, durably and atomically.** A Cancel on the pill (§Q8 slot):
   - Takes the conversation's landing lock, the one `finish_session` holds from its consent check to the move of
     the target.
   - Writes a stored `MergeCancelled { asked_in }` orchestrator step. `held_consent` and `fresh_consent` already
     skip a consent whose `asked_in` has a `Merged` step (`M/landing.rs:1655`, `:1734`); they skip a cancelled one
     the same way. So the answered card or message can't count again, after a restart included.
   - Clears `MergePending` and `merge_held`, and tells the thread `[merge cancelled by the user]`.
   - A `[merge due]` already queued finds no pending merge and no valid consent, so `finish_session` asks again.
   - `finish_session` re-reads `MergePending` and the cancellations under the lock, right before it moves the
     target. If the move already happened, the Cancel answers "Too late: already merged" and changes nothing.

**Verify.**
- Rust tests:
  - `propose_merge` is refused while a writer runs, and accepted after it lands.
  - "Merge it" while 2 writers run → `MergePending`; both land → `[merge due]` → `finish_session` merges with no new
    question.
  - "Wait" written meanwhile → refused.
  - A writer fails → `[merge stopped]` and no merge.
  - A finding the thread recorded as open stops the merge, at medium and low severity too. An unrecorded one
    holds it until recorded.
  - "Merge when done", "merge once they finish" and "merge after the workers land" are accepted as `WorkersFinish`;
    "merge once the tests pass" and "merge after you fix the docs" are still refused.
  - Cancel, then a restart, then the old card's answer or the old message → no merge, and the thread asks again.
  - Cancel racing a `[merge due]`: whichever takes the landing lock first wins. No merge after the Cancel returns.
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
2. **Daemon restart → resume, after the run's state is known.** `recover` resumes running write and read tasks the
   same way, instead of stopping them (`M/lifecycle.rs:158-164`). Tasks taken over in a terminal, and quota waits,
   keep their paths.
   - **Overnight state comes first.** Today startup calls `recover` and only then `resume_runs`
     (`M/mod.rs:277-278`), which finds overdue runs. And a resumed session skips the wind-down admission check
     (`admit_run_task` vs `admit_new_run_task`, `M/overnight/admission.rs:154-180`). So `recover` first classifies
     each task's overnight run, with `resume_runs`' own deadline and Stop logic split out as a pure function:
     - **Running** (before its wind-down instant): the task resumes as above.
     - **Winding down, inside the wind-down allowance** (the run is `WindingDown` and now is before its cleanup
       cutoff, `cutoff_ms` in `M/overnight/wind_down.rs:411`; or the restart found it past its wind-down instant but
       before its deadline, so it enters the wind-down now): the task may resume only to hand off. Its first input
       says: "[Brigadier] You were interrupted, and the overnight run is ending. Start nothing new; write your
       handoff and submit your report now." The same applies to a crash resume (step 1) during a wind-down.
     - **Past the deadline or the cleanup cutoff, stopped by the user, or ended:** nothing resumes, not even to
       hand off. PLAN §10.9 sends a clock jump past the deadline straight to report and cleanup, and §10.10's
       recovery order step 4 admits no replacement once it is crossed. The task's work is preserved mechanically
       (§10.9 step 3: its commits and branch kept, unverified diffs, untracked task files and the transcript saved as
       artifacts), it stops with the run's reason, and the report lists it as unfinished.
   - A task the user stopped, or one in a conversation whose workers the user stopped all at once, never resumes.
   - The same order applies on every start: no worker resumes before its run's state is known.
   - **As built:** the verdict is the pure `overnight::interrupted(run, now)` (`M/overnight/wind_down.rs`), used by
     both the crash resume and `recover`. A run records when it began winding down (`winding_since_ms`, stamped
     wherever a run is recorded `WindingDown`), so its cutoff counts from there across restarts; `wind_down_run`
     uses the same instant. A restart with no recorded CLI session stops the task as before.
3. **Vendor switch only for quota, rate limits, auth or an outage:** `AttemptEnd::Limit`, `ErrorKind::Auth`, and
   `Overloaded/Server/Network` after the transient retry (`fallback::verdict`, `M/fallback.rs:66-90`). Never for
   `ErrorKind::Process`.
4. **Twice in a row → stop and say what kills it.** A task that dies again within its next resumed attempt (no
   report between) stops with `blocked_reason` set to the two exits (code or signal, the last stderr line) and the
   thread is told `[task-N stopped: died twice] …`. The thread tells the user.
   - **As built:** "stops" is `TaskState::Blocked` with that reason, its worktree and session kept, so
     `message_worker` resumes it once the cause is fixed (and clears its deaths) and `stop_task` ends it. The CLI's
     exit now carries its signal (`ProviderEvent::Exited.signal`). The thread's instructions gained no line (its
     byte budget is spent); the message names what to do, and revision 2's note tells open threads.
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
  - After a restart past an overnight run's deadline or its cleanup cutoff, or after the run ended, no task of
    that run resumes; each task's work is kept (branch, commits, saved diff) and the report lists it. In a run
    winding down inside its allowance, the resumed task gets only the hand-off input and starts no new session.
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
4. **Cold-cache rebirth stays, only above about 150k** (the user's answer, grill Q12). After the cache lifetime
   (about an hour idle), `M/cold.rs` reborns a thread from its checkpoint instead of resuming at the cache-write
   price. Its floor `COLD_REBIRTH_MIN_TOKENS` (`crates/core/src/knowledge.rs:204`) goes from 60k to 150k:
   - A thread under ~150k that sat idle over an hour resumes normally. Re-caching it is cheap.
   - A bigger one gets the cold-cache rebirth from a briefing, with decisions kept whole (step 3).
   - `cold_rebirth_min_tokens` stays capped by the prepare threshold, so on a small window it is never above the
     ordinary rebirth.

**Verify.**
- Rust tests:
  - `rebirth_thresholds(Claude, Some(1_000_000))` = (400k, 450k), and (150k, 180k) on a 200k window.
  - A briefing over the cap keeps every decision body and drops exchanges first.
  - `cold_rebirth_min_tokens(Claude, Some(1_000_000))` = 150k. A cold turn at 120k resumes; at 200k with a current
    checkpoint it is reborn (`CacheExpired`) with every decision body in the briefing.
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
   - The thread records each finding's status with `record_findings` (Q7's finding model, built first in phase 3).
   - The findings it records as open go back as one batch to the same worker, resumed, not to new fix tasks.
   - A fix task's landing (`subject` set, role fix) starts no review, except one targeted re-check
     (`review_code` scoped to the fix's range and the finding) when the thread marked the finding high severity.
   - What is left goes to the user as a list in the answer.
   - **The landed worker stays resumable until its review batch settles.** Today a landing disposes the task's
     ledger owner (`dispose_task`, `M/workers.rs:3950-3999`): that removes its CLI session files
     (`Artifact::ClaudeSession`, `P/claude/files.rs:60-90`), its temp folder and its worktree. Then the landing
     deletes the merged task branch (`M/landing.rs:751-770`). The lifecycle becomes:
     1. **At landing:** the worktree and scratch folder go as today; they hold build output and nothing the resume
        needs. The task's CLI session artifacts move to a new ledger owner `review-hold:<task>`. The task branch is
        kept, though merged.
     2. **Findings come back:** the thread delegates them as one fix task with `subject = task-N`. That task starts
        with `Origin::Resume` on task-N's native session, same model and account. Its worktree is recreated at
        task-N's old path (so the transcript's paths hold), on task-N's branch fast-forwarded to the session tip.
        Its first input is the findings batch, with a `[workspace]` line that says the worktree was recreated on
        the session tip. Only one fix task at a time resumes a given session.
     3. **The batch settles** when the review is clean, the thread records no open finding for it, or the fix task
        ends (landed, failed or stopped). Then `review-hold:<task>` is disposed and the task branch is deleted if it
        is still merged. The session's merge, archive or deletion settles every batch still open.
     4. **Fallback:** if the session files are gone anyway (a CLI upgrade, the user's cleanup), the fix task starts
        a fresh session of the same model with task-N's report and handoff files. This is now the exception.
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
7. **Measurement (done-when of phase 3): a fair A/B.**
   - **The task, frozen at phase start:** one implement request in a clone at a fixed base, e.g. B's "Settings >
     Configuration" request at its base. Its text, base commit and models are written to
     `docs/evidence/<date>-session-fixes-tokens.md` before the "before" run.
   - **Frozen acceptance checks,** written before either run and committed with the task: executable commands (tests
     and a script) that pass only when the request is done. Neither arm sees them; both must pass them.
   - **The same Brain seed:** both arms start from one copy of the project's Brain store at the base, copied into each
     arm's own dev data dir. Nothing else carries over between arms.
   - **The same conditions:** a dev daemon per arm, the same models, effort and accounts; Claude workers both times.
   - **Independent verification of both results:** the acceptance checks, then one reviewer of the other vendor that
     sees both diffs against the request, unlabelled, and says whether each does the whole request. An arm that
     fails either makes the comparison void until it is rerun.
   - **What is compared:**
     - worker metrics per worker, from `tools/ab/worker_tokens.py`;
     - **total usage per provider** over everything the arm ran (thread, workers, reviews, Brain jobs), split
       Claude and Codex, as input, cached, cache writes and output;
     - **elapsed time** from the request to the answer.
   - One run per arm. If a result is within 10% of a gate, a second pair is run and the means are compared.
   - `tools/ab/worker_tokens.py` is a new tool, ported from the session's `workers.py`/`outs.py` analysis (gone from
     `/tmp`; rebuilt from `routing.sqlite` `turn_usage` and the worker event streams). It prints per worker:
     - calls, average context and start context;
     - tokens: input, cached, cache writes, output;
     - Brain queries, facts in and facts out;
     - `run_check` calls and cache hits.
     It also prints the arm's per-provider totals and elapsed time.

**Verify.**
- Rust tests:
  - A fix task's landing starts no review. A high-severity finding gets one scoped re-check.
  - Findings go to the resumed worker as one message.
  - After a landing, the task's CLI session files and branch survive until its batch settles, then go; the fix task
    resumes the original native session in a worktree recreated at the old path on the session tip.
  - The pack contains the Brain section when the Brain has matches.
  - `learned` items become Brain nodes.
  - The worker settings JSON contains the new keys and the denied tools.
- **Measured** (both arms pass the frozen acceptance checks and the independent review):
  - Worker tokens on the fixed task ≤ 0.6× before, raw and without cache reads.
  - Total usage per provider not above before for any provider, and the sum ≤ 0.75× before. Elapsed time not
    above 1.25× before.
  - Every worker makes ≥ 1 `query_brain`, and the brief carries ≥ 1 Brain fact.
  - 0 follow-up review-fix tasks beyond one batch.
  - `run_check` calls ≤ half of before, with the cache hits logged.

### Q7: the review line shows open findings, as the thread recorded them
**Where.** The daemon side (finding items, `record_findings`, stored statuses) is built first in phase 3, because
Q6.1's single review batch and Q3's merge stop both read the recorded statuses. The review line and finding list are
phase 5.

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
1. **GitHub Actions only, for now** (the user's answer). After a push (the thread's own `git push`, which it knows it
   ran), the thread calls `ci_status { remote, branch, sha }`, a cheap probe that says one of:
   - **watchable:** the pushed tree has `.github/workflows/*.yml|yaml`, the remote is on GitHub, and `gh auth status`
     passes for that host. The thread asks once with `ask_user`: "Watch CI for 97c298d5?" with Watch / No.
   - **not watchable:** CI of another kind (`.gitlab-ci.yml`, `.circleci/`, `Jenkinsfile`, `azure-pipelines.yml`,
     `bitbucket-pipelines.yml`), or `gh` missing or not signed in. The probe returns a link when it can make one (the
     host's page for the commit or its pipelines). The thread says in one line that it can't watch this repo's CI
     from here, gives the link if there is one, and doesn't offer to watch.
   - **no CI:** none of the above. The thread says nothing about CI.
   - Instructions live in `M/prompts.rs` next to the push rule (`:99`).
2. **A new tool, `watch_ci { remote, branch, sha }`** (plumbing). `gh` comes from the user's PATH; every call names
   the repository (`--repo owner/name`, from the remote the push went to).
   - **Runs bound to the push.** Every 15 s it lists runs with `gh run list --repo R --commit <sha> --branch <branch>
     --json databaseId,workflowName,status,conclusion,url,headSha,headBranch,event`, and keeps only runs whose
     `headSha` and `headBranch` match. A run for another commit or branch is never counted.
   - **Jobs per run.** For each kept run it reads `gh run view <id> --repo R --json
     status,conclusion,jobs` (`jobs[].name`, `status`, `conclusion`). Job counts and failing job names come from
     there, never from the run list.
   - **Waiting for runs to appear** vs **no CI here:** the pill reads "Waiting for CI to start…" until the first
     run appears. If none appears within 3 minutes, the watch ends with `[ci 97c298d5 · no runs] No GitHub Actions
     run started for this push (its workflows may not run on this branch).` That is reported, not treated as red.
   - **Several workflows.** Runs that appear later (staggered workflows, `workflow_run` follow-ups) join the watch.
     It ends when every kept run is completed and no new run has appeared for 60 s.
   - While waiting, the "Checking CI…" pill shows the runs and "2 of 5 jobs done", live.
   - At the end it delivers `[ci 97c298d5 · failed · 2 of 5 jobs failed]` with each failing job's workflow and name,
     and the key lines of `gh run view <id> --repo R --log-failed` (trimmed, the full log as an artifact). Or
     `[ci 97c298d5 · passed · 5 jobs]`.
   - `gh` missing or signed out mid-watch: the tool says so and the thread tells the user in one line.
3. **On red,** the thread fixes the failure (a worker, or a tiny fix), pushes the fix to the same branch and watches
   again, as decided. The user's yes to watching covers re-pushing fixes for that run's failures, and nothing else.
   On green it reports the result.

**Verify.**
- Rust tests with a fake `gh` on PATH (a script that replays canned JSON per call):
  - **Delayed discovery:** the run list is empty for the first 3 polls, then shows a run → the watch waits, then
    follows it. Empty for 3 minutes → `[ci … no runs]`, not failed.
  - **Several workflows:** two workflows, the second appearing after the first completes → both are watched, and the
    result counts both runs' jobs.
  - **Binding:** a run for the same branch at another sha, and one for another branch at the same sha, are ignored.
  - **Jobs:** "2 of 5 jobs failed" and the failing names come from `run view --json jobs`; a failure delivers the job
    and its log lines.
  - `ci_status` returns watchable, not watchable (GitLab file; `gh` missing; `gh auth status` failing) with the link,
    and no CI.
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
   - The thread's prompt says: never claim "compiling" or "running" without checking, by the process's state or a
     worker's look with the computer tools (§3, "Computer use").
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
5. The final check compares the live app with the mockup: a side-by-side screenshot in the report, the live side
   captured with computer use on the dev build (§3, "Computer use").

**Verify.** Live, dev build:
- "Add a compact density toggle to the sidebar" → the thread offers a mockup; Skip → no mockup task.
- Yes → the mockup shows in the chosen surface, then the work, then the side-by-side.
- "Fix the typo in Settings" → no offer.

### Computer use: every role knows it, and live checks use it (added 2026-10-11)
The user's ask: Brigadier's macOS computer use (`crates/computer`, the `computer` MCP server's `apps`, `launch`,
`observe`, `act`, `zoom`; `docs/COMPUTER-USE.md`, `COMPUTER-USE-PLAN.md`) must be known to the thread, the workers and
Brigadier's own briefs, and used where a live look settles a question.

**Today.**
- **Workers:** on macOS every worker gets the `computer` server (`M/workers.rs:1435-1461`). It sits behind the CLI's
  tool search (`computer_server`, `M/conversation.rs:2066-2084`) and is loaded from the start only for an operate
  task.
- **Operate tasks:** they have full instructions (`operate_rules`, `M/prompts.rs:825-840`).
- **Other workers:** their prompts never mention the tools (`WORKER_CODE_TOOLS`, `WORKER_CHECKS`, `:681-690`). B's
  workers still found them and called `computer/act` about 76 times.
- **The thread** has no computer server. Its prompt mentions operate tasks once (`:88`) and says "screenshots only if
  the user asked".
- So reports say "not checked in the live app" while the tools sit unused, and nothing routes Q9's "check before
  claiming it runs" or Q10's live comparison to them.

**Change.**
1. **Roles.**
   - Every worker keeps the deferred `computer` server, and operate tasks keep it loaded.
   - The thread gets none. It routes a look to the worker on the task, or to an operate task for a longer job. This
     keeps screenshots out of the thread's context.
2. **The thread's guidance** (`M/prompts.rs`, next to `:88`), one paragraph:
   - On macOS, every worker can open an app, look at it and use it.
   - Put a live check in the brief's done-when for a visible UI change: "check it in the running dev build with the
     computer tools".
   - Use an operate task for jobs over about five GUI steps.
   - Before saying a preview or dev app runs (Q9), have it checked, by its process state or a worker's look.
   - Q10's final comparison is a live capture.
   - "Screenshots only if the user asked" becomes: screenshots in the chat only if the user asked. A worker's live
     check is evidence in its report.
3. **The worker guidance,** a new `WORKER_COMPUTER` block, given to write and verify tasks whose change shows in an
   app:
   - To check UI live: start the build this task made, the way the project starts it (the context pack names the
     command when the Brain or the repo's docs know it).
   - Then `observe` → one `act` with an `expect` on each step → `zoom` for small targets.
   - Put the act's result, or a screenshot artifact, under verification.
   - Read structure before pixels, as operate tasks do.
4. **The context pack** (`M/context_pack.rs`) gains the project's dev-app command when known (for brigadier-ai:
   `pnpm tauri:debug-app`, then `tools/open-dev-app.sh <scratch data dir>`), so workers don't rediscover it.
5. **Safety limits,** said in both blocks and enforced where code can:
   - Never the user's installed copy of the app or its data. Use the build this task made, with a scratch data
     folder when the app has one. For Brigadier itself: the dev identity (`Brigadier Dev.app`), never
     `/Applications/Brigadier.app` or `~/Library/Application Support/Brigadier`.
   - The helper already refuses the host Brigadier and the never-touched apps (`docs/COMPUTER-USE.md` §Safety). It
     also gains a refusal for any Brigadier bundle the worker didn't launch.
   - Act only on windows the worker launched or the task names, as operate tasks already do.
6. **Open sessions** hear the thread's change as a versioned note (§6).

**Verify.**
- Prompt tests:
  - The thread prompt has the routing paragraph.
  - A UI write task's brief has `WORKER_COMPUTER` and the pack's dev-app command.
  - A pure Rust task's brief has neither.
- Helper test: launching or acting on a Brigadier bundle the worker didn't launch is refused.
- Live, dev build (a Brigadier session on the brigadier-ai repo): "make the sidebar's Workers header bold".
  - The worker's report shows an `act` or screenshot of `Brigadier Dev.app` under verification, and no "not checked
    in the live app".
  - The helper log shows no action on `/Applications/Brigadier.app`.

### REA for study tasks (added 2026-10-11)
The user's ask: offer REA (`github.com/morluto/rea`, MIT, npm `rea-agents`) to tasks that study an app without its
source. In B, each "study the reference app" worker spent 6–12M tokens reading the app by hand.

**What REA is** (researched 2026-10-11; read only, nothing installed):
- **What it does:** an MCP server and CLI, stdio, started as `rea mcp` (`scripts/rea.mjs`), with about 130 tools.
  - **Static JS/Electron/ASAR, needing nothing extra:** `analyze_javascript_application`,
    `recover_javascript_sources`, `trace_application_feature`, `compare_application_versions`, `inspect_artifact`,
    `extract_artifact`.
  - **.NET:** static, nothing extra.
  - **Native binaries:** need Hopper, Ghidra 12.1 with JDK 21, or IDA. `rea doctor` detects them.
  - **Web:** needs a Chrome-family browser.
  - **Evidence:** every conclusion comes with evidence (`get_evidence_bundle`).
- **Version:** 6.3.0 (2026-10-10), Node `^22.19 || ^24.11 || >=26`. It has npm provenance (SLSA, built from the
  release workflow) and registry signatures.
- **One maintainer,** and a very fast release pace: 30 versions since July.
- **Install scripts:** no install or postinstall script. 33 direct dependencies, about 148 transitive.
- **Network:** analysis is local, with no telemetry. It writes `~/.rea/cache` and `~/.rea/state`, and temp folders for
  extractions.
- **`npx rea-agents setup` edits the user's agent configs:** `~/.claude.json`, `~/.claude/skills`,
  `~/.codex/config.toml` and `~/.agents/skills`, with backup files. It can also download Hopper.

**Change.**
1. **Brigadier adds REA as its own MCP server,** like `brigadier` and `computer`, in the session's
   `--strict-mcp-config` list (Codex: its per-process MCP config). It is never registered in the user's configs.
   - Only for **study tasks**: scout and research tasks whose brief names an app or binary to study, chosen by the
     thread with a `study: true` flag on `delegate_task`.
   - Behind tool search, like `computer`.
2. **Pinned and verified, the whole tree.** Brigadier ships a reviewed `package.json` and `package-lock.json` for
   REA (`rea-agents@6.3.0` and every transitive dependency, each with its `integrity` value) and installs from them
   into its data dir with `npm ci --prefix <data>/tools/rea --ignore-scripts`, on first use and with a progress row.
   `npm ci` installs exactly the lockfile and fails if the manifest and lockfile disagree (npm v11 docs, `npm ci`).
   - It checks the top-level tarball's `sha512` against the value pinned in our code and npm provenance with `npm
     audit signatures`, and only then exposes the install (it is written to a staging folder and moved into place
     after these checks pass).
   - It runs `node <prefix>/node_modules/rea-agents/scripts/rea.mjs mcp` directly: no `npx`, no global install.
   - A version bump is a code change with its new manifest, lockfile and pin, reviewed like any dependency.
3. **Never `rea setup`, never `rea update`.** No change to `~/.claude.json`, `~/.codex`, skills or Hopper.
   - Phase 3 checks, from REA's docs and code, whether its cache and state folder can be moved. If yes, it goes to
     the task's scratch folder.
   - If not, `~/.rea` is granted to study tasks as a writable root and documented as REA's own folder.
4. **Static analysis by default.** The brief says: static JS/Electron/ASAR and .NET first. Native engines are used
   only if `rea doctor` finds one already installed; Brigadier never installs one.
5. **Sandbox: Brigadier's own wrapper, not the CLI's.** An MCP server is a separate process the CLI starts; the
   CLI's command sandbox doesn't cover it (Codex's permissions docs, "Scope and enforcement": the sandbox governs
   command execution, MCP servers have their own controls). So the server entry runs REA under Brigadier's Seatbelt
   wrapper (the one `run` uses, `crates/sandbox`): the target app read-only, the task's scratch folder (and REA's
   cache, step 3) writable, nothing else writable. Linux and Windows get the same through their `crates/sandbox`
   backends, or no REA where a backend can't express it.
6. **The worker guidance** for study tasks:
   - Start with `analyze_javascript_application` or `inspect_artifact` on the target, follow features with
     `trace_application_feature`, and cite REA's evidence ids in the report.
   - Read files by hand only where REA has no answer.
   - Our rules hold: no source-app names in what lands; findings are paraphrased, never vendored code.

**Verify.**
- Rust tests:
  - A study task's spec lists the REA server, and other tasks' don't.
  - The install is `npm ci --ignore-scripts` from the shipped manifest and lockfile, refuses a tarball whose
    integrity differs, and exposes nothing before its checks pass.
  - Confinement through a real MCP call: REA, started by the study task's server entry, can read the target and
    write the scratch folder, and an `extract_artifact` aimed outside them fails with "Operation not permitted".
  - No user config file is written (a fake `HOME` stays unchanged).
- **Measured:** one frozen study question about an Electron app's bundle (for example "how does it show a file
  preview beside the chat, and what triggers it"), with its expected findings written down first. Run once with REA
  and once without, on the same model and Brain seed, with Q6's A/B rules.
  - With REA: worker tokens ≤ 0.5× without.
  - Both arms reach the written findings, judged by an independent reviewer.

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
- Bug 6's ref check restores the live session branch if the thread deletes it anyway, and tells it. A removed live
  worktree is recreated by `ensure_target` on the next message, with the same note.

**Verify.**
- Rust test: after a merge, the next message re-creates the worktree, and the thread's turn input contains the
  `[workspace]` note.
- Rust test: the live session branch deleted between turns is restored at its recorded tip, and the thread hears it.

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
- Below Full access it would not stop a worker either: every writer's sandbox may write the repository's whole
  `refs`, `logs` and (Codex) `packed-refs.lock` (`commit_roots`, `M/workers.rs:4208-4226`). At Full access nothing is
  sandboxed at all (`Access::Full`: Claude's sandbox off, Codex's `--dangerously-bypass-approvals-and-sandbox`,
  `P/codex/mod.rs:377`).
- Brigadier already removes everything a session owns through the cleanup ledger, so the thread's manual cleanup is
  never needed.

**Fix: protected paths, enforced by the OS sandbox on every execution path.** No command parsing. A parser would
see only Claude's `Bash` and our `run`, not Codex's built-in shell (which the thread keeps, `THREAD_FEATURES`,
`P/codex/mod.rs:98-101`), scripts or `eval`. An OS rule holds for every process the CLI starts.
1. **What is protected.** For each session (the thread or a worker), Brigadier derives a protected set from what it
   already knows:
   - **Refs:** the repository's `refs/heads/brigadier/` and `logs/refs/heads/brigadier/`, except the session's own
     branch: a worker's task branch, or the thread's session branch. Also the base the session merges into, when
     the session has a branch of its own. Only the engine moves a target, after `check_merge` and the consent check
     (Q2).
   - **Brigadier's data dir,** except the folders this session's spec already lists as its own: its worktree,
     scratch and test folders, worker home, CLI home.
   - **Other sessions' temp and test folders:** `/tmp/brigadier-<12 hex>` (`CLAUDE_CODE_TMPDIR`) and
     `brigadier-test-*`, except the session's own.
2. **Below Full access (the CLIs' own sandboxes).** `commit_roots` stops granting the whole `refs` and `logs`. A
   writer gets its own git dir, `objects`, and its own branch's ref, `.lock` and reflog. The thread also gets
   `refs/remotes/` for push tracking.
   - **`packed-refs.lock` stays granted; `packed-refs` itself is protected.** A commit in a linked worktree takes
     and gives back the shared lock, and without it every Codex commit printed "Unable to create
     packed-refs.lock: Operation not permitted" (`3278280c`, docs/evidence/2026-10-07-thread-phase1.md:55). Holding
     the lock is harmless on its own: replacing `packed-refs` means renaming the lock over it, which needs write
     access to `packed-refs`, and that is not granted. Phase 1 proves both halves by test (a commit prints no git
     error; a rename of the lock over `packed-refs` fails). If the test shows a sandbox can't keep them apart, the
     alternative is chosen by test before the grant changes, never by dropping the lock.
   - Checked by test (2026-10-11): Claude 2.1.296's sandbox, with the worktree as an added folder (a worker's or the
     thread's cwd is never the worktree), grants exactly the listed files, a not-yet-existing `.lock` included; it
     doesn't widen to the shared git folder as it does for a cwd that is a linked worktree. Codex 0.162.1's
     profile does the same. `git pack-refs` prunes a packed ref's emptied folder, which a sandboxed git can't make
     again: the engine makes it whenever it grants the roots (a CLI's start, each ref check).
   - Claude also gets the protected set as `filesystem.denyWrite`, plus `Edit`/`Write`/`NotebookEdit` deny rules, as
     already done for `hooks` and `config` (`P/claude/mod.rs:614-627`): its file tools work outside the OS sandbox.
     A deny wins over every allow in Claude's sandbox (sandbox-runtime 0.0.79 writes the denies after the allows),
     so a deny that would cover the session's own ref (`refs/heads/brigadier` for a worker) is left out: its grant
     never covered those refs anyway. It matters for a thread on the user's own checkout, which is writable whole.
   - Codex needs no deny list: no writable root covers a protected path.
   - Brigadier's own Seatbelt for `run` and `run_check` uses the same roots, a `.lock` that doesn't exist yet
     included (its existing folders resolved, nothing more granted).
   - A ref update writes `<ref>.lock` and renames it over the ref. If a CLI's sandbox can't grant a path that
     doesn't exist yet, each task branch gets a folder of its own (`brigadier/<short>/task-N/work`) and that folder
     is granted. Phase 1 decides this by test.
3. **At Full access: the thread only** (revised 2026-10-11, the Delegator's decision after a test). A Seatbelt
   profile can't nest: inside one, `sandbox-exec` fails with `sandbox_apply: Operation not permitted` (exit 71).
   Wrapping every Full-access process would break whatever applies its own sandbox: `swift build` (its manifest
   sandbox), Bazel's darwin sandbox, and Brigadier's own Seatbelt and Codex-sandbox tests, so a Full-access worker
   or `run_check` running `tools/full-checks.sh` on Brigadier would fail. So:
   - **The thread's CLI** (Claude or Codex, so Codex's built-in shell too) and **the thread's own `run` commands**
     start inside Brigadier's Seatbelt with an allow-all profile: `(allow default)`, then `(deny file-write* …)`
     for the protected set, then `(allow file-write* …)` for its own exceptions (later rules win; a regex rule
     covers a not-yet-existing `.lock`). A protected path inside an exception (a local-checkout thread's workspace
     holds the shared `.git`) is denied again after them, and the exceptions inside it allowed again: the narrower
     rule wins. Every child inherits it. A `run_unsandboxed` command the user approved runs as approved.
   - Its protected set at Full access: other branches and the base (step 1), Brigadier's data dir except its scratch
     folder and workspace, every `/tmp/brigadier-<12 hex>` and `/tmp/brigadier-test-*` folder. The CLI's own homes
     (configuration, history) and its own temp folder are given back by the adapter. Its own temp folder can't be
     removed, nor the folders in it three deep (`rm -rf` empties files and leaves the layout the CLI relies on).
   - **Full-access workers and `run_check` run unwrapped,** as today. The trade-off, accepted: a Full-access worker
     can still delete another session's temp or test folders, other worktrees and Brigadier's data dir, and
     can move or delete branches. Branches are covered by the ref check (step 5: protected branches restored,
     `main` reported); folders are not. That's accepted because the incident's actor was the thread (its own
     cleanup); workers have a narrow task in their own worktree, the worker prompt says to clean up only through
     `clean_up` (4.7), and test folders and worktrees of a finished task are made again or were disposable.
     Below Full access every worker stays inside its CLI's sandbox (step 2).
   - The Full-access description in Settings says so in one clause: the lead can't change other sessions' branches
     and folders or Brigadier's own data.
4. **Why git can't get around it.** Git's files backend takes the loose `<ref>.lock` for every update or delete of a
   ref, packed or not. So `git branch -D`, `git update-ref -d`, `git push . :ref`, `git worktree remove` and `rm -rf`
   on a protected path fail with "Operation not permitted".
   - `packed-refs` stays writable at Full access, so the user's own branch work isn't broken.
   - Editing `packed-refs` by hand would be deliberate evasion, outside the threat model (a careless agent). Step 5
     catches it.
5. **The engine's ref check** (the backstop). The engine records every live branch it owns with its tip and its
   owner (a task branch: that task's worker; the session branch: the thread). It checks them before any landing or
   merge (the caller: the landing task's worker) and after every thread turn (the caller: the thread). Workers
   commit on their own branches and the thread commits small edits on the session branch, both outside the engine,
   so the check tells legitimate moves from damage:
   - **The caller's own branch moved** (any move, a rewrite such as `--amend` included): legitimate. Its new tip
     is recorded.
   - **Any branch moved forward** (the new tip descends from the recorded one): commits were added, nothing can be
     lost, and a concurrent worker's or the thread's commits look exactly like this. Its new tip is recorded, never
     restored.
   - **A protected target deleted, or moved so its recorded tip is no longer an ancestor:** restored at its
     recorded tip, and the thread is told: `[restored brigadier/…/session: it was deleted outside Brigadier]`. The
     protected targets for a caller are the branches of step 1: other sessions' task and session branches, and,
     for a worker, the session branch. A live branch deleted by its own owner is restored too (bug 5): no session
     deletes a branch the engine still uses. A rewrite of another branch while its own worker is live is
     ambiguous (the worker may have amended it): its new tip is recorded and the old tip logged with the note
     `[brigadier/…/task-5 was rewritten while its worker ran: <old> → <new>]`. Live means its CLI runs (mid-turn
     or between turns, a resumed one included) or it is open in the user's terminal; a worker waiting for quota
     or stopped after dying twice runs nothing, so a rewrite of its branch is put back.
   - **The base (`main`)** is the user's branch, not the engine's: the user may move or reset it from their own
     terminal at any time. The check never restores it. A deletion or non-forward move of the base seen at a check
     is told to the thread, which tells the user. The sandbox (steps 2–3) is the base's protection.
   - **Serialized with landings.** Every move the engine makes (a landing's fast-forward, its rewrite of the task
     branch onto the target without litter or AI co-authors, a merge into the base, a branch it creates or
     deletes, a stopped task's branch rewritten to keep its work without the snapshot or AI co-authors) and the
     check itself run under one per-repository ref lock in the engine, and the engine updates its recorded tip
     inside that same critical section. So the check never sees the engine's own move as foreign, and a restore
     can't undo a landing that ran a moment earlier. The check reads its records only once it holds that lock, so
     a branch the engine forgot and deleted just before is never recreated from an older copy.
   - This also covers platforms whose sandbox can't express allow-all-but rules (Linux and Windows today).
6. **A `clean_up` tool** (plumbing) for the thread:
   - It lists and removes only what the ledger records under this conversation's owners (`session:`, `task:`,
     `review:`, `review-hold:`, `workers:`, `orch:` of this id) and that is no longer in use: merged task branches
     `brigadier/<own short id>/…`, finished tasks' worktrees and test folders.
   - It returns what it removed and what it kept, and why.
   - The prompt says: "Brigadier cleans up after you; use clean_up if the user asks. Never delete branches,
     worktrees or temp folders by hand. 'Operation not permitted' on another session's branch or folder is
     Brigadier protecting it: leave it."
7. **Dropped from the first draft:** the `PreToolUse` command parser and the `run` check. Step 3 covers every path
   they covered, and the ones they missed.

**Verify.**
- Rust tests:
  - `clean_up` removes this session's merged task branch and keeps another conversation's.
  - The protected set for a worker and for the thread: the right refs, folders and exceptions; `commit_roots` no
    longer contains the whole `refs` or `logs`, and still contains `packed-refs.lock` (not `packed-refs`).
  - The ref check restores a deleted live task branch and leaves a merged, released one deleted.
  - Commits survive the check: a worker's commits on its task branch (an `--amend` included) and the thread's
    commit on the session branch are recorded, not restored, whether the check runs before another task's landing
    or after the thread's turn. A concurrent worker's new commits on its own branch survive a check whose caller is
    the thread.
  - A worker's force-move of the session branch to an older commit is restored at the check before its landing; a
    `main` moved by the user is never touched.
  - Serialization: a landing and a check started together never restore the landing's move (the recorded tip is
    updated under the same lock).
  - The live-owner amend exception (outline-review correction #1): during task-1's landing check, task-2's amend
    while its worker is live is recorded and told, never restored; with its worker gone, the same rewrite is put
    back.
  - At Full access `packed-refs` stays writable (correction #3): a hand edit of it deleting a protected ref is put
    back by the check. Below Full access a rename over `packed-refs` fails (sandbox tests above).
- Sandbox tests (macOS, with a real `git` and `sandbox-exec`), under the thread's Full-access profile and under each CLI's
  scoped sandbox settings:
  - `git branch -D brigadier/<other>/task-1` fails, with the ref loose and again with it packed; the ref survives.
  - `git update-ref refs/heads/main <sha>` fails; a commit on the session's own branch works and prints no git
    error (a Codex worker included: the shared `packed-refs.lock` is taken and given back).
  - Renaming a file over `packed-refs` (`mv packed-refs.lock packed-refs`, after taking the lock) fails.
  - `rm -rf /tmp/brigadier-test-*` removes only the session's own folders; `rm -rf /tmp/brigadier-<other temp>`
    fails; `rm -rf /tmp/my-scratch` works.
  - Codex's built-in shell (a Codex thread at Full access, live) gets the same refusals.
- Live, dev build: ask the thread to "clean up all branches and tmp" with another session's task running, at
  Approve for me and at Full access, once on Claude and once on Codex. Nothing of the other session is touched.

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
- Open sessions hear it as a versioned `[instructions]` note (§6, "Instruction changes reach open sessions"), with no
  restart.

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
3. `run_check` sets `TMPDIR` to the task's test folder itself. It drops a leading prefix only when the prefix is
   proven redundant with the environment and limits it already runs the command with, and then runs the command
   without it, so the cache key is always the command actually executed (`M/checks.rs`):
   - `TMPDIR=<dir> ` or `export TMPDIR=<dir>;` where `<dir>` is exactly the task's test folder that `run_check`
     sets anyway.
   - `ulimit -n N;` only when it leaves both the soft and the hard limit as the check already has them (step 1):
     `-S` with N the soft limit, `-H` with N the hard one, and without either only when N is both (bash's `ulimit -n
     N` sets both limits, zsh's the soft one). A `ulimit -n 8192` under a raised soft limit lowers it, so it stays.
   - Anything else stays in the command and in the key, and runs as written: a `TMPDIR` pointing elsewhere, a
     `ulimit` that changes either limit or another resource, other variables. (Revised after the outline review:
     the first rule dropped any `ulimit -n N` at or below the soft limit, which changes the limits.)
   - The worker prompt says not to add these prefixes.

**Verify.**
- Rust tests:
  - The daemon's spawned child reports `ulimit -n` ≥ 8192.
  - Two `run_check` calls with and without a redundant `ulimit -Sn <soft>;` / `TMPDIR=<test folder>` prefix on the
    same tree → the second is `cached`, and the command run is the one without the prefix.
  - A `TMPDIR=/somewhere/else` prefix, or a `ulimit -n 8192` that would lower the soft limit or set the hard one,
    keeps its own cache entry and is run as written.
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

**The comparison uses computer use** (§3, "Computer use"): the mockup worker, or the final check's worker,
launches the dev build and captures the live screen with the computer tools, next to the mockup's screenshot.

**Mockups stay opt-in:** offered as a card and built only on yes. If the user wants to see options first, a mockup of
the surface itself is offered the same way: built only on yes.

## 6. Phases

The order puts the riskiest engine fixes first. Each phase lands as small commits on its own branch from the
integration branch `session-fixes` (§7). Each "done when" also includes:
- `tools/full-checks.sh` passing on the integrated tree;
- one review by the other vendor;
- for UI phases, the native dev-app checks of THREAD-PARITY §7, done with computer use on the dev build (§3,
  "Computer use");
- for a phase that changes instructions, the note test below.

**Where each decision and bug is built:**

| Item | Phase, step |
|---|---|
| Bug 1 (Claude workers killed on landing) | 1.1 |
| Bug 8 (test environment) | 1.2 |
| Bug 6, protected paths and the ref check (cleanup safety) | 1.3; `clean_up` and its prompt 4.7 |
| Q4 (worker resume, overnight state first) | 1.4 (row label too) |
| Q2 (merges by the LLM, engine checks) | 1.5 |
| Bug 2 (resume lookup) | 2.1 |
| Bug 3 (permission change rebirth) | 2.2 |
| Q5 (rebirth at 400k, briefing, cold rebirth above 150k) | 2.3 |
| Q7 (review status by the thread) | 3.2 daemon, 5.4 UI |
| Q6 (tokens, Brain-first, measured) | 3.1, 3.3–3.8 |
| REA for study tasks | 3.7 |
| Q3 (merge consent hold) | 4.1 daemon, 5.1 pill |
| Q1 (CI watch) | 4.2 daemon, 5.1 pill |
| Q9 (no silences) | 4.3 daemon, 5.6 UI |
| Bug 4 (duplicate answers) | 4.4, 5.5 UI |
| Bug 7 (short tool names) | 4.5 |
| Bug 5 (removed-but-there worktree) | 4.6 |
| Computer use known and routed | 4.8 |
| Q8 (live-only strip, one slot) | 5.1–5.3 |
| Q10 (opt-in mockups) | 6.1–6.4 |

### Instruction changes reach open sessions
Resumed CLIs keep the instructions they started with: Claude's are in the session record
(`--system-prompt-snapshot`, `P/claude/mod.rs:1101`), Codex's `developer_instructions` were set at the start (PLAN §7,
THREAD-PARITY §5). The user's answer: the gentlest way, a full start-over only where unavoidable (expect none).
- **Behaviour changes go as a short versioned note, with no restart.** `Told` gains `revision: u32`. Each phase that
  changes the thread's or the workers' instructions adds one entry to a `REVISIONS` list in `M/prompts.rs`: its
  number and a short `[instructions]` note saying what changed, for example "From now on, a late event gets one
  short update line, never a new ending." A resumed session whose `told.revision` is older gets the notes it missed,
  once, at the start of its next turn, through the existing `prompts::notes` path (`:475`). Its `told.revision` is
  then current. New sessions start on the current instructions and hear nothing.
- **New tools: the CLI resumes the same session between turns, with a note.** A tool list is read when the CLI
  starts. When a phase adds a tool to a role (`record_findings`, `watch_ci`, `ci_status`, `clean_up`, the REA
  server), the next turn of an open session closes its idle CLI and resumes the same native session (the path bugs 2
  and 3 make reliable). The note names the new tool: "You now have record_findings: …". Its context stays.
- **`CONTRACT` stays at 7.** A bump, which starts sessions over from a briefing, is kept for a role change no note
  can carry. None is expected in this plan. If a phase finds one, it stops and asks.
- **Workers** resumed by Q4 or Q6.1 get the same notes in their resume input.
- **Every phase that changes instructions** ships its revision in the same commit. The test: a session told at
  revision N−1 resumes at N with exactly the new note, and one at N hears nothing. This applies to phases 1 (bug 8's
  worker line, Q4's interrupted note), 3 (Q6, Q7's `record_findings`, REA), 4 (Q1, Q3, Q9, bugs 4–7, computer use)
  and 6 (the mockup offer).

### Phase 1: engine safety (worker kills, the test environment, ownership, worker resume, merges)
**Scope**, commit-sized steps:
1. Bug 1: `sweep_dir` in `SessionSpec`, unset for the shared home; doc comment; providers and core tests.
2. Bug 8: the daemon's `RLIMIT_NOFILE`; `run_check` queued before spawn, never paused mid-run; canonical check keys
   with the `TMPDIR`/`ulimit` handling; worker prompt line (revision note).
3. Bug 6's protection: the protected set per session; `commit_roots` narrowed to the session's own branch; Claude's
   `denyWrite` and file-tool deny rules; `packed-refs.lock` kept, `packed-refs` protected; the Full-access Seatbelt
   profile for every CLI and for `run`/`run_check`; the engine's ref check (own branch and forward moves recorded,
   protected targets restored, the base never restored, one ref lock shared with landings). The sandbox tests of
   bug 6.
4. Q4: crash → resume (same session, model, worktree, the interrupted note); daemon restart → resume, with each
   overnight run's state classified first (hand-off only inside the wind-down allowance, nothing past the deadline or
   cutoff); "died twice" stop; vendor switch only for limits, auth or an outage; the
   "Interrupted — resuming" row.
5. Q2: the merge brief (real git), `check_merge` (the merge in HEAD's ancestry, both inputs ancestors, the final
   tree; contested and uncontested deletions), merge landing as an engine fast-forward with the expected-tip check
   and no litter guard, the same worker resumed when the target moved. Then delete the pre-merge code and its
   tests.

**Why this order inside the phase.**
- Bug 1 first: every live check after it needs workers that survive a landing.
- Bug 8 before Q4: the resume relies on cached checks.
- Bug 6's protection before Q4 and Q2: resumed and merging workers must already be unable to touch others' refs, and
  Q2's "only the engine moves the target" rests on it.
- Q2 last: it is the largest change, and it uses Q4's resume for "the worker is told what's missing".

**Done when:**
- Bug 1's three checks pass; the live run shows 0 Claude workers killed by a landing.
- Bug 8: `ulimit -n` ≥ 8192 in a worker's `run_check`; a second identical check with a redundant prefix is cached,
  and one with a non-redundant prefix is not; 0 SIGSTOP to a running check (signal log).
- Bug 6: its sandbox tests pass under the thread's Full-access profile (the incident's commands: `git branch -D` of
  another session's branch, `rm` of another session's folders and of the thread's own temp folder, all refused) and
  under both CLIs' scoped sandboxes; Full-access workers are covered by the ref-check tests; the live "clean up everything"
  request at Full access, on Claude and on Codex, leaves a parallel session intact; a normal commit (with no
  `packed-refs.lock` error on Codex), a merge task and a push still work at every level; worker and thread commits
  survive the ref check, and a concurrent worker's commits survive a check made for the thread.
- Q4's tests pass; live `kill -9` of a worker CLI resumes the same `nativeId`; a second kill stops the task with both
  exits named; a restart past an overnight deadline or cleanup cutoff resumes nothing and keeps each task's work in
  the report; a restart inside the wind-down allowance resumes a task only to hand off.
- Q2's tests pass, the correction-commit and concurrent-landing cases included. Live replay of B's merge lands a
  two-parent commit with all of `main`'s files (`git diff --stat` shows none missing), and the thread doesn't ask
  the user about conflicts.
- Live checks that involve the app use computer use on the dev build, never the installed app.

**Risks.**
- Resuming a Claude session whose process was killed mid-tool-call: the transcript may end on an unanswered tool use.
  Check live on the installed CLI version. If `--resume` refuses it, start a fresh session of the same model with the
  handoff files, which still beats switching vendor.
- **The Full-access profile** runs the thread's CLI under a sandbox for the first time (workers stay unwrapped, Bug 6
  step 3). Some tools behave differently under any
  Seatbelt profile. `(allow default)` keeps that small, and phase 1's done-when runs the full checks, a dev-app
  launch and a push at Full access. Anything that breaks is listed with its fix before the phase ends.
- Narrowed `commit_roots` can break git commands that write shared state (`git stash`, `rerere`, `gc --auto`'s
  pack-refs). Those are cross-session hazards anyway: `git stash` fails with a clear error, and the worker prompt
  says to commit instead.
- The shared `objects` folder stays writable to writers (commits need it). Deleting objects would be deliberate
  destruction, outside the threat model. A per-task object store is a later step if it is ever needed.
- The heavy-command classifier may miss a test runner. The check-level queue covers `run_check`, and shell runs
  stay as today.

### Phase 2: thread rebirth and resume (Q5, bugs 2 and 3)
**Scope:**
1. Bug 2: paged lookup and the `ConvLive` cache.
2. Bug 3: permission change and idle close resume, with the note.
3. Q5: the 400k/450k thresholds on big windows; decisions whole; excerpts cut first; the cold-cache floor at 150k.

**Done when:**
- Bug 2's and bug 3's tests pass.
- Live: a 1M-window session passes 150k and keeps its session id (0 rebirths). A permission switch resumes.
- The rebirth tests show every decision body kept at the cap. The cold-cache tests resume under 150k and are reborn
  above it.

**Risks.**
- A 400k context costs more per turn than a reborn 30k one. Above 150k, an expired cache still leads to a cold
  rebirth (the user's answer), so a big idle thread isn't rewritten at the cache-write price.
- Resuming after a CLI upgrade may fail. The existing Recovery path still catches a resume that fails.

### Phase 3: token economy and Brain-first (Q6, Q7's finding model, REA), measured
**Scope:**
1. The fixed task, its frozen acceptance checks, the Brain seed, and the "before" run with
   `tools/ab/worker_tokens.py` (the tool comes first).
2. Q7 daemon: findings as items, `record_findings`, stored statuses; `merged_reviews` removed. Q6.1 and Q3 read
   these statuses, so they come before both.
3. Q6.1: one review per change; open findings to the resumed worker in one batch, with the landed worker kept
   resumable until its batch settles; no review of fixes except the targeted re-check.
4. Q6.2 and Q6.3: prompt changes for checks and code reading; the Codex reading line.
5. Q6.4: worker handoff at 200k.
6. Q6.5: the article settings, each checked against the installed CLI's docs.
7. REA for study tasks: the install from the reviewed manifest and lockfile (`npm ci --ignore-scripts`); the server
   for study tasks only, under Brigadier's sandbox wrapper; the guidance; its measured study question, with and
   without.
8. Q6.6: Brain facts in the pack, the "Brain first" steer, `learned` write-back, per-worker Brain metrics on the row
   and in the Inspector.
9. The "after" run, verified independently, written to `docs/evidence/<date>-session-fixes-tokens.md`.
10. The phase's instruction revision and new-tool notes.

**Done when:**
- Q6's "Measured" criteria hold on the fixed task, with both arms passing the frozen checks and the independent
  review: worker tokens ≤ 0.6× before; no provider's total above before, and the sum ≤ 0.75×; elapsed ≤ 1.25×;
  ≥ 1 Brain query per worker; ≥ 1 Brain fact in each brief; 0 extra review-fix tasks; `run_check` calls halved.
- REA's measured study question: worker tokens with REA ≤ 0.5× without, and both reach the written findings.
- Q7's daemon tests pass. The Brain counters show on the worker rows.

**Risks.**
- The Brain may have little about a fresh area; the measurement must use an area the Brain has seen (B's).
- An unknown settings key may be ignored silently or break the start. Each key is checked against the installed
  version and dropped if it isn't listed.
- A landed worker's session files are now kept until its batch settles. They are small, and the session's merge or
  deletion releases them all.
- **REA's supply chain:** one maintainer and a fast release pace. Ship a reviewed manifest and lockfile with every
  dependency's integrity, require provenance, install with `npm ci --ignore-scripts`, never `npx`, never `rea setup`
  or `rea update`. A new version is a reviewed code change.

### Phase 4: the thread's behaviour (Q1, Q3, Q9, bugs 4–7, computer use)
**Scope:**
1. Q3 daemon: `propose_merge` waits for writers; `MergePending` with the `WorkersFinish` condition; `[merge due]`,
   `[merge stopped]` on any open finding; the durable, atomic Cancel; consent stale only by words.
2. Q1 daemon: `ci_status` and `watch_ci` (GitHub Actions; runs bound to repo, branch and sha; jobs from `run view`;
   waiting vs no runs; several workflows), the `[ci …]` messages, the ask-after-push instruction.
3. Q9 daemon: preview-exit messages, the 30-minute `[status due]`, the instructions.
4. Bug 4: the update-line rule; envelopes changed.
5. Bug 7: callable tool names for Claude.
6. Bug 5: the `[workspace]` note when a merged session's worktree is re-created.
7. Bug 6: the `clean_up` tool and its prompt lines.
8. Computer use: the thread's routing paragraph, `WORKER_COMPUTER`, the pack's dev-app command, the helper's
   refusal of a Brigadier bundle the worker didn't launch.
9. The phase's instruction revision and new-tool notes (§6, "Instruction changes reach open sessions").
10. The red CI on `97c298d5` (`ThreadScrollRender.test.ts:60`): check it on the current `main` first. Fix it here if
    it's still red.

**Done when:**
- All Rust tests of Q1, Q3, Q9, bugs 4–7 and computer use pass.
- **Live, dev build:**
  - "merge when done" with 2 writers merges after they land, with no second question; Cancel on the pill stops it,
    and the old consent doesn't come back after a restart;
  - a push in a repo with GitHub Actions asks to watch, and a red run reaches the thread with its failing jobs; a
    repo with GitLab CI gets the one-line "can't watch" with its link;
  - a 40-minute request shows one status line;
  - 10 fresh threads show 0 "No such tool";
  - B's fixture shows one ending plus updates;
  - the UI request of §3 "Computer use" shows a live check of `Brigadier Dev.app` in the worker's report.
- **Notes, not restarts:** a session open on the phase-3 revision resumes with the phase-4 note and keeps its native
  session id. A session with no new tool hears the note with no CLI restart. 0 sessions start over.

**Risks.**
- `gh` may be missing or logged out. `ci_status` says so; the thread says one line; it never blocks.
- Some Claude review texts won't split into findings; anything unparsed is one finding (Q7).
- A note is weaker than instructions written at the start: an old session may follow its first instructions on a
  detail. Each note is phrased as a replacement ("From now on…"), which the instructions' `AUTHORITY` line already
  gives precedence (`M/prompts.rs:343`).

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
- Native dev-app checks in Compact, Normal and a narrow window, with computer use on `Brigadier Dev.app`.

**Risks.** Three stacked rows crowd a small window. Cap at three rows, and let the slot collapse to one line with a
count when the window is narrow.

### Phase 6: opt-in mockup previews (Q10)
**Scope:**
1. The thread's offer (instruction plus an `ask_user` card shape).
2. The mockup task kind or brief (real components, fake data, fixture page, screenshots).
3. The surface from §5.
4. The final side-by-side check in the report, with the live side captured by computer use.
5. The phase's instruction revision: open threads hear the mockup offer as a note.

**Done when:** the three live checks of Q10 pass, with screenshots of the mockup in its surface and of the final
side-by-side. A thread open since phase 5 offers a mockup after its note, without starting over.

**Risks.** A mockup of a deep screen may need state the fixtures don't have. The worker builds a fixture board for it
(as `thread-t1-2026-10-08.events.json` was), and a screen it can't fake is said in the offer.

### Why this order
- Phase 1 holds the bugs that lose work: missing files, killed workers, and agents deleting others' branches. Every
  later phase's live checks need workers that survive, merges that keep files, and refs nobody else can move.
- Phase 2 next: rebirths cost the thread its context during every later live check.
- Phase 3 before phase 4: Q7's finding model is built first in phase 3, because Q6.1's single review batch and
  phase 4's merge stop (Q3) both read the statuses the thread records. Measuring tokens before the behaviour changes
  keeps the numbers clean.
- The UI comes after the daemon it shows.
- Mockups come last: they are new, opt-in, and depend on nothing else here.
- Two changes from the brief's suggested order: bug 8 (test environment) moves from "thread behaviour" into phase 1,
  because flaky tests and cache misses undermine every later done-when and Q4's cached checks depend on it. Bug 6's
  protection moves into phase 1 too, because Q2 and Q4 rest on it.

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
  - **Live UI checks use computer use** on `Brigadier Dev.app` (`tools/open-dev-app.sh <scratch data dir>`), never
    the installed app. A report never says "not checked in the live app" for a check computer use could do.
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
  `--help` or current docs, and cite it in the commit or report. This covers `claude`, `codex`, `gh`, assistant-ui
  and REA (its pinned version's docs and integrity).
- **Instruction changes ship as notes.** A commit that changes the thread's or the workers' instructions adds its
  revision note in the same commit (§6, "Instruction changes reach open sessions"). Never bump `CONTRACT` without
  asking.
- **Before each report:** `tools/full-checks.sh`, `cargo fmt`, `clippy`, `gen-ts`, and the `pnpm` checks, on the
  exact tree.
- **One phase at a time:** commit, `report.md` for that phase only, status `done`, stop. A fresh verifier checks it
  before the next phase.

## 8. The user's answers to the plan's open questions (2026-10-11)

1. **Cold-cache rebirth: kept, only above about 150k context** (grill Q12). A thread under ~150k that sat idle over
   an hour resumes normally; a bigger one gets the cold-cache rebirth from a briefing, with decisions kept whole.
   Built in Q5 step 4.
2. **CI watch is GitHub Actions only, for now.** With other CI, or `gh` missing or not signed in, the thread says
   in one line after the push that it can't watch this repo's CI from here, gives the CI link if it can find one,
   and doesn't offer to watch. Built in Q1 step 1.
3. **Instruction changes reach open sessions the gentlest way.** Behaviour changes go as a short versioned note,
   with no restart. New tools mean the CLI resumes the same session between turns, with a note, keeping its context.
   A full start-over only where unavoidable (none expected). This replaces the first draft's single `CONTRACT`
   bump; see §6, "Instruction changes reach open sessions".

The same message added two asks, now in §3: computer use known to every role and used for live checks, and REA for
study tasks.

## 9. Not in this plan

From the audit backlog, left out on purpose:
- **"Full access thread still said push needs go-ahead" (A).** By design: pushing happens only when the user asks
  for exactly that, at every level (`M/prompts.rs:99`). Q1 changes only what follows a push.
- **The Q&A habit "as you recommend" (7 of 9 answers).** The grill took no decision; the cards already mark the
  recommended option.
- **The dev-app start that failed silently (A)** is covered by Q9's rule on failures, not as a bug of its own.

## 10. Review triage (Codex plan review, 2026-10-11)

### 10.1 First review

The review's 12 findings were checked against the code at `7d8898ff`. All 12 hold and are adopted. None was proved
wrong. One goes further than the review said.

| # | Finding | Checked in the code | Adopted in |
|---|---|---|---|
| 1 | Review fixes can't resume a landed worker | `dispose_task` disposes the task's ledger owner, which removes `ClaudeSession` files (`P/claude/files.rs:60-90`) and the worktree; the landing then deletes the merged branch (`M/landing.rs:751-770`) | Q6.1 lifecycle: `review-hold:<task>`, worktree recreated at the old path, release when the batch settles |
| 2 | Restart recovery can restart work after a deadline or Stop | `recover` runs before `resume_runs` (`M/mod.rs:277-278`); a resumed session takes `admit_run_task`, which skips the wind-down check (`M/overnight/admission.rs:154-180`) | Q4 step 2: run state classified first; winding down → hand-off only; stopped/ended → no resume |
| 3 | Codex's built-in shell bypasses the guard | The Codex thread keeps its shell (`THREAD_FEATURES`, `P/codex/mod.rs:98-101`); hooks are off (`:117`) | Bug 6: OS-enforced protected paths on every path; the parser is dropped |
| 4 | Shared git-dir access defeats ownership | **Broader than stated:** every writer below Full access already gets the whole `refs`, `logs` and `packed-refs.lock` (`commit_roots`, `M/workers.rs:4208-4226`), not only merge tasks; Full access has no sandbox at all | Bug 6 steps 1–5, phase 1.3; Q2 steps 3 and 5 |
| 5 | The deletion exception permits silent loss | The first draft exempted any file the other side deleted | Q2 step 2: contested vs uncontested deletions |
| 6 | Cancel doesn't revoke consent | `merge_held` is in memory only (`M/conversation.rs:277`); a consent is spent only by a `Merged` step (`M/landing.rs:1655`, `:1734`) | Q3 step 5: stored `MergeCancelled`, under the landing lock |
| 7 | "Merge when done" is refused | `merge_consent::check` refuses "when", "once", "after" (`M/merge_consent.rs:22`, `:418`) | Q3 step 2: the `WorkersFinish` condition |
| 8 | Only high-severity findings stop a pending merge | The first draft narrowed the grill's "a real review bug stops the merge" | Q3 step 3: any open finding stops it; severity only picks re-checks |
| 9 | Phase 3 needs phase 4's finding model | `ReviewState::Findings` stores only a count | Phase 3.2: Q7 daemon before Q6.1; UI stays in phase 5 |
| 10 | One phase-4 bump can't deliver all instruction changes | Claude resumes keep the snapshot prompt (`P/claude/mod.rs:1101`) | §6 "Instruction changes reach open sessions": versioned notes in each phase, mockups included; no bump (the user's answer 3) |
| 11 | The token gate isn't a fair A/B | The first draft froze only the task and base | Q6.7: same Brain seed, frozen acceptance checks, independent verification, per-provider totals, elapsed time |
| 12 | CI polling lacks job results | `gh run list` has no jobs; `gh run view --json jobs` does (checked on `gh` 2.101.0) | Q1 step 2: runs bound to repo, branch and sha; jobs per run; waiting vs no runs; tests with delayed discovery and several workflows |

**Design notes on #3 and #4 (the simplest design that holds).**
- **What it protects against:** a careless agent running ordinary commands, not a deliberate attacker.
- **Every execution path:** Claude's `Bash` and file tools, Codex's built-in shell, scripts, `eval`, and Brigadier's
  `run` and `run_check`. All of them are processes under an OS sandbox the session can't leave.
- **Below Full access** that sandbox is the CLI's own: it already exists, and only its write list changes.
- **At Full access** it is a new allow-all Seatbelt profile with a short deny list. It doesn't nest, because the
  CLIs' own sandboxes are off at that level.
- **Git is covered** because every ref update or delete takes the loose `<ref>.lock`, packed or not.
- **The engine's ref check** restores live branches from its own record. It covers what the sandbox can't express:
  hand edits of `packed-refs`, and platforms without allow-all-but rules.
- **Alternatives rejected:**
  - Command parsing misses paths by design, which is the review's point.
  - Per-task clones with separate ref stores would hold too, but they rebuild the whole worktree model (warm clones,
    landings, `is_merged`) for the same protection.

### 10.2 Second review (verdict REVISE, 8 findings)

Checked against the code at `95b2498c` (= `main` 7d8898ff plus the plan). All 8 hold and are adopted; none was
rejected. Two adoptions are narrower than the review's wording, for the reasons given.

| # | Finding | Checked in the code | Adopted in |
|---|---|---|---|
| 1 | Ref restoration can undo legitimate commits | Workers commit on their task branches and the thread on the session branch, outside the engine; step 5 restored "a live branch moved outside the engine", which is every such commit. A check run for one caller also sees other workers' concurrent commits | Bug 6 step 5: the caller's own branch and any forward move are recorded, never restored; only protected targets deleted or rewound are restored; one per-repository ref lock shared with landings, the recorded tip updated inside it. **Narrower:** the base (`main`) is never restored, only reported: it is the user's branch and the user may move or reset it from their own terminal; the sandbox protects it from sessions. A rewrite of a branch whose own worker is live is ambiguous (an `--amend`) and is recorded and logged, not restored. Tests: worker and thread commits survive |
| 2 | Recovery after a deadline contradicts PLAN §10.9 | §10.9: "A clock jump past the deadline goes straight to report/cleanup"; §10.10 recovery step 4 admits no replacement once crossed; `resume_run` already ends an overdue run (`M/overnight/recovery.rs:76-86`); the cutoff is `cutoff_ms` (`M/overnight/wind_down.rs:411`) | Q4 step 2: hand-off-only resume only inside the wind-down allowance (before the cutoff); past the deadline or cutoff nothing resumes, the work is preserved mechanically (§10.9 step 3) and reported. Q4 verify and phase 1 done-when |
| 3 | REA isn't confined by the CLI's sandbox | MCP servers are processes the CLI starts outside its command sandbox (Codex permissions docs, "Scope and enforcement"); Brigadier starts Codex's app-server directly | REA step 5: REA runs under Brigadier's own sandbox wrapper (`crates/sandbox`), target read-only, the task's output writable; a test through a real MCP call. Built in phase 3 |
| 4 | Dropping `packed-refs.lock` brings back a git error | `3278280c` and docs/evidence/2026-10-07-thread-phase1.md:55: without the lock every Codex commit printed "Unable to create packed-refs.lock" | Bug 6 step 2: the lock stays granted, `packed-refs` itself is protected (renaming the lock over it needs write access to it); both proved by test, and any alternative chosen by test first |
| 5 | HEAD must be the merge commit, breaking the correction loop | Q2 step 2 required "the tip is a merge commit"; Q2's own refusal asks the worker to restore files, which it does with a new commit | Q2 step 2: the merge commit in HEAD's ancestry, both inputs ancestors of HEAD, the checks on the final tree; a test of a refused merge repaired by a later commit |
| 6 | No recovery when the target advances | Today's landing keeps an expected tip and retries on `LandBlock::TipMoved` (`M/landing.rs:453-466`); the plan's fast-forward had no such path | Q2 step 3: the expected-tip check kept; on a moved target nothing lands and the same merge worker is resumed against the new tip, then re-validated; a concurrent-landing test and done-when case |
| 7 | Cache normalization can equate different commands | Bug 8 stripped any `TMPDIR=…` and `ulimit -n N` before keying | Bug 8 step 3: only prefixes proven redundant with what `run_check` already sets (TMPDIR equal to the task's test folder; a `ulimit -n` that leaves both the soft and the hard limit unchanged, per the outline review) are dropped, and the command runs without them, so the key is the command executed; everything else stays in the key |
| 8 | REA's install isn't fully pinned | Only the top-level package and tarball were pinned; about 148 transitive dependencies weren't | REA step 2: a reviewed manifest and lockfile with integrity values, `npm ci --ignore-scripts` (npm v11 `npm ci` docs), the install exposed only after its checks pass. Built in phase 3 |
