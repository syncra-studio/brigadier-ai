# Token sources review (2026-10-17)

The user sent two pages and six Reddit posts on cutting token use, and asked what we can apply without losing
quality. This doc gives every idea in them a verdict. Each verdict is checked against Brigadier's code and against
usage data already on disk.

Rule for this review: we take an idea only where Claude Code, Codex or Brigadier doesn't already do it. Where
Brigadier already has it, the doc cites the code. The bar is Q58 (a change merges on the first look; tokens are a
tripwire, not a gate) and Q68 (the proof needed before an auto-merge).

## Worth doing

1. **Fix the cheap model for chores.** Brain jobs, model research and commit-message writing ask for the provider's
   cheapest model. When that lookup fails, they fall back to the first model in the CLI's list, which is Opus.
   - Overnight on 2026-10-16/17, five Brain enrichment jobs ran on Opus: 165 calls, 11.7M cache reads, 0.48M cache
     writes and 99k output.
   - Effect: chores go back to a small model, with no quality risk (Brain nodes are unverified by design).
   - Size: XS–S.
2. **Run Q68's fresh verifier as review-grade work, not as a chore.** Today a `verify` task gets the light floor,
   medium effort and the cheaper of two near-equal models.
   - Effect: keeps the auto-merge proof strong. It costs a little more, and the tripwire line can say "verification".
   - Size: XS.
3. **Put cache and waste numbers in the tripwire report.** Brigadier already records each cache write's cause and
   the prefix each call missed. The report should also show:
   - the share of prompt tokens read from cache;
   - writes by cause;
   - invalid tool calls;
   - the model tier behind each chore.
   - Effect: a tripwire line explains itself, and a mistake like item 1 shows up in the next phase report.
   - Size: S.
4. **Escalate a cheap chore once when its check fails.** A scout or research report whose evidence doesn't hold
   up is rerun once, one tier up, with the reason logged. It never starts expensive by default.
   - Effect: cheap routing stays safe. The saving is small, because the thread now does most work itself.
   - Size: M. Do it after 1–3.

Not worth doing, with numbers below:
- a `$var` mechanism for tool arguments (about 0.1% of usage);
- schema flattening (0.007% of tool calls fail validation);
- per-call model switching inside a session (it loses the cache);
- anything that caps, compresses or summarises.

## What the data on disk says

All numbers are from read-only probes run on 2026-10-17. The scripts are below. "Weighted" uses the price weights
of earlier Brigadier studies: input 1, cache read 0.1, one-hour cache write 2, five-minute write 1.25, output 5.
These are estimates, because neither vendor publishes how subscription limits weigh tokens.

**Claude transcripts, 14 days** (`~/.claude/projects`: 861 files and 80,070 calls, deduplicated by message id):
- Opus 5.5 is 98.4% of weighted usage, Sonnet 5 is 1.1% and the rest is under 1%.
- About 92% is the user's own sessions (entrypoint `cli`: Delegator runs and their workers) and 6.8% their
  subagents. Brigadier-spawned sessions (`sdk-cli`) made only 153 calls here. The bench daemons' sessions aren't in
  this folder, so Brigadier's own traffic comes from the bench evidence docs and the installed app's `turn_usage`.
- **Cache:** 98.6% of all prompt tokens were cache reads. Weighted usage was 70.1% cache reads, 14.1% one-hour
  writes, 3.4% five-minute writes and 12.4% output.
- **Context:** 39.2% of weighted usage came from calls at 200–300k tokens of context, 38.7% at 100–200k and 4.8%
  above 300k.
- **What re-wrote a prefix already paid for** (the previous call's prompt minus this call's cache read, counted
  when over 2k tokens): 25.1M tokens, which is 14.6% of all cache writes. By cause:

  | Cause | Share of all cache writes |
  |---|---|
  | Idle over an hour | 9.4% |
  | Idle 5 min to 1 h | 3.0% |
  | No idle gap, cause unknown | 1.6% |
  | After a compaction | 0.6% |

  The other 85% of writes are new content. The idle-over-an-hour rewrites are about 1.8% of weighted usage.
