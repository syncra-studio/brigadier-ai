# Baseline: what Brigadier does today about long sessions and usage (2026-10-14)

Taken from `docs/PLAN.md` §7, `docs/THREAD-PLAN.md`, `docs/SESSION-FIXES-PLAN.md` (Q5, Q6, §11) and the code at
`d669bca0`. Paths: `M/` = `crates/core/src/manager/`, `P/` = `crates/providers/src/`. Installed CLIs on this machine:
Claude Code 2.1.299, codex-cli 0.162.1, both on subscription sign-in.

## 1. Which decisions supersede §7

| §7 says | Superseded by | Today (code) |
|---|---|---|
| A pure orchestrator that only talks (also PLAN §1, §2 principle 1) | THREAD-PLAN Q1: the main session is one long-lived CLI "thread" with tools (`Read,Grep,Glob,Bash,Edit,Write,WebSearch,WebFetch`), tiny edits itself, delegates the rest | `THREAD_TOOLS` at `P/claude/mod.rs:76` |
| Orchestrator rebirth at `min(150k, 75% window)` | SESSION-FIXES Q5: prepare at 400k, swap at 450k when the window is ≥ 500k; old shares otherwise | `crates/core/src/knowledge.rs:279-295` |
| Cold-cache rebirth floor 60k | SESSION-FIXES Q5.4: 150k | `knowledge.rs:204` |
| Worker hand-off at 160k, then 300k / 70% (§7 item 2, Q9) | SESSION-FIXES Q6.4: 200k or 70% of the window, whichever first; the hand-off writes `checks.md` so the fresh session reruns only affected checks (`e0653432`) | `knowledge.rs:222-238`; `M/prompts.rs:1148` |
| Rebirth briefing caps decisions (`DECISIONS_BUDGET`) | Q5.3: decisions exempt from every cut; transcript excerpts shrink first, to `MIN_EXCHANGES_OVERFLOW` = 3 | `M/rebirth.rs:55-61`, `:1215` |
| Fixed pipeline: outline → reviewer → verifier per phase | THREAD-PLAN: the thread decides; one non-blocking other-vendor review per landed change (a setting since 10-11) | `M/review_runs.rs` |
| Review → fix → review chains (one chain ≈ 60M tokens) | SESSION-FIXES Q6.1: one review per change; open findings go as one batch to the same worker, resumed; landed sessions held until settled | `M/review_hold.rs` |
| Each worker runs the full suite before reporting (Q6.2) | SESSION-FIXES §11 (2026-10-14): workers run only their own change's checks; the integrated tree is checked once per tip; failures go back to the worker that broke it | `M/integrated_checks.rs`; `M/prompts.rs:1148` |
| Workers "may" query the Brain | Q6.6 Brain first: Brain facts (≤ 3k tokens) in the context pack, a steer if no `query_brain` early, `learned` write-back, counters on the worker row | `M/brain_first.rs`, `M/context_pack.rs`, `M/brains.rs:1206` |
| Lean worker start | Q6.5 adds `disableBundledSkills`, `disableClaudeAiConnectors`, `disableRemoteControl`, more denied tools | `P/claude/mod.rs:69`, `:741-746` |

§7's rules still stand everywhere: no budgets or caps, nothing dropped silently, lossless, no settings, judged per
completed task.

## 2. What exists vs what is only proposed

**Built and on:**
- Thread with tools, lean start (`--strict-mcp-config`, `--setting-sources project`), Brigadier's MCP server only.
- **Compaction is off only for the thread.** The thread (orchestrator session) launches with `auto_compact: false`
  (`M/conversation.rs:2830`; also `M/rebirth.rs:360`, `M/escalation.rs:273`), which turns off Claude's
  auto-compaction (`P/claude/mod.rs:399`) and pushes Codex's `model_auto_compact_token_limit` out of reach
  (`P/codex/mod.rs:64`, `:1180`). Brigadier rebirths the thread instead.
- **Workers, reviews, research, Brain jobs and git-text jobs launch with `auto_compact: true`** (`M/workers.rs:1870`,
  `M/review_runs.rs:1226`, `M/research.rs:263`, `M/brain_jobs.rs:568`, `M/git_actions.rs:521`). A worker is meant
  to be handed off at 200k / 70% before the CLI compacts, but nothing stops the CLI compacting first (WEAK-SPOTS L2).
- `idleCompaction: false` is set in every Brigadier Claude session's settings (`P/claude/mod.rs:725`).
- Orchestrator/thread rebirth with a briefing (handoff 3k, state 2k, Brain 5k tokens; decisions whole), cold-cache
  rebirth from a warm-time checkpoint above 150k (`M/cold.rs`).
