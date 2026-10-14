# Research digest: long sessions, lower usage, speed and grounding (2026-10-14)

What the vendors, the research literature and practitioners say, graded, and what each means for Brigadier. The
baseline (what Brigadier already does, measured or rejected) is `BASELINE.md`. Probes run for this digest are in
`PROBES.md`. Full source notes (longer, with every URL) are in `sources/`: `vendor.md`, `papers.md`,
`practice.md`.

**Grades.** Evidence: **M** measured with numbers, **C** controlled study, **V** vendor claim, **A** anecdote or one
run. Applicability to Brigadier, which drives the user's unmodified CLIs on subscription sign-in: **CLI✓** verified
on the installed CLI (help output, binary, protocol schema or our probe), **CLI?** documented for the CLI but not
verified here, **API** API-only (no use to Brigadier), **Prompt** works through instructions alone.

Installed: Claude Code 2.1.299 and codex-cli 0.162.1, both on subscription sign-in. (Claude Code auto-updated to
2.1.300 on 2026-10-14 after the research; the CLI✓ flags were re-checked in 2.1.300's help and the binary strings in
2.1.299 and 2.1.300; the second pass, §7, worked on 2.1.300.) Adapters:
`crates/providers/src/claude/mod.rs` (stream-json `claude -p`) and `crates/providers/src/codex/mod.rs`
(`codex app-server`).

## 1. The strongest findings, in one page

1. **Quality falls as context grows, long before the window is full, on current models too.** Recall of one fact
   at 1M is solved; reasoning across many facts and changing state is not. (Chroma 2025; Du 2025; NoLiMa, ICML 2025;
   Chow 2026; AgentLongBench 2026. Grade C, replicated.) Brigadier's thread rebirths at 400–450k on 1M windows,
   which is late by this evidence. But Brigadier has no quality measurement at size, so the trigger can't be tuned
   yet.
2. **A fresh session from one consolidated brief recovers most of what a long conversation loses.** Concatenating the
   pieces into one fresh prompt restored 95.1% of full-spec performance (Laban et al., ICLR 2026, 200k+
   conversations). Grade C, strong. This supports Brigadier's rebirth over CLI compaction.
3. **Free-text summaries drop standing rules, and the loss compounds every round.** Compactors kept 17% of injected
   constraints (Wang 2026). One CLI's built-in compact prompt kept 53% of safety rules after one round and 10% after
   five (Zerhoudi, CIKM 2026). Extracting constraints separately and appending them after the summary kept over 90%
   (Wang 2026; the abstract doesn't say word for word), and pinning them outside the summary brought violations back
   to 0% (Chen 2026). Repeated full rewrites erode detail; incremental itemised updates are better (ACE,
   ICLR 2026). Grade C. **For Brigadier:** the decisions ledger, carried whole, already is such a block. The weak
   part is anything that reaches the next session only through the model's own prose note (WEAK-SPOTS C4, C5, C8).
4. **Hiding old tool output is the cheapest, best-evidenced saving.** Masking old observations cut cost by about half
   against the raw agent and matched or beat LLM summaries on solve rate. Summaries made runs 13–15% longer, and an untuned hybrid lost
   money to cache invalidation (Complexity Trap, NeurIPS 2025 workshop, SWE-bench Verified 500, 5 configurations).
   Trajectory reduction saved 21–36% of cost at equal success (AgentDiet, FSE 2026). Goal-directed trimming of tool
   output saved 23–54% (SWE-Pruner 2026). Grade C. **For Brigadier:** it can't edit a CLI's history, so masking
   *old* results isn't available. Trimming at the source is: a Claude `PostToolUse` hook's `updatedToolOutput`
   replaces what the model sees, a `PostToolBatch` hook receives exactly what the model saw for every tool (so it can
   store it whole), and on Codex Brigadier's own `run` tool or, if Brigadier delivers its own Codex hooks, a
   `PostToolUse` hook (corrected in the second pass, §7). With a stored copy this is lossless. The other lever is an
   earlier fresh session.
5. **Caching is worth 41–80% of cost, and dynamic content or tool-set changes in the prefix break it** (Don't Break
   the Cache 2026, 500+ sessions). Input tokens, not output, drive most agent cost, and the same task varies up to 30×
   in tokens between runs (Bai et al. 2026). Grade C. **For Brigadier:** single runs can't show a saving. This
   repeats §7's rule: judge per completed task, with several runs.
6. **"Done" claims and LLM judges that read transcripts are unreliable; checks of the real state work.** 75.8% of
   failed coding-agent runs that stated a status claimed success (AppWorld, where agents write code against app
   APIs). LLM judges reading the transcript scored AUROC ≤
   0.65 (Advani 2026). Self-correction without outside feedback often makes answers worse; a sound external
   verifier helps (Huang, ICLR 2024; Stechly, ICLR 2025). Capable models game contradictory tests (ImpossibleBench,
   ICLR 2026). Grade C. The unreliable judges are the ones that **read a transcript**: a judge that has the tree and a
   shell and runs things agreed with human evaluators far better ("as reliable as our human evaluation baseline",
   Agent-as-a-Judge, arXiv 2410.10934; second pass, §7). **For Brigadier:** integrated checks on the real tree and the "[not checked], never [met]"
   rule are right, and a review is worth most when it inspects and runs, not when it reads a diff and a report. Turning worker claims into Brain facts (G1) and accepting an overnight phase on a summary (G2) go
   against this evidence.
7. **Storing an agent's own claims as memory spreads its errors** (Xiong 2025, arXiv preprint: adding only
   evaluator-approved memories gained 15–26 points absolute over adding everything, across four agents; HaluMem 2025). Grade C. This bears directly on G1.
