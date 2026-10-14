# Improvement plan: long sessions, small tasks, usage and grounding (2026-10-14, revised after Codex review)

This plan draws on `DIGEST.md` (the evidence), `WEAK-SPOTS.md` (the code at `d669bca0`), `PROBES.md` and
`BASELINE.md`.

Every item keeps §7's rules: no caps or budgets, nothing dropped silently, lossless, no settings, judged per
completed task. Every item also keeps the settled decisions that superseded parts of §7. These are SESSION-FIXES
Q5 (every decision body whole in a rebirth briefing), §11 (integrated checks) and THREAD-PLAN (reviews can be turned
off, and then nothing waits for one). No item needs the Agent SDK, an API key, `--bare` or a request-rewriting proxy.

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
  - What counts: the thread, every worker, reviews, fix tasks, integrated checks, Brain jobs, background jobs and
    native subagents (`abcd1e95` counts the Agent tool), until the request's last review batch and check settle.
  - How it is reported: per provider, raw and without cache reads, with cache writes split by lifetime (1 h / 5 min)
    and by cause (P0.1), plus the quota-window share from `quota_samples` and elapsed time.
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
- an approach tried and ruled out.

Each run forces **at least two seams of the kind under test**. Size seams come from the debug thresholds
(`BRIGADIER_REBIRTH_TOKENS`, `BRIGADIER_WORKER_HANDOFF_TOKENS`), which change sizes only. The other seams need their
own injection: a quota or error reroute from an injected provider error, a crash by killing the CLI process, and a
daemon restart by stopping and restarting the debug daemon at a chosen wait point. Later requests then depend on
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
  - Record each cache write with its lifetime: parse `cache_creation.ephemeral_1h_input_tokens` and
    `ephemeral_5m_input_tokens` in `P/claude/parse.rs`, and Codex `cacheWriteInputTokens` from
    `thread/tokenUsage/updated`.
  - Tag each write with a cause Brigadier already knows: fresh start, resume after close, cold resume, rebirth,
    hand-off, reroute, effort change, MCP or tool-set change (the computer-use server coming or going), CLI upgrade,
    or "unexplained".
  - A per-request report in the Inspector, counted through settlement.
- **Why:**
  - Cache writes are about 25% of usage (§7), and nothing can be tuned without knowing their cause.
  - Probe P1 confirmed that Claude `-p` writes at 1 h, the 2× price.
  - Practitioners and the Claude Code team treat cache hit rate as the key health metric (DIGEST §4).
- **CLI:**
  - Claude 2.1.299 stream-json `usage.cache_creation.*` was seen in the probes (CLI✓).
  - Claude's SessionStart hook fields `prompt_cache_likely_expired` and `estimated_cache_write_usd` are documented
    (CLI?). Bounded probe: one resume after 61 min with `--include-hook-events`.
  - Codex `ThreadTokenUsage.cacheWriteInputTokens` is in the schema (CLI✓).
- **Effect:** none directly. It enables every usage claim below.
- **Measure:** "unexplained" writes are under 10% of write tokens over a week of the user's own use.
- **Risk:** low (read-only plumbing). **Size:** M.

### P0.2 Build V2 and V3 into `tools/ab`
- **What:** the multi-request continuity battery and the small-ask set as frozen fixtures with scoring scripts,
  beside `worker_tokens.py`. V1's cold/warm regime, alternating order and completion-rate accounting go into the
  same harness.
- **Why:** every item below needs one or the other. The research has no study of exact identifiers or dead ends
  surviving a seam, and V2 measures exactly that.
- **Effect:** none directly; V1–V3 run on it. **Measure:** on an unchanged build, three reruns of each fixture give
  the same pass or fail per planted item.
- **Risk:** low. **Size:** M.

