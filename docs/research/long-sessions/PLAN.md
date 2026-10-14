# Improvement plan: long sessions, small tasks, usage and grounding (2026-10-14)

From `DIGEST.md` (evidence), `WEAK-SPOTS.md` (code at `d669bca0`), `PROBES.md` and `BASELINE.md`. Every item keeps
§7's rules: no caps or budgets, nothing dropped silently, lossless, no settings, judged per completed task. No item
needs the Agent SDK, an API key, `--bare` or a request-rewriting proxy. Nothing here is built; each item is a
proposal with how to prove it.

## 0. How every item is validated

A saving counts only at **equal completed-task quality**. Each item names which of these tests it needs.

**V1. Completed-task A/B** (the SESSION-FIXES Q6.7 protocol, tightened):
- **Frozen before either arm runs:** the request text, the base commit, the acceptance checks (hidden from both
  arms), the Brain seed (one copy of the store, copied into each arm's data directory), models, effort, accounts,
  CLI versions and machine load.
- **Independent completeness check:** the frozen checks, then a blind review from the other vendor against the
  whole request (sealed shuffle of labels). An arm that isn't complete voids the pair. The 10-12 rerun showed that
  passing the frozen checks alone isn't enough.
- **Usage counted through settlement:** the thread, every worker, reviews, fix tasks, integrated checks, Brain
  jobs, background jobs and native subagents (`abcd1e95` counts the Agent tool), until the request's last review
  batch and check settle. Reported per provider, raw and without cache reads, with cache writes split by lifetime
  (1 h / 5 min) and by cause (P0.1), plus the quota-window share from `quota_samples` and elapsed time.
- **n ≥ 3 per arm.** One task varies up to 30× in tokens between runs (Bai et al. 2026), and Codex cache hits swing
  about 45% per turn (PROBES P4). A result inside the spread is "no difference".
- API-price weighting is an estimate (neither vendor publishes how subscription limits weigh tokens). Usage that
  isn't recorded stays "unknown" and is never filled in.

**V2. Continuity battery** (for items touching rebirth, hand-off, reroute or restart): one long task where, early
and well outside the verbatim tail, the run plants:
- a user constraint ("never touch `X`");
- a settled decision with its reason;
- three exact details (a commit hash, a path with line number, a number with units);
- a failed check with its error line;
- an approach tried and ruled out.

Debug thresholds (`BRIGADIER_REBIRTH_TOKENS`, `BRIGADIER_WORKER_HANDOFF_TOKENS`) then force **at least two seams of
the kind under test** (size, quota reroute, crash, daemon restart). After the last seam, follow-up asks depend on
each planted item. **Score:** each item recalled exactly or acted on correctly, as a pass or fail per item (target
all pass). Also: no ruled-out approach retried, no constraint broken, and "unknown" instead of a guess when the
item really was lost.

**V3. Small-ask set:** 12 frozen asks with known answers:
- 4 code questions ("where is X set?");
- 4 tiny edits;
- 2 status questions;
- 2 "explain this error".

Each runs on a cold and a warm session, with a seeded and an empty Brain. **Measured:** model calls, worker starts,
time to the final answer, correctness (exact), and usage through settlement.

## 1. Enablers first

### P0.1 Know where cache writes come from (§7 next 1, WEAK-SPOTS N1)
- **What:**
  - Record each cache write with its lifetime: parse `cache_creation.ephemeral_1h_input_tokens` and
    `ephemeral_5m_input_tokens` in `P/claude/parse.rs`; Codex `cacheWriteInputTokens` from
    `thread/tokenUsage/updated`.
  - Tag each write with a cause Brigadier already knows: fresh start, resume after close, cold resume, rebirth,
    hand-off, reroute, effort change, MCP or tool-set change (computer-use server on or off), CLI upgrade, and
    "unexplained".
  - A per-request report in the Inspector, counted through settlement.
- **Why:** cache writes are about 25% of usage (§7), and nothing can be tuned without knowing their cause.
  Probe P1 confirmed Claude `-p` writes at 1 h, the 2× price.
- **CLI:**
  - Claude 2.1.299 stream-json `usage.cache_creation.*`, seen in the probes (CLI✓).
  - Claude SessionStart hook fields `prompt_cache_likely_expired` and `estimated_cache_write_usd` (CLI?). Bounded
    probe: one resume after 61 min with `--include-hook-events`, reading the hook input.
  - Codex `ThreadTokenUsage.cacheWriteInputTokens` (CLI✓, schema).
- **Effect:** none directly; it enables P0.2 and every usage claim below.
- **Measure:** "unexplained" writes are under 10% of write tokens over a week of the user's use.
- **Risk:** low (read-only plumbing). **Size:** M.

### P0.2 Build V2 and V3 into `tools/ab`
- **What:** the continuity battery and the small-ask set as frozen fixtures, with scoring scripts, beside
  `worker_tokens.py`.
- **Why:** every item below needs one or the other. The gap noted in the research (no study measures exact
  identifiers or dead ends surviving a seam) is exactly what V2 measures.
- **Risk:** low. **Size:** M.

## 2. Long multi-hour sessions

Ordered by expected value against cost. Each names its weak spots and evidence.

| # | Item | Weak spots | Evidence | Expected effect | Validate | Risk | Size |
|---|---|---|---|---|---|---|---|
| A1 | **Pinned constraints and negative knowledge across every seam.** The rebirth briefing and the worker hand-off carry the current request's user constraints **word for word**, extracted from the user's messages by code, not summarised. The thread's rebirth note gains the worker note's headings: ruled out and why; checks that failed, with their error lines; exact identifiers in use. Every cut in a briefing names the message id and the omitted range, so `search_transcript` can fetch it | C4, C5 | DIGEST 3 (verbatim constraint block: > 90% retention against 17–53%; compounding loss per round) | Fewer broken constraints and repeated dead ends after rebirth; small usage cost (a few hundred tokens per briefing) | V2 (size seams ×2), then V1 on one long task | Low: adds text, removes nothing. A huge pasted spec must be pinned by reference, not inline | S–M |
| A2 | **One successor contract for every seam.** Quota/error reroute, crash and restart successors get what a size hand-off gets: transcript, note (or, with no note, a generated "state of work" from the transcript, marked as unchecked), `checks.md`, decisions, and the full untrimmed tool outputs. A successor across providers gets the same files. Restart with missing CLI files uses the fresh-successor path instead of stopping | C1, C3, C9, C10 | DIGEST 2 (fresh consolidated brief recovers 95%); Handoff Tax (a short verified brief beats the whole trajectory) | Reroutes stop redoing work; read-only research survives a quota hit | V2 with forced reroute and restart seams | Med: shared code across four paths; must not slow the size hand-off | L |
| A3 | **Survive a daemon restart.** Persist the cold-cache checkpoint when it's written and reload it at start. Recover Blocked and Paused workers by their wait reason instead of stopping them. Reconcile an interrupted landing against the target branch's ancestry before saying "nothing landed" | C7, C2, w02 #3 | Baseline (cold rebirth exists; restart loses it) | One avoided cold resume of a 400k thread saves about 400k of 1 h writes per restart; no lost waiting workers overnight | V2 with a daemon kill at each wait point; flow tests | Low/Med | M |
| A4 | **Only verified facts become Brain facts.** `learned` items and handoff-derived lines are stored as *claims* with their source (task, message id). They become facts when the landing's integrated checks pass and its review batch settles without a finding against them, and they are superseded when a finding contradicts them. Brain-first briefs label claims as such | G1 | DIGEST 6, 7 (stored own errors spread; ~10% gain from evaluator-approved memories only) | Fewer wrong "conventions" propagated; can't be measured as usage | V2 variant: a seeded false `learned` claim must not reach a later worker as fact | Low | M |
| A5 | **Grounding gates.** Overnight acceptance needs per-criterion evidence ([met] with a check id or output reference) plus a settled review. A verifier gets the lead's whole report (risks, open questions, artifacts). A worker question that times out may proceed only on a recorded safe default; consent and product choices stay waiting. `run_check` reruns integration, smoke and live commands unless they declare their inputs | G2, G4, G5, G3 | DIGEST 6 (false success claims, judges of transcripts unreliable) | Fewer "verified" results that aren't; small extra runs for integration checks | V1 on an overnight-style task with an uncovered criterion; flow tests | Med: more waiting overnight. The safe default is the answer | M |
| A6 | **Headroom inside one turn.** Check context after every model response. Past the threshold during a long turn, steer the thread (as workers already are) to end at a safe point. A compaction that happens anyway triggers a rebuild from durable state, not just a log line. Cap the worker guard below the window | L1, L2 | Codex compacts at its own ceiling; baseline | Prevents rare uncontrolled compactions (mostly Codex and 200k windows) | Fault test with large tool results; V2 on a 200k-window model | Low | M |
| A7 | **A bounded, current decision ledger.** An index of active decisions with scope and supersession, all binding ones whole, older reasons paged on demand by id. Measure the whole briefing before launch and keep working headroom | C6 | ACE (itemised, incremental beats rewrite) | Keeps rebirth cheap on multi-day sessions | V2 with 300 seeded decisions, 50 superseded | Med: a decision wrongly marked superseded is a loss, so supersession is explicit only | M |
| A8 | **Lossless worker output, the cheap way.** Claude Code already stores large Bash output in its session folder and shows the model a ~2 KB preview, so the model-side saving exists. The gap is Brigadier's copy: its transcript clips tool output at 8 KiB, and the CLI's file goes when the session files go. So at hand-off, reroute and review-hold, copy the session's `tool-results/` files into the blob store with the transcript, and point at them. Add the thread's output hook to workers only if V1 shows the CLI's own preview leaves real cost (for example failing commands, which reportedly get an excerpt and no file) | C9 (+ §7 next 2) | DIGEST 4 (trim at the source, keep the full output); r3 (the CLI's own persist-to-file) | Losslessness for successors at almost no cost; usage effect likely small | V2 with a seam after a large failing test run; V1 only if the hook is added | Low | S |
| A9 | **Supervision.** A watchdog for the thread's own turn (accepted and silent). A worker watchdog that tells progress from chatter, with command-specific deadlines (foreground servers) | W1, W2 | Audit | Fewer stalled overnight hours | Fault injection (silent provider) | Low | M |
| A10 | **Earlier rebirth on big windows: an experiment, not a change.** Run V1 and V2 with the thread's rebirth at 200k and 300k against today's 400k on 1M windows | Baseline Q5 | DIGEST 1, 2 (quality falls with length well before the limit; fresh consolidated prompt recovers it) | Unknown: possibly better late-session quality, at the cost of more rebirths | V1 (usage) + V2 (continuity) + a late-session quality probe: the same follow-up asked at 100k and at 350k | Med: each rebirth costs a briefing write; §7 holds that only completed-task results decide | S + runs |