8. **Effort: moderate is usually the frontier, and the top setting mostly adds cost.** Opus 5 at low effort
   resolved 17/25 hard tasks against 18 at high and 16 at xhigh, at $0.75 against $1.89 and $3.62 per resolved
   task. Sonnet 5 needed high (12 at low against 19 at high, p = 0.016). The effort runs used Claude Code 2.1.220
   (Weinberger 2026; small n, grade A/C). In the same paper, "think very deeply" and "try multiple approaches"
   wording multiplied reasoning tokens 1.6–7.4× at comparable success, measured on six open models, not in Claude
   Code. Vendors: Opus 5.5 at medium is at least Opus 5 at high, and "low comes close" (V). **For Brigadier:**
   the effort to use depends on the model (see WEAK-SPOTS E1). Lower effort is not assumed to keep quality; it is a
   thing to measure.
9. **Multi-agent pays only for truly parallel work.** Every multi-agent variant lost 39–70% on sequential planning,
   and independent agents amplified errors 17× against 4× with a coordinator (Kim et al. 2025, v1: 180
   configurations; the April 2026 revision reports 260 configurations and a worst case of −70% on sequential planning). Five of six multi-agent workflows trailed one agent while using 1.4–4.3× the tokens (Fu 2026).
   Vendor: multi-agent ≈ 15× chat tokens, a poor fit for most coding (Anthropic 2025). Grade C. **For Brigadier:**
   delegation is right for context isolation and parallel parts, wrong as a default for a small, sequential ask
   (WEAK-SPOTS S1).
10. **A cheap, verified scout hand-off beats routing by guessed difficulty.** No learned router beat "always the
    strongest model" on solve rate. A small scout that writes a ~4 KB hand-off (ranked files with line ranges, a
    reproduction, dead ends tried), with its claims replayed and false ones removed, matched the best model at about
    one fifth of the cost per solve, and the hand-off, not the routing, drove the gain (Scrouting 2026). Escalating
    with the full cheap trajectory recovers less than half the gap (Handoff Tax 2026). Grade C, single studies.

## 2. Vendor guidance and CLI capability

### 2.1 Context and continuity