- Worker hand-off at 200k / 70% with note, last messages, `<scratch>/handoff/` and `checks.md`.
- Lossless output store for the **thread's** command output only: Claude through a `PostToolUse` hook, Codex through
  Brigadier's `run` tool; the model gets a digest plus `read_artifact` (`M/tool_output.rs`). Workers have no output
  hook (`only_a_thread_gets_the_output_hook`, `P/claude/mod.rs:2335`; `M/workers.rs:1874`).
- Code index tools `code_search`, `code_refs`, `project_map` (TypeScript definitions fixed 10-02). No outline,
  single-symbol or line-range read tool.
- Effort is chosen per task by the thread (`M/prompts.rs:103`: low / medium / high), not per step.
- Per-turn usage in `routing.sqlite` `turn_usage` (input, cached, cache writes, output, step kind, duration, context);
  quota readings in `quota_samples`; thread context growth per request (`M/thread_metrics.rs`); per-worker Brain
  counters and tokens on the worker row.
- Prewarmed worktree for the next writing task (`M/prewarm.rs`), dependency/build-cache copies (`M/warm.rs`).
- A/B tooling in `tools/ab` (incl. `worker_tokens.py`).

**Proposed, not built:**
- Keep-warm fork to keep the cache alive on purpose (§7 item 3 proposal; needs the user's view on background calls).
- Cache writes split by cause; a per-task usage report in the Inspector.
- Lossless output store for workers / heavy build output.
- Precise, batched reads from the code index.
- Effort per step.

## 3. What the measurements already say

- **Where usage goes (§7, 2026-09-30):** cache reads 59%, cache writes 25%, output 16%. 58% of usage comes from calls
  at 200–500k context. Tool results ≈ 30%, mostly successful file reads. Thinking is 32% of output.
- **Thread vs /delegator in cmux (2026-10-08, n = 1 per arm):** Brigadier was 3–7× faster to the final answer and used
  ~8× fewer raw Claude tokens on both tasks, with the same work done (`docs/evidence/2026-10-08-thread-ab.md`).
- **Session-fixes token gates (2026-10-11/12): missed.** 3 runs per arm: worker tokens 0.87× (gate ≤ 0.6×), sum 0.89×
  (≤ 0.75×), `run_check` calls 0.75× (≤ 0.5×). The blind review found **no** result complete in either arm, though all
  passed the frozen checks. Found in the runs: workers ending a turn while waiting on a background subagent were
  failed as "stopped without a report" (4 of 6 runs); the Brain skeleton job adds 17–25 nodes that differ per run.
  Phase 3's gates stand as missed; what to do is the user's call.
- **REA for study tasks:** dropped (1.12× the tokens, slower, further from the findings).
- **Rejected or not integrated:** budgets/caps, early compaction and context caps, quality settings, a fresh
  `claude -p` per message as default (≈ 2.5× worse for the orchestrator), RTK, headroom, Graft, graphify, cognee,
  caveman wording (concise wording saved no output tokens: run-to-run spread up to 3×), `--bare`, batch APIs,
  request-rewriting proxies. These stay rejected unless new evidence is named.

## 4. §7 "Next, in order": coverage

| §7 item | Status at `d669bca0` | What is unmeasured | Needs research |
|---|---|---|---|
| 1. Per-task usage in the Inspector, cache writes by cause | **Partial.** `turn_usage` rows per turn with step kind; thread context growth and worker tokens/Brain counters shown. No per-task report; cache writes are not attributed to a cause (fresh start, expiry, tool-list change, rebirth, hand-off, resume after close) | How much of the 25% cache-write share each cause makes | Which cache-write causes the CLIs expose (see DIGEST) |
| 2. Lossless command-output store | **Thread only** (`M/tool_output.rs`). Workers' Bash output reaches the model untrimmed | Worker share of build/test output after §11 moved full suites off workers (was < 1% overall) | Whether worker output is still worth it after §11 |
| 3. Precise, batched code-index reads | **Not built.** Search and refs return locations; reading is still `sed`/`Read` | Share of worker tokens from whole-file reads today | Outline/symbol/line-range read tools and their measured effect elsewhere |
| 4. Thinking / effort per step | **Per task only** (`M/prompts.rs:103`). §7 notes Opus 5.5 keeps the cache across an effort switch (measured once) | Quality at lower effort on our tasks; Codex cache across effort changes | Vendor statements on cache and effort; whether lower effort keeps quality (not assumed) |