- **Tool results:** about 41.3M tokens in all (characters ÷ 4). Bash was 88% of it, with a mean of 2,052 characters
  per result.
- **Subagent results:** 118 results came back as task notifications. The median was 5.4k characters, the p90 10.8k
  and the largest 18.7k, about 179k tokens in 14 days. They come back as conclusions, not dumps.
- **Tool calls:** 81,129 calls. 2.35% failed, mostly Bash commands exiting non-zero. **6 (0.007%)** failed on their
  arguments (`InputValidationError` and the like).
- **Copying:** the model's tool-call inputs came to about 13.9M tokens. Of that, 0.40M tokens (2.8%) were lines of
  40 or more characters that had already come back in an earlier tool result of the same session. Most were in
  Bash and Write. In the Agent tool's prompts it was 0 of 165k. At output weight that is about 0.1% of weighted
  usage.

**Codex sessions, 14 days** (`~/.codex/sessions`, 60 sessions): 97.1% of input tokens were cached.

**The installed app's `turn_usage`** (a copy of `~/Library/Application Support/Brigadier/routing.sqlite`, 61 rows,
2026-10-16 05:33 to 2026-10-17 10:55):

| Time (UTC) | Step | Model | Jobs | Calls | Cache reads | Writes | Output |
|---|---|---|---|---|---|---|---|
| 10-16 23:40–23:43 | brain | claude-haiku-4-5 | 2 | 44 | 2.55M | 0.17M | 22k |
| 10-16 23:46 – 10-17 05:04 | brain | `opus` | 5 | 165 | 11.71M | 0.48M | 99k |
| 10-16 23:12–23:48 | upkeep (model research) | a Codex model the registry tiers frontier | 41 rows | 41 | 2.64M | — | 17k |

**Brigadier's own benches** (already published):
- Phase O (`docs/evidence/benchmark-2026-10-16-phase-o.md`): calls per task were at or under plain Claude Code's,
  no invalid tool call in the 9 diagnostic runs, and none in V3 at `19ca6bdf`.
- A10 (`docs/evidence/2026-10-17-a10-earlier-rebirth.md`): rebirth at 300k cost 35% fewer tokens than at 400k, with
  the same answers.

## Every idea, with a verdict