| Finding | Source (date) | Grade | Applicability |
|---|---|---|---|
| What survives a Claude Code compaction: system prompt, root CLAUDE.md, unscoped rules, the plan, up to 5 recent files (≤ 5k tokens each), skills ≤ 5k each / 25k total. Nested CLAUDE.md, path rules and hook context are summarised away | code.claude.com/docs/en/context-window (current) | V | CLI?. Irrelevant while Brigadier keeps compaction off, but it is the list of what a Brigadier brief must re-supply |
| A fresh context window often beats compaction: "latest models are extremely effective at discovering state from the local filesystem". Use `progress.txt`, `tests.json`, `init.sh`; tell the model its context will be renewed so it doesn't stop early | platform.claude.com prompting best practices (current) | V | Prompt |
| Long-running harness: an initializer writes a feature list in JSON (models overwrite JSON less readily than Markdown), a progress file and the first commit; each later session runs pwd → git log → progress → smoke test; one feature at a time; "compaction alone wasn't enough" | anthropic.com/engineering/effective-harnesses-for-long-running-agents (2025-11-26) | V/A | Prompt + files |
| A separate sceptical evaluator beats self-evaluation ("confidently praising the work"); files and a "sprint contract" define done. Opus 4.5+ needed fewer context resets than Sonnet 4.5 | anthropic.com/engineering/harness-design-long-running-apps (2026-03-24) | A (costs given) | Pattern |
| Context editing + memory tool: +39% on an internal agentic-search eval, 84% fewer tokens on a 100-turn eval | claude.com/blog/context-management (2025-09-29) | M (vendor) | **API**. The binary contains `clear_tool_uses_20250919`, and "clearing old tool results" is listed as an expected cache rebuild, so Claude Code may clear results itself. No setting is documented |
| Idle compaction: on in interactive sessions with the 1 h TTL, ≥ 200k tokens, ~54 min idle; `idleCompaction: false` turns it off | settings reference; changelog 2.1.299 | V | CLI✓. Brigadier already sets it off (`P/claude/mod.rs:725`) |
| Auto-compact window settable 100k–1M (`--autocompact`, `autoCompactWindow`, `CLAUDE_CODE_AUTO_COMPACT_WINDOW`); per-subagent `autoCompactWindow` since 2.1.296 | model-config docs; changelog | V | CLI✓ (`--autocompact` in help) |
| Codex app-server: `thread/compact/start`, `thread/fork` (`lastTurnId`, `excludeTurns`, `ephemeral`), `thread/resume` (`excludeTurns`), `thread/inject_items` (append model-visible items without a turn), `turn/start` with `model`/`effort`, `thread/goal/set` with `tokenBudget`, `thread/tokenUsage/updated` with cached and cache-write counts | developers.openai.com/codex/app-server (current) | V | CLI✓ (in the schema from `codex app-server generate-json-schema`, 0.162.1) |
| Codex compaction keys: `model_auto_compact_token_limit`, `compact_prompt`, `experimental_compact_prompt_file`, `tool_output_token_limit` | Codex config reference | V | CLI✓ (binary strings; set with `-c`). Brigadier sets the first (`P/codex/mod.rs:1180`) |
| Codex 0.157–0.162: resume from the latest compaction boundary; "install full context in compaction replacement history"; partial-history subagent forks removed; tool declaration mode kept across resumed windows | Codex release notes (2026-09-25..10-08) | V | CLI✓ (installed 0.162.1) |
| **Claude hooks Brigadier can use** (second pass): `Stop` with `decision: "block"` and a `reason` keeps the model working, `stop_hook_active` marks a continuation, at most 8 continuations in a row (`CLAUDE_CODE_STOP_HOOK_BLOCK_CAP`); `PostToolBatch` once per batch before the next model call, with `tool_response` = "the same content the model receives"; `PostToolUse` `updatedToolOutput` replaces what the model sees; `PostToolUseFailure.error` ≈ the failed result the model got, middle-truncated; `PreCompact` exit 2 skips a proactive compaction, and fails the request for a recovery one; `PostCompact` gets `compact_summary`; hook-injected context is capped at 10,000 characters (a file path and a 2,000-character preview past that) | code.claude.com/docs/en/hooks (re-fetched 2026-10-15) | V | CLI✓ for presence (all names in the 2.1.300 binary) and for delivery (Brigadier passes its thread's hook in `--settings`, `P/claude/mod.rs:758`). Behaviour in `-p` stream-json: CLI? |
| **Claude Bash output limits**: valid output inline to ~30,000 characters, then a session-folder file path plus a 2,000-character preview; failing output inline to ~10,000, then a head-and-tail excerpt from the read-back window and **no file**; `BASH_MAX_OUTPUT_LENGTH` = read-back window, 30,000 default, ceiling 150,000; `bashOutputMaxChars` up to 128,000 (2.1.261+) | code.claude.com/docs/en/tools-reference (re-fetched) | V | CLI✓ (names in the 2.1.300 binary). Settles PLAN probe 2 (PROBES, second pass) |
| **Codex hooks**: stable, on by default; `PostToolUse` (`decision: "block"` replaces the tool result with the hook's feedback), `PreCompact` (`continue: false` stops compaction), `Stop` (block = continue with `reason` as a new prompt), `SessionStart`, `UserPromptSubmit`, and more; configured in `hooks.json` or `[hooks]` tables of `config.toml` beside an active config layer (`~/.codex`, or a trusted `<repo>/.codex`); non-managed hooks must be trusted, or run with `--dangerously-bypass-hook-trust` | developers.openai.com/codex/hooks (redirects to OpenAI's docs site; re-fetched) | V | CLI✓ for presence (`codex features list`: `hooks stable true`; the flag in `codex --help`; `hook/started`, `hook/completed`, `hooks/list` in the app-server schema). **Brigadier turns them off** (`P/codex/mod.rs:116-123`). Delivering Brigadier's own hooks without the user's: unverified, the page doesn't say whether `-c` can set them |
| Codex structured output: `turn/start` takes an `outputSchema` for that turn. Claude: `--json-schema` returns `structured_output` in print mode | app-server doc; CLI reference | V | CLI✓ (schema; `claude --help`). Little use: worker reports are already typed `submit_report` calls |
| Claude `--resume` accepts "the absolute path to a session's `.jsonl` transcript file" in place of an id | code.claude.com/docs/en/cli-reference (re-fetched) | V | CLI? Whether a transcript resumes under another account's config folder is unverified (PLAN §5) |
| `-p` cost on resume: `total_cost_usd` covers the conversation's earlier runs too; on SIGTERM the turn in progress is left unfinished, and `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1` continues it on resume; `--bare` "will become the default for `-p` in a future release" | headless docs (w04, not re-fetched here) | V | CLI? The env name is in the 2.1.300 binary. Watch item: when `--bare` becomes the `-p` default, Brigadier must opt out to keep subscription sign-in |
| Codex memories off by default; "a recall layer, not the only source for rules that must always apply" | Codex memories docs | V | CLI✓ (`codex features list`). Same lesson as finding 7 |
| ExecPlans / long-horizon runs: a living PLANS.md (progress, decision log, "done when", validation commands) carried a ~25 h, ~13M-token run | developers.openai.com blog and cookbook (undated) | A | Prompt + files |

### 2.2 Caching

| Finding | Source | Grade | Applicability |
|---|---|---|---|
| Subscription: the main conversation caches for **1 h**; subagents, forks, compaction and titles for 5 min. TTL overrides: `promptCacheTtl`, `subagentPromptCacheTtl`, `ENABLE_PROMPT_CACHING_1H`, `FORCE_PROMPT_CACHING_5M` | code.claude.com/docs/en/prompt-caching | V | **CLI✓ by probe P1**: a Brigadier-style `claude -p` session wrote 11,456 tokens, all at 1 h |
| Effort changes keep the cache on the 5.5 models (subscription or API key) | same page | V | **CLI✓ by probe P2**: an effort switch on resume rewrote a fixed ~9.4k and kept a 115k history cached |
| Cache breakers: model switch, fast mode on, MCP server connect/remove when tools aren't deferred, compaction, CLI upgrade. Kept: file edits, CLAUDE.md edits (applied only at /clear, /compact or restart), permission mode, output style | same page | V | CLI? Bears on Brigadier's thread (its MCP set is fixed at start) and on the computer-use server coming and going |
| On resume, the system prompt is the one recorded at the start; `--system-prompt-snapshot` reuses it verbatim until compaction | same page; `claude --help` | V | CLI✓. Brigadier relies on it (`[settings]` notes, §7 item 5) |
| `--exclude-dynamic-system-prompt-sections` moves cwd, env, memory paths and git status out of the system prompt "for cross-user prompt-cache reuse" | `claude --help` 2.1.299 | V | CLI✓ (help). **Not used by Brigadier.** It shares the cache within one conversation by giving all its Claude workers one start folder (`M/workers.rs:1601-1611`), but not across conversations |
| SessionStart hook input on resume/fork carries `prompt_cache_likely_expired`, `context_tokens`, `seconds_since_last_response`, `estimated_cache_write_usd` | code.claude.com/docs/en/hooks | V | CLI? (`--include-hook-events` in help). A ready-made signal for cache-write causes (§7 next item 1) |
| "Never change tools mid-session; don't switch models mid-session (late in a session it costs more than it saves; hand off to a subagent instead); put updates in messages, not system-prompt edits" | claude.dev blog, "prompt caching is everything" (2026-04-30) | V/M | Pattern. Brigadier's notes-instead-of-restarts (§7 item 5) already follow it |
| OpenAI: the whole rendered prefix must match, including tools, effort and verbosity; on GPT-6 a `configuration_update` item changes effort without breaking the cache | developers.openai.com prompt-caching guide | V | API. `configuration_update` is in the codex binary; whether Codex uses it is unknown. **Probe P4 was inconclusive**: Codex cache hits swung 13k ↔ 25k from turn to turn with or without an effort change |
| OpenAI cache lifetime on GPT-5.6 and later: one value, 30 minutes, "reusing the prefix refreshes its lifetime without another cache-write charge"; writes 1.25× input, reads 0.1× (0.05× on GPT-6.1 Sol); entries live on single machines and "traffic above 15 requests per minute can lead to overflow routing" | developers.openai.com/api/docs/guides/prompt-caching (re-fetched 2026-10-15) | V | API doc for the cache Codex's requests hit; no model list, so gpt-6.1-sol is unverified. Explains P4. Brigadier gives Codex no lifetime (WEAK-SPOTS N5) |
| Anthropic: the prefix is `tools`, `system`, then `messages`; "Modifying tool definitions (names, descriptions, parameters) invalidates the entire cache" | platform.claude.com prompt-caching (re-fetched) | V | Applies to every Claude session. Why the checkpoint fork, launched with no tools, rewrites the whole thread (WEAK-SPOTS C11) |

### 2.3 Grounding and speed

| Finding | Source | Grade | Applicability |
|---|---|---|---|
| Allow "I don't know"; quote before claiming over long documents; retract a claim without a supporting quote | platform.claude.com reduce-hallucinations | V | Prompt |
| "Never speculate about code you have not opened"; verify the answer against test criteria before finishing | prompting best practices | V | Prompt |
| For unattended runs, treat a text-only end of turn as a report, not as done; keep a checklist file; cap auto-continuations at 2–3 | prompting Claude Opus 5.5 | V | Prompt + harness |
| Hallucination comes from grading that rewards guessing; abstaining should beat a wrong answer | openai.com/index/why-language-models-hallucinate (2025-09-05) | M | Prompt (reward "unknown") |
| Output tokens dominate latency: halving output can halve latency; halving input gives 1–5% | OpenAI latency guide | V | Prompt (short answers), routing |
| Opus 5.5 at medium ≥ Opus 5 at high; removing "think carefully" lines makes replies start sooner; "treat earlier answers as settled" cuts follow-up thinking; time budgets make teams finish sooner | prompting Claude Opus 5.5 | V | CLI✓ (`--effort`) + Prompt |
| Fast mode: Opus "up to 2.5x faster", same quality, $8/$40 per MTok on Opus 5.5; on subscriptions "available via usage credits only and not included in the subscription rate limits"; in `-p` only with `--settings '{"fastMode": true}'`; enabling it later in a conversation pays the whole context once at the uncached fast price | code.claude.com/docs/en/fast-mode (re-fetched) | V | CLI? Paid outside the subscription: a user decision (PLAN §6) |
| Agent teams use about 7× the tokens of a standard session when teammates run in plan mode; keep CLAUDE.md under 200 lines, move detail to on-demand skills | code.claude.com/docs/en/costs | V/M | Prompt, layout |

## 3. Research literature

Grouped findings with the strongest sources (full list with dates, sizes and URLs in `sources/papers.md`):

- **Context rot.** Chroma "Context Rot" (2025-07, 18 models: a 300-token focused prompt beat the 113k full history
  for every model). Du et al. arXiv 2510.05381 (accuracy −13.9% to −85% from length alone, even with perfect
  retrieval; fix: recite the evidence, then answer). NoLiMa arXiv 2502.05167, ICML 2025 (11 of 13 models below half
  their short-context score at 32k). Laban et al. arXiv 2505.06120, ICLR 2026 (−39% multi-turn; fresh consolidated
  prompt 95.1%). Sinha et al. arXiv 2509.09677, ICLR 2026 (self-conditioning on own errors). Chow arXiv 2605.02173
  (1M: lookup solved, multi-hop not). SlopCodeBench arXiv 2603.24755 (agents erode their own code over 196
  checkpoints). **Contradicting:** LoCoBench-Agent arXiv 2511.13998 reports robustness when agents fetch with tools
  instead of holding everything in context. That supports just-in-time retrieval, not long contexts.
- **Continuity.** Wang et al. arXiv 2608.11242; Zerhoudi arXiv 2608.22752 (CIKM 2026); Chen arXiv 2606.22528; ACE
  arXiv 2510.04618 (ICLR 2026); Slipstream arXiv 2605.08580 (a summary checked against what the agent later needed:
  +8.8 points, −39.7% latency); AutoCompact arXiv 2610.02163 (keep conclusions, workspace state, remaining steps);
  Context-Folding arXiv 2510.11967 (fold sub-tasks into short outcomes: ~10× smaller context); LongMemEval arXiv
  2410.10813. **Gap:** no study measured whether exact identifiers or "dead ends" survive a summary.
- **Compression.** Complexity Trap arXiv 2508.21433; AgentDiet arXiv 2509.23586 (FSE 2026); ACON arXiv 2510.00615
  (ICLR 2026); SWE-Pruner arXiv 2601.16746; Don't Break the Cache arXiv 2601.06007; How Do AI Agents Spend Your Money
  arXiv 2604.22750.
- **Delegation.** Kim et al. arXiv 2512.08296; MAST arXiv 2503.13657 (NeurIPS 2025); Fu et al. arXiv 2606.05670;
  Anthropic multi-agent research system (2025-06); Handoff Tax arXiv 2608.24358.
- **Grounding.** Huang arXiv 2310.01798; Stechly arXiv 2402.08115; Advani arXiv 2606.09863; Tang et al. arXiv
  2605.29442 (20,574 real sessions: 91.5% of resolutions needed user correction; false self-reports a growing
  share); ImpossibleBench arXiv 2510.20270; Kalai et al. arXiv 2509.04664; memory: Xiong arXiv 2505.16067, HaluMem
  arXiv 2511.03506.
- **Second pass additions.** Agent-as-a-Judge arXiv 2410.10934 (a judge agent with tools "dramatically outperforms
  LLM-as-a-Judge and is as reliable as our human evaluation baseline" on DevAI, 55 tasks; w04's ~90% against ~70%
  agreement is from search, not re-confirmed). LongCLI-Bench arXiv 2602.14337 (Feb 2026, 20 tasks: pass rates below
  20%, most tasks stall below 30% completion, self-correction gives marginal gains, plan injection and interactive
  guidance give much larger ones). Recursive Language Models arXiv 2512.24601 (v3 2026-05-11: the long prompt held as
  an external store the model inspects and recurses over, inputs "two orders of magnitude beyond" the window, at
  comparable cost; w04's +26% against compaction is not re-confirmed). AbstentionBench arXiv 2506.09038 (reasoning
  tuning lowers abstention; reported by w04, not re-fetched). AgentRewind arXiv 2608.14380 (rewind memory of what
  failed; no numbers in the abstract; reported by w04).
- **Effort.** Weinberger & Hozez arXiv 2608.01347; Cuadron arXiv 2502.08235 (overthinking); Gema arXiv 2507.14417
  (inverse scaling, contested).
- **Fast paths.** RouteLLM arXiv 2406.18665 (chat); Scrouting arXiv 2608.04804; SWE-Router arXiv 2607.00053;
  Efficient Agents arXiv 2508.02694.

Caveat: many 2026 results are single, unreviewed preprints. The replicated core is: length hurts; fresh consolidated
prompts recover it; self-checking is weak and external checks work; masking old tool output is a safe, large saving.

## 4. Practitioners and open-source harnesses

Full notes with every URL are in `sources/practice.md`. Most are anecdotes or self-reported numbers. The few measured ones
are marked **M**.

| Finding | Source (date) | Grade | Fits an unmodified CLI? |
|---|---|---|---|
| Every harness that ran for days restarts often and carries state in files plus git: a fixed "done" list workers can't edit, a short progress baton (verified, next action, failed approaches, blockers), git as the record | Ralph loop (ghuntley.com/ralph, 2025-07); HumanLayer ACE-FCA (2025-08, target 40–60% context use, ~7 h for 35k LOC); Continuous Claude; Anthropic C-compiler run (2026-02-05: 16 agents, ~2,000 sessions, 2B input tokens, a running list of failed approaches) | A; M for the compiler run | Yes |
| Planner → workers → judge per cycle works. A shared lock (20 agents did the work of 2–3) and an integrator agent both made throughput worse. "Periodic fresh starts are still needed to combat drift" | cursor.com/blog/scaling-agents (2026-01-14) | M (scale, no control) | Partly |
| Replace compaction with a goal-directed hand-off the user can edit: a new thread gets only the context relevant to the stated goal | ampcode.com/news/handoff (2025-10-23) | D | Yes. This is close to Brigadier's design |
| Claude Code already writes large Bash output to `<session>/tool-results/<id>.txt` and shows the model a ~2 KB preview with the path | Claude Code issues (2026) | D/A | Built in. Brigadier's Claude workers get it already; Brigadier's own transcript still clips at 8 KiB (C9) |
| A dropped constraint after auto-compaction (a `varchar(128)` rule from turn 8, lost around turn 28) cost 10 turns of wrong work and a 15-turn undo; many independent reports of CLAUDE.md rules dropped after compaction | zivtech.com "Economics of Claude Code, Part 3" (2026-04/06); claude-code issues | M/A | The case for pinned constraints (PLAN A1) |
| The built-in LSP tool against grep on a 149k-LOC repo: 31–33% fewer tool-output tokens, and grep alone missed references on Sonnet 4.6 (9/11, 249/260; Opus found all with either) where LSP missed none, when the model was told to trust the result without re-reading | circleci.com/blog/claude-code-lsp (2026-06-24) | M (small n) | Yes, a built-in tool switched on by config. Brigadier doesn't enable it (PLAN B2) |
| Test and build output as "failures only, passes counted" is the largest single output saving; full output kept by id is the safe form | RTK (self-reported 60–90%); Context Mode (self-reported); Carlini's logging rules | A | Yes |
| A new Claude Code session's preamble is ~33k tokens, ~75k with instruction files, MCP and plugins. Splitting one task across two subagents took input from ~121k to ~513k | Systima capture via secondary sources (2026-07) | M (secondary) | Applies: each worker start pays a preamble. Brigadier's lean start measured 15–30k |
| A worktree takes 5–15 s on a non-trivial repo, often more than a ~30 s task | Bernstein docs | A | Brigadier already prewarms (`M/prewarm.rs`) |
| A compressed 8 KB docs index, always in AGENTS.md, scored 100%. On-demand skills reached at most 79%, and 53% when not told to use them (never called in 56% of cases) | vercel.com/blog/agents-md-outperforms-skills (2026-01) | M (one framework) | Yes. Supports a small always-present index over long instruction files (PLAN B5) |
| Tool definitions fill the window: Playwright MCP ~13.7k tokens for 21 tools against ~225 for scripts plus a README; 12 servers used 72% of a 200k window before the first message; deferring tools cut ~77k to ~8.7k. Tool design (short output, few tools) mattered more than the protocol | mariozechner.at (2025-08, 2025-11, 120 runs); gurubase.io (2026-05) | M | Yes. Brigadier's thread loads 47 tools (S2). Claude's tool search defers MCP tools by default |
| KV-cache hit rate is "the most important metric"; keep the prefix stable and context append-only; never add or remove tools mid-task; keep failures in context; recite the goal by rewriting a todo file | Manus context engineering (2025-07-18) | A | Partly |
| The Claude Code team alerts on cache hit rate and declares incidents when it drops. One heavy user measured ~35 cache reads per write and found a subagent silently on the wrong model | simonwillison.net quoting the team (2026-02); spin.atomicobject.com (2026-03) | A / M (one user) | Yes (PLAN P0.1) |
| False success: in a public incident an agent deleted a production database during a code freeze and misreported its tests. 63% of one model's successful benchmark solutions fetched the fix; strict isolation cut scores by 14–21 points; "keep working without stopping" prompts raised cheating | incident database #6113 (2025-07); cursor.com/blog/reward-hacking-coding-benchmarks (2026-06-25) | A; M | Supports harness-run checks (PLAN A5) |
| OpenAI's reference orchestrator over `codex app-server`: one thread per worker run, continuation turns send only guidance, `agent.max_turns` 20, a run with no agent event for `stall_timeout_ms` = 300,000 (5 min) is killed and retried, no retry timers restored after a restart (re-poll and re-dispatch), token totals from absolute `thread/tokenUsage/updated` with deltas | github.com/openai/symphony `SPEC.md` (re-fetched) | V/A (a spec, no measurements) | Yes. Supports a shorter stall deadline (W1) and restart-from-durable-state (A3) |
| "Share full traces, not just the task"; "actions carry implicit decisions", and agents acting on conflicting decisions produce bad results | cognition.com/blog/dont-build-multi-agents (2025-06-12; w04) | A | Supports B1, and a context pack that carries the decisions bearing on a task |
| AGENTS.md as "a table of contents, not an encyclopedia" pointing into `docs/`; linters whose messages teach; recurring drift clean-up | OpenAI "Harness engineering" (2026-02-11), **primary not readable** (403); secondary summaries only | A, unverified | Supports B5's small always-present index |
| Memory files are an attack surface: an npm `postinstall` rewrote memory files and added a hook, and the agent then advised hard-coding keys | blogs.cisco.com (2026-04-01) | M (demo, patched) | Supports provenance on Brain entries (PLAN A4) |

## 5. Items §7 rejected: is there new evidence?

| Rejected | New evidence? |
|---|---|
| Budgets, caps, early compaction, context caps | No new evidence for caps. The context-rot evidence argues for **earlier fresh sessions with a lossless carry-over**, which §7 allows (rebirth, hand-off). It is not a cap and drops nothing silently. Flagged as new evidence for an **earlier trigger**, to be measured, not adopted |
| Fresh `claude -p` per user message as the default | No. Laban's 95% recovery is for one consolidated prompt, not per-message restarts, and §7's 2.5× cost measurement stands |
| Lossy proxies (headroom), RTK, Graft, graphify, cognee | No. RTK's public claims (60–90%) are still self-reported bytes÷4 estimates, against our measured 0.21%. The research supports trimming tool output, but the lossless store Brigadier already has for the thread is the compatible form |
| Concise wording to save output | Partly new: wording that asks for more thinking ("think deeply", "try several approaches") multiplies reasoning cost 1.6–7.4× (Weinberger 2026). This is about removing cost-raising phrases, not terse output. Checked: `M/prompts.rs` contains none of them |
| `--bare` | Still unusable: help says OAuth and keychain are never read |

## 6. Citation spot-check (2026-10-14)

Sixteen citations the plan leans on most were fetched again and compared with the text above.
- **Confirmed as stated:**
  - Laban et al. (39% drop, 200,000+ conversations, Concat at 95.1% of Full; ICLR 2026 per OpenReview);
  - Zerhoudi (53% → 10%);
  - Wang (17%; over 90% with extraction);
  - Chen (0% → 30%, up to 59%; 0% with pinning);
  - Advani (75.8%, AUROC ≤ 0.65);
  - Weinberger's effort table (17/18/16, 12/19/17, $0.75/$1.89/$3.62, p = 0.016);
  - Complexity Trap (about half the cost of the raw agent; summaries 13–15% longer);
  - Kim et al. v1 (39–70%, 17.2× against 4.4×);
  - Scrouting (159 against 158 of 266, about a fifth of the cost, a ~4 KB hand-off with replayed reproductions);
  - Bai et al. (input drives cost; up to 30×);
  - Don't Break the Cache (41–80%);
  - Chroma (18 models; focused prompts beat full ones for every model);
  - CircleCI (31% and 33%);
  - the Claude Code prompt-caching and costs pages (TTL buckets, override names, effort keeping the cache on the 5.5
    models, cache breakers, "clearing old tool results" as an expected rebuild);
  - the Opus 5.5 prompting guide (medium ≥ Opus 5 at high).
- **Corrected:**
  - Xiong's gain (15–26 points, not ~10%; no venue listed);
  - Weinberger's wording multipliers (open models, not Claude Code);
  - which study the over-90% figure comes from (Wang, not Chen);
  - AppWorld as the setting of the 75.8%;
  - Kim et al.'s later revision;
  - the plan-mode condition on the 7× agent-team figure;
  - which model missed references with grep.
- **Not confirmed:** whether Wang's extractor keeps the constraints word for word.

## 7. Second pass (Fable 5.1 + Mythos 5.1)

At the user's request two more workers looked at the first pass: **w04 (Fable 5.1)** searched the internet and the
installed CLIs for what it missed, and **w05 (Mythos 5.1)** read the code for weak spots and red-teamed `PLAN.md`.
Their full texts are in the run's `msgs/w04-fable-research.md` and `msgs/w05-mythos-investigation.md`. **w06 (Opus
5.5)** adjudicated every claim before anything was adopted: code claims by reopening the cited lines at `d669bca0`,
CLI claims against `claude --help` 2.1.300, `codex --help`, `codex features list`, the app-server schema and both
binaries, web claims by re-fetching the pages the plan relies on, and usage numbers by re-running w05's queries
read-only on the installed app's store.

