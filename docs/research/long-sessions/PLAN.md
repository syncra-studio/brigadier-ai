# Improvement plan: long sessions, small tasks, usage and grounding (2026-10-14, revised after Codex review; reordered 2026-10-15 after the second pass)

This plan draws on `DIGEST.md` (the evidence), `WEAK-SPOTS.md` (the code at `d669bca0`), `PROBES.md` and
`BASELINE.md`. The second pass (DIGEST §7: w04 on Fable 5.1, w05 on Mythos 5.1, adjudicated by w06) added items
A11–A15, changed P0.1, P0.3, A2, A3, A5, A6, A9 and V1/V2, and changed the order (§4).

Every item keeps §7's rules: no caps or budgets, nothing dropped silently, lossless, no settings, judged per
completed task. Every item also keeps the settled decisions that superseded parts of §7. These are SESSION-FIXES
Q5 (every decision body whole in a rebirth briefing), §11 (integrated checks) and THREAD-PLAN (reviews can be turned
off, and then nothing waits for one). No item needs the Agent SDK, an API key, `--bare` or a request-rewriting proxy.

A third pass (2026-10-15, `SESSION-FORENSICS.md`) took apart one real session in the installed app. It added §3b
(F1–F5, the fixes for what the user hit, now first in §4), B8 (a smaller fixed thread start, with its numbers) and
§6 items 7–9.

Nothing here is built. Each item is a proposal with how to prove it. Items that would change a settled decision are
listed separately in §6, "Needs your decision".

## 0. How every item is validated

A saving counts only at **equal completed-task quality**. Each item names which of the tests below it needs.

**V1. Completed-task A/B** (the SESSION-FIXES Q6.7 protocol, tightened):
- **Frozen before either arm runs:**
  - the request text, base commit and acceptance checks (the checks hidden from both arms);
  - the Brain seed (one copy of the store, copied into each arm's data directory);
  - models, effort, accounts and CLI versions;
  - machine load.
- **Cache regime declared and controlled.** PROBES P3 showed that a later fresh session reuses an earlier session's
  whole fixed start within the hour, so arm order can bias usage.
  - Each run is declared **cold** (no session with the same flags in the same start folder within the last hour)
    or **warm** (one warm-up session just before it).
  - Dependency installs and build caches are equally warm in both arms (the same `warm.sh` state).
  - Arm order alternates (A B B A …).
  - Cold and warm results are reported separately and never pooled.
- **Every attempt is kept.**
  - Completeness is judged first by the frozen checks, then by a blind review from the other vendor against the
    whole request (labels in a sealed shuffle).
  - Incomplete attempts are not dropped. Each arm reports its **completion rate**.
  - It also reports usage **through verified completion**: the incomplete attempt, the retries, the repairs and the
    fix tasks it took to reach a result that passes both checks, or "not completed" if none did.
  - A cheaper arm with more incomplete attempts does not win.
- **Usage is counted through settlement:**
  - What counts: the thread, every worker, reviews, fix tasks, integrated checks, Brain jobs, background jobs,
    the thread's note-writing forks (`handoff` rows) and native subagents (`abcd1e95` counts the Agent tool), until
    the request's last review batch and check settle. Brain enrichment and checkpoint forks can land inside one arm's
    window and are large (WEAK-SPOTS C11, S8): count them per arm, or keep enrichment off during runs and say so.
  - How it is reported: per provider, raw and without cache reads, with cache writes split by lifetime (1 h / 5 min)
    and the rewritten share (P0.1), plus thread turns per request, the quota-window share from `quota_samples` and
    elapsed time. Codex is reported by its cached share of input, with its write counter kept (every observed value is
    0, from an optional field that defaults to 0, so Codex write charges are unknown, not zero).
- **n ≥ 3 per arm per regime.** One task varies up to 30× in tokens between runs (Bai et al. 2026). Codex cache hits
  swing about 45% per turn (PROBES P4). A result inside the spread is "no difference".
- **API-price weighting is an estimate**, because neither vendor publishes how subscription limits weigh tokens.
  Usage that isn't recorded stays "unknown" and is never filled in.

**V2. Continuity battery** (for items touching rebirth, hand-off, reroute or restart). The battery is **several
requests in one session**, with these planted early and far outside the verbatim tail:
- a standing user constraint ("never touch `X`") set in request 1;
- a correction to it, or an exception, in request 2 ("except in tests");
- a settled decision with its reason;
- three exact details: a commit hash, a path with a line number, and a number with units;
- a failed check with its error line;
- an approach tried and ruled out;
- an item that exists only in the **thread's own** tool output (a file it read or a check it ran itself);
- a **false** `learned` line in a worker report, which the thread must catch before it reaches a later brief as fact.

Each run forces **at least two seams of the kind under test**. Size seams come from the debug thresholds
(`BRIGADIER_REBIRTH_TOKENS`, `BRIGADIER_WORKER_HANDOFF_TOKENS`), which change sizes only. The other seams need their
own injection: a quota or error reroute from an injected provider error, an **account switch** on the same model,
a **"prompt is too long"** error, a crash by killing the CLI process, and a daemon restart by stopping and
restarting the debug daemon at a chosen wait point. Later requests then depend on
each planted item.

**Score:** each item passes or fails on its own. It passes if recalled exactly or acted on correctly, and the
constraint is applied *with its exception*. The target is all pass. Also scored:
- no ruled-out approach retried;
- no constraint broken;
- "unknown" given instead of a guess where an item really was lost;
- the briefing's size, measured rather than assumed.

**V3. Small-ask set:** 12 frozen asks with known answers:
- 4 code questions ("where is X set?");
- 4 tiny edits;
- 2 status questions;
- 2 "explain this error".

Each runs cold and warm, with a seeded and with an empty Brain. **Measured:** model calls, worker starts, time to
the final answer, exact correctness, and usage through settlement.

**R. Rollout to sessions already open.** A resumed CLI keeps the instructions it started with (§7 item 5,
SESSION-FIXES §6). So every item that changes a prompt, an instruction or a tool set must:
- carry its instruction-revision note (the `[instructions]`/revision mechanism in `M/prompts.rs`) **in the same
  commit**;
- bring new tools in through the same-session resume path;
- pass a test on **a session created before the change and resumed after it**. Fresh-session A/B results do not
  validate a change for long-lived sessions.

Items marked **R** below need this.

## 1. Enablers first

### P0.1 Know where cache writes come from (§7 next 1, WEAK-SPOTS N1)
- **What:**
  - Record each Claude cache write with its lifetime: parse `cache_creation.ephemeral_1h_input_tokens` and
    `ephemeral_5m_input_tokens` in `P/claude/parse.rs`.
  - Record per call an estimate of the **missed prefix** at the cache boundary: within one CLI session, the previous
    request's prompt (its input + cache read + cache write) should come back as this request's cache read, so
    `missed ≈ max(0, previous prompt − cache read)`. Brigadier's context figure can't be used for this: it adds output
    tokens (`P/claude/parse.rs:1009-1014`), which aren't cached until a later request. A fork, resume or rebirth is
    compared with the prompt of the session it came from (C11's fork: a 252k prompt came back as 544 read); a
    compaction, a new session with nothing to inherit and the first call of a session are reported apart, not
    estimated. It is labelled an estimate wherever it shows (the prompt can legitimately change, for example a
    `[settings]` note), and V1's accounting stays the raw input, cache read, cache write and output, not this figure.
    Flag any call that missed more than 20k. This finds causes Brigadier can't name. (w05 proposed
    `cache_write − Δcontext`; Codex's review showed that form goes negative on warm calls and understates rewrites.)
  - Tag the calls whose cause Brigadier knows (fresh start, resume after close, cold resume, rebirth, note fork,
    hand-off, reroute, effort change, MCP or tool-set change, CLI upgrade), so the report can say which known
    cause the rewritten tokens came from and how much is left unexplained.
  - Codex: keep recording `cacheWriteInputTokens` (all 725 observed rows report 0, but the schema field is optional
    with a default of 0, so this may mean "not reported"; subscription write charges are unknown), and record the
    cached share of input per turn, which is what Codex comparisons rely on.
  - A per-request report in the Inspector, counted through settlement, with thread turns per request.
- **Why:**
  - Cache writes are about 25% of usage (§7), and nothing can be tuned without knowing their cause.
  - Probe P1 confirmed that Claude `-p` writes at 1 h, the 2× price.
  - Practitioners and the Claude Code team treat cache hit rate as the key health metric (DIGEST §4).
- **CLI:**
  - Claude 2.1.299 stream-json `usage.cache_creation.*` was seen in the probes (CLI✓).
  - Claude's SessionStart hook fields `prompt_cache_likely_expired` and `estimated_cache_write_usd` (resume and fork
    only, 2.1.251+) are documented and in the 2.1.300 binary (CLI?). Bounded probe: one resume after 61 min with
    `--include-hook-events`.
- **Effect:** none directly. It enables every usage claim below.
- **Measure:** "unexplained" missed-prefix tokens are under 10% of write tokens over a week of the user's own use.
- **Risk:** low (read-only plumbing). **Size:** M.

### P0.2 Build V2 and V3 into `tools/ab`
- **What:** the multi-request continuity battery and the small-ask set as frozen fixtures with scoring scripts,
  beside `worker_tokens.py`. V1's cold/warm regime, alternating order and completion-rate accounting go into the
  same harness.
- **Why:** every item below needs one or the other. The research has no study of exact identifiers or dead ends
  surviving a seam, and V2 measures exactly that.
- **Includes** the second pass's V1/V2 additions (§0): `handoff` and Brain rows counted, the account-switch and
  context-window seams, the thread-only item and the false `learned` line.
- **Effect:** none directly; V1–V3 run on it. **Measure:** on an unchanged build, three reruns of each fixture give
  the same pass or fail per planted item.
- **Risk:** low. **Size:** M.

### P0.3 Durable raw output and a transcript export that can't lie (the dependency of A2's crash, restart and cross-provider paths, and of A8)
- **What:**
  - Store each tool result **before** Brigadier's 8 KiB normalisation clip (`OUTPUT_CLIP` in `P/claude/parse.rs:18`
    and `P/codex/parse.rs:17`) in the blob store under an id recorded on the event, for both providers and every
    tool. The clipped event carries a reference to the stored copy.
  - **On Claude, by one `PostToolBatch` hook** on workers and the thread, passed in `--settings` the way the thread's
    output hook already is (`P/claude/mod.rs:758-759`). Its `tool_response` is "the same content the model receives"
    for every tool in the batch, keyed by `tool_use_id`, so a successor's transcript is what the model saw by
    construction. A large valid Bash result is already a file in the CLI's session folder (2,000-character preview to
    the model); A8 copies it.
  - **On Codex**, through Brigadier's `run` tool as today, plus a `PostToolUse` hook **only if** the Codex hooks probe
    (§5) finds a way to deliver Brigadier's own hooks without loading the user's.
  - Replace `worker_transcript()` (`M/worker_handoff.rs:445-478`, which treats a failed page read as an empty page
    and rebuilds the whole history in memory) with an export that streams pages to a file. It propagates every
    read or decode failure, marks the export **incomplete** with the missing ranges, and is never called "full"
    unless it is.
  - Fault tests: a page read error, a decode error, and an export interrupted mid-way. In each case the result is
    marked incomplete, and no successor is told it has the whole transcript.