Sources: **T** = tokenminning.ai (a community wiki; read with its guide pages), **W** = the tetrate.io
token-optimization article, **R1–R6** = the Reddit posts in the order the user pasted them (R6 = "let AI use
variables"). Both pages loaded. T gives savings ranges from outside studies, and W gives almost no numbers. Neither
shows its own evals, so both are read as marketing.

Verdicts: **have** = already in Brigadier; **partly**; **missing**; **rejected** = tried or ruled out before;
**n/a** = not applicable, or against a rule.

| # | Idea | Source | Brigadier today | Verdict | Evidence |
|---|---|---|---|---|---|
| 1 | Meter every call: per task, agent and owner, with cached tokens on their own line | T Art. I, W, R5 | `turn_usage` per task, step, account and request (`crates/core/src/routing/store.rs:36`, columns at `:79`–`:136`, `StepKind` `:152`) | have | 61 live rows with `write_1h`, `write_5m`, `missed`, `cause` |
| 2 | Judge by cost per successful outcome, not token totals | W, T, R4, R5 | Q58: tokens per merged change, first-look merge rate | partly | Decided 10-17; no code yet (no "first look" measure in `crates/`) |
| 3 | Budgets, fiscal ceilings, session or loop caps, `--max-turns`, degrade at 90% | T Art. IV, W | none | n/a (rule) | The user rejected caps and budgets of every kind (PLAN §7 Rules) |
| 4 | Route each kind of work to the cheapest model that is good enough | T, W, R1, R2, R4, R5 | Per-category floors (`crates/router/src/decide.rs:164`); the cheaper of near-equal models for scout and verify (`:188`); cheapest first for git text (`:181`); chores on `cheapest()` (`crates/core/src/manager/git_actions.rs:587`) | have, with a bug | `cheapest()` falls back to the CLI's first model (`git_actions.rs:619`): Brain jobs on `opus` (table above). Proposal 1 |
| 5 | Cheap first, escalate when a quality check fails, log each escalation | T (cascade), R3 | Fallback only on limits and errors (`crates/core/src/manager/fallback.rs:74`); a failed model is excluded from the retry (`decide.rs:239`); outcome learning (`crates/core/src/manager/outcomes.rs:1`) | partly | No escalation on a failed check. Proposal 4 |
| 6 | A frontier call needs a logged reason | T Art. II | Routing explains each pick (`decide.rs:1993` `score_why`) | have | |
| 7 | Move most calls to open-weight models | R5 | Only through CLIs Brigadier supports; local models are Phase 7 (`docs/PLAN.md:322`) | n/a now | Subscription CLIs only, no proxy |
| 8 | Hand mechanical calls inside a session to a small model | R1 (implied), T Claude Code page | Brigadier routes per session or task, not per call | n/a | A switch rewrites the whole prefix. The mean call reads 155k of context (12.4B ÷ 80k): 15.5k weighted on the same model against 310k × the other model's price ratio. It pays only if the other model is over 20× cheaper |
| 9 | Benchmark before switching models | T | Bench and A/B harness (`tools/ab`); models on trial until they have outcomes (`crates/router/src/merge.rs:15`–`17`) | have | |
| 10 | Stable prefix, dynamic content last, no timestamps or ids in the system prompt | T, W, R1, R5 | `[today]` and `[settings]` notes replace instruction text (`crates/core/src/manager/prompts.rs:832`); the note fork reuses the thread's start (`crates/core/src/manager/rebirth.rs:454`); tool set fixed at start, rare tools behind ToolSearch (`crates/providers/src/claude/mod.rs:79`–`81`) | have | 98.6% of Claude and 97.1% of Codex input cached |
| 11 | Measure the cache hit rate first | R5, T | Write causes (`store.rs:278`); missed-prefix estimate (`crates/core/src/routing/meter.rs:197`) | have (measured), partly (reported) | Not in the phase report yet. Proposal 3 |
| 12 | Longer cache lifetime, or keep the cache warm | T | One-hour writes; rebirth when the cache is cold (`crates/core/src/manager/cold.rs:34`) | partly | Keep-warm is still a proposal awaiting the user (PLAN §7 item 3). Idle-over-1h rewrites are 9.4% of writes in the user's own sessions |
| 13 | Semantic cache: answer similar questions from stored replies | T, W | none | n/a | Coding asks are unique, and a wrong cached answer breaks Q58. T itself rates code generation "low" |
| 14 | Cache results of deterministic work | W | `run_check` results kept per tree (`crates/core/src/manager/checks.rs:1`, `cached_check` `:1155`) | have | |
| 15 | Prompt hygiene: say each rule once, drop scaffolding | T, W, R2 | Phase O cut the instructions from 17.4k to 12.5k bytes and the first call from 19.6k to 17.4k tokens | have | `benchmark-2026-10-16-phase-o.md`. Q58 stops further trimming for its own sake |
| 16 | Block a merge when a prompt grows more than 5% | T Art. V, W | Size tests: `prompts.rs:3338` (thread instructions < 11,500 bytes); `flow/checks_tests.rs:505`; `flow/lean_work_tests.rs:645` | partly; keep as is | A new blocking gate goes against Q58 (tripwire, not gate) |
| 17 | Schema-enforced outputs with length limits | T, W | Typed `submit_report` (`crates/core/src/tools.rs:1327`); 3,600-byte report envelope (`crates/core/src/manager/workers.rs:97`); typed MCP tools | have | |
| 18 | Flatten nested schemas to cut retries | R5 | Bad arguments come back as a plain error (`crates/mcp-server/src/catalog.rs:870`) | n/a | 6 of 81,129 tool calls (0.007%); 0 invalid calls in Phase O |
| 19 | `max_tokens` and output caps | T, W | none | rejected | PLAN §7 "Not doing": output caps. The CLIs set this themselves |
| 20 | Ask for concise answers | W | One voice and short replies (PLAN §7 item 5) | have | Measured: no output saving (spread up to 3×) |
| 21 | Sliding window, rolling summaries, compaction | T, W | none on purpose; workers off CLI compaction (BUILD-PLAN D2.3) | rejected | The user's rule; R5 says the same failed for them |
| 22 | Truncate or summarise tool output before it enters context | T Art. III, W | Lossless instead: whole output stored as `out-<id>` with a digest (`crates/core/src/manager/tool_output.rs:1`, `crates/core/src/digest.rs:21`, `:61`); shell output whole up to 30,000 bytes (`digest.rs:27`) | have (lossless) | Lossy cuts rejected (PLAN §7 "Lossless"; cutting tool output 38% raised one bill 7%) |
| 23 | Fetch the lines needed, not whole files | T, W, R1 | `code_search`, `code_refs`, `project_map` (`crates/core/src/manager/brains.rs:2887`, `:2942`, `:2991`); Brain caps of 4 facts, 3 entities and 2 passages, then page 2 (`crates/brain/src/lib.rs:465`) | have | Read results average 2.9k characters |
| 24 | Subagents return conclusions, not raw dumps | R1, T | Report "about 800 tokens at most", longer parts as artifacts (`prompts.rs:1678`); envelope cap with `read_report` for the rest (`workers.rs:97`) | have | Subagent results: median 5.4k characters, about 179k tokens in 14 days |
| 25 | Structured memory instead of re-explaining | T, R2 | Brain, `remember`, evidence re-read on each use (`crates/core/src/manager/evidence.rs:1`) | have | |
| 26 | Clear task boundaries; brief has paths, the check and done-when | R2, T Claude Code page | `delegate_task` spec with `include_request` (`crates/core/src/tools.rs:148`, `:190`) | have | |
| 27 | Small specialised roles | R2 | Thread, workers, Brain jobs on a read-only tool set (`crates/providers/src/claude/mod.rs:84`), commit-message writer | have | |
| 28 | Improve the process from real failures | R2 | Q58 measures, GitHub issues, outcome learning | have | |
| 29 | Skip unneeded calls: batching, no detours | R4, W | Quiet envelopes batched for 15 s (`crates/core/src/manager/conversation.rs:184`); review only after a passing check (`crates/core/src/manager/unreviewed.rs:95`); the thread does small work itself (`prompts.rs:121`–`122`) | have | Phase O: calls at or under plain |
| 30 | Usage grows to fill what's saved | R4 | Q58 reports tokens per merged change | n/a (observation) | |
| 31 | Tool outputs as named variables passed by reference (`$var`) | R6 | Reading by reference exists: `out-<id>` and `read_artifact`, `read_report`, evidence by `out-<id>`, the hand-off folder (`crates/core/src/manager/worker_handoff.rs:1`). There is no `$var` in tool arguments | partly; don't build | See "Variables by reference" |
| 32 | Prompt compressors | R1, R5, W, T | none | rejected | The user's rule; 2026-09-30 study: headroom 0.45% lossless, RTK 0.21% (PLAN §7) |
| 33 | Pick the prompt by task | W | Per-role instructions; rules sent as notes when first needed (BUILD-PLAN C1 b, d) | have | |
| 34 | Short CLAUDE.md, unused MCP servers off | T Claude Code page | Project instructions capped at 32 KiB (`crates/core/src/manager/instructions.rs:39`); lazy tool groups (BUILD-PLAN C2); lean worker tools, 15.0k to 10.1k (PLAN §7 item 4) | have | |
| 35 | Use a mid-tier model for most coding work | T Claude Code page | Implement, review and merge floor at Frontier (`decide.rs:164`) | n/a | Against Q58/Q68's quality bar |
| 36 | A new session per task | T Claude Code page | Every tab and worker is its own session (Q64, Q65); worker hand-off at 300k or 70% (PLAN §7 item 2) | have | Ending sessions to save tokens is rejected |
| 37 | Batch API, priority queues | W | Account order by soonest weekly reset (Q59) | n/a | Batch discounts exist only on API billing (PLAN §7) |
| 38 | Retry with backoff | W | Transient errors retried, then a hand-off (`fallback.rs:74`) | have | |
| 39 | Count tokens before sending | W | none | n/a | The CLI builds the request |
| 40 | Local inference | T | Phase 7 (`docs/PLAN.md:322`) | planned | |
| 41 | Don't use an LLM where a rule will do | T | The engine does the plumbing (ref check `crates/core/src/manager/refcheck.rs:1`, check cache, digest, quiet envelopes); the LLM judges | have | |
| 42 | A gateway or proxy that routes and caches | T, R3 | none | n/a (rule) | No proxy that holds the user's sign-in |
| 43 | Streaming, speculative decoding, MoE, better attention | W | | n/a | Provider-side |
| 44 | RAG: fewer, re-ranked chunks | T, W | Brain caps and paging (`crates/brain/src/lib.rs:465`, "ask with page N" `:1096`) | have | |
| 45 | Measure how much of the context window is used | W | `turn_usage.context` | have | |

## Variables by reference (R6)

R6's case: a tool returns 10,000 rows of JSON, and the model copies them out again token by token as the next tool's
input. The post claims −82% tokens on a three-turn analytics task with a small hosted model.

**What Brigadier already does by reference:**
- Long thread output is stored whole as `out-<id>`. The model gets a digest and pages with `read_artifact`
  (`crates/core/src/manager/tool_output.rs:1`).
- A check's whole output is an `out-<id>` too (`checks.rs:1`). Claims cite it as evidence, and the evidence is
  re-read from the store when used (`evidence.rs:1`).
- A worker report over 3,600 bytes is shortened, with "read_report task-N gives it whole" (`prompts.rs:2102`).
- A handed-off worker gets the spec, progress, diff and transcript as files, not pasted text
  (`worker_handoff.rs:1`).
- A brief takes the user's request verbatim through a flag (`include_request`, `tools.rs:190`) instead of the
  thread retyping it.

**What `$var` would add:** references in the arguments the model writes. On our traffic that buys almost nothing:
- Coding agents read files and logs. They rarely forward a big result into another tool. Only 2.8% of tool-call
  input repeats lines from earlier results, about 0.40M tokens in 14 days, or about 0.1% of weighted usage. Agent
  briefs copied 0 of 165k tokens.
- The CLI's own tools (Bash, Edit, Write, apply_patch) parse their own arguments. Resolving `$var` there would mean
  rewriting tool calls: a PreToolUse hook on Claude, and nothing on Codex, whose hooks stay off. That is the kind of
  request rewriting the rules keep out.
- Brigadier's own tools could take `out-<id>` in place of text, as evidence already does. But the briefs that would
  use it (`delegate_task`, `message_worker`) carry prose, not data.

**Verdict:** don't build `$var`. If a future task does show a tool forwarding big stored output, the cheap form is
to let `delegate_task` accept `out-<id>` and report ids next to `attachments`. Re-measure first with
`probe2.py` (below).

## The proposals in detail

### 1. Fix the cheap model for chores

- **What's wrong:** `cheapest()` (`crates/core/src/manager/git_actions.rs:587`) looks for the family word (`haiku`
  or `luna`) among the listed models that the registry doesn't reserve for one kind of work.
  - If none matches, it takes `models.iter().find(|m| !m.legacy && available(&m.id))` (`:617`–`:619`), which is the
    first model in the CLI's list.
  - For Claude that is `opus`, the CLI's default (`cache/models-claude.json`: opus, fable, sonnet, haiku, …).
  - The recorded model of every Brain job from 23:46 on 10-16 is exactly `opus`.
  - Likely trigger: the `haiku` alias now merges to the Haiku 5.5 entry (`crates/router/src/merge.rs:236`–`250`, an
    alias that resolves to a newer model inherits the newer entry). The registry keeps that entry for operating apps
    only (`only: operate`), so the family match is filtered out.
  - The same function serves model research (`crates/core/src/manager/research.rs:212`) and Brain jobs
    (`crates/core/src/manager/brain_jobs.rs:391`). Codex model research ran on a frontier-tier model the same night;
    that cause isn't pinned down.