**What w04 (Fable) found.** Codex 0.162.1 has a stable hooks engine (PostToolUse, PreCompact, Stop and more) that
Brigadier switches off; Claude's `Stop`, `PostToolBatch` and `PreCompact` hooks give a done gate, exact capture of
every tool result and a way to refuse compaction; Claude's Bash output limits are documented, which answers PLAN
probe 2; OpenAI documents a 30-minute cache lifetime and explains P4; Brigadier gives Codex no cache lifetime; fast
mode in `-p`; judges with tools are reliable where transcript readers aren't; Symphony's 5-minute stall deadline;
twelve PLAN changes and seven probes.

**What w05 (Mythos) found.** The checkpoint and hand-off fork shares no cache prefix with the thread (216k written,
measured) and is then counted as a cache refresh; the thread never sees `learned` and judges workers on 3,600 bytes;
every limit, account switch, sign-in error and "prompt is too long" takes the thin reroute path, which same-session
resume would avoid, and building A6 before that pushes more workers down it; the thread wakes per message at full
context (measured); `search_transcript` misses the thread's own tool output; Brain lines carry no provenance; four
first-pass errors; a new build order.

**Adopted** (each now in WEAK-SPOTS or PLAN, with its evidence):
- The fork's missing prefix and the false refresh (C11; PLAN A11), confirmed in the code and re-measured: 544 read,
  216,616 written for a 252k thread, then 77k for the briefing, about 294k where a plain cold resume writes ~252k.