### P0.3 Durable raw output and a transcript export that can't lie (the dependency of A2 and A8)
- **What:**
  - **Before** the 8 KiB normalisation clip (`OUTPUT_CLIP` in `P/claude/parse.rs:18` and `P/codex/parse.rs:17`),
    store each tool result's full available text in the blob store under an id, recorded on the event, for both
    providers and for every tool, not just Bash. The clipped event then carries a reference to the stored copy.
  - Replace `worker_transcript()` (`M/worker_handoff.rs:445-478`, which treats a failed page read as an empty page
    and rebuilds the whole history in memory) with an export that streams pages to a file. It propagates every
    read or decode failure, marks the export **incomplete** with the missing ranges, and is never called "full"
    unless it is.
  - Fault tests: a page read error, a decode error, and an export interrupted mid-way. In each case the result is
    marked incomplete, and no successor is told it has the whole transcript.
- **What the CLIs don't give Brigadier, named rather than assumed:**
  - **Claude:** a failing command's output reaches hooks only as the CLI's excerpt (`PostToolUseFailure`; already
    noted at `M/tool_output.rs`). Very large Bash output is kept in the CLI's own `tool-results/<id>.txt` and the
    model sees a ~2 KB preview (DIGEST §4). Brigadier sees what stream-json sends; whether that is the full text or
    the preview must be probed.
  - **Codex:** command output in history is bounded by `tool_output_token_limit`. Whether the app-server's completed
    item carries the full aggregated output must be probed.
  - What can't be had is recorded on the event as "CLI gave an excerpt only", so no successor treats it as
    complete.
- **Why:** WEAK-SPOTS C9, C10. §7's "lossless" rule can't be claimed for successors until this exists.
- **Validate:** the fault tests; V2 with a seam right after a large failing test run, on both providers.
- **Effect:** none on usage. Successors are told exactly what they have and what they lack.
- **Risk:** disk use. The blob store has no size bound of its own (`crates/store/src/blob.rs:129-145`), and the 64 MiB
  limit on command output is enforced by the readers, not the store, so stored outputs grow until their session is
  deleted. Disk growth is measured in V1, and a failed write marks the export incomplete. **Size:** M.

## 2. Long multi-hour sessions

Ordered by expected value against cost. Each item names the weak spots it addresses and the evidence for it.