- **Change:**
  - Pick by the registry tier from the merged catalog: lowest tier first (light, then standard), never strong or
    frontier.
  - When nothing light or standard is available, skip the chore and log why. Spare-quota chores can wait.
  - Tests: an alias that resolves into a reserved entry; a list whose first model is frontier.
- **Measured under Q58:** quality is unchanged, because Brain nodes stay unverified and are re-checked on use
  (`evidence.rs:1`). The tripwire report (proposal 3) shows the tier of every `brain`, `upkeep` and git-text row.
- **Size:** XS–S, one function and two tests.
- **Risk:** if the CLI lists no light model at all, Brain enrichment pauses. That is acceptable; it runs on spare
  quota.
- **Follow-up issue:** this is a real bug and should become a GitHub issue (see "Needs the user").

### 2. Q68's fresh verifier at review grade

- **What's wrong:** Q68 asks, for big or risky changes, for "a fresh verifier without the build's memory" that tries
  to break the change. If it is started as a `verify` task, the router gives it:
  - the light floor (`decide.rs:164`);
  - medium effort (`decide.rs:203`);
  - the cheaper of two near-equal models (`decide.rs:188`).

  That is a blind downgrade on the one step that decides an auto-merge.
- **Change:** the auto-merge verifier runs with `quality: "high"` (Frontier floor, `crates/core/src/manager/tools.rs:250`)
  at high effort. Prefer the other vendor where it can take the work, the same way the review does. Plain "run these
  checks" verify tasks stay cheap.