## 3. Small and short tasks

| # | Item | Weak spots | Evidence | Expected effect | Validate | Risk | Size |
|---|---|---|---|---|---|---|---|
| B1 | **Answer small questions directly.** The thread answers a code question itself when the Brain, `code_search`/`code_refs` and up to about 3 precise reads answer it, and quotes the file:line it relied on. Scouts are for exploration, many files, or the web. A one-line edit stays a thread self-edit. Prompt change in `M/prompts.rs:101-103` and the Brain-miss hint at `M/brains.rs:779` | S1 | DIGEST 9 (delegation costs several times the tokens and hurts sequential work); vendor "never speculate about code you have not opened" | Fewer model hops and no 15–30k worker start for small asks; faster answers | V3: calls, worker starts, latency, exact correctness | Low: the thread's context grows slightly. The tiny-edit rule stays | S |
| B2 | **Precise reads** (§7 next 3). Two routes, measured against each other: (a) enable Claude Code's built-in LSP tool for workers and the thread where the project has a language server, with the instruction to trust its results without re-reading the file; (b) index tools `code_outline` (a file's symbols with line ranges), `read_symbol` (one definition with its doc comment) and a batched line-range read, all with file:line headers. (b) works for Codex too | N2 | §7 (file reads are the biggest tool-result cost); DIGEST 4 (SWE-Pruner 23–54%); CircleCI LSP vs grep: 31–33% fewer output tokens and no missed references | Fewer whole-file reads; fewer missed call sites for weaker models | V1 on two implement tasks + V3; count Read/`sed` bytes and calls per task. Bounded probe first: does the LSP tool run in a `-p` session, and what does it cost at start? | Low: a read that misses context costs another call, so calls are measured, not just bytes. Enabling a tool mid-session breaks the cache, so set it only at session start | M |
| B3 | **Effort by model and step.** Floors per model family from measured data: today's evidence says Sonnet implement at high, Opus 5.5 implement possibly at medium, scouts low. The thread names the effort for the job; the router applies the model's floor. On Claude 5.5 models, change effort between steps of one worker (P2: a fixed ~9.4k rewrite, the history stays cached) | E1 (+ §7 next 4) | DIGEST 8; PROBES P2 | Lower thinking cost on Opus where quality holds; fewer failed Sonnet low-effort tasks | V1, n ≥ 3, per model × effort, on frozen implement and scout tasks. Lower effort is adopted only where completeness holds | Med: quality loss if the evidence doesn't transfer. That's why it is measured per model | S–M |
| B4 | **Share the fixed start across conversations.** Probe `--exclude-dynamic-system-prompt-sections` for workers and the thread. If a new conversation's first worker then reads its 15–25k start from the cache within the hour, adopt it (it also removes the need for the shared start folder). Codex: unchanged (§7: a leaner Codex prompt lost the cache) | — | `claude --help` 2.1.299; PROBES P3 | Saves a ~20–25k 1 h write (≈ 2× that in input-equivalents) per new conversation started within the hour of another | Bounded probe: two conversations, two worktrees, with and without the flag; then V1 | Low: one flag; drop it if the probe shows no read | S |
| B5 | **Load only what applies.** Repository instructions: root and ancestor rules always, subtree rules when files there are touched, and a list of the rules left out. Context pack: reads relevant to this task (named files and symbols), not the whole conversation's | S3, S4 | Vendor (keep instruction files short, load detail on demand); DIGEST 1 (focused prompts beat long ones) | Smaller worker starts, fewer unrelated files | V1 on a task in a sub-package of a monorepo; first-call context and completed-task usage | Med: a rule loaded too late is a missed constraint. Ancestor rules stay pinned | M |
| B6 | **Prewarm only when a writing task is likely** (the thread called `delegate_task` once in the request, or the request names a change) | S5 | Audit | Less I/O on question-only sessions | V3 timing on a busy machine | Low | S |
| B7 | **A verified scout brief.** When a scout precedes an implementer, its report includes ranked files with line ranges, a reproduction command and dead ends tried. Brigadier runs the reproduction with `run_check` before the brief goes out, and drops claims that fail | — | DIGEST 10 (Scrouting: verified hand-off, ~1/5 the cost per solve) | Cheaper implementers, fewer repeated explorations | V1 on two bug-fix tasks with and without | Low | M |

