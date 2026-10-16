# Brigadier rebuild plan (2026-10-15)

The plan the rebuild run follows. It turns the user's decisions Q35–Q42 (memory `brigadier-decisions`) and this
folder's research (`PLAN.md`, `WEAK-SPOTS.md`, `SESSION-FORENSICS.md`, `DIGEST.md`, `PROBES.md`) into phases in build
order, each with files, checks and a ship gate.

- **Base:** current `main` (`c9a73e38`). `launch` (`50695633`) is already merged into it.
- **Paths:** `M/` = `crates/core/src/manager/`, `P/` = `crates/providers/src/`, `D/` = `apps/desktop/src/`. Line
  numbers are at `c9a73e38` unless marked `@d669bca0` (the research's base).
- **Item names:** F1–F5, A1–A15, B1–B8, P0.1–P0.3, V1–V3 and R are `PLAN.md`'s ("research A11" here).
  U1–U9 and C/G/S/L/W/N are `WEAK-SPOTS.md`'s. This plan's own steps are named by phase: A0–A11, B0–B5, C1–C6,
  D1.0–D1.7, E1–E3, D2.1–D2.4 (with D2.3b).
- **What `launch` already did, and this plan builds on:**
  - the thread makes small changes itself (about 100 lines or fewer, `M/prompts.rs:107`);
  - a worker reports once its checks pass, with its review in the background;
  - no re-checks after landing, and findings beyond the request are dismissed;
  - long reports are taken whole;
  - `include_request` passes the user's words verbatim;
  - unreported new files are handed back instead of dropped;
  - Approve for me is the default.

  Two of these are replaced: the 100-line limit (B1) and the background-only review for bigger work (B2).

The goal, in the user's words: the longest-running sessions, the fewest tokens, never hallucinating, never
forgetting, never doing things on its own, and output as good as plain Claude Code or Codex or better. A normal ask
must feel as direct and as cheap as the plain CLI.

## 1. Principles

### 1.1 The thread model, in plain words (Q35–Q39)

- **The thread is the worker.** A session's thread reads, edits, runs and commits in its own workspace, the way a
  plain Claude Code or Codex session does. A question gets an answer with the file:line it came from. A normal change
  is made, checked and committed by the thread itself.
- **Workers are for jobs that need them:** big work, parallel parts, very long jobs, a job the thread can't do itself
  (a look at a running app, since the thread has no computer tools), or the user's request. With "Start workers on its
  own" on, the thread decides and says so in one line first ("Starting 2 workers: the exporters and the CLI wiring;
  about 10 min"). With it off, it asks first.
- **Brigadier's extras switch on only when they help:** rebirth instead of compaction, same-session resume on limits
  and account switches, cross-vendor review, parallel workers, verified-only memory, the visual thread and cards. None
  of them is a step every ask pays for.
- **Nothing acts on its own.** Brigadier never makes any of these unless the user asked:
  - a merge;
  - a deletion of the user's workspace;
  - a saved fact without evidence;
  - a spare worktree;
  - a Desktop file or a clipboard write.

  The engine's cleanup of Brigadier's own temporary things stays (task scratch, a finished task's worktree), after
  what the user needs from them is kept (A1).
- **The workspace belongs to the session (Q36).** It is picked on the composer rail:
  - **local checkout:** the thread works in the user's checkout;
  - **worktree:** the session's own branch and worktree. It stays through merges, and goes only on archive, delete or
    a clear "Close workspace".
- **Merging is the user's act (Q37).** It happens only from the side panel's "Merge into main" button or the user's
  own words. The thread may say once, in plain text, that the work is ready.
- **Reviews scale with risk (Q39), as in the /delegator skill:**
  - tiny edits and answers get the thread's own real checks;
  - bigger or risky changes and all worker work get one other-vendor review in the background. Its findings are judged
    against the code and the request, never applied blindly, and the valid ones are fixed before "done";
  - big jobs also get an other-vendor plan review first and one fresh verifier with evidence per point at the end;
  - full suites run once, on the merged result.
- **The Brain stays, and only evidence makes a fact (Q38).** It is local SQLite with Model2Vec and FTS5, carries
  decisions, rules and hand-offs through rebirth, and holds the code index. A worker's "learned" item becomes a fact
  only with evidence (a passing check, or a file:line that was re-read). Until then it is an unverified claim, shown
  with its source and never briefed as truth.
- **Final answers say what was checked and what wasn't.** The thread fixes its own known defects instead of writing
  "To check:" for them.

### 1.2 What is removed or simplified, and why

| Removed or simplified | Where today | Why |
|---|---|---|
| "Delegate early: at most a quick query_brain or code_search, then delegate_task" | `M/prompts.rs:107` | Q35. A short ask became thread → scout → report → thread (S1); delegation costs a 15–30k worker start and hurts sequential work (DIGEST 9) |
| The ~100-line ceiling on the thread's own changes | `M/prompts.rs:107` (`launch` `cab67d97`) | Q35: the thread does normal work. Parallelism, length and missing tools decide, not size |
| A lead with an outline for normal asks | `M/prompts.rs` "A lead of multi-step or risky work sends an outline"; Q34 | Q35. Outlines and plan reviews are for big jobs only (Q39) |
| Merge cards and the `propose_merge` tool | `M/landing.rs:1299`, `:2866`; `crates/mcp-server/src/catalog.rs:187-202`; `M/prompts.rs:118`, `:303` | Q37. They are also the most likely source of the unasked merge (A5) |
| Merge consent from a stored card answer | `fresh_consent`, `M/landing.rs:2137-2210` | Q37: an old answer must never authorise a merge after the change |
| Removing the session worktree and branch after a merge; "the next message starts fresh from main" | `finish_session` `M/landing.rs:1481` → `release_merged_session` `M/lifecycle.rs:687`, `forget_session_worktree` `:751` | Q36. The user read it as the thread deleting its workspace (U9) |
| Pre-warming a worktree on every user message and after every delegation | `M/conversation.rs:1121`, `M/workers.rs:1498`, `M/prewarm.rs:113` | Q35: no spare worktrees. It also leaks on quit (U6) |
| Worker `learned` and `decisions` written as Brain facts | `learn_report` `M/brains.rs:1061` (Decision `:1190`, Convention `:1205`) | Q38. Unchecked claims spread (G1, DIGEST 7) |
| Rebirth-note decision lines stored as orchestrator decisions | `M/brains.rs:1385` | Q38. They are the thread's own claims unless they cite a user decision |
| "To check:" for things the thread can check or fix | `M/prompts.rs:135` | U3: the user had to say "fix it yourself" |
| Question-card waits counted as work | `M/requests.rs:660-680`, test `:773` | Q41.6: 17.5 of 29 minutes were the user reading cards (U7) |
| Enter anywhere in the view answers a card's lit choice | `D/app/conversation/ActionCards.tsx:145-175`, `:963` | A5: one stray key could merge |
| The "Short replies" setting | `crates/core/src/model.rs:826-828`; `D/app/settings/ConfigurationPage.tsx:78`, `:140` | The no-knobs rule: the best voice is always on (§1.4) |
| The whole tool catalog always loaded | `533bac83`; `crates/mcp-server/src/catalog.rs` | Q40, **only if the benchmark passes** (C2) |

### 1.3 Older decisions this plan overrides

- **`docs/PLAN.md` §2.1, "the orchestrator only talks"** (and Q2's pure orchestrator). THREAD-PLAN Q1 gave the thread
  tools; Q35 makes doing the work itself the default.
- **Q34's lead per request, with its outline,** for normal asks. For big jobs Q34 still holds: an advisory
  other-vendor plan review, one fresh verifier, and landing with no card.
- **THREAD-PLAN Q9:** worktree and branch removed after a merge, and the next message starts fresh. This also retires
  `ensure_target`'s post-merge disposal retry (`M/workers.rs:2502-2517`), and old post-merge disposals are cancelled
  at upgrade before the startup sweep (A3).
- **Merge cards everywhere:** THREAD-PLAN's card, research F3 (better card wording), and research §6 item 8 ("Merge
  and keep working", now moot because a merge always keeps working).
- **Auto-learned facts:** SESSION-FIXES Q6.6's `learned` write-back as Convention nodes.
- **`docs/THREAD-PARITY-PLAN.md:197-202`:** "a wait on an open question card keeps the span open".
- **`533bac83`, "load all tools up front"**, once C2's benchmark passes.
- **`launch`'s "about 100 changed lines or fewer"** as the limit on the thread's own work.

`docs/PLAN.md` (including its stale `:109`, "no merge card"), `docs/THREAD-PLAN.md` and `docs/THREAD-PARITY-PLAN.md`
are updated in the same commits as the code.

### 1.4 Settings › Configuration: behaviour toggles (confirmed by the user, 2026-10-15)

**"Review the work"** stays (`review_work`, `crates/core/src/model.rs:851`), on by default.
- **On:** reviews scale with risk (B2). Tiny edits get only the thread's own real checks. Bigger or risky changes and
  all worker work get one other-vendor review in the background. It is **not** a review of every change.
- **Off:** no review starts and nothing waits for one.

**New: "Start workers on its own"** (`start_workers_on_own`), on by default.
- **On:** the thread decides when workers help (big, parallel or very long jobs, or work it can't do itself) and
  announces them in one line first. Otherwise it does the work itself.
- **Off:** the thread asks first, with `ask_user`, one card per batch. Either way, a worker starts at once when the user
  asks for one.

**"Short replies" is removed.**
- **The voice:** fixed at the best default. The answer comes first, in a few plain lines, with details folded under
  `### Details`.
- **Changing it:** a user who wants longer or shorter answers says so in words. That is kept as a personal preference
  (Personal Brain, `M/prompts.rs:144`) and briefed to every session.
- **Stored values:** the field is still read but ignored. A stored `false` is copied once into the preference "longer
  answers", so nobody loses their choice silently. Sessions told `short_replies: false` get one `[settings]` note that
  the voice is now fixed (the `told` comparison, `M/prompts.rs:908`).
- **Why:** the 10-01 no-knobs rule. The short voice is the measured better default (Q32, PLAN §7).

**No other behaviour or quality toggles:** none for effort, rebirth, tool loading, token savers or reply length.

**Rows that stay, and why:**
- "Use spare quota to deepen the Brain" (`enrich_brain`, Advanced) is a spending choice, not a quality knob. C5
  measures it.
- "Show the Full access notice" is display only.

## 2. How each phase is judged

### 2.1 The tests (from `PLAN.md` §0)

- **V1, completed-task A/B:**
  - frozen: request, base, hidden checks, Brain seed, models, effort, CLI versions and machine load;
  - cold and warm regimes, declared and never pooled, with arm order alternating;
  - every attempt kept, with completion rate first;
  - usage through settlement (thread, workers, reviews, fix tasks, checks, Brain jobs, note forks);
  - n ≥ 3 per arm per regime. A result inside the spread is "no difference".
- **V2, continuity battery:** several requests in one session, with at least two forced seams of the kind under test.
  - Planted: a constraint with its exception; a decision with its reason; a hash, a path:line and a number with
    units; a failed check with its error line; a ruled-out approach; a thread-only item; a false `learned` line.
  - Each item passes or fails on its own. The target is all pass.
- **V3, small-ask set:** 12 frozen asks (4 code questions, 4 tiny edits, 2 status, 2 "explain this error"), cold and
  warm, with a seeded and with an empty Brain.
  - Measured: model calls, worker starts, time, exact correctness, usage through settlement.
- **R, rollout to open sessions:** §5.

### 2.2 The benchmark (branch `bench`, read-only here)

`bench/` runs Brigadier against plain Claude Code (`claude -p`) and plain Codex (`codex exec`) on a TypeScript fixture
with hidden tests and two blind judges (`docs/evidence/benchmark-2026-10-14.md` on `bench`). Models: Opus 5.5 and
gpt-6.1-sol, both at high.

| Task | What it measures | Claude Code median, n = 3: wall s / Claude raw (no cache reads) | Codex median, n = 3: wall s / Codex raw (no cached) | Brigadier `launch`, n = 1: wall s / Claude raw (no cache reads) + Codex review |
|---|---|---|---|---|
| T1 | latent bug from a failing test | 58 / 131k (18k) | 66 / 118k (24k) | 57 / 341k (21k) + 89k |
| T2 | multi-file feature from a spec | 185 / 829k (75k) | 254 / 346k (50k) | 264 / 1.41M (110k) + 157k |
| T3 | cross-module type refactor | 672 / 2.26M (142k) | 438 / 502k (74k) | 492 / 2.88M (167k) + 195k |
| T4 | four parallel modules | 70 / 234k (32k) | 161 / 199k (28k) | 115 / 671k (40k) + 109k |
| T5 | sync→async migration | 280 / 818k (83k) | 334 / 381k (54k) | not run |
| T6 | performance, no output change | 97 / 250k (31k) | 187 / 176k (31k) | not run |
| T7 / T7L | 8-feature backlog (n = 2) / 24-feature long task | T7 in the doc; T7L frozen, not run | same | not run |

- Every plain run and all four `launch` runs passed.
- The benchmark is on hold and shared with another session. Each phase books its runs and uses its own daemon and data
  directory, never the installed app's.

**Pass conditions**, used by every gate (§4):
- **Runs:** n ≥ 3 per arm per regime, cold and warm reported apart, arm order alternating. Plain-arm runs are reused
  only if CLI versions, models and machine are unchanged.
- **Quality:** Brigadier passes the hidden tests in every run of every row it runs. Its median of high judge findings
  is no higher than the better plain arm's. A failed run counts as not completed, and its repairs count in its usage.
- **Time:** medians against the phase's time targets, with the cache regime stated.
- **Tokens: the token gates (Q44, confirmed by the user).** These are the only token targets in this plan:
  - **the measure:** Brigadier's raw tokens through settlement, all vendors summed (the thread, workers, the other
    vendor's review, fix tasks, checks and Brain jobs during the run), against plain Claude Code's raw tokens on the
    same bench task. It is a ratio of medians, per task and per regime, cold and warm both. Claude raw, raw without
    cache reads and the other vendor's share are also shown apart;
  - **only at equal hidden-test quality:** a ratio counts only when Brigadier passes the hidden tests in every run
    of that task, as plain Claude Code does. For T7L, which scores per feature, equal quality means Brigadier's
    median of features passed is at least plain Claude Code's;
  - **after phase B:** every small task (T1–T6) ≤ 1.5×;
  - **after phase C:** every small task ≤ 1.15×, and the long task T7L ≤ 1.0×;
  - **after C, every later phase** (D1, E, D2) must still meet C's gates on the rows it runs;
  - **a phase that misses its gate doesn't ship:** nothing is pushed or installed, and the phase goes back.
  - **a review by the Q48 line (Q52, the user, after phase C's gate bench):** on a task where the thread's own
    change gets its review by the Q48 line or a risky mark, the token gate compares Brigadier's run without that
    review: the other vendor's review tokens and the thread's turn that handles its findings are left out. Their
    cost is reported apart, next to the gate number, as is the full total. Reviews stay on at the Q48 line.
- **"No loss"** (for changes that must cost nothing, such as Q40): the median is inside the previous Brigadier build's
  min–max or better, on every row, in both regimes.

## 3. Phases, in build order

| Order | Phase | Why here |
|---|---|---|
| 1 | **A: What hurt the user**, opening with the V3 and V1 harness (A0) and ending with the memory safeguards (A11) | Confirmed in real sessions, small, and what the user sees. A2 and A6 are judged on V3 cases, so V3 comes first. A11 closes Q35's "no saved facts on its own" and must exist before B and C lean harder on the Brain |
| 2 | **B: The thread is the worker**, opening with P0.1 (B0) and including the fork cache fix (B5) | The new model itself. B5's proof is one `handoff` row tagged by P0.1. B5 is small, measured, and paid on every idle long thread |
| 3 | **C: Fewest tokens** | Measured against B's call pattern with P0.1. Effort by model (research B3) waits for V2's abstention score, so it moves to D1.7 |
| 4 | **D1: Never forgets, without the output store** | V2 is built first; constraints (D1.1) come before selective loading (D1.6); effort floors (D1.7) need V2 |
| 5 | **E: Never hallucinates** | Small and binding (Q38). Its in-turn checks need nothing from P0.3 |
| 6 | **D2: Never forgets, with the output store** | Workers come off compaction only after same-session resume (D1.3) and the output store (D2.1), or a forced hand-off passes clipped output (PLAN §4). The thread's in-turn overflow guard (D2.3b) uses the same store and seams |
| 6b | **O: Never more tokens than plain** | Q57: after D2 shipped, every task at or under plain Claude Code. Measured per call against plain first |
| 7 | **X: Experiments** | A10 only after P0.1, V2 and B5. An experiment, never a silent threshold change |

**Where the P0 measurement goes:** each fixture is built before its first consumer.
- **V3, V1's accounting and the per-request usage report** open A (A0). A2 and A6 are V3 cases, every phase after A
  is judged on V3 or V1, and B5's idle-gap run is a V1 run.
- **P0.1** (cache writes by lifetime, a missed-prefix estimate per call, cause tags) opens B (B0), because B5's proof
  is one `handoff` row and C's savers are claims about cache writes.
- **V2** opens D1 (D1.0), before its first consumers: D1.1–D1.4, then D1.7 (effort floors, which need V2's "unknown
  instead of a guess" score), E and D2.
- **What waits for D2.1:** V2 items planted beyond the 8 KiB clip. D1.3 is accepted on items inside the clip with the
  loss labelled. Its lossless gate is rerun in D2's validation.

---

### Phase A: What hurt the user

Scope: the confirmed failures from SESSION-FORENSICS, Q36, Q37 and Q41.6, then the memory safeguards.

**A0. The V3 and V1 harness first** (P0.2's V3 and V1 parts).
- **V3 as fixtures** in `tools/ab`, with scoring scripts. The 12 asks run cold and warm, each with a seeded and with an
  empty Brain. A's deliverable cases (A1, A2, A6) are added to the set as extra frozen cases.
- **V1's accounting in the same harness:** cold and warm regimes, alternating order, completion rate, and usage
  through settlement.
- **A per-request usage report** in the Inspector, through settlement, with thread turns and model calls per request.
- **Done when:** three reruns of each fixture on an unchanged build agree per ask, and a request report sums to
  `turn_usage`.

**A1. Deliverables survive (F1, U1, U2).**
- **Links:** before disposal, the core maps each scratch or `outputs/` path to its stored output (`report_files`
  `M/outputs.rs:201`; `finish_read_task` `M/workers.rs:4660` → `dispose_task` `:4683`). Links to those paths are
  rewritten in worker text, addenda and thread messages.
- **File chips:** the chip (`D/components/assistant-ui/markdown-text.tsx:129-158`) opens and saves the stored copy
  through `save_artifact` and `artifact_copy` (`M/outputs.rs:668`, `:681`) instead of `revealPath`. A chip whose file
  is gone says "This file is no longer here" and offers the stored copy's Save, never an error toast.
- **Addenda:** an addendum whose task's scratch is gone says so in its envelope.
- **Rules (R):**
  - thread (`M/prompts.rs:126`): "a worker's outputs open and save from its task card; never give a scratch path;
    never point the user at a file you haven't just seen exist";
  - worker (`M/prompts.rs:1283`): "never link your scratch path; name the output file".
- **Done when:**
  - flow test: a read task writes `outputs/x.md`, links it, reports and is disposed; its link in the worker's text,
    the addendum and the thread's answer each opens the stored copy;
  - render test: a dead path shows the notice and Save.

**A2. Deliverables have a home (U4).**
- **Rule (R):** a file the thread hands over goes through the task card's Save, a "Save…" chip under the final answer
  for each stored output, or a question about where it goes. Nothing is written to the Desktop or the clipboard
  unless the user asked.
- **Done when:** a V3 case ("where's my file?" after a read task) gets the card's Save, with no `pbcopy` and no write
  outside the workspace in the thread's commands.

**A3. The workspace is never removed on its own (Q36).**
- **Merge keeps the workspace:** `finish_session` (`M/landing.rs:1481`) no longer calls `release_merged_session`
  (`M/lifecycle.rs:687`). The worktree and branch stay, and the branch is fast-forwarded to the merge commit. The next
  change continues from what is on main, with build caches kept.
- **"Close workspace":** an action in the branch row's ⋯ menu (`D/app/conversation/GitActions.tsx:61`,
  `PinnedSummary.tsx:759`). It runs the archive path's `wind_down` (`M/lifecycle.rs:607`, with `keep_session_changes`
  first) for this session only, after a confirm that names what goes.
- **Retired: every merge-driven removal path, not only the first one.**
  - `release_merged_session`'s merge caller in `finish_session`, and `forget_session_worktree`'s merge use
    (`M/lifecycle.rs:751`).
  - The retry in `ensure_target` (`M/workers.rs:2502-2517`). It is not a broken-worktree retry: when a merged
    session's `session:` owner is still being disposed (a post-merge removal that failed), it finishes that disposal
    and then calls `forget_session_worktree`. Under Q36 that would delete an open session's workspace, so it goes.
  - The rule in `M/prompts.rs:303` changes (R).
- **Reconcile old post-merge disposals at upgrade, before the startup sweep.**
  - The launch sweep (`crates/core/src/ledger.rs:604-640`) finishes every disposal the previous daemon had started,
    before the session manager recovers anything.
  - A one-time reconcile runs first. Any `session:` owner still marked disposing whose conversation is not archived
    or deleted has its disposal cancelled: it was a merge's removal. The worktree stays, and the session's setup path
    is restored if the worktree still exists.
  - Owners disposed by archive, delete, project removal or uninstall (`wind_down`, `M/lifecycle.rs:607`) keep
    today's cleanup.
- **Task worktrees** still go when their task ends, with outputs and unfinished work kept: they are Brigadier's own
  scratch, not the user's workspace.
- **Done when:**
  - flow test: after a merge, the worktree and branch remain, the branch is at the merge, and the next request works
    in the same path;
  - flow test: Close workspace removes it only after the confirm;
  - archive and delete behave as today;
  - upgrade test: a store with a failed post-merge disposal (`session:` owner disposing, worktree on disk,
    conversation live) is opened by the new daemon. The startup sweep, `ensure_target` and the next request all leave
    the worktree in place, and the session works in it;
  - the same test with an archived conversation still removes it.

**A4. Merge only when the user asks (Q37), with no cards.**
- **Remove the card:** delete `propose_merge` (`M/landing.rs:1299`, `merge_offer` `:2866`, catalog
  `crates/mcp-server/src/catalog.rs:187-202`) and stop creating `QuestionKind::Merge`. The merge lines in
  `M/prompts.rs:118`, `:136` ("ask the merge again") and `:303` become one rule (R): "When the work is ready to merge
  you may say so once, in plain text. Merge only on the side panel's button or the user's own words."
- **Add the button:** "Merge into main" (the session's base) on the branch row (`PinnedSummary.tsx:759`), shown only
  for a worktree session with commits ahead.
  - It calls a new IPC request, `mergeSession`, which records a `MergeRequested` event with its origin (click,
    client name, pid).
  - The thread gets "[merge] The user clicked Merge into main; call finish_session". A merge waits behind running
    writers, as held merges do now.
- **Retire card consent:** `fresh_consent` (`M/landing.rs:2137-2210`) stops reading `board.questions`. Consent is only
  an unused `MergeRequested` newer than the user's latest message, or `user_words` quoted from that message, as today.
- **Reconcile at upgrade:**
  - open merge cards are withdrawn (`withdraw_question`, `M/cards.rs:638`), with one line to the thread;
  - answered cards stay but are inert;
  - a held merge whose consent came from a card answer is cancelled, and the thread is told;
  - a held merge whose consent came from the user's words stays.
- **Done when:**
  - flow tests: a stored "Merge into main" answer, replayed at a restart or answered in a session created before the
    change and resumed after it, never authorises `finish_session`; a click does, once; the user's words do, as today;
  - the catalog has no `propose_merge`;
  - render test: the button shows only when there is something to merge.

**A5. The merge-card mystery: traced read-only on 2026-10-15, then fixed.**
- **What the record shows** (session `01a1366e…33fbb16e`, a /tmp copy of `brigadier.db`, deleted after):
  - **11:03:54:** the card was created (seq 160880), with options "Merge into main" and "Not yet" and
    `recommended: null`.
  - **11:05:46 to 11:20:25:** no stream wrote anything.
  - **11:20:25:** seq 160913 recorded `answer: "Merge into main"`, `answeredAtMs` 1791966025709. No user message came
    before or after it.
  - **Then:** the engine sent "[answer] The user chose to merge … Call finish_session now" (`M/cards.rs:585`), the
    thread merged 25 commits (`a182860a`), and the worktree went at 11:20:40.
- **Every path that can answer a card** (checked at `c9a73e38`; the same at `d669bca0`):
  - **IPC `answerQuestion`** (`crates/daemon/src/server.rs:1755` → `answer_question`, `M/cards.rs:413`) is the only
    code that records an `answer` on a question card. Its callers are the desktop app (`ActionCards.tsx:917` via
    `D/state/actions.ts:885`) and the A/B recorder (`tools/ab/recorder.py:63`), which runs only on its own daemon.
  - **No daemon-internal path answers one:**
    - `withdraw_question` (`M/cards.rs:638`) closes a card with no answer;
    - overnight, Approve for me, timeouts, steering, workers and MCP tools have no route to `answer_question`;
    - nothing replays answers at restart;
    - worker question timeouts concern worker questions, not cards.
  - **The 10-14 daemon log** shows one IPC client throughout: `connection 1`, client "Brigadier", the desktop app. It
    was connected from 10-13 17:36 UTC until the quit at 08:52 UTC, and the answer came at 08:20:25 UTC.
- **So the answer came from the desktop app's window, and most likely from the card's keys:**
  - the card takes focus when it appears (`D/app/conversation/ActionCards.tsx:709`);
  - with no recommendation, its lit choice is the first, "Merge into main" (`initial = item.recommended ?? 0`,
    `:963`);
  - a window-level capture listener answers the lit choice on a plain **Enter anywhere in the view** outside a text
    field or button (`useCardKeys`, `:145-175`);
  - **"1"** on the focused card also picks it (`:752`).

  So one Enter after clicking into the thread or side panel, 17 minutes later, merges. The record keeps no origin, so
  a click can't be ruled out, and a computer-use agent's keystroke in the installed app would look the same. No
  engine path answered it on its own.
- **Fix:** A4, plus three changes.
  1. **No consequential default:** a card with a choice that is hard to undo has no lit choice until the user moves to
     one, even when it takes focus on its own. Enter does nothing until then.
  2. **No window-wide Enter for those cards:** only Enter on the focused card after a deliberate selection, or a
     click. Digits need the card focused.
  3. **Every answer records its origin** (click, enter, digit or typed, plus the client), so the next question like
     this is answered from the data.
- **The cards this covers:**
  - `ChoiceCard` (`ActionCards.tsx:646`) whenever one of its choices is consequential: merge cards that remain
    (overnight's run card, `PinnedSummary.tsx:883`) and "Implement this plan?";
  - the plan rail's direct approval, `PhasePlanAction` (`ActionCards.tsx:1100`), which today approves a plan on a
    window-wide Enter with no card focused at all;
  - the approval card's auto-focused Allow (`ActionCards.tsx:422-456`), which gets the same rule: Enter allows only
    after the user has focused Allow.
- **Done when:**
  - render test, the dangerous case first: a consequential `ChoiceCard` mounts and takes focus on its own. It shows no
    selection, and Enter right after the mount answers nothing;
  - render tests: a deliberate selection (arrow key or click on a choice) followed by Enter submits that choice; a
    click on a choice submits it;
  - render test: `PhasePlanAction` ignores Enter anywhere in the window and approves only on its own focused control
    or a click;
  - render tests: Enter in the thread body, the side panel or the titlebar with a consequential card open answers
    nothing; "1" with the card unfocused answers nothing;
  - unit test: the answer event stores its origin;
  - A4's flow tests pass.

**A6. The thread fixes its own known defects (F2, U3). R.**
- **Rule** (`M/prompts.rs:135`): "'To check' is only for what nobody here can check or fix. A known defect in a
  deliverable you fix yourself, or send back, before you answer."
- **The same edit says** the thread's workspace and outputs are writable at its permission level.
- **Done when:** a V3 case with a planted factual error in a scout's deliverable gets an answer with no "To check" for
  it, and the delivered file is right.

**A7. A stopped worker doesn't show "done" (U5).**
- `D/app/conversation/WorkerSummary.tsx:112-114` and `workerPresentation.ts` count "done" only for work that landed
  or was used. A task stopped without landing shows "stopped".
- **Done when:** a render test shows "1 done · 1 stopped".

**A8. No spare worktrees (Q35, U6, S5).**
- **Remove the pre-warms:** the one on each user message (`M/conversation.rs:1121`) and the refill after a delegation
  (`M/workers.rs:1498`). A worker's worktree is made when `delegate_task` starts. The dependency and build-cache copy
  (`M/warm.rs`, about 0.8 s in the 10-14 log) still runs then.
- **Clear old leaks:** at launch, sweep `task-next-*` ledger owners with no task (`crates/core/src/ledger.rs:604`
  skips them today).
- **Done when:**
  - flow test: a question-only request, a self-edit request and a quit leave no `task-next-*` worktree;
  - the launch sweep removes a planted orphan;
  - B's bench rows report the first worker's start time, so the cost of no pre-warm is measured.

**A9. The timer shows work and waiting-on-you apart (Q41.6, U7).**
- `UserRequest` gains `waited` intervals, open while one of the request's question cards is open
  (`M/requests.rs:660-680`).
- `D/app/conversation/RequestBlock.tsx:177` shows "Worked 11m · waited for you 18m", or work time alone if there was
  no wait.
- The test at `M/requests.rs:773` flips: 10 s of work, 30 s on the card and 5 s more is 15 s worked and 30 s waited.
- **Done when:** that test and a render test pass, and THREAD-PARITY-PLAN is updated.

**A10. Small honest-UI fixes (F5).**
- `~/` is expanded before `absolute()` joins a path (`M/reads.rs:358`).
- One read of the weekly-quota projection that showed "459%" at 23% used. Fix it only if it overreacts to one busy
  hour, with a unit test.

**A11. Memory safeguards (research A13, the minimum of A4, and a migration; Q38).**
- **The thread sees `learned`:** the report envelope lists it like the other lists, and `report_text` includes it (the
  envelope code around `M/prompts.rs:1684`, `:1771`).
- **Worker items become claims:** `learn_report` (`M/brains.rs:1061`; callers `M/workers.rs:3683`, `:3910`, `:3933`,
  `M/landing.rs:790`) stores worker `learned` and `decisions` as claims, with their source (task, report version) and
  the evidence state `unverified`.
- **Rebirth-note decisions:** these lines (`M/brains.rs:1385`) become claims, sourced by session and generation,
  unless they cite a user decision.
- **User decisions stay decisions:** `learn_user_decision` (`:1303`) covers the user's words, card answers and outline
  go-aheads.
- **The thread's own `remember` writes go through the same rules.** Today `remember_tool` (`M/brains.rs:792-850`)
  writes Decision, Convention and Contract nodes directly, with orchestrator provenance and no evidence. After A11:
  - **a user decision** (or a personal preference) needs a `user_message` reference. The engine checks that the
    message exists in this conversation, is the user's, and contains any quoted span given. With a valid reference
    the node is a user decision. Without one it is refused for that kind, with the reason;
  - **anything else the thread records** is a thread claim, `unverified`, with the session as its source;
  - **evidence it names** (`files` with line ranges, a check id) is kept for E1, which decides promotion.
- **Wrong claims:** the thread can replace a claim it finds wrong (`remember` with `replaces`, which exists today).
- **Labels:** `brain_lines` (`M/context_pack.rs:170`) and `query_brain` print each line's source and state ("user
  decision", "worker claim, unverified", "enrichment, unverified") and put verified facts first.
- **Migration** (one store migration, run once at upgrade):
  - existing Convention and Decision nodes of origin `Report`, and orchestrator decisions with no user source, become
    unverified claims with their provenance kept;
  - nothing is deleted;
  - enrichment and skeleton nodes are labelled "unverified";
  - user-origin decisions are untouched.
- **Rebirth briefing** (`M/rebirth.rs:1172-1223` @d669bca0): decision bodies stay whole (Q5, Q41.1), each with its
  label. A claim is never written as a settled decision.
- **Done when:**
  - migration test: a store with report, orchestrator and user nodes comes out with the right states and provenance;
  - flow test: a false `learned` line that passed checks and a clean review reaches a later worker's brief, a
    `query_brain` answer and a forced rebirth briefing labelled "worker claim, unverified", never as a decision;
  - flow test: an unsupported claim saved through `remember` (a "convention" with no user message and no evidence)
    reaches a worker's brief and a forced rebirth briefing labelled "thread claim, unverified";
  - flow test: a `remember` decision citing a real user message is a user decision; one citing a message that isn't
    the user's, or a span not in it, is refused;
  - R.

**Validation:**
- the flow and render tests above;
- V3's deliverable cases (A1, A2, A6);
- bench T1 and T2, n = 3 warm: "no loss" against `launch`, because A changes prompts. No token target.

**Rollout (R):**
- one revision note covers A1, A2 and A6, and one covers A4 with the tool removal;
- A11 changes envelopes and labels only;
- A4's tool removal reaches open sessions at their next resume or rebirth (§5).

**Risk:**
- an open session may still call `propose_merge`. It must get a refusal that says what to do, not an error;
- A11 makes more lines read "unverified". V2 later measures whether briefs stay readable;
- A3 keeps worktrees longer, so disk use grows. Settings › Storage lists them.

---

### Phase B: The thread is the worker

Scope: Q35 and Q39 in the engine and the prompts, the measurement B and C need, and the fork cache fix.

**B0. P0.1 first** (V3, V1's accounting and the request report already exist from A0).
- **P0.1:**
  - parse `cache_creation.ephemeral_1h_input_tokens` and `ephemeral_5m_input_tokens` (`P/claude/parse.rs`);
  - estimate the missed prefix per call (previous prompt − cache read, labelled an estimate);
  - tag known causes: fresh start, resume, cold resume, rebirth, note fork, hand-off, reroute, effort change,
    tool-set change, CLI upgrade;
  - keep Codex's cached share per turn.
- **Done when:**
  - the Inspector's request report shows writes by lifetime and by cause;
  - the C11 fork row of 10-14, replayed from a fixture, shows about 252k missed with cause "note fork".

**B1. The thread does normal work itself (Q35). R.**
- **`M/prompts.rs:107` is rewritten.** The thread reads, edits, runs and commits normal work itself, at any size it
  can hold as one coherent change.
- **Workers only for:**
  - parts that can run at once;
  - very long jobs (hours, or more than the thread's context should carry);
  - work needing tools it lacks (app runs with computer use, visual checks);
  - the user's request.
- **Before workers start:** with the toggle on, one line first; with it off, `ask_user` first (§1.4).
- **Outlines and leads only for big jobs:** "A lead of multi-step or risky work sends an outline" becomes "for a big
  job: one plan, reviewed by the other vendor before building".
- **Checks:** the thread runs its own change's checks with `run_check`. The session's full checks run once per
  request, after its last commit or landing, on the merged tip, not after each commit (the queue in
  `M/integrated_checks.rs`). Failures come back as today.
- **Done when:**
  - V3: 0 worker starts on all 12 asks;
  - flow test: a 3-file feature with no parallel part is done with no `delegate_task`;
  - flow test: a T4-shaped job starts workers after one announcement line, and asks first with the toggle off;
  - flow test: full checks run once per request.

**B2. Reviews scaled to risk (Q39). R.**
- **Tiny** (an answer, or an edit the thread's own checks fully cover): no review.
- **Bigger or risky:** one other-vendor review in the background (`M/review_runs.rs`).
  - **Measured per change, not per commit.** The engine tracks the request's **unreviewed diff**: every thread commit
    since the last commit a review covered, taken together. When that accumulated range crosses the size line (150
    changed lines of code or more than 5 files of code, a cheap engine rule; the user, Q48), one review of the whole
    range starts. Tests and docs don't count (files in test folders, `*_tests.rs`, `*.test.ts(x)`, `*.md`, `docs/`).
    Phase C re-tunes the line from V1.
  - **Before the final answer** the engine checks again. If the request's unreviewed diff is over the line, or holds
    a commit the thread marked risky (persistence, security, migrations, concurrency, deletion, public API), a review
    of that range starts. "Done" waits for it.
  - **Every worker landing** gets its review, as today.
  - **Judging:** findings are judged against the code and the request. Valid ones are fixed before the final answer,
    so the request isn't "done" while a review of its work is unsettled; this replaces `launch`'s "the review comes
    after". Out-of-scope findings are dismissed with their reason. The size line is tuned from V1, not guessed.
- **Big jobs:** `review_plan` before building and `start_verifier` at the end, with evidence per "done when" point.
- **"Review the work" off:** no other-vendor review and nothing waits. The thread's own checks stay, and the answer
  says no review ran (Q43).
- **Done when:**
  - flow tests per tier: which review starts, and when "done" may be written;
  - flow test: four commits of 10 lines each in 3 files, each under the line, get one review of their combined range
    before "done";
  - flow test: one tiny commit gets no review;
  - the off switch skips all of them;
  - T2's final answer lists what the review found and what was fixed.

**B3. Small asks answered directly, with file:line (research B1). R.**
- **Answers:** the thread answers code questions from the Brain, `code_search`, `code_refs` and a few precise reads,
  quoting the file:line it relied on.
- **Brain miss:** the hint that suggests a scout (`M/brains.rs:779` @d669bca0) becomes "read the code yourself".
- **Done when:** V3's 4 code questions and 2 error explanations are exactly right, with file:line and no worker.

**B4. The two toggles (§1.4).**
- **`start_workers_on_own`:** default on, read by B1's rule, and reaching open sessions through the `[settings]` note
  (`M/prompts.rs:908`).
- **"Short replies":** removed, as §1.4 says.
- **Done when:** model and render tests pass, and an open session told `short_replies: false` gets one note.

**B5. The note fork shares the thread's prefix (research A11).**
- **Claude, which is where the cost was measured** (C11; Codex threads aren't checkpointed, N5):
  - **Same spec as the thread:** the checkpoint and hand-off fork (`M/rebirth.rs:337-366`, today `ToolSet::None` and
    no MCP servers) launches from a clone of the thread's live `SessionSpec`, with the same tools, MCP servers,
    model, effort and settings. Only origin, ephemerality, recording and folder ownership change.
  - **It keeps `Access::ReadOnly`** (`M/rebirth.rs:344`), as today, and its prompt keeps "don't call any tool".
  - **Every call is refused before it runs:** a `PreToolUse` command hook in the fork's `--settings` denies every
    tool, native and MCP. Hooks are client-side, so the prefix stays the same. Brigadier's MCP server also refuses
    calls from a fork owner. Tools are not removed: `--disallowedTools` would change the prefix again.
- **Codex: unchanged until its probe passes.**
  - Today a Codex fork has `ToolSet::None`, which only turns off web search: Codex's own shell and patch tools stay,
    under `Access::ReadOnly` (`P/codex/mod.rs:1187-1194` @d669bca0).
  - Codex hooks are off for every Codex session (`DISABLED_FEATURES`, `P/codex/mod.rs:116-123`). Codex's hook coverage
    has documented exceptions, so a hook alone can't promise "no tool runs" there.
  - A Codex fork changes only after three things pass: the Codex hooks-isolation probe (PLAN §5); tests that its
    native shell and patch calls are denied, beyond the read-only sandbox; and a measurement that it shares the
    thread's prefix.
- **No false refresh:** the `refreshed` extension (`M/cold.rs:164-172`) goes, for both providers.
- **Done when:**
  - one Claude `handoff` row reads about the thread's context and writes a few thousand, with the deny hook in place;
  - a Claude test fork asked to call `Bash`, `Edit` and an MCP tool is refused each time, with nothing changed on
    disk;
  - a test confirms the fork's spec still has `Access::ReadOnly`;
  - the Codex fork's spec is unchanged unless its probe and denial tests are recorded as passing;
  - V1 with an idle gap over 50 minutes, through settlement.

**Validation:**
- **V3** (cold and warm, seeded and empty): 12/12 exactly correct, 0 worker starts, model calls per ask ≤ plain Claude
  Code's + 1, time ≤ 1.25× its median.
- **Bench T1–T6** (§2.2, n = 3, cold and warm):
  - every run passes, with high findings ≤ the better plain arm;
  - **token gate (Q44): every small task ≤ 1.5× plain Claude Code** (T1 is about 3.3× on `launch`, all vendors
    summed: 341k Claude + 89k Codex against 131k);
  - wall time ≤ 1.25× plain Claude Code's median;
  - T4: wall time ≤ plain Claude Code's.

**Rollout (R):** B1–B3 are one revision note, and B4 a settings note. A session created before B must take the new
rule on resume and then do a V3 tiny edit itself.

**Risk:**
- the thread's context grows faster, so it is reborn more often. D1 follows C for that reason, and V2 runs on B's call
  pattern;
- waiting for reviews slows bigger requests. This is measured on T2 and T3;
- B5's fork holds real tools, so its deny hook and test are part of the item.

---

### Phase C: Fewest tokens

Scope: the per-call fixed cost, thread wakes and background spend, measured with P0.1, V3 and V1.

**C1. A smaller fixed start (research B8; R for (b)).**
- **(a)** `delegate_task`'s 6 kB schema trimmed to one line per field, with rare-field detail moved into the brief
  rules that use it;
- **(b)** the peers, previews, mockups and overnight sections sent as `[settings]`-style notes when they first apply;
- **(c)** tool descriptions that repeat the instructions shortened (`crates/mcp-server/src/catalog.rs`, 19 kB of
  descriptions).

- **(d), added in phase C (approved):** the rules only a worker job, a follow-up or another session needs leave the
  instructions. Where each goes:
  - **`delegate_task`'s description** (kept under Claude Code's 4,096-character cut while it loads up front):
    batches and dependent parts, efforts, the self-contained brief with `include_request`, no /tmp paths in a spec,
    task kinds with operate, the live look and the mockup flow, `subject`, and "merge it when done" while workers
    write. Its fields keep one line each;
  - **a note with the first worker message** (report, question, outline or landing) in each CLI session: answering
    and steering workers, judging reports, landing at once, approving an outline, worker reviews, findings checked in
    the merged code, fix tasks, and not restating reports;
  - **the follow-up block and another session's message themselves:** both may come into a running turn, where no
    note goes, so each carries the few rules it needs;
  - **their tools:** previews and their sandbox rules in `start_preview`, a failed CI's fix in `ci_status`, when to
    write to another session in `message_session`.

  The instructions keep a one-line pointer to each. A session on instructions from before has the rules already: its
  revision note marks them told.
- **(e), added in phase C (approved):** a passing `run_check` whose output is over 1 kB replies with its warning lines
  and its last lines, the whole output one `read_artifact` away. A failure comes back as before.

Target: 3–5k fewer tokens per thread call.
- **Done when:**
  - first-call context is measured before and after (P0.1);
  - a flow test per moved section shows its note arrives before first use;
  - V3 counts invalid tool calls, with no rise.

**C2. Lazy tool groups, behind the benchmark (Q40).**
- **Always loaded,** about 10 everyday tools: `run_check`, `delegate_task`, `message_worker`, `answer_worker`,
  `ask_user`, `note_for_user`, `query_brain`, `remember`, `code_search`/`code_refs`, `finish_session`,
  `read_report`/`read_artifact`.
- **On demand,** behind Claude Code's tool search: browser, computer use, overnight, CI, previews, pages, peers,
  accounts, planning beyond `ask_user`, `record_findings`, `suggest_full_access`. That saves 6–7k tokens per call.
- **First a probe:** is the cached prefix stable with deferred schemas, and how often do V3 and V1 runs pay the search
  hop? The tool set stays fixed at session start.
- **Ship only if** the bench shows "no loss" in quality and speed against C1's build (§2.2), in both regimes.
  Otherwise the change stays unmerged and `533bac83` stands.
- **Done when:** the probe is recorded, the gate passes, and R holds (the tool set changes only at a resume or a
  rebirth).

**C3. Batched wake-ups (research A14).**
- Envelopes that need nothing now (a landing done, checks passed, a clean review, a status) wait a short window. They
  go together in one turn (`take_one_request`, `M/conversation.rs:6215`) when the window ends, or earlier if something
  urgent arrives: a question, a failure, a report or a user message. Nothing is dropped.
- **As built:** the window is 15 s, and it runs only while more of the envelope's request still runs (a task that
  works, or one whose envelope is on its way). With nothing else running, the news is what the answer waits for, so
  it wakes the thread at once and adds no wait to the answer. The quiet ones are a landing done, checks passed, a
  clean review of the thread's own changes and a status due (`Envelope::quiet`).
- **Done when:** V1 shows fewer thread turns per request at equal quality, and a flow test shows a question still
  wakes the thread at once.

**C4. Effort by model and step: moved to D1.7.** Its acceptance needs V2's "unknown instead of a guess" score, and V2
is built in D1.0.

**C5. Brain enrichment cost (S8).**
- **The spend:** about 2.98M Haiku cache writes in 14 days, against the thread's 0.50M.
- **Measure, then cache:** measure its share of each quota window with P0.1. Make its jobs reuse one cached start
  (fewer, longer jobs on the same prefix). Its nodes stay unverified (A11), so they never need to be right to be safe.
- **If the weekly window pays more than spare five-hour quota,** report the numbers to the user: when it runs is their
  setting.
- **Done when:** a week of P0.1 data names its share, and `turn_usage` shows fewer writes per job.
- **As built:**
  - **The measurement.** From a read-only copy of the user's `turn_usage` (14 days to 2026-10-14), 37 Claude Brain
    jobs wrote 2.98M and read 54M cached tokens. Each job is a fresh Haiku session of 6–92 calls, and every call read
    Claude Code's whole default start: every built-in tool, bundled skills, connectors and the Artifact tool.
  - **The change.** A job gets only `Read`, `Grep` and `Glob` besides its own tools, with the lean start's settings
    (`ToolSet::Reader`). Claude's per-session system prompt sections (`--exclude-dynamic-system-prompt-sections`)
    move into its first message. An enrichment job's role, place and index digest are its system prompt, so the
    jobs of a batch share one cached start, and only its targets are its message.
  - **The skeleton pass keeps the digest in its message.** With the digest in the system prompt it recorded 11 and
    9 nodes against 18 and 19, and a skeleton runs once per project, so there is no start to share.
  - **The result** on the V3 fixture, with a dev daemon on Haiku, 4 enrichment jobs before and 8 after:
    - writes per job 30.7k → 23.2k on average (−24%);
    - reads per call 31–36k → 14–25k;
    - cached reads per recorded node 55k → 27.5k;
    - the skeleton pass recorded 18 and 19 nodes before and 21 and 16 after.
  - **Rule R doesn't apply.** A Brain job is a one-shot session that ends with its turn, so no open session carries
    the old start.
  - **Follow-up issue:** the week of P0.1 data. It can't exist inside the phase.

**C6. Later in C, each with its probe first (PLAN §5):**
- B4, `--exclude-dynamic-system-prompt-sections` (cold and warm apart);
- B2, precise reads (the LSP tool, or `code_outline`/`read_symbol` with worktree freshness);
- B7, a verified scout brief.

Each ships only if V1 or V3 shows a gain at equal quality.

**As probed:**
- **B4.** On V3 e1 at `e4711fff`, the thread's first call already read 9,366 of its 17,224-token start from another
  session's cache (Claude Code's static part) and wrote 7,858.
  - The flag alone moves only Claude Code's few per-machine sections, since the thread's instructions open with the
    session's own lines (project, repository, workspace, branch).
  - Sharing the whole start needs those lines moved into the first message, under rule R. That would turn about 7.8k
    of writes into reads on a session's or a rebirth's first call, with raw tokens unchanged, so it would not move the
    gate.
  - Not built; filed as a follow-up with these numbers.
- **B2.** On the same run, every tool result the thread read came to 8.9 kB (about 2.2k tokens over 6 calls), most of
  it whole small files read with `cat`. Precise reads could save under 2k tokens an ask on V3 and the bench
  fixture, inside the spread. Not built.
- **B7.** The thread does the work itself since phase B: V3 started no scouts, so a verified scout brief has nothing
  to save there (0 worker starts in all 27 of phase B's V3 runs). Not built; revisit when V1 shows scouts again.

**C7. The session's own worktree starts warm (added in phase C, approved).** It gets the copy-on-write copies of the
checkout's dependency installs and build caches that a worker's worktree gets (`warm`), and the thread hears so in a
`[workspace]` note. Every Brigadier bench run at phase B's head lost two calls to a first check failing on a missing
`node_modules`, then `npm ci`, then the check again: about 70k tokens on T1.
- **Done when:** a flow test shows the thread's worktree holding the checkout's `node_modules`, with the note on its
  first turn only.

**C8. The review line, tuned from V1 (Q48).** V1 cases just under and just over the line (about 150 changed lines of
code, 5 code files), and a small change marked risky, measure each review's cost and whether its findings would have
been missed. The line is settled from those numbers, and the phase report says what was chosen and why.
- **As measured** (V3, four new frozen asks `c8*`, warm, seeded, n=2 each, at `240a7506`; all 8 runs pass their
  hidden checks):

  | Ask | Code changed | Review | Codex (raw) | Findings |
  |---|---|---|---|---|
  | `c8u` csv export | 61–71 lines, 3 files | none (under the line) | 0 | — |
  | `c8u-risky`, the same asked as risky | 61–72 lines, 3 files | marked risky by the thread | 75–95k | clean, twice |
  | `c8o` credit notes | 309–317 lines, 6 files | over the line | 99–131k | 1 valid P2 (refunded tax could sum past the line's, from rounding each credit), fixed with a test; then clean |
  | `c8r` data version | 20 lines, 1 file | marked risky by the thread, unasked (persistence) | 52–68k | 1 valid P2 (an explicit `null` version read as 1), fixed; then clean |

  - **What a review costs.** It adds one thread turn. With the same `c8u` change and a review, the request used +17%
    Claude and +85k Codex tokens at the median, about +38% in all, and the answer came 53 s later at the median.
  - **What reviews would have missed.** Neither finding was something the hidden checks test, so no review turned a
    failing check into a pass. Both were real bugs a user could hit.
  - **The line stays at 150 changed code lines or more than 5 code files, plus the risky mark.**
    - Under it, the forced reviews found nothing at about a third more tokens.
    - Over it, and on the risky persistence change the thread marked by itself, they found real bugs at one turn and
      50–130k Codex tokens.
  - **The gap.** The runs left a gap between 72 and 309 lines, so the line's exact place in that band is not measured.
    Bench T2 (141–146 code lines in 5 files) sits just under it. A follow-up adds cases at about 120 and 200 lines.

**C9. The thread commits in the same call as its last check (added in phase C, Q50).**
- **The user's answer (Q50).** The engine never commits for the thread: commits stay the thread's decision, with the
  user's. But the thread may save the call itself.
- **The call it saves.** On V3 e1 at `e4711fff` the thread's own `git commit` was the one call above plain (6 calls
  against 4–5), about 23k tokens. Every edit request in a session worktree pays it.
- **The field.** `run_check` takes `commit: "<message>"`. When the check passes, everything the thread changed is
  committed with that message and its trailer, in the same call, also when the check finishes in the background.
  When the check fails, nothing is committed. On the user's own checkout it commits nothing and says why, since
  M6's rule stands there.
- **What stays the thread's.** A commit that fixes review findings still goes through git with both trailers.
- **Where it runs (the ship review).** The `git add` and `git commit` run as the check ran, at the session's access
  and in its environment, since git runs the repository's hooks and filters. Nothing is committed when a file changed
  while the check ran (an edit made while it ran in the background, or a file the check wrote): the reply names the
  files, and the check run again commits.
- **Revision 35** (`COMMIT_ON_PASS_REVISION`) tells open sessions.
- **Done when:**
  - a flow test shows a pass committing and a failure committing nothing;
  - nothing is committed on the user's checkout;
  - a session from before hears the note once;
  - V3 e1–e4 keep their exact correctness with fewer calls. It ships only then.
- **As measured** (V3 e1–e4, warm, seeded, n=2 each, at `0de12840`):
  - all 8 runs are exactly correct;
  - every run committed through `run_check`'s `commit`, with no separate `git commit` call;
  - model calls went from 8–9 (phase B, `1da10ace`) to 5–7 (median 6), against plain Claude Code's 4–5;
  - tokens per ask went from 285–340k to 95–153k (median 121k), against plain's 71–97k;
  - no tool call was refused for its input.

**C10. What the gate bench found (added in phase C, after the gate bench at `8179f06a` missed).**
- **The bench** (n=2 warm against plain Claude Code): T1 1.30×, T2 0.81×, T3 1.37×, T4 1.57×, T5 1.21×, T6 0.93×,
  T7L 0.91×. Every small-task run passed. Triage: `workers/w37` of the run, `msgs/w37-triage.md`.
- **The drivers:**
  - T3 and T5 fail on the review over the Q48 line. The turn that handles its findings costs 340–660k Claude tokens
    (4–7 calls at about 95k of context each), plus 115–170k of Codex. Without it, T3 is 1.01× and T5 is 0.72×.
  - T1 and T4 fail on one run each, mostly on the model's own detours.
- **(a) The thread's own shell output comes back whole up to 30,000 bytes**, the CLI's own limit. Above that it gets
  the digest, as before (`digest::BASH_WHOLE_MAX`).
  - All 25 digested outputs in the bench were `cat`s of source or tests, and every one was read again in a call of
    its own (`Read`, another `cat` or `read_artifact`).
  - Q4's own reason not to trim file reads applies to them.
  - `run` and `run_check` keep the 8 KB line.
  - Revision 36 (`SHELL_WHOLE_REVISION`) tells open sessions.
- **(b) The review of the thread's own changes starts only on a passing check** (`unreviewed::ThreadCheck`).
  - No review starts while its `run_check` runs, in the call or in the background, the turn's end included.
  - No review starts on the files its newest check failed on.
  - The pass that follows starts the held review and says so in its reply, or in the message it sends from the
    background.
  - In both T3 runs the review had started on a failing first check, and one finding each time was the failing
    test that the check itself reported.
  - With no check at all, the review starts as before. After a restart it also starts as before, because the state
    is held in memory.
- **(c) A review's findings message says to look up `record_findings` in the same message as the first fix or
  check.** That lookup had taken a call of its own in 2 of 6 review turns.
- **(d) The thread sends its last edit and the `run_check` that checks it in the same message.** Plain Claude Code
  chains edit and check in one shell call, and the thread spent a call on each.
- (c) and (d) are revision 37 (`SAME_MESSAGE_REVISION`).
- **Done when:**
  - flow tests show a failing check starting no review and the pass after it starting one;
  - a background pass starts the review it held;
  - a turn ending on a pending check that then fails starts none;
  - a 20 kB shell output reaches the model whole;
  - a session from before hears revisions 36 and 37 once;
  - the re-bench at the new head meets C's gates as Q52 measures them.
- **The re-bench at `654ef7d9`** passed every gate but T4 at 1.19× (T1 0.66×, T2 0.67×, T3 0.63×, T5 0.51×, T6 1.05×,
  T7L 0.81×). Triage: `workers/w52` of the run.
  - T4's per-call cost matches plain's. In three of four runs the thread spent one more call (36–42k tokens) after
    `run_check` had committed, on `git status` and `git show --stat`, to confirm the commit and list its files.
  - T5's one high finding was the judges' error: the flagged test type-checks (`region` is a `string`), and the
    eval's `tsc` passed.
- **(e) `run_check`'s commit reply is the commit's record.** It names the commit's files and lines and, from a real
  `git status` after the commit (tracked and untracked, past `.gitignore`), "Nothing is left uncommitted." or the
  paths left. The thread is told not to check the commit again with git, and to try anything by hand before that
  last check. Revision 38 (`COMMIT_RECORD_REVISION`) tells open sessions.
  - **Done when:** a commit's reply names an ignored file nowhere and a change `git add -A` couldn't take by its
    path; a session from before hears revision 38 once; the T4 (n=4) and T5 (n=2) re-bench meets C's gates.

**Validation:**
- **Bench T1–T6 and T7L,** n = 3, cold and warm (T7L's plain-arm runs are made first, since it is frozen and not run
  yet):
  - every small-task run passes, and T7L passes at least as many features as plain Claude Code; high findings ≤ the
    better plain arm;
  - **token gates (Q44): every small task ≤ 1.15× plain Claude Code, and T7L ≤ 1.0×;**
  - wall time ≤ plain Claude Code's median on T1 and T4.
- **V3:** tokens per ask through settlement are reported against plain Claude Code's (no separate target).
- **C2's own "no loss" gate.**

**Risk:** a missing field doc leads to wrong calls (C1, counted); a deferred tool adds a hop (C2, gated); batching
delays non-urgent reactions by seconds (C3).

---

### Phase D1: Never forgets (without the output store)

**D1.0. V2 as fixtures** (P0.2's V2 half).
- **Seams:**
  - size seams (`BRIGADIER_REBIRTH_TOKENS`, `BRIGADIER_WORKER_HANDOFF_TOKENS`);
  - an injected quota or error reroute;
  - an account switch on the same model;
  - "prompt is too long";
  - a killed CLI;
  - a daemon restart at a chosen wait point.
- **Done when:** three reruns on an unchanged build agree per item.

**D1.1. Standing constraints, word for word (research A1). R.**
- **Recording:** a `constraint` memory kind, recorded by the thread with `@remember`. Each one points at the user's
  message (the whole message, or an exact span with its id), has a scope (request, session or project), and names
  what it supersedes. Supersession is never inferred.
- **Briefing:** every rebirth briefing and worker brief carries every active constraint in scope, verbatim.
- **The rebirth note** (`M/rebirth.rs:80-88` @d669bca0) gains: what was ruled out and why; failed checks with their
  error lines; exact identifiers in use.
- **Cuts:** every cut names the message id and range.
- **Done when:** V2's constraint and its exception survive two size rebirths and reach a worker brief verbatim, with
  the briefing's size reported.

**D1.2. A decision index (research A7, Q41.1).**
- A compact index (id, scope, status, supersedes) on top of whole decision bodies. No paging.
- **Done when:** V2 with 300 seeded decisions, 50 superseded: all bodies present, the index right, and the briefing
  size shown in the Inspector.

**D1.3. Same-session resume (research A12).**
- **Same model can continue** (usage or credits limit, account switch, sign-in error): resume the same CLI session
  instead of the thin successor (`M/fallback.rs:285`). Claude resumes by absolute transcript path from the other
  account's config folder; Codex by thread, if its probe allows.
- **"Prompt is too long":** goes to the size hand-off, labelled not lossless until D2.1.
- **Probes first** (PLAN §5).
- **Done when (interim, in D1):**
  - V2 with an injected limit and an account switch, on both providers: every planted item answered, since the
    session is resumed whole;
  - V2 with a context-window error: every item planted inside the 8 KiB clip is answered. Items beyond the clip may be
    lost, but the successor's brief says its transcript is clipped, and the successor answers "unknown" rather than
    guessing for any it lost;
  - no context-window error goes to a reroute.
- **Done when (lossless, rerun in D2's validation after D2.1):** the context-window seam answers every planted item,
  including those beyond the clip, on both providers.

**D1.4. Restart survival (research A3).**
- **Checkpoint:** the cold-cache checkpoint is persisted when written and reloaded at start (`M/cold.rs`).
- **Waiting workers:** Blocked and Paused workers are recovered by their wait reason (`M/lifecycle.rs:113-190`
  @d669bca0).
- **Landings:** an interrupted landing is reconciled against the target's ancestry.
- **Codex:** gets no guessed cache lifetime.
- **Done when:** V2 with a daemon kill at each wait point (question, outline, review, user pause, quota wait) loses no
  worker and no item.

**D1.5. Watchdogs (research A9).**
- **The thread:** first a watchdog on its own turn, for a CLI that accepted the turn and went silent.
- **Workers:** a shorter stall deadline that tells progress from chatter, with per-command deadlines
  (`M/watchdog.rs`), its minutes set by fault injection.
- **Done when:** a silent provider and a hung command are caught within the set time, with no false stop on a long
  legitimate build.

**D1.6. Load only what applies (research B5), after D1.1.**
- **Rules:** subtree repository rules load when files there are touched, with the left-out rules listed
  (`M/instructions.rs`).
- **Context packs:** carry the task's own reads (`M/reads.rs`, `M/context_pack.rs`).
- **Always pinned:** root and ancestor rules, and every active constraint.
- **Done when:** V1 on a monorepo sub-package task shows a smaller first call at equal completion, and V2's constraint
  item still passes.

**D1.7. Effort by model and step (research B3, moved from C4).**
- **Floors:** per model family, from measured data (`crates/router/src/decide.rs:190-218` @d669bca0). The thread names
  the effort for the job; the router applies the model's floor.
- **Switching cost:** Claude 5.5 can switch effort between steps for one ≈9.4k write (PROBES P2). Codex is assumed to
  miss its cache on each switch until its probe says otherwise.
- **Done when:** V1 per model and effort on frozen tasks, with completion rates. A lower effort is adopted only where
  completion holds and V2's "unknown instead of a guess" score doesn't drop.

**Validation:**
- V2: all items pass on both providers, except D1.3's beyond-clip items, which wait for D2;
- bench T7, n = 3: every feature plain Claude Code passes, Brigadier passes too. CLI compaction events are reported;
  the guarantee that none happen is D2.3's;
- C's token gates still hold (Q44): the small tasks ≤ 1.15× and T7L ≤ 1.0×, rerun on this phase's candidate.

**Risk:** a constraint wrongly superseded is a loss, so supersession is explicit only. D1.3 depends on the CLIs
accepting a copied transcript, so it is probed first.

### Phase E: Never hallucinates (after D1, before D2)

**E1. Facts need evidence (the rest of research A4, Q38).**
- **Promotion:** a claim becomes a fact only with evidence for that claim. Evidence is one of two things:
  - a cited file:line that still says so, re-read by the engine when the claim is used;
  - a check id whose output shows it.

  A review counts only through the evidence it points to: a finding that cites a file:line or a check output for this
  claim supplies that evidence, which is then verified like any other. A review is not an evidence route of its own.
  Passing checks or a clean review of the landing is not evidence for an unrelated claim.
- **Independent of the review switch:** promotion works the same with "Review the work" on or off. Q43's off switch
  stops other-vendor reviews only, not memory verification.
- **Contradiction:** a contradicting finding supersedes the claim.
- **Done when:**
  - V2's false `learned` line never reaches a brief as fact;
  - a true claim with a file:line is promoted after the re-read, and a true claim with a check id is promoted from
    that check's output, each with reviews on and with reviews off;
  - a review finding with no file:line or check output for the claim promotes nothing.

**E2. Overnight "verified" needs evidence per criterion (research A5). R.**
- **Overnight acceptance** (`M/overnight/run.rs:448-567` @d669bca0): each criterion is [met] only with a check id or an
  output reference, bound to the accepted tip. With reviews on, a settled review is also required. With reviews off,
  the morning report says no review ran.
- **Verifiers and reviewers** get the lead's whole report (`M/phases.rs:878-895` @d669bca0).
- **A timed-out worker question** proceeds only on a recorded safe default (`M/workers.rs:3408-3433` @d669bca0).
- **`run_check`** reruns integration, smoke and live commands unless they declare their inputs (G3).
- **`submit_report`** refuses a [met] with no evidence.
- **A Claude `Stop` command hook** refuses only a turn that ends with no report, or with a hand-off note missing its
  parts. It never refuses a rightly waiting turn, and each waiting case has a flow test.
- **Done when:** V1 on an overnight-style task with a criterion the suite doesn't cover, with reviews on and off, plus
  the waiting-case flow tests.

**E3. Final answers state what was checked (Q39). R.**
- **Contents:** `### Details` always lists the checks run with their results, the review state, and what wasn't
  checked.
- **Running code:** a claim that code compiles or works is made only after a check in this request.
- **Done when:** an audit of every V3 and bench final answer finds the three parts, and no unchecked "compiles" or
  "works".

**Validation:** V2's false-claim item; V1 overnight-style; bench T3 and T5, n = 3, every run passing, with high
findings ≤ the better plain arm, and C's token gates still holding on them (Q44).

**Risk:** more waiting overnight (E2), though a safe default counts as an answer; a hook loop, capped at 8 by the CLI.

### Phase D2: Never forgets (the output store and compaction)

**D2.1. Durable raw output (research P0.3).**
- **The guarantee:** every supported tool path stores its result **before** Brigadier's 8 KiB normalisation clip
  (`OUTPUT_CLIP`, `P/claude/parse.rs:18`, `P/codex/parse.rs:17`). The clipped event carries a durable reference to the
  stored copy, and the exported transcript carries those references.
- **Claude:** a `PostToolBatch` hook in `--settings` stores each tool result as the model saw it, by `tool_use_id`,
  for workers and the thread.
- **Codex, which has no hook to lean on:** Codex hooks stay off (`DISABLED_FEATURES`, `P/codex/mod.rs:116-123`), and
  Brigadier's `run` covers only what is run through it, not a worker's native commands. So the store sits in the
  parse layer, independent of hooks:
  - a command's `aggregated_output` is stored before `clip()` (`P/codex/parse.rs:418`);
  - an MCP tool call's result text is stored before `clip()` (`:482`);
  - a dynamic tool call's output is stored the same way;
  - each event carries the stored copy's reference.

  Whether the app-server's `aggregated_output` is itself complete or bounded by `tool_output_token_limit` is a probe
  (PLAN §5). Where it is bounded, the event is marked "the CLI gave an excerpt only", and no successor is told it has
  more. Codex hooks are added only if the hooks-isolation probe passes, and they are never the only store.
- **Export:** `worker_transcript()` is replaced by a streamed export that marks itself incomplete, with the missing
  ranges.
- **Done when:**
  - fault tests (page read error, decode error, interrupted export) each mark the export incomplete;
  - **preservation, per provider:** a worker command prints a planted marker 50 KiB into a 200 KiB output. On
    Claude: Bash and an MCP tool. On Codex: a native command and an MCP tool. The original CLI session files are then
    deleted, and the marker is still retrieved through the exported event's reference;
  - where Codex gives only an excerpt, the event says so, and the test records the bound.
- **Rollout block:** Codex's forced hand-off (D2.3) is not switched on until Codex passes the preservation test, or
  until its excerpt bound is labelled and V2's beyond-clip items for Codex pass from the stored copies.

**D2.2. Keep the CLI's own output files (research A8).**
- Before a hand-off, reroute, review-hold cleanup or disposal, copy the `tool-results/` files that Claude's transcript
  references into the blob store.
- **Done when:** a test deletes the original session folder and still retrieves every referenced file.

**D2.3. Workers off CLI compaction (research A6), after D1.3 and D2.1.**
- **Launch settings:** Claude workers launch with `auto_compact: false` (`M/workers.rs:1871`); Codex gets its limit
  set far above the window.
- **Guard:** the hand-off point sits below each provider's real ceiling.
- **Steer:** the wrap-up steer repeats after each further stretch of growth, and a hard line interrupts the turn and
  hands off.
- **Hook:** a `PreCompact` hook blocks proactive compaction and triggers the hand-off (Claude; Codex only through a
  passed hooks probe).
- **Done when:** oversized tool results on a 200k-window model give a hand-off and no compaction, on both providers.
  On Codex this waits for D2.1's preservation gate.

**D2.3b. The thread's in-turn overflow guard.**
- **The gap today:** on `main` a thread's context report only records usage (`M/conversation.rs:4264-4280`). Rebirth
  is considered only after the turn ends (`consider_rebirth`, `:4729`). A compaction is only logged as a contract
  breach (`:4281`).
- **Why it matters more now:** under Q35 the thread does the implementation work itself, so one long turn can cross
  Codex's compaction ceiling (Codex clamps its limit to about 90% of the window) or Claude's hard limit ("prompt is
  too long").
- **Guard:**
  - **a steer line** below each provider's ceiling: the first context report past it sends the thread a mid-turn
    steer to commit what it has and wrap up. It repeats after each further stretch of growth;
  - **a hard line** nearer the ceiling, safely below Codex's clamp: Brigadier interrupts the turn and runs the
    thread's rebirth at once, without waiting for the turn to end. The successor continues the request;
  - **"prompt is too long" on the thread** goes to that same rebirth, never to a failed request;
  - **on Claude,** the thread's `--settings` also carries the `PreCompact` hook, so any proactive compaction is
    blocked and turned into the rebirth.
- **Durable recovery:** the interruption is recorded before the CLI is stopped ("interrupted for rebirth at N tokens",
  with the request id). A daemon restart in between finishes the rebirth and resumes the request instead of losing it
  or replaying the turn. The successor's briefing names the interrupted step and the last commit, using D2.1's stored
  outputs and D1.1's ruled-out notes.
- **Done when:**
  - **threads and workers, both providers, on a 200k-window model:** an oversized tool result inside one turn gives a
    steer, then an interrupt and rebirth (thread) or hand-off (worker), and no `CompactionStarted`;
  - a "prompt is too long" injected on the thread gives a rebirth and a continued request;
  - a daemon kill between the interrupt and the rebirth recovers and finishes the request;
  - V2's planted items survive the in-turn seam.

**D2.4. Honest successor seams (research A2) and the thread's own work in transcript search (research A15).**
- **Successors** of a crash, missing CLI files or a cross-provider move get the streamed export, the note,
  `checks.md`, the decisions and stored-output references.
- **`search_transcript`** indexes the thread's own commands and reads.
- **Done when:** V2 with crash, restart and cross-provider seams passes, and the thread-only item is found after two
  rebirths.

**Validation:**
- V2, all items, on both providers, including D1.3's context-window seam with items beyond the clip (its lossless
  gate) and D2.3b's in-turn seam;
- bench T7L, n = 3: Brigadier passes as many features as the better plain arm or more, with no CLI compaction and
  no lost constraint. Tokens and time are reported per vendor, with the cache regime;
- C's token gates still hold (Q44): T7L ≤ 1.0× and the small tasks ≤ 1.15×.

**Risk:** disk growth from stored outputs and a file write per tool batch, both measured in V1. A mid-turn thread
interrupt (D2.3b) may cut a step in half: the steer comes first, and the successor gets the last commit and the
interrupted step by name.

### Phase O: Never more tokens than plain (Q57)

The user, 10-16: "We need to beat everybody's ass on token usage, no hallucination, reliability, speed." The token gate
becomes **≤ 1.0× plain Claude Code on every bench task** (it was ≤ 1.15×), with the full ratio (reviews included)
reported beside it. We keep the CLIs' own prompt caching, deferred tools and system prompt, and cut only what Brigadier
adds on top.

**O1. Measure first.** A Codex chore (O-1) ran plain and Brigadier on T1, T4 and T6 (n = 2 each) and Brigadier on T5
(n = 1) at `79dda573`, keeping the transcripts, and a per-call script split every model call into uncached input, cache
reads and writes, output, the tools it called and what was fed into it. One-word `claude -p` turns split the first
call's fixed start into its parts:

| First call's context (tokens) | Before O | After O2–O5 |
|---|---|---|
| Plain Claude Code (every built-in tool) | 16,089 | 16,089 |
| The thread's built-in tools and Claude Code's prompt | 7,486 | 7,486 |
| + Brigadier's instructions | 13,180 (+5,694) | 11,602 (+4,116) |
| + its always-loaded tools (as text, an estimate) | 17,520 (+4,340) | 14,885 (+3,283) |

- Brigadier's start was about 1.4k above plain's on every call; the thread drops plain's Agent, TodoWrite and similar
  built-ins (8.6k), and pays 5.7k of instructions and 4.3k of tool schemas.
- Calls per task were at or under plain's (T1 5–6 against 6, T4 6–8 against 10–11, T6 7–11 against 10), so the rest
  of the gap is per call. Where Brigadier spent a call plain didn't, it was its edit and its check in separate calls,
  or a try by hand after the commit.
- No wake turn that only said [quiet] happened in any of the 7 runs; T5's one [quiet] call ends the turn while its
  review runs.
- T5's findings turn was 6 calls at 58–68k of context (391k), plus the review's 145k of Codex: a fix through git with
  the Fixes trailer took a call of its own, and recording the findings another.

**O2. A smaller start (revision 70).** The instructions say each rule once (17.4 kB → 12.5 kB for a Claude thread on a
worktree). A rule's procedure moves to the message or tool that needs it, and its trigger stays in the instructions:
judging and recording findings into the review's findings message, the merge steps into the `[merge …]` messages and
`finish_session`, how `run`, `run_check`, `remember` and `ask_user` work into their own descriptions. Kept whole: the
rules against acting on one's own (merges, pushes, approvals, Desktop and clipboard, deletions, workers announced), E3's
final answer, D1's Brain and constraint rules, the Q48 review line and the [quiet] rule. A size test keeps the
instructions under 11.5 kB.

**O3. Fewer calls.** A Claude thread sends its last edit, any try of it by hand and the `run_check` that checks it in
one message, and tries nothing after the commit. `remember`'s field docs are shorter.

**O4. A cheaper findings turn.** The thread's `run_check` takes `fixes`, the ids of the review findings its change fixes.
When the check passes, its commit carries their Fixes trailer and they are recorded fixed, checked by that check (its
`[evidence: out-…]`), so the fix needs no git commit or `record_findings` call of its own. A failing check commits and
records nothing.

**O5. `run` behind the tool search (revision 71).** A Claude thread runs its commands in its own shell (whole up to
30 KB) and loads `run` through ToolSearch when it needs an output kept whole. `note_for_user` stays loaded (a
diagnostic T1 run spent a call on a note after its commit, which a search would have made two) and goes in the same
message as the last `run_check`. A commit's reply says nothing more runs once the work is done. Measured on its own in
9 diagnostic runs: no ToolSearch hop, no `run` call, no invalid tool call; the first call's context fell to 17.4k
against plain's 17.6k.

**Not built, and why:**
- holding `run_check` until its review returns: a call held past a minute goes to the background anyway, so the
  [quiet] call stays;
- the passing check's reply is already cut to 768 bytes of tail;
- the thread's shell resetting to its scratch folder after each command: it keeps `--resume` working; a follow-up.

**Done when:**
- every bench task T1–T6 and T7L has its gate ratio ≤ 1.0× plain (T1–T6 n = 2, T4 n = 4 middle-two mean, T7L n = 2),
  with the full ratio reported for each;
- no regression: hidden tests pass every run, the E3 audit passes, V2 (with the lookup battery) and V3 keep their
  verdicts, V3's invalid tool calls don't rise, high findings ≤ plain;
- rule R for revision 70, with a test on a session from before;
- the per-task breakdown before and after is in `docs/evidence`.

### Phase X: Experiments

- **Research A10: earlier thread rebirth.** 200k and 300k against today's 400k on 1M windows, run as V1 + V2 + a
  late-session quality probe. It runs in an experiment build, after P0.1, V2 and B5. Production thresholds change only
  on the user's go-ahead with the numbers (Q41.2).
- **Not proposed** (Q41.3–5): Claude fast mode; Codex `tokenBudget`; raising the 3,600-byte report cap, until the D
  phases' continuity tests show loss.

## 4. The ship gate (every phase)

In this order, on the phase's integrated candidate: a branch holding the phase's merged work on top of `main`.

1. **Full checks on the integrated tree:**
   - `TMPDIR=/tmp tools/full-checks.sh --cross --live`: fmt, generated types, build, clippy, tests, typecheck, lint
     and app tests, then the live smoke with the real CLIs;
   - `cargo check --release --locked --workspace --exclude brigadier-desktop --all-targets`;
   - `pnpm test` under CI-like load.
2. **The GPU-off startup check:** first paint and startup marks against `origin/main`, with the GPU off and the CPU
   throttled. A regression fails the gate.
3. **Codex review of the phase's diff and one fresh verifier** (Opus), with evidence per "done when". Valid findings
   are fixed, and steps 1–2 rerun on the fixed tree.
4. **The phase's V-tests and bench rows on the candidate, before anything is pushed** (§2.2 pass conditions, n
   stated, cold and warm apart, completion rate first). A failed condition sends the phase back, with nothing pushed
   or installed. This includes the token gates (Q44, §2.2): a phase that misses its gate doesn't ship. For C2 a
   failure is final: lazy tools don't ship.
5. **Push to `main`, watch CI until green, reinstall `/Applications/Brigadier.app`**, without asking (Q42). A red CI is
   fixed and pushed at once. The install follows the installed-app recipe and is checked to start.

Bench results go into `docs/evidence/benchmark-*.md` with the build's hash, and are never pooled with another build's.

## 5. Instruction rollout (rule R)

A resumed CLI keeps the instructions it started with. So every change to a prompt, an envelope's standing text or a
tool set must:

- **carry its revision note in the same commit:** a new `Revision` in `REVISIONS` (`M/prompts.rs`, after
  `LEAN_WORK_REVISION = 27`), whose `thread` text states the rule as "From now on, …". Revisions are never
  renumbered. Settings-driven text uses the `told` comparison (`M/prompts.rs:908`);
- **handle tool-set changes safely:** a removed tool (`propose_merge`) answers late calls with a refusal that says what
  to do instead. A new or deferred tool set reaches an open session only at its next resume or rebirth, since the
  tool set is part of the cached prefix, announced by the note;
- **be tested on a session made before the change:** the flow test creates a session at the previous revision and
  resumes it after the change. It checks that the note arrives once, the old behaviour is gone (no merge card is
  offered), and the new one happens (the thread does a V3 tiny edit itself). A fresh-session result doesn't validate
  a change for long-lived sessions.

## 6. The "WOW" checklist

Each item must be true at the end; how it is checked is in brackets.

1. A code question is answered in the thread, with the file:line it relied on and no worker (V3, 4/4 exact, 0 worker
   starts).
2. A tiny edit takes about as long as in plain Claude Code (V3, ≤ 1.25× time, ≤ +1 model call).
3. Small tasks cost about what plain Claude Code costs, at equal hidden-test quality: every small task (T1–T6)
   ≤ 1.5× after phase B and ≤ 1.15× after phase C, all vendors summed (Q44 token gate; a miss stops the phase).
4. A multi-file feature is as good as the plain CLIs' work or better (bench T2 and T3: every run passing, high findings
   ≤ the better plain arm).
5. Parallel work beats the plain CLI on time (bench T4: wall time ≤ plain Claude Code's).
6. Before any worker starts, the thread says so in one line, or asks if "Start workers on its own" is off (flow test;
   bench transcript audit).
7. No merge happens unless the user clicked "Merge into main" or wrote it (A4 and A5 flow tests, including stored
   answers, restart and old sessions).
8. A stray Enter or "1" never answers a consequential card, including one that has just taken focus on its own and
   the plan rail's approval (A5 render tests).
9. After a merge, the workspace is still there and the next message works in it (A3 flow test).
10. A workspace goes only on archive, delete or "Close workspace" (A3 tests).
11. No spare worktrees: none after a question, a self-edit or a quit (A8 flow test).
12. Every deliverable link opens, even after its worker's folder is gone (A1 flow and render tests).
13. No unasked Desktop files or clipboard writes (A2 V3 case; command audit).
14. The thread never hands the user a defect it can fix (A6 V3 case).
15. "Worked 11m · waited for you 18m": the user's reading time is never shown as work (A9 tests).
16. A stopped worker says "stopped", never "done" (A7 render test).
17. No claim is briefed as fact without evidence, whether a worker reported it or the thread saved it with
    `remember`, and every Brain line says where it came from (A11 and E1 tests, with reviews on and off; migration
    test).
18. Final answers list checks run, the review state and what wasn't checked (E3 audit).
19. Bigger changes get one other-vendor review, judged on the request's whole unreviewed diff, not commit by commit,
    and valid findings are fixed before "done"; tiny edits get the thread's own checks only (B2 flow tests,
    including the four-small-commits case; T2's final answer).
20. A usage limit or an account switch continues the same session with nothing lost (D1.3 V2 run).
21. Nothing planted early is lost after two rebirths, including a constraint with its exception and a ruled-out
    approach (V2, all items, both providers).
22. A daemon restart loses no waiting worker (D1.4 V2 run).
23. A hung worker or a silent thread is caught in minutes, not 50 (D1.5 fault injection).
24. No worker or thread is compacted by its CLI, even inside one long turn; workers hand off and threads are reborn
    mid-turn (D2.3 and D2.3b tests on both providers; no compaction in T7L).
25. The 24-feature long task finishes at least as many features as the best plain CLI, using no more tokens than
    plain Claude Code (bench T7L ≤ 1.0× from phase C on: Q44 token gate).
26. An idle long thread's note fork reads the cache instead of rewriting it (B5's `handoff` row).
27. Settings has exactly two behaviour toggles, "Review the work" and "Start workers on its own", and no quality knobs
    (render test, §1.4).

## 7. Out of scope, and what needs the user

- **Needs the user:**
  - C5's numbers, if enrichment's weekly cost calls for changing when it runs;
  - A10's go-ahead, with its numbers;
  - the report cap, after the D phases' continuity tests.
- **Not proposed** (PLAN §7, Q41): caps, budgets, early compaction, letting the CLI compact, per-step model switching
  by guessed difficulty, more agents by default, hooks that call a model, fast mode, `tokenBudget`.
- **Probes before building:** PLAN §5 lists them, and each item names the probe it depends on. An item whose probe
  fails is reported, not forced.