| # | Item | Weak spots | Evidence | Expected effect | Validate | Risk | Size |
|---|---|---|---|---|---|---|---|
| A1 | **Standing constraints and negative knowledge across every seam and every request. R**<br>(a) The user's standing constraints, with their corrections and exceptions, are kept as **verbatim source spans**: the complete user message, or the exact quoted span with its message id. Each has a durable scope (this request, this session, this project) and **explicit supersession** (a later span marks which earlier one it replaces). The thread records them with `@remember` and a new `constraint` kind, pointing at the message; Brigadier does not extract free text "by code". Every rebirth briefing and worker brief carries every active constraint in that scope, word for word.<br>(b) The thread's rebirth note gains the worker note's headings: ruled out and why; checks that failed, with their error lines; exact identifiers in use.<br>(c) Every cut in a briefing names the message id and the omitted range, so `search_transcript` can fetch it | C4, C5 | DIGEST 3 (constraints kept apart from the summary: over 90% retained against 17–53%, and violations back to 0%; loss compounds per round); practice: the `varchar(128)` case | Fewer broken constraints and repeated dead ends after rebirths and across requests. Briefing size grows with the number of active constraints: measured in V2, not assumed small | V2 (multi-request, size seams ×2) and R; then V1 on one long task | Med: a constraint wrongly marked superseded is a loss, so supersession is never inferred. A long pasted spec is pinned by reference to its stored message, not inline | M |
| A2 | **One successor contract for every seam.** Needs P0.3 first. Quota/error reroute, crash and restart successors get what a size hand-off gets: the streamed transcript export (P0.3) with its completeness mark, the note, `checks.md`, the decisions, and references to the stored raw outputs. With no note, the successor gets a "state of work" built from the transcript and marked unchecked. A successor across providers gets the same files. A restart whose CLI files are missing uses the fresh-successor path instead of stopping | C1, C3, C10 | DIGEST 2 (a fresh consolidated brief recovers 95%); Handoff Tax (a short verified brief beats the whole trajectory) | Reroutes stop redoing work; read-only research survives a quota hit. Lossless only as far as P0.3 records it, and the successor is told where it isn't | V2 with forced reroute, crash and restart seams, on both providers | Med: shared code across four paths, which must not slow the size hand-off | L |
| A3 | **Survive a daemon restart.** Persist the cold-cache checkpoint when it is written, and reload it at start. Recover Blocked and Paused workers by their wait reason instead of stopping them. Reconcile an interrupted landing against the target branch's ancestry before saying "nothing landed" | C7, C2, w02 #3 | BASELINE (cold rebirth exists; a restart loses it) | One avoided cold resume of a 400k thread saves about 400k of 1 h cache writes per restart. No waiting workers lost overnight | V2 with a daemon kill at each wait point; flow tests | Low/Med | M |
| A4 | **Only evidenced facts become Brain facts. R**<br>(a) Worker-authored items (`report.learned` → Convention nodes at `M/brains.rs:1200`, `report.decisions` → Decision nodes at `M/brains.rs:1185`) are stored as **claims** with their source (task, message id). The decision lines of the thread's own rebirth note (`M/brains.rs:1382`: orchestrator origin, no task) are claims too, with the session and the note's generation as their source, unless they point at a user decision.<br>(b) **User-authorised decisions** (the user's words, card answers, outline go-aheads) stay decisions as today: they are authority, not empirical claims.<br>(c) An empirical claim becomes a fact only with **evidence for that claim**: a file:line it cites that still says so, a check id whose output shows it, or a review that examined it. Passing integrated checks or a clean review of the landing is **not** evidence for an unrelated claim.<br>(d) A claim without evidence stays "unverified" and is shown as such in briefs and Brain answers. A finding that contradicts it supersedes it.<br>(e) With reviews off, nothing waits for a review; claims simply stay claims unless other evidence supports them | G1 | DIGEST 6, 7 (stored own errors spread; 15–26 points gained by adding only evaluator-approved memories); practice: memory files as attack surface | Fewer wrong "conventions" passed on to later workers; not measurable as usage | V2 variant: a seeded **false** `learned` claim that survives passing checks and a clean review must still reach a later worker labelled "unverified", never as fact; and R | Low/Med: more claims shown as unverified, and the brief must stay readable | M |
| A5 | **Grounding gates.**<br>(a) Overnight acceptance needs per-criterion evidence: [met] with a check id or an output reference, bound to the accepted tip. **When reviews are on**, a settled review is also required. **When reviews are off**, acceptance keeps the real criterion checks and the morning report says plainly "no review was run".<br>(b) A verifier gets the lead's whole report: risks, open questions and artifacts.<br>(c) A worker question that times out may proceed only on a recorded safe default. Consent and product choices stay waiting.<br>(d) `run_check` reruns integration, smoke and live commands unless they declare their inputs | G2, G4, G5, G3 | DIGEST 6 (false success claims; LLM judges of transcripts are unreliable) | Fewer "verified" results that aren't; a few extra runs for integration checks | V1 on an overnight-style task with a criterion the suite doesn't cover, with reviews on and off; flow tests | Med: more waiting overnight, though the safe default is an answer | M |
| A6 | **Workers never reach their CLI's own compaction.**<br>(a) Launch Claude workers with `auto_compact: false` (`M/workers.rs:1870`), as the thread already does. Codex workers get `model_auto_compact_token_limit` set far above the window in the same way. Codex still clamps it to about 90% of the window, so for Codex it is (b) and (c) that keep a worker from compacting.<br>(b) The hand-off guard stays **safely below each provider's real compaction ceiling**: Codex about 90% of its window, Claude's auto-compact point when on. It is capped so `start + at/2` can't exceed it.<br>(c) Context is checked after **every model response**, not only at turn end. Past the threshold mid-turn, the worker or thread is steered to stop at a safe point. If the CLI compacts anyway, Brigadier rebuilds from durable state (A2) and doesn't just log it. A steer is not proof that compaction was prevented | L1, L2 | Codex compacts at its own ceiling; BASELINE (workers launch with compaction on) | No uncontrolled compactions in workers or threads; continuity kept by Brigadier's hand-off | Oversized tool results on both providers, on a 200k-window model, showing no `CompactionStarted` and a hand-off instead; V2 | Med: with compaction off, a worker that ignores the steer can hit "prompt is too long". So the hard stop and rebuild in (c) are part of the item | M |
| A7 | **A decision index, additive only.** Keep **every decision body in full** in every rebirth briefing, as SESSION-FIXES Q5 settles and `M/rebirth.rs:1172-1223` implements. Add a compact index on top (id, scope, status, supersedes) so the reborn thread can see which decisions are active and which were superseded. Measure the whole briefing before launch and report it in the Inspector. Paging decision bodies out is **not** proposed here; see §6 | C6 | ACE (itemised, incremental beats rewrite) | Easier navigation of a large ledger, at a small extra size. It does **not** reduce the ledger's cost on multi-day sessions | V2 with 300 seeded decisions, 50 superseded: all bodies present, the index correct | Low | S |
| A8 | **Workers' long command output, the cheap way.** Needs P0.3. Claude Code already stores large Bash output in its session folder and shows the model a ~2 KB preview, so the model-side saving exists. A8 copies those `tool-results/` files into the blob store with the transcript at hand-off, reroute and review-hold. Add the thread's output hook to workers only if V1 shows real cost left (for example failing commands, which reportedly get an excerpt and no file) | C9 (+ §7 next 2) | DIGEST 4 (trim at the source, keep the full output); practice (the CLI's own persist-to-file) | Losslessness for successors at almost no cost; the usage effect is likely small | V2 with a seam after a large failing test run; V1 only if the hook is added | Low | S |
| A9 | **Supervision.** A watchdog for the thread's own turn (a CLI that accepted the turn and went silent). A worker watchdog that tells progress from chatter, with command-specific deadlines (for example foreground servers) | W1, W2 | Audit | Fewer stalled overnight hours | Fault injection (a silent provider) | Low | M |
| A10 | **Earlier rebirth on big windows: an experiment, not a change.** V1 and V2 with the thread's rebirth at 200k and 300k against today's 400k on 1M windows | Baseline Q5 | DIGEST 1, 2 (quality falls with length well before the limit; a fresh consolidated prompt recovers it) | Unknown: possibly better late-session quality, at the cost of more rebirths | V1 (usage) + V2 (continuity) + a late-session quality probe (the same follow-up asked at 100k and at 350k) | Med: each rebirth costs a briefing write, and changing Q5's thresholds is the user's call (§6) | S + runs |

## 3. Small and short tasks

| # | Item | Weak spots | Evidence | Expected effect | Validate | Risk | Size |
|---|---|---|---|---|---|---|---|
| B1 | **Answer small questions directly. R.** The thread answers a code question itself when the Brain, `code_search`/`code_refs` and up to about 3 precise reads answer it, and it quotes the file:line it relied on. Scouts are for exploration, many files, or the web. A one-line edit stays a thread self-edit. Prompt change in `M/prompts.rs:101-103`, plus the Brain-miss hint at `M/brains.rs:779` | S1 | DIGEST 9 (delegation costs several times the tokens and hurts sequential work); practice (a new session's preamble ~33k, a worktree 5–15 s); vendor ("never speculate about code you have not opened") | Fewer model hops and no 15–30k worker start for small asks; faster answers | V3 (calls, worker starts, latency, exact correctness) and R | Low: the thread's context grows slightly; the tiny-edit rule stays | S |
| B2 | **Precise reads** (§7 next 3). **R.** Two routes, measured against each other:<br>(a) Enable Claude Code's built-in LSP tool for workers and the thread where the project has a language server, telling the model to trust its results without re-reading the file.<br>(b) Index tools `code_outline` (a file's symbols with line ranges), `read_symbol` (one definition with its doc comment) and a batched line-range read, all with file:line headers.<br>**Freshness:** reads come from the **caller's effective worktree**, not the project's indexed base (`task_index()` at `M/brains.rs:1021` picks the index by project today). Each read checks the indexed file's hash against the worktree file and reparses it when stale, so a worker's own edits, rebases and uncommitted changes are read correctly. (b) works for Codex too | N2 | §7 (file reads are the biggest tool-result cost); DIGEST 4 (SWE-Pruner 23–54%); CircleCI LSP vs grep: 31–33% fewer output tokens and no missed references | Fewer whole-file reads, and fewer missed call sites for weaker models | Bounded probe first (does the LSP tool run in a `-p` session, and what does it add at start?). Then V1 on two implement tasks + V3, counting Read/`sed` bytes and calls per task. Freshness tests: a definition changed and left uncommitted in the worktree, a rebased branch, and a file another worker changed on main | Low/Med: a read that misses context costs another call, so calls are measured, not just bytes. Turning a tool on mid-session breaks the cache, so it is set only at session start | M |
| B3 | **Effort by model and step.** Floors per model family come from measured data. Today's evidence says Sonnet implement at high, Opus 5.5 implement possibly at medium, scouts at low. The thread names the effort for the job, and the router applies the model's floor. On Claude 5.5 models, effort can change between steps of one worker (P2: a fixed ~9.4k rewrite, with the history still cached) | E1 (+ §7 next 4) | DIGEST 8; PROBES P2 | Lower thinking cost on Opus where quality holds; fewer failed Sonnet low-effort tasks | V1 per model × effort on frozen implement and scout tasks, with completion rates. A lower effort is adopted only where completion holds | Med: quality loss if the evidence doesn't transfer, which is why it is measured per model | S–M |
| B4 | **Share the fixed start across conversations.** Probe `--exclude-dynamic-system-prompt-sections` for workers and the thread. If a new conversation's first worker then reads its 15–25k start from the cache within the hour, adopt the flag (it also removes the need for the shared start folder). Codex stays as it is (§7: a leaner Codex prompt lost the cache) | — | `claude --help` 2.1.299; PROBES P3 | Saves a ~20–25k 1 h write (≈ 2× that in input-equivalents) per new conversation started within an hour of another | Bounded probe: two conversations in two worktrees, with and without the flag. Then V1 **with the cold and warm regimes reported separately**, since this item is exactly about warmth | Low: one flag, dropped if the probe shows no cache read | S |
| B5 | **Load only what applies. R.** Repository instructions: root and ancestor rules always, subtree rules when files there are touched, and a list of the rules left out. Context pack: reads relevant to this task (named files and symbols), not the whole conversation's | S3, S4 | Vendor (keep instruction files short, load detail on demand); practice (a small always-present index beat on-demand skills); DIGEST 1 | Smaller worker starts, fewer unrelated files | V1 on a task in a sub-package of a monorepo, measuring first-call context and completed-task usage; R | Med: a rule loaded too late is a missed constraint, so ancestor rules stay pinned | M |
| B6 | **Prewarm only when a writing task is likely**: the thread called `delegate_task` once in the request, or the request names a change | S5 | Audit | Less I/O on question-only sessions | V3 timing on a busy machine | Low | S |
| B7 | **A verified scout brief. R.** When a scout comes before an implementer, its report includes ranked files with line ranges, a reproduction command and the dead ends it tried. Brigadier runs the reproduction with `run_check` before the brief goes out, and drops claims that fail | — | DIGEST 10 (Scrouting: a verified hand-off at about 1/5 the cost per solve) | Cheaper implementers, fewer repeated explorations | V1 on two bug-fix tasks, with and without | Low | M |

## 4. Order

1. **P0.1, P0.2, P0.3.** Nothing else can be proven, or called lossless, without them.
2. **A1, A3, A6, B1.** Each is cheap or safety-critical, and each has a clear test.
3. **A2** (after P0.3), **A4, A5.** The larger continuity and grounding fixes.
4. **B2, B3, B4.** The usage savers, each measured with V1.
5. **A7, A8, A9, B5, B6, B7.**
6. **A10.** An experiment, only after P0.1 and V2 exist, and only with the user's go-ahead (§6).

## 5. Bounded probes the docs can't settle

| Probe | Settles | Cost |
|---|---|---|
| Claude resume after more than 60 min with `--include-hook-events`: does SessionStart carry `prompt_cache_likely_expired` and `estimated_cache_write_usd` in stream-json? | P0.1 cause tags | 2 small calls + a wait |
| Claude stream-json for a Bash call printing 200 KB (success, then failure): does the tool result carry the full text or the CLI's preview? | P0.3: what Claude exposes | 2 calls |
| Codex app-server `item/completed` for a command printing 200 KB: full aggregated output or bounded by `tool_output_token_limit`? | P0.3: what Codex exposes | 2 turns |
| Two conversations' first workers in different start folders, with and without `--exclude-dynamic-system-prompt-sections` | B4 | 4 small calls |
| Codex effort switch over the app-server, n ≥ 10 alternating turns per condition | B3 for Codex | ~20 small turns |
| Codex `thread/inject_items` between turns: is the next turn's prefix cached? | A cheaper path for `[settings]` notes and steers on Codex | ~6 turns |
| Does the built-in LSP tool run in a `-p` worker session, which config turns it on in 2.1.299, and how many tokens does it add to the start? | B2 route (a) | ~3 calls |
| Claude worker with `auto_compact: false` fed oversized tool results: does it stop with "prompt is too long" or compact anyway, and what does Brigadier see? | A6 | ~5 calls |
| Does Claude Code clear old tool results by itself (the binary carries `clear_tool_uses_20250919`)? Read usage before and after 30 large tool results in a `-p` session | Whether in-CLI masking already happens to workers | ~35 calls |

## 6. Needs your decision

These would change something already settled, or spend tokens on experiments. They are **not** part of the
compliant plan above.

1. **Paging decision bodies out of rebirth briefings (changes SESSION-FIXES Q5).** Over days, the ledger of whole
   decision bodies can grow large (WEAK-SPOTS C6: up to 10,000 decisions, no bound against the window). A change to
   Q5 could carry active decisions whole and page older, superseded or narrow-scope reasons on demand by id.
   **My recommendation:** not now. Build A7's index and P0.1's measurement first, and decide when a real session
   shows the ledger's size in the briefing.
2. **A10: earlier thread rebirth on 1M windows (changes Q5's 400k/450k).** This is an experiment that spends
   tokens on repeated V1/V2 runs. **My recommendation:** run it once P0.1 and V2 exist, because the context-rot
   evidence is strong but nothing shows how much it matters for Brigadier's thread.
3. **Which items to build, and in what order.** §4 is my recommendation.

## 7. Not proposed, and why

- Caps, budgets, early compaction and context caps: the user rejected them. A10 is a fresh session with a lossless
  carry-over, and only as a measured experiment.
- Letting the CLI compact: the evidence (DIGEST 3) says compaction loses constraints, more so each round.
  Brigadier's rebirth with a verbatim ledger is the better-supported design, and A6 extends it to workers.
- Switching models mid-session for small steps: it breaks the cache (vendor), and the escalation tax is large
  (DIGEST 10). Brigadier keeps the switches it already makes: a fallback reroute to another model after quota,
  sign-in, context-window or repeated errors (`M/fallback.rs`), and a model the user changes. What is not proposed is
  switching per step by guessed difficulty.
- Routing by guessed difficulty: no router beat "always the strongest" on solve rate (Scrouting). B7's verified
  brief is the supported form.
- More agents by default: multi-agent hurts sequential work (DIGEST 9).