- **Measured under Q58:** first-look merge rate and escaped bugs. The tripwire line names "verification" when a
  task's tokens rise because of it.
- **Size:** XS, in the auto-merge build.
- **Risk:** more tokens on big or risky changes only. That is accepted under Q58 ("verification ships").

### 3. Cache and waste lines in the tripwire report

- **Today:** `turn_usage` has `write_1h`, `write_5m`, `missed`, `cause` and `cause_missed` (`store.rs:127`–`136`,
  causes at `:278`). Nothing puts them in the phase report next to the tripwire.
- **Change:** the bench and phase report prints, per task:
  - the share of prompt tokens read from cache;
  - writes by cause;
  - the missed prefix by cause;
  - the number of `Invalid arguments for …` tool errors (`catalog.rs:870`; today nothing counts them);
  - the model tier per step for `brain`, `upkeep` and git-text rows.

  A row that breaks a simple rule is flagged, for example a chore above standard tier, or a "no idle gap, cause
  unknown" miss over 50k.
- **Measured under Q58:** this is the evidence for the tripwire's "what bought it" line. It never blocks.
- **Size:** S. A report script over `turn_usage`, plus a counter for bad tool arguments.
- **Risk:** none to quality.

### 4. Escalate a cheap chore once on a failed check

- **Today:**
  - scouts and research already start on cheaper models (`decide.rs:164`, `:188`);
  - a failed model is only excluded after an error or a limit (`fallback.rs:74`);
  - outcome learning lowers its score later (`outcomes.rs:1`).