- Same-session resume for limits, account switches and sign-in errors, and "prompt is too long" through the size
  hand-off (C1 widened; PLAN A12), and w05's ordering argument: A6 waits for A12, because with compaction off a worker
  that ignores the steer hits "prompt is too long" and today that becomes a thin successor on a bigger-window model.
- `learned` shown to the thread and provenance on Brain lines (G6, G7; PLAN A13); wake-up coalescing (S6; PLAN A14);
  `search_transcript` over the thread's own tool output (C12; PLAN A15).
- P0.1 measured by arithmetic (`cache_write − Δcontext`) instead of cause tags only; Codex writes dropped (always 0).
- The first-pass corrections: L2's steer is mid-turn, once per turn (not at turn end); C7 is the lesser problem
  next to C11; V1 must count `handoff` and Brain rows; V2 gains an account-switch seam, a context-window seam, a
  planted false `learned` line and an item that lives only in the thread's own tool output.
- Codex hooks as a lever (N4), Claude hooks for exact capture (P0.3), refusing compaction (A6(d)) and a narrow done
  gate (A5(e)); Bash output limits (probe 2 dropped); Codex's 30-minute lifetime for a Codex checkpoint (A3, behind
  a probe); W1 raised to High and the thread watchdog moved earlier (A9); a reviewer that runs things, not one that
  reads (A5(b)); verifying the first report of a long phase hardest (A5(a), from LongCLI-Bench).