## 4. Order

1. **P0.1, P0.2.** Nothing else can be proven without them.
2. **A1, A3, B1:** cheap and safe, each with a clear test.
3. **A2, A4, A5:** the larger continuity and grounding fixes.
4. **B2, B3, B4:** the usage savers, each measured with V1 at n ≥ 3.
5. **A6, A7, A8, A9, B5, B6, B7.**
6. **A10** (an experiment), only after P0.1 and V2 exist.

## 5. Bounded probes the docs can't settle

| Probe | Settles | Cost |
|---|---|---|
| Claude resume after > 60 min with `--include-hook-events`: does SessionStart carry `prompt_cache_likely_expired` and `estimated_cache_write_usd` in stream-json? | P0.1 cause tags | 2 small calls + a wait |
| Two conversations' first workers in different start folders, with and without `--exclude-dynamic-system-prompt-sections` | B4 | 4 small calls |
| Codex effort switch over the app-server, n ≥ 10 alternating turns per condition | B3 for Codex | ~20 small turns |
| Codex `thread/inject_items` between turns: is the next turn's prefix cached? | A cheaper path for `[settings]` notes and steers on Codex | ~6 turns |
| Claude worker PostToolUse `updatedToolOutput` on Bash: is the trimmed text what the next request carries, with the cache warm? | A8 | ~4 calls |
| Does the built-in LSP tool run in a `-p` worker session (which config turns it on in 2.1.299), and how many tokens does it add to the start? | B2 route (a) | ~3 calls |
| Does Claude Code clear old tool results by itself (the binary carries `clear_tool_uses_20250919`)? Read `/context` or usage before and after 30 large tool results in a `-p` session | Whether in-CLI masking already happens to workers | ~35 calls |

## 6. Not proposed, and why

- Caps, budgets, early compaction and context caps: rejected by the user. A10 is a fresh session with a lossless
  carry-over, and only as a measured experiment.
- Letting the CLI compact: the evidence (DIGEST 3) says compaction loses constraints, more so each round.
  Brigadier's rebirth with a verbatim ledger is the better-supported design.
- Model switching mid-session for small steps: it breaks the cache (vendor) and the escalation tax is large (DIGEST
  10). Brigadier switches models only between tasks.
- Routing by guessed difficulty: no router beat "always the strongest" on solve rate (Scrouting). B7's verified
  brief is the supported form.
- More agents by default: multi-agent hurts sequential work (DIGEST 9).