- **Change:** when a `scout` or `research` report's `learned` lines carry evidence that no longer re-reads
  (`evidence.rs` gives the verdict), or the report has no evidence at all for a factual claim, Brigadier reruns the
  task once, one tier up. It records the escalation (cheap model, reason, new model) in the routing explanation and
  in `outcomes`. It escalates once, never downward, and implement, review and merge are untouched.
- **Measured under Q58:** the scout bench with known answers (the 2026-10-01 A/B scored 13 facts per answer) and
  V3. It must show no fewer facts than today. Escalations per 100 chores go in the report.
- **Size:** M.
- **Risk:** an escalation loop. The cap of one escalation is a routing rule, not a usage cap: the task still
  finishes on the stronger model.

## Needs the user's decision

- **Keep the cache warm** (PLAN §7 item 3, still waiting on the user's view about background calls). New number:
  rewrites after more than an hour idle are 9.4% of all cache writes, about 1.8% of weighted usage. That is in the
  user's own sessions; Brigadier threads already rebirth when cold. No proposal here, because the measured share
  is too small to bring the question back.
- **May chores use Haiku 5.5?** The registry keeps it for operating apps only. Proposal 1 doesn't need it.
  Recommendation: leave the registry as it is.

Nothing in proposals 1–4 changes a settled decision. Proposal 2 applies Q68 as written.

## Paste-ready for the Workspaces/auto-merge Delegator prompt

> Fold in these items from `docs/research/token-sources-2026-10-17.md`:
> 1. Fix `cheapest()` (`crates/core/src/manager/git_actions.rs:587`). Chores pick the lowest registry tier
>    available (light, then standard), never strong or frontier, and skip with a log line otherwise. Brain jobs ran
>    on Opus overnight on 10-16/17 because the fallback took the CLI's first model. Add tests for an alias that
>    resolves into a reserved entry and for a list that starts with a frontier model.
> 2. Q68's fresh verifier runs with `quality: "high"` at high effort, on the other vendor where it can, never on
>    the `verify` category's light floor or the cheaper of two near-equal models. Plain check-running verify tasks
>    stay cheap.
> 3. The phase and bench report prints, per task: cache-read share, writes and missed prefix by cause (from
>    `turn_usage`), a count of bad tool arguments, and the tier behind each brain, upkeep and git-text row. It flags
>    a chore above standard tier. The tripwire line cites these numbers; nothing blocks.
> 4. After 1–3, if time allows: escalate a scout or research task once, one tier up, when its evidence fails to
>    re-read, and log why. Prove it with the known-answer scout bench and V3.
>
> Don't build: `$var` arguments, schema flattening, per-call model switches, caps, compression or summarising.
> The research has the numbers.

## How the numbers were made

Read-only scripts, kept outside the repo during the review:
- `probe.py`: weighted usage, cache shares, prefix rewrites by idle gap, tool-result sizes and errors, and model mix
  over `~/.claude/projects`, deduplicated by message id.
- `probe2.py`: the same split by entrypoint, plus copied lines (a line of 40 or more characters in a tool call's
  input that already came back in an earlier tool result of the same session, by exact hash).
- `notif.py`: the sizes of subagent results returned as task notifications.
- `codex.py`: each Codex session's last token count.
- `turn_usage`: `sqlite3` on a copy of the installed app's `routing.sqlite`.

Limits:
- Tokens for tool text are estimated as characters ÷ 4.
- The idle causes are inferred from the time gap, not from the CLI.
- The copy measure misses copies shorter than 40 characters or reformatted.
- The 14 days are mostly the user's own Delegator sessions. Brigadier's own bench traffic comes from the evidence
  docs.