- **What the CLIs don't give Brigadier, named rather than assumed:**
  - **Claude:** a failing command's output reaches the model and hooks only as a ~10,000-character head-and-tail
    excerpt with no file (tools-reference; `PostToolUseFailure.error` is the same middle-truncated text). Whether
    raising the read-back window (`BASH_MAX_OUTPUT_LENGTH`, up to 150,000, internal, not a setting) brings the end of
    a long failing log into that excerpt is the §5 probe. Long checks belong in `run_check`, which is stored whole.
  - **Codex:** command output in history is bounded by `tool_output_token_limit`. Whether the app-server's completed
    item carries the full aggregated output must be probed.
  - What can't be had is recorded on the event as "CLI gave an excerpt only", so no successor treats it as
    complete.
- **Why:** WEAK-SPOTS C9, C10. §7's "lossless" rule can't be claimed for successors until this exists.
- **Validate:** the fault tests; V2 with a seam right after a large failing test run, on both providers.
- **Effect:** none on usage. Successors are told exactly what they have and what they lack.
- **Risk:** disk use. The blob store has no size bound of its own (`crates/store/src/blob.rs:129-145`), and the 64 MiB
  limit on command output is enforced by the readers, not the store, so stored outputs grow until their session is
  deleted. Disk growth is measured in V1, and a failed write marks the export incomplete. A hook adds a file write
  per tool batch, measured as turn latency in V1. **Size:** M.

## 2. Long multi-hour sessions

Numbered in the order they were found (A11–A15 come from the second pass); §4 gives the build order. Each item names
the weak spots it addresses and the evidence for it.