**Rejected or narrowed, and why:**
- **w05: worker task text sits in `--append-system-prompt`, and each worker's cwd is its own worktree, so B4 will
  show nothing.** Wrong for Claude workers: the task is the first user message and the start folder is shared
  (WEAK-SPOTS §7). B4 stays, with one narrower open question (`--add-dir` in the dynamic sections).
- **w04: a JSON-schema report turn makes report acceptance a parse.** Reports are already typed `submit_report`
  calls validated inside the worker's turn. Not proposed. The cheap evidence check goes into that validation (A5(e)).
- **w04: a `Stop` hook as the main done gate.** Narrowed: `submit_report` already refuses a bad report inside the
  turn, so a `Stop` hook adds value only for a turn that ends with no report or with a hand-off note missing its
  parts (C8). Kept for those.
- **w04: move A9 up because Symphony stalls at 5 minutes.** Agreed on moving it up, not on the number: Symphony
  counts any agent event as activity, Brigadier deliberately doesn't, so the deadline is set by fault injection.
- **w04: set `BASH_MAX_OUTPUT_LENGTH=150000` for workers now.** Deferred to a probe: the docs say it widens the
  window a failing excerpt is cut from, but not whether that window is the start or the end of the output, and a
  failing test log's useful part is usually the end.
- **w04: P0.1 adds the OpenTelemetry cache split as a cross-check.** Not needed: stream-json carries the split.
- **Usage figures w05 gave and w06 couldn't recompute** don't appear; every number in the docs was re-queried (two
  moved slightly as the store grew: 73 → 75 thread turns on 10-14, 700 → 725 Codex rows).
- **Settled rules are untouched.** Claude fast mode (paid usage credits) and Codex's `tokenBudget` are listed in
  PLAN §6 for the user, both recommended against for now; nothing here changes Q5 or the no-caps rule. The report
  cap is kept; showing `learned` costs nothing extra, and raising the cap is a §6 question to decide after V3.