| # | Item | Weak spots | Evidence | Expected effect | Validate | Risk | Size |
|---|---|---|---|---|---|---|---|
| A1 | **Standing constraints and negative knowledge across every seam and every request. R**<br>(a) The user's standing constraints, with their corrections and exceptions, are kept as **verbatim source spans**: the complete user message, or the exact quoted span with its message id. Each has a durable scope (this request, this session, this project) and **explicit supersession** (a later span marks which earlier one it replaces). The thread records them with `@remember` and a new `constraint` kind, pointing at the message; Brigadier does not extract free text "by code". Every rebirth briefing and worker brief carries every active constraint in that scope, word for word.<br>(b) The thread's rebirth note gains the worker note's headings: ruled out and why; checks that failed, with their error lines; exact identifiers in use.<br>(c) Every cut in a briefing names the message id and the omitted range, so `search_transcript` can fetch it | C4, C5 | DIGEST 3 (constraints kept apart from the summary: over 90% retained against 17–53%, and violations back to 0%; loss compounds per round); practice: the `varchar(128)` case | Fewer broken constraints and repeated dead ends after rebirths and across requests. Briefing size grows with the number of active constraints: measured in V2, not assumed small | V2 (multi-request, size seams ×2) and R; then V1 on one long task | Med: a constraint wrongly marked superseded is a loss, so supersession is never inferred. A long pasted spec is pinned by reference to its stored message, not inline | M |
| A2 | **One successor contract for the seams a resume can't cover.** Needs P0.3. A12 takes limits, account switches and sign-in errors by resuming the same session; A2 covers the rest: a crash with no resumable session, a daemon restart whose CLI files are missing, and a move to another provider. These successors get what a size hand-off gets: the streamed transcript export (P0.3) with its completeness mark, the note, `checks.md`, the decisions, and references to the stored raw outputs. With no note, the successor gets a "state of work" built from the transcript and marked unchecked. A restart whose CLI files are missing uses this path instead of stopping. `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1` is tried first where a killed Claude session is still resumable (documented, CLI?) | C1, C3, C10 | DIGEST 2 (a fresh consolidated brief recovers 95%); Handoff Tax (a short verified brief beats the whole trajectory); RLM (the transcript as a searchable store beats a summary) | Crash, restart and cross-provider successors stop redoing work. Lossless only as far as P0.3 records it, and the successor is told where it isn't | V2 with forced crash, restart and cross-provider seams, on both providers | Med: shared code across three paths, which must not slow the size hand-off | M (smaller since A12) |
| A3 | **Survive a daemon restart.** Persist the cold-cache checkpoint when it is written, and reload it at start. Recover Blocked and Paused workers by their wait reason instead of stopping them. Reconcile an interrupted landing against the target branch's ancestry before saying "nothing landed". **Codex may get a cache lifetime later**, not from OpenAI's 30 minutes, which the guide gives as a *minimum* retention ("at least 30 minutes", possibly longer): Brigadier's lifetime is an expiry that triggers a rebirth, so a guessed one could replace a session whose cache is still warm. Only if repeated controlled observations (§5: several idle gaps, n ≥ 10 resumes per gap, under 15 requests per minute, on gpt-6.1-sol) show a reliable drop in the cached share past some idle time, distinct from P4's routing swings; and only if a Codex note fork shares the thread's prefix | C7, C2, N5, w02 #3 | BASELINE (cold rebirth exists; a restart loses it); OpenAI caching guide; Symphony (re-derive from durable state, restore no timers) | After A11: one avoided cold resume of a 400k thread saves about 400k of 1 h cache writes per restart. No waiting workers lost overnight | V2 with a daemon kill at each wait point; flow tests; for Codex, the repeated idle-gap observations first | Low/Med | M |
| A4 | **Only evidenced facts become Brain facts. R**<br>(a) Worker-authored items (`report.learned` → Convention nodes at `M/brains.rs:1200`, `report.decisions` → Decision nodes at `M/brains.rs:1185`) are stored as **claims** with their source (task, message id). The decision lines of the thread's own rebirth note (`M/brains.rs:1382`: orchestrator origin, no task) are claims too, with the session and the note's generation as their source, unless they point at a user decision.<br>(b) **User-authorised decisions** (the user's words, card answers, outline go-aheads) stay decisions as today: they are authority, not empirical claims.<br>(c) An empirical claim becomes a fact only with **evidence for that claim**: a file:line it cites that still says so, a check id whose output shows it, or a review that examined it. Passing integrated checks or a clean review of the landing is **not** evidence for an unrelated claim.<br>(d) A claim without evidence stays "unverified" and is shown as such in briefs and Brain answers. A finding that contradicts it supersedes it.<br>(e) With reviews off, nothing waits for a review; claims simply stay claims unless other evidence supports them | G1 | DIGEST 6, 7 (stored own errors spread; 15–26 points gained by adding only evaluator-approved memories); practice: memory files as attack surface | Fewer wrong "conventions" passed on to later workers; not measurable as usage | V2 variant: a seeded **false** `learned` claim that survives passing checks and a clean review must still reach a later worker labelled "unverified" **in the brief itself** (A13(b) prints the label), never as fact; and R | Low/Med: more claims shown as unverified, and the brief must stay readable | M |
| A5 | **Grounding gates.**<br>(a) Overnight acceptance needs per-criterion evidence: [met] with a check id or an output reference, bound to the accepted tip. **When reviews are on**, a settled review is also required. **When reviews are off**, acceptance keeps the real criterion checks and the morning report says plainly "no review was run". The **first report of a long phase** gets this check in full even when nothing else does, since long tasks fail early (LongCLI-Bench).<br>(b) A verifier or reviewer gets the lead's whole report (risks, open questions, artifacts), works in the worktree with a shell and is told to run things; a review that ran no command and read no file is labelled "read-only review" in the report.<br>(c) A worker question that times out may proceed only on a recorded safe default. Consent and product choices stay waiting.<br>(d) `run_check` reruns integration, smoke and live commands unless they declare their inputs.<br>(e) **Cheap checks inside the turn:** `submit_report` refuses a "done when" marked [met] that names no check id, command or output reference, and says what to add (the worker repairs it in the same turn, as it does for the size check). A Claude `Stop` command hook (no model call) refuses only a turn that ends with no report, or a hand-off note without its required parts (C8), with the reason as its message; `stop_hook_active` and the CLI's cap of 8 bound it. **It never refuses a turn that is rightly waiting**, the cases `worker_turn_completed` already lets end without a report (`M/workers.rs:3077-3112`): a lead waiting for its outline's go-ahead, a worker whose review is pending while reviews are on, a worker with its own background command or subagent still running (also the hook's `background_tasks` input), and a worker with a question out. The hook asks Brigadier for the worker's wait state rather than guessing it, and these paths are tested, including the background wait fixed in `e5534377`. **R** | G2, G4, G5, G3, C8 | DIGEST 6 (false success claims; transcript-reading judges are unreliable, judges that run things agree with humans); LongCLI-Bench (failures early, self-correction marginal) | Fewer "verified" results that aren't; fewer turns lost to reports without evidence; a few extra runs for integration checks | V1 on an overnight-style task with a criterion the suite doesn't cover, with reviews on and off; repair rounds and turns per accepted report; flow tests for each waiting case (outline, review, background job, question) showing the Stop hook lets the turn end; R | Med: more waiting overnight, though the safe default is an answer; a hook loop (bounded) | M |
| A6 | **Workers never reach their CLI's own compaction. After A12(a) and P0.3.**<br>(a) Launch Claude workers with `auto_compact: false` (`M/workers.rs:1870`), as the thread already does. Codex workers get `model_auto_compact_token_limit` set far above the window in the same way. Codex still clamps it to about 90% of the window, so for Codex it is (b), (c) and (d) that keep a worker from compacting.<br>(b) The hand-off guard stays **safely below each provider's real compaction ceiling**: Codex about 90% of its window, Claude's auto-compact point when on. It is capped so `start + at/2` can't exceed it.<br>(c) The wrap-up steer, already sent mid-turn, is **sent again after each further stretch of growth** (not once per turn), and past a hard line near the ceiling Brigadier interrupts the turn and runs the size hand-off. A "prompt is too long" error goes to the size hand-off (A12(b)), never to a reroute.<br>(d) **Compaction becomes a seen event:** a Claude `PreCompact` hook (in `--settings`) blocks a proactive automatic compaction and triggers the hand-off; if one happens anyway, `PostCompact`'s `compact_summary` is stored with the breach. On Codex the same through `PreCompact` `continue: false`, only if the Codex hooks probe finds a delivery path | L1, L2 | Codex compacts at its own ceiling; BASELINE (workers launch with compaction on); Claude changelog 2.1.293 (compaction made Claude redo finished work); hooks docs | No uncontrolled compactions in workers or threads; continuity kept by Brigadier's hand-off | Oversized tool results on both providers, on a 200k-window model, showing no `CompactionStarted` and a hand-off instead, and the hook firing first; V2 | Med: with compaction off, a worker that ignores the steer can hit "prompt is too long", which is why A12 comes first and (c) has a hard stop. The forced hand-off in (c) builds its successor from `worker_transcript()`, whose tool results are already clipped at 8 KiB and which may have no note after an interruption, so A6 waits for P0.3 and its V2 run plants details beyond the clip. Blocking a compaction triggered to *recover* fails the request, so only proactive ones are blocked | M |
| A7 | **A decision index, additive only.** Keep **every decision body in full** in every rebirth briefing, as SESSION-FIXES Q5 settles and `M/rebirth.rs:1172-1223` implements. Add a compact index on top (id, scope, status, supersedes) so the reborn thread can see which decisions are active and which were superseded. Measure the whole briefing before launch and report it in the Inspector. Paging decision bodies out is **not** proposed here; see §6 | C6 | ACE (itemised, incremental beats rewrite) | Easier navigation of a large ledger, at a small extra size. It does **not** reduce the ledger's cost on multi-day sessions | V2 with 300 seeded decisions, 50 superseded: all bodies present, the index correct | Low | S |
| A8 | **Workers' long command output, the cheap way.** Needs P0.3. Claude Code already stores large Bash output in its session folder and shows the model a ~2 KB preview, so the model-side saving exists. A8 copies those `tool-results/` files into the blob store with the transcript at hand-off, reroute and review-hold. Add the thread's output hook to workers only if V1 shows real cost left. Failing commands get a ~10,000-character excerpt and no file (documented), so for them only `run_check` keeps the whole output | C9 (+ §7 next 2) | DIGEST 4 (trim at the source, keep the full output); practice (the CLI's own persist-to-file) | Losslessness for successors at almost no cost; the usage effect is likely small | V2 with a seam after a large failing test run; V1 only if the hook is added | Low | S |
| A9 | **Supervision.** First, a watchdog for the thread's own turn (a CLI that accepted the turn and went silent). Then a shorter worker stall deadline that still tells progress from chatter, with command-specific deadlines (for example foreground servers); the minutes are set by fault injection, with OpenAI's reference orchestrator (5 minutes without any event) as the outside reference | W1, W2 | Audit; Symphony `SPEC.md` | Fewer stalled overnight hours (today ~50 minutes per hung worker, unbounded for the thread) | Fault injection (a silent provider, a hung command), time to detect, and false stops on long legitimate commands | Low | M |
| A10 | **Earlier rebirth on big windows: an experiment, not a change.** V1 and V2 with the thread's rebirth at 200k and 300k against today's 400k on 1M windows | Baseline Q5 | DIGEST 1, 2 (quality falls with length well before the limit; a fresh consolidated prompt recovers it) | Unknown: possibly better late-session quality, at the cost of more rebirths | V1 (usage) + V2 (continuity) + a late-session quality probe (the same follow-up asked at 100k and at 350k) | Med: each rebirth costs a briefing write, and changing Q5's thresholds is the user's call (§6) | S + runs |
| A11 | **The note-writing fork shares the thread's prefix, and stops counting as a refresh.** Launch the checkpoint and hand-off fork from a clone of the thread's live `SessionSpec`, changing only origin, ephemerality, recording and folder ownership: the same built-in tools, the same MCP servers and catalog role, the same model, effort and settings. The fork keeps today's read-only access and keeps "don't call any tool" in its prompt, but a prompt and MCP-side refusal can't stop the **native** `Bash`, `Edit` and `Write` of a writable thread's tool set. So every call is refused before it runs, without removing any declaration: a `PreToolUse` command hook in the fork's `--settings` that denies every tool, native and MCP (hooks are client-side and not part of the prompt), with Brigadier's MCP server also refusing calls from a fork owner. Don't remove tools (`--disallowedTools` with bare names removes them from context and breaks the prefix again). Drop the `refreshed` extension in `M/cold.rs:159-172` now, and restore it only if a fork measurably reads the thread's cache | C11, C7 | Anthropic prompt caching (tool definitions are the first prefix level); `turn_usage` row of 2026-10-14 03:46 (544 read, 216,616 written) | Removes a write the size of the whole thread from every checkpoint and every threshold rebirth: about 216k on the one measured fork, more on a 400k thread. Without the `refreshed` extension, no resume of a cold CLI between minute 60 and ~110 | One `handoff` row with cache read ≈ the thread's context and a write of a few thousand, **with the deny hook in place** (the probe also shows whether the hook changes the cache); a test fork that is asked to call a native tool and an MCP tool, and is refused both times with nothing changed on disk; then V1 with an idle period over 50 minutes, both arms counted through settlement. The note itself is unchanged, so V2 isn't needed | Low/Med: a fork with real tools could act on the workspace, so the deny hook and its test are part of the item, not optional | S |
| A12 | **Resume the same session where it still exists.**<br>(a) On a usage limit, a credits limit or a sign-in error where the same model can continue (another account of the provider, or the same account after its reset), resume the **same CLI session** instead of starting the thin successor: Claude by transcript (`--resume` takes the absolute path of the session's `.jsonl`, so the file can be made reachable from the other account's config folder), Codex by thread if the §5 probe shows a thread resumes under another account. The cache is cold either way; the context is whole.<br>(b) "Prompt is too long" (`ContextWindow`) goes to the worker size hand-off (`hand_over_worker`: transcript, note if any, `checks.md`), never to `fallback`. Until P0.3 that transcript's tool results are clipped at 8 KiB, so (b) is **better than today's reroute but not lossless**, and says so to the successor; its V2 test of details beyond the clip runs after P0.3.<br>(c) Where (a) can't apply (a different model, a different provider), the reroute uses A2 | C1 (widened), L2 | Vendor CLI reference (`--resume` by path); w05's ordering argument (A6 before this turns compaction into a thin cross-model successor) | Limits, account switches and sign-in errors stop losing work: read-only research survives, implementers continue mid-task. A cold resume of the whole context costs one write of it, the same as a successor's re-reading, without the loss | V2 with an injected limit, an account switch and a context-window error, on both providers: the successor answers every planted item | Med: a resumed session on a second account depends on the CLI accepting the copied transcript (probe first); a session too large for its model's window still needs (b) | M |
| A13 | **The thread sees what workers teach the Brain, and briefs say where each fact came from.**<br>(a) The report envelope lists `learned` like the other lists, and `report_text` includes it. For a **read-only** task the Brain takes the report's items as soon as it is submitted (`learn_report` runs before the thread is woken, `M/workers.rs:3668-3670`, and creates Convention and Decision nodes, `M/brains.rs:1185-1217`), so the thread can't strike a line before it is kept. A13 therefore brings A4(a)'s first step forward: worker-authored `learned` and `decisions` are stored as **unverified claims** with their source when they are taken, and the thread can supersede one it finds wrong. For a write task they are kept only when it lands, after the thread has read them.<br>(b) `brain_lines` prints each line's origin and evidence state ("user decision", "worker claim, unverified", "enrichment, unverified") and orders verified facts before claims.<br>The 3,600-byte report limit is kept (it already counts `learned`); whether to raise it is §6 item 5 | G6, G7, G1 | DIGEST 7 (storing an agent's own unchecked claims spreads its errors) | The thread can detect and supersede a wrong convention (for a read-only task, after it is stored as a claim, not before); later briefs show it as unverified or not at all, never as settled | V2's planted false `learned` line, caught by the thread or reaching a later brief labelled "unverified", never as fact | Low: a slightly longer envelope; the brief must stay readable | S |
| A14 | **Coalesce thread wake-ups, never drop them.** Envelopes that need nothing from the thread now (a landing done, checks passed with nothing to do, a review with no findings, a status) wait a short window, or until an urgent envelope (a question, a failure, a report) or the user arrives, and then go in one turn. Questions and failures stay immediate | S6 | `turn_usage` 2026-10-14: 26 of 75 thread turns answered with under 400 output tokens and read 9.1M | Fewer full-context thread turns per request, and slower context growth toward the next rebirth | V1 thread turns and reads per request, and V3 latency, at equal completed-task quality | Low: a reaction that waits seconds longer; nothing urgent waits | S/M |
| A15 | **`search_transcript` covers the thread's own work.** Index the orchestrator log's provider events (the thread's commands, reads and their stored outputs by `read_artifact` alias), and make the briefing say what the search covers and how to fetch a cut range | C12, C4, C5 | RLM (a long history kept as an external store the model searches beats compaction) | A reborn thread finds what its predecessor read or ran instead of rerunning or guessing | V2's thread-only planted item, after two rebirths | Low: a larger index | S/M |

## 3. Small and short tasks

| # | Item | Weak spots | Evidence | Expected effect | Validate | Risk | Size |
|---|---|---|---|---|---|---|---|
| B1 | **Answer small questions directly. R.** The thread answers a code question itself when the Brain, `code_search`/`code_refs` and up to about 3 precise reads answer it, and it quotes the file:line it relied on. Scouts are for exploration, many files, or the web. A one-line edit stays a thread self-edit. Prompt change in `M/prompts.rs:101-103`, plus the Brain-miss hint at `M/brains.rs:779` | S1 | DIGEST 9 (delegation costs several times the tokens and hurts sequential work); practice (a new session's preamble ~33k, a worktree 5–15 s); vendor ("never speculate about code you have not opened") | Fewer model hops and no 15–30k worker start for small asks; faster answers | V3 (calls, worker starts, latency, exact correctness) and R | Low: the thread's context grows slightly; the tiny-edit rule stays | S |
| B2 | **Precise reads** (§7 next 3). **R.** Two routes, measured against each other:<br>(a) Enable Claude Code's built-in LSP tool for workers and the thread where the project has a language server, telling the model to trust its results without re-reading the file.<br>(b) Index tools `code_outline` (a file's symbols with line ranges), `read_symbol` (one definition with its doc comment) and a batched line-range read, all with file:line headers.<br>**Freshness:** reads come from the **caller's effective worktree**, not the project's indexed base (`task_index()` at `M/brains.rs:1021` picks the index by project today). Each read checks the indexed file's hash against the worktree file and reparses it when stale, so a worker's own edits, rebases and uncommitted changes are read correctly. (b) works for Codex too | N2 | §7 (file reads are the biggest tool-result cost); DIGEST 4 (SWE-Pruner 23–54%); CircleCI LSP vs grep: 31–33% fewer output tokens and no missed references | Fewer whole-file reads, and fewer missed call sites for weaker models | Bounded probe first (does the LSP tool run in a `-p` session, and what does it add at start?). Then V1 on two implement tasks + V3, counting Read/`sed` bytes and calls per task. Freshness tests: a definition changed and left uncommitted in the worktree, a rebased branch, and a file another worker changed on main | Low/Med: a read that misses context costs another call, so calls are measured, not just bytes. Turning a tool on mid-session breaks the cache, so it is set only at session start | M |
| B3 | **Effort by model and step.** Floors per model family come from measured data. Today's evidence says Sonnet implement at high, Opus 5.5 implement possibly at medium, scouts at low. The thread names the effort for the job, and the router applies the model's floor. On Claude 5.5 models, effort can change between steps of one worker (P2: a fixed ~9.4k rewrite, with the history still cached). On Codex, assume a full cache miss per effort switch (effort is part of OpenAI's cached prefix) until the §5 probe shows Codex sends `configuration_update`. Lower effort is not expected to reduce made-up answers (AbstentionBench: reasoning tuning lowers abstention), so V2's "unknown instead of a guess" score is part of the quality check | E1 (+ §7 next 4) | DIGEST 8; PROBES P2 | Lower thinking cost on Opus where quality holds; fewer failed Sonnet low-effort tasks | V1 per model × effort on frozen implement and scout tasks, with completion rates. A lower effort is adopted only where completion holds | Med: quality loss if the evidence doesn't transfer, which is why it is measured per model | S–M |
| B4 | **Share the fixed start across conversations.** Probe `--exclude-dynamic-system-prompt-sections` for workers and the thread. If a new conversation's first worker then reads its 15–25k start from the cache within the hour, adopt the flag (it also removes the need for the shared start folder). Codex stays as it is (§7: a leaner Codex prompt lost the cache) | — | `claude --help` 2.1.299; PROBES P3 | Saves a ~20–25k 1 h write (≈ 2× that in input-equivalents) per new conversation started within an hour of another | Bounded probe: two conversations in two worktrees, with and without the flag. Then V1 **with the cold and warm regimes reported separately**, since this item is exactly about warmth. Within one conversation Claude workers already share a start folder and keep the task in their first message, so the probe also checks whether each worker's `--add-dir` worktree path lands in the dynamic system-prompt sections (w05's broader doubt didn't hold, WEAK-SPOTS §7) | Low: one flag, dropped if the probe shows no cache read | S |
| B5 | **Load only what applies. R.** Repository instructions: root and ancestor rules always, subtree rules when files there are touched, and a list of the rules left out. Context pack: reads relevant to this task (named files and symbols), not the whole conversation's, plus the ledger decisions that name the task's files or symbols (actions carry implicit decisions); the user's standing constraints (A1) always | S3, S4 | Vendor (keep instruction files short, load detail on demand); practice (a small always-present index beat on-demand skills); DIGEST 1 | Smaller worker starts, fewer unrelated files | V1 on a task in a sub-package of a monorepo, measuring first-call context and completed-task usage; R | Med: a rule loaded too late is a missed constraint, so ancestor rules stay pinned | M |
| B6 | **Prewarm only when a writing task is likely**: the thread called `delegate_task` once in the request, or the request names a change | S5 | Audit | Less I/O on question-only sessions | V3 timing on a busy machine | Low | S |
| B7 | **A verified scout brief. R.** When a scout comes before an implementer, its report includes ranked files with line ranges, a reproduction command and the dead ends it tried. Brigadier runs the reproduction with `run_check` before the brief goes out, and drops claims that fail | — | DIGEST 10 (Scrouting: a verified hand-off at about 1/5 the cost per solve) | Cheaper implementers, fewer repeated explorations | V1 on two bug-fix tasks, with and without | Low | M |
| B8 | **A smaller fixed thread start.** Brigadier adds ≈19k tokens to **every** thread call: its MCP catalog ≈14k (48.0 kB with `run`, as under Full access; `delegate_task` 6 kB; a byte estimate) and its instructions ≈5k (20 kB), plus up to ≈1.5k of Personal Brain preferences, on top of Claude Code's own ≈12k (SESSION-FORENSICS §4, measured at `d669bca0`: first call's prompt 31.2k; 30 calls in the forensics session's 8 turns). Without changing a rule:<br>(a) trim `delegate_task`'s schema to one line per field, with the detail for rare fields (operate `target`/`end_state`, `mockup`/`matches_mockup`, `phase`/`role`, `image_generation`, `quality`) moved into the brief rules that use them: ≈1k per call;<br>(b) send the peers, previews, mockups and overnight sections as `[settings]`-style notes when they first apply, as the browser and reviews sections already are: ≈1–2k per call;<br>(c) shorten tool descriptions that repeat the instructions (19 kB of descriptions): ≈1–2k per call.<br>Loading rare tool groups lazily behind tool search (≈6–7k more per call) reverses `533bac83` and is in §6 item 9. **R** for (b) | S2 | Bench T1 on `launch`: 429k against plain Claude Code's 131k, ~10 thread calls at an average ~38k each (SESSION-FORENSICS §4.1); orch seq 161118, 161136 | ≈3–5k per thread call, ≈30–50k on a 10-call small task (≈7–12% of T1); more with §6 item 9 | V3 on the bench's T1 and T2 (first-call context, calls, tokens, pass rate) with (a)–(c) on and off; a flow test per moved section that its note arrives before the first use; R | Low: a missing field doc leads to a wrong call, so V3 counts invalid tool calls | S |

## 3b. User-facing fixes from one real session (SESSION-FORENSICS, 2026-10-15)

These are confirmed in the installed app's data (`brigadier.db` rows in SESSION-FORENSICS §1–§3) and come before
everything else in §4. They are small and each one cost the user something real: a lost deliverable, a false claim,
a request they had to repeat, or a workspace that seemed to vanish. None needs a decision except where §6 says so.

| # | Item | Weak spots | Expected effect | Validate | launch | Size |
|---|---|---|---|---|---|---|
| F1 | **Deliverables open from where they're kept.** (a) Brigadier rewrites any link into a task's scratch or `outputs/` (in worker text, addenda and thread messages) to that task's stored output, which opens and saves from the blob as the task card does (`M/outputs.rs:668`, `:681`). (b) An addendum whose task's scratch is gone says so. (c) Thread rule: "a worker's outputs are opened and saved from its task card; never give a scratch path; never point the user at a file you haven't just seen exist". (d) Thread rule: hand over a file with the card's Save or ask where it goes; no Desktop or clipboard writes the user didn't ask for. **R** for (c), (d) | U1, U2, U4 | No dead links; no false "also saved at"; no invented locations | Flow test: a read task writes `outputs/x.md` and links it, reports, is disposed; the link in the worker's text, the addendum and the thread's answer opens the stored copy. R | Partial: `3755744e` folds an addendum into an unread report; the dead link stays | S |
| F2 | **Fix known defects before answering.** `M/prompts.rs:129`'s "To check" becomes "only for what nobody here can check or fix; a known defect in a deliverable you fix yourself, or send back, before you answer". Under Full access the thread may edit a stored output's saved copy. **R** | U3 | The user never has to say "fix it yourself" | V3 case: a scout's deliverable with one planted factual error the thread can see; pass if the answer has no "To check" for it and the delivered file is right. R | No | S |
| F3 | **The merge card says what it does, and comes at the right time.** (a) The card's text: "Merges into `main` and closes this workspace; your next message starts fresh from `main`." (b) Thread rule (`M/prompts.rs:293`, `:112`): offer `propose_merge` once the whole request's work has landed, or when the user raises merging; never after each task. **R** for (b). A "Merge and keep working" option is §6 item 8 | U9 | No surprise when the worktree goes; fewer early merge cards | Flow test: card text; a two-task request offers the merge once, after the second landing. R | No | S |
| F4 | **No leaked pre-warms.** Drop pre-warm slots when the app quits; at launch, sweep `task-next-*` owners with no task. Then B6 (pre-warm only when a writing task is likely) | U6, S5 | No orphaned worktrees after a quit | Flow test: pre-warm, quit before the 10-minute timer, relaunch: the worktree is gone | No | S |
| F5 | **Small honest-UI fixes.** (a) A task stopped without landing shows "stopped", not "done" (U5). (b) `~/` is expanded before a read path is joined onto the workspace (`M/reads.rs:358-363`) (U8). (c) One look at the weekly-quota projection that sent the mockup to Codex at "459%" with 23% used (SESSION-FORENSICS §2, observation) | U5, U8 | The workers row and the read log say what happened | Unit tests for (a) and (b); (c) is a read of the projection code | No | S |

What `launch` already covers from this pass: `3755744e` (addendum folding, partial for U2), `cab67d97` (the thread
makes a small change itself, so fewer thread calls; B8's multiplier) and the long-report "Outputs … the user saves
them from the task card" line (already on main at `M/prompts.rs:1600`). Nothing in `launch` touches the merge card,
the timer, links to scratch paths, pre-warm cleanup or the "To check" rule.

## 4. Order

Rebuilt on 2026-10-15 from the current order, w04's (Fable) and w05's (Mythos). The rule used: first what is small,
confirmed and costs something on every long session today; then what makes the rest provable; then the continuity
fixes in the order that never makes a seam worse; then safety; then the usage savers that need measurement.

0. **F1–F5** (§3b), F1, F2 and F3 first. All S, all confirmed in one real session, and they are what the user sees:
   a lost deliverable, a false claim, a request repeated, a workspace that seemed to vanish. F1 (c), F2 and F3 (b)
   are one prompt revision.
1. **A11** (fork shares the prefix; drop the false refresh). S, confirmed in the code and in the store, and
   validated by one `handoff` row. Today every checkpoint costs more than the cold resume it was meant to avoid
   (about 294k against about 252k on the measured one).
2. **P0.1** (lifetime split and the rewritten-tokens arithmetic) and **P0.2** (V2 with the second-pass seams and
   items, V3, V1's accounting). Everything after is measured on them.
3. **A13** (`learned` visible, provenance in briefs) and **A1** (constraints verbatim, negative knowledge in the
   rebirth note). A13 is S; A1 is the best-evidenced continuity fix (DIGEST 3).
4. **A12** (resume the same session on limits, account switches and sign-in errors; "prompt is too long" through the
   size hand-off, labelled not lossless until P0.3).
5. **A3** (persist the checkpoint, recover waiting workers and landings) and **A9** (thread watchdog first, then a
   shorter worker stall deadline).
6. **B1** and **A14**, measured together on V3 and V1 thread turns: both cut full-context thread turns for small
   things. **B8** (a)–(c) goes with them, on the same V3 runs: B1 and A14 cut the number of thread calls and B8 the
   size of each, and the bench gap is the product of the two (SESSION-FORENSICS §4). `launch`'s self-edit
   (`cab67d97`) already starts on the call count.
7. **P0.3** (`PostToolBatch` capture on Claude; Codex after its hooks probe; honest export), **then A6** (workers
   off CLI compaction, repeated steer and a forced hand-off, `PreCompact` on Claude), then **A2** (crash, restart,
   cross-provider), **A15**, **A4**, **A5**. A6 comes after A12, or a compaction becomes a thin successor on another
   model, and after P0.3, or its forced hand-off passes on clipped tool results (Codex review of the second pass).
8. **B2, B3, B4, B5, B6, B7, A7, A8.**
9. **A10**, an experiment, only after P0.1 and V2 exist and only with the user's go-ahead (§6).

**Why not w04's order** (P0.1–P0.3 first, then A1, A3, A6, A9, a hook-based done gate, B1): it has no place for the
fork fix, the report and provenance fix, coalescing or same-session resume, which were found after it was written;
it builds A6 before any reroute fix; and it puts P0.3 first, though A12 now covers the commonest seams without an
export. **What changed from w05's order:** A1 moves up beside A13 (w05 had it in step 5), because constraint loss
is the strongest finding in the literature and V2 tests both; A6 moves after P0.3 (w05 had it right after A12), so
the new forced hand-off doesn't pass on clipped output; A5 gains the cheap in-turn evidence check and the narrow
`Stop` hook from w04.

## 5. Bounded probes the docs can't settle

| Probe | Settles | Cost |
|---|---|---|
| One checkpoint fork launched from a clone of the thread's spec with a deny-all `PreToolUse` hook: is cache read ≈ the thread's context, the write a few thousand, and are native and MCP calls both refused? | A11 (also its validation) | 2 forks on an idle test thread over 50k |
| Claude resume after more than 60 min with `--include-hook-events`: does SessionStart carry `prompt_cache_likely_expired` and `estimated_cache_write_usd` in stream-json? | P0.1 cause tags | 2 small calls + a wait |
| Claude `--resume <absolute .jsonl path>` from a second account's config folder: does it resume with the whole history, and under which account? Codex: does a thread copied into another account's `CODEX_HOME` resume? | A12(a) | 2 small calls per provider |
| Claude `-p` stream-json with a `PostToolBatch` hook in `--settings`: does it fire, and is `Read`'s `tool_response` the line-numbered text? | P0.3 on Claude | 2 calls |
| Claude: a command failing with ~100 KB of output, `BASH_MAX_OUTPUT_LENGTH` at its default and at 150,000: does the excerpt show the end of the output in either case? | Whether raising the read-back window helps failing test logs | 2 calls |
| Codex: Brigadier's own `PostToolUse` and `PreCompact` hooks without the user's (an inline `-c` `[hooks]` override with `features.hooks=true`, or a per-session `CODEX_HOME` with `hooks.json` and `--dangerously-bypass-hook-trust`): do they fire under app-server, does sign-in survive, are the user's hooks kept out? | P0.3 and A6(d) on Codex; WEAK-SPOTS N4 | ~3 turns |
| Codex app-server `item/completed` for a command printing 200 KB: full aggregated output or bounded by `tool_output_token_limit`? | P0.3: what Codex exposes | 2 turns |
| Codex threads on gpt-6.1-sol resumed after several idle gaps (for example 10, 25, 40, 60 min), n ≥ 10 resumes per gap, under 15 requests per minute: does the cached share drop reliably past some gap, beyond P4's routing swings? | Whether A3 should give Codex a lifetime at all | ~40 small turns over several hours |
| Claude `PreCompact` exit 2 on a worker with auto-compaction on and oversized results: is the proactive compaction skipped, and what happens at the hard limit? | A6(d) (replaces the first pass's A6 probe) | ~5 calls |
| Claude `Stop` hook returning `decision: "block"` once: does the worker continue with the reason, and does `stop_hook_active` arrive on the second stop? | A5(e) | 2 calls |
| Two conversations' first workers in different start folders, with and without `--exclude-dynamic-system-prompt-sections`, and two workers of one conversation with different `--add-dir` worktrees | B4 | 6 small calls |
| Codex effort switch over the app-server, n ≥ 10 alternating turns per condition, one thread, under 15 requests per minute, inside 30 minutes; does Codex send `configuration_update`? | B3 for Codex | ~20 small turns |
| Codex `thread/inject_items` between turns: is the next turn's prefix cached? | A cheaper path for `[settings]` notes and steers on Codex | ~6 turns |
| Does the built-in LSP tool run in a `-p` worker session, which config turns it on in 2.1.300, and how many tokens does it add to the start? | B2 route (a) | ~3 calls |
| Does Claude Code clear old tool results by itself (the binary carries `clear_tool_uses_20250919`)? Read usage before and after 30 large tool results in a `-p` session | Whether in-CLI masking already happens to workers | ~35 calls |

Dropped: "Claude stream-json for a Bash call printing 200 KB: full text or preview?", answered by the docs (PROBES,
second pass).

## 6. Needs your decision

These would change something already settled, spend money outside the subscription, or spend tokens on
experiments. They are **not** part of the compliant plan above.

1. **Paging decision bodies out of rebirth briefings (changes SESSION-FIXES Q5).** Over days, the ledger of whole
   decision bodies can grow large (WEAK-SPOTS C6: up to 10,000 decisions, no bound against the window). A change to
   Q5 could carry active decisions whole and page older, superseded or narrow-scope reasons on demand by id.
   **My recommendation:** not now. Build A7's index and P0.1's measurement first, and decide when a real session
   shows the ledger's size in the briefing. The second pass found nothing that argues otherwise.
2. **A10: earlier thread rebirth on 1M windows (changes Q5's 400k/450k).** This is an experiment that spends
   tokens on repeated V1/V2 runs. **My recommendation:** run it once P0.1, V2 and A11 exist (A11 first, because
   every rebirth today also pays a full-thread fork write), since the context-rot evidence is strong but nothing
   shows how much it matters for Brigadier's thread.
3. **Claude fast mode for the thread (money outside the subscription).** Opus 5.5 "up to 2.5x faster" at the same
   quality, paid from usage credits at $8/$40 per MTok, not from the subscription; in `-p` it must be on from the
   session's start (`--settings '{"fastMode": true}'`), because turning it on later pays the whole context once at
   the uncached fast price. It could be a separately reported V3 arm for interactive small asks. **My
   recommendation:** not now. B1 and A14 cut small-ask latency for free; revisit if V3 still shows slow answers.
4. **Codex `thread/goal/set` with a `tokenBudget` (a budget, against the no-caps rule).** **My recommendation:** no.
   Nothing in either pass shows a budget saving tokens at equal completed-task quality.
5. **The 3,600-byte worker report limit** (the ≈800-token report design in `docs/PLAN.md`). A13 shows `learned` to
   the thread within today's limit. Raising the limit (w05 suggests 8–12k bytes, with artifacts beyond) would let a
   scout's findings and an implementer's failing checks reach the thread's judgement directly. **My
   recommendation:** keep the limit until V3's scout questions and V2 show whether findings are being lost into
   artifacts the thread doesn't open; then decide with the numbers.
6. **Which items to build, and in what order.** §4 is my recommendation.
7. **"Worked for" counting the user's time on question cards (changes THREAD-PARITY-PLAN's card-wait rule,
   `docs/THREAD-PARITY-PLAN.md:197-202`).** In the forensics session the timer showed 28 m 56 s for about 11 minutes
   of work: 17.5 minutes were the user reading two cards (WEAK-SPOTS U7). Either stop the clock on question cards,
   as the merge card already does, or show both ("Worked 11m · waited for you 18m"). **My recommendation:** show
   both: the wall time stays visible and nobody reads their own reading time as slowness.
8. **"Merge and keep working" (changes THREAD-PLAN Q9: after a merge the worktree and branch are removed).** In the
   earlier session the user describes (01a1366e), the session worktree went 15 s after their own merge click, by
   design; consent can also be given in words. The user reads it as the thread deleting its workspace. (Task
   worktrees go at every task's end, also by design, which F1 and U5 make visible rather than change.) F3 makes the card say so. A second option would keep the worktree after the merge, its branch moved to
   the new base tip, with closing as a separate step. **My recommendation:** yes, as the card's second option, after
   F3 ships; the fresh-start path stays for "Merge and close".
9. **Lazy tool groups behind tool search (reverses `533bac83`, which took tool search out of the thread so nothing
   waits behind a search).** Rare groups (overnight, CI, previews, pages, peers, the planning and approval tools
   beyond `ask_user`/`note_for_user`, `suggest_full_access`, `record_findings`) are about 20–25 kB, ≈6–7k tokens on
   every thread call, ≈60–70k on the bench's small task (≈15% of 429k). The cost is one extra search hop when a rare
   tool is needed, and the tool set must stay fixed at session start for the cache. **My recommendation:** a bounded
   probe first (cached prefix stable with deferred schemas; how often V3 and V1 runs pay the hop), after B8 (a)–(c),
   then decide with the numbers.

## 7. Not proposed, and why

- Caps, budgets, early compaction and context caps: the user rejected them. A10 is a fresh session with a lossless
  carry-over, and only as a measured experiment. Codex's `tokenBudget` is listed in §6 only because it was raised.
- Letting the CLI compact: the evidence (DIGEST 3) says compaction loses constraints, more so each round.
  Brigadier's rebirth with a verbatim ledger is the better-supported design, and A6 extends it to workers.
- Switching models mid-session for small steps: it breaks the cache (vendor), and the escalation tax is large
  (DIGEST 10). Brigadier keeps the switches it already makes: a fallback reroute to another model after quota,
  sign-in, context-window or repeated errors (`M/fallback.rs`), and a model the user changes. What is not proposed is
  switching per step by guessed difficulty.
- Routing by guessed difficulty: no router beat "always the strongest" on solve rate (Scrouting). B7's verified
  brief is the supported form.
- More agents by default: multi-agent hurts sequential work (DIGEST 9).
- Hook types that call a model (`prompt` and `agent` hooks) for gating: they spend model calls on judgement the
  thread already makes. The engine's hooks stay command hooks: plumbing and cheap checks.
- A JSON-schema final turn for worker reports (`--json-schema`, Codex `outputSchema`): reports are already typed
  `submit_report` calls, validated inside the turn.
- Answering status asks from the board without a model call (raised by w05 for S7): a product change, not a
  saving to measure here. Ask for it if wanted.
