# R1: What the vendors say and ship on long sessions (to 14 Oct 2026)

Scope: Anthropic and OpenAI official docs, engineering posts, cookbooks and changelogs, newest first, checked against the installed `claude` 2.1.299 and `codex` 0.162.1.

Tags used below:
- Evidence: **M** = measured, with numbers. **V** = vendor claim. **A** = anecdote or single run.
- Applicability: **CLI-help** = seen in installed help output. **CLI-bin** = string found in the installed binary or generated protocol schema, but not in help. **CLI-doc** = documented for the CLI but not checked locally. **API** = API-only. **?** = unclear.

Codex documentation URLs are given as `developers.openai.com/codex/...`. They currently redirect to another OpenAI docs host, and the text was read there.

---

## 1. Context engineering for long-running agents

### Anthropic

- **https://code.claude.com/docs/en/context-window** (undated, current). After compaction:
  - The system prompt and output style still apply. The root CLAUDE.md, unscoped rules, auto memory and the plan-mode plan are re-injected from disk. Git status is re-read.
  - Up to 5 most-recently-modified read or edited files are re-read. A file over 5,000 tokens comes back as a path only.
  - Invoked skill bodies are re-injected, capped at 5,000 tokens each and 25,000 in total, dropping the oldest first. The skill listing is not reloaded.
  - Path-scoped rules and nested CLAUDE.md files are summarized away. Context added by hooks is summarized too.
  - SessionStart hooks with matcher `compact` run and their output is added.
  - Since v2.1.198 the summary request uses the session's thinking setting.
  - V / CLI-doc. Keys: `/compact <focus>`, `/rewind` "Summarize from here/up to here", `/autocompact 500k`.
- **https://code.claude.com/docs/en/model-config** (undated). Auto-compact runs at the model limit by default. Native-1M models compact at "about 967K tokens". Sonnet/Opus 4.6 without 1M, or with `CLAUDE_CODE_DISABLE_1M_CONTEXT=1`, compact at 200K.
  - The window can be set from 100K to 1M through `/autocompact` (saved per model under `modelSettings` since 2.1.288), the `autoCompactWindow` setting, the `--autocompact` flag or the `CLAUDE_CODE_AUTO_COMPACT_WINDOW` env var (highest precedence).
  - V / **CLI-help** (`--autocompact <auto|tokens>`). The binary also contains `autoCompactEnabled`, `DISABLE_AUTO_COMPACT`, `DISABLE_COMPACT` and `CLAUDE_AUTOCOMPACT_PCT_OVERRIDE` (CLI-bin).
- **https://code.claude.com/docs/en/settings-reference#idlecompaction** (undated). Idle compaction is on by default when all of these hold:
  - an interactive terminal session;
  - the 1-hour cache TTL;
  - context of at least 200,000 tokens (change with `CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS`);
  - about 54 minutes idle.

  It compacts shortly before the cache expires. Turn it off with `idleCompaction: false`. The prompt-caching page says this needs v2.1.290+, but the changelog lists it as "Added" in 2.1.299. V / CLI-doc + CLI-bin.
- **https://code.claude.com/docs/en/sessions** ("Resume from a summary"). On Pro/Max, resuming a session idle for more than about 1 hour and over 100,000 tokens offers a dialog: "Resume from summary" (runs `/compact`), "as-is", or "Don't ask again". V / CLI-doc (interactive only).
- **https://code.claude.com/docs/en/sub-agents** (undated).
  - Subagents get a fresh context and return a summary. A *fork* inherits the full history, system prompt, tools and model, and shares the parent's cache. Fork mode is on by default in interactive sessions and off with `-p`/SDK; override with `CLAUDE_CODE_FORK_SUBAGENT`.
  - Nesting goes up to 3 levels deep. The default limit is 20 concurrent subagents (`CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS`). Stopped or finished subagents can be resumed with `SendMessage`.
  - Frontmatter `autoCompactWindow` (v2.1.296+; write it as a plain integer, because "200k" parses as 200), `effort`, and `experimental.cacheTtl: 5m|1h`. Subagents auto-compact on their own.
  - V / CLI-doc. The `--agents` JSON is **CLI-help**.
- **https://code.claude.com/docs/en/costs** (undated).
  - Clear between tasks.
  - Put "# Compact instructions" in CLAUDE.md.
  - Keep CLAUDE.md under 200 lines and move workflow detail into skills, which load on demand.
  - Delegate verbose operations to subagents.
  - Use a PreToolUse hook to filter test output, cutting a 10k-line log "from tens of thousands of tokens to hundreds".
  - Agent teams use "approximately 7x more tokens".
  - "`/compact` reads the conversation it summarizes … `/clear` costs nothing."
  - Grades: V, with M only for the team multiplier. CLI-doc.
- **https://www.anthropic.com/engineering/managed-agents** (8 Apr 2026). The session log is a durable context store outside the context window, and the "brain" (harness) is separated from the "hands" (sandboxes) so a crash loses no state. Only the search snippet was seen; the page was not opened. V / ? (hosted product).
- **https://www.anthropic.com/engineering/harness-design-long-running-apps** (24 Mar 2026).
  - Context resets (a fresh agent plus a structured handoff file) beat compaction for Sonnet 4.5, which showed "context anxiety". Opus 4.5 "largely removed" it, so the harness switched to one session with automatic compaction. Opus 4.6 removed the need for sprint decomposition.
  - A separate evaluator agent beats self-evaluation, because agents were "confidently praising the work".
  - Agents communicate through files and a "sprint contract" that defines "done".
  - Costs: solo run 20 min/$9 versus full harness 6 h/$200; simplified harness 3 h 50 min/$124.70.
  - A (single runs, with numbers) / applies as a pattern.
- **https://www.anthropic.com/research/long-running-Claude** (23 Mar 2026). Long runs used CLAUDE.md, a CHANGELOG.md progress file and a "Ralph loop" (re-prompt until the success criteria are met). Only the search snippet was seen. A / pattern.
- **https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents** (26 Nov 2025).
  - The first session is an initializer that writes `init.sh`, `claude-progress.txt`, a JSON feature list (all marked failing) and the first commit.
  - Later sessions take one feature at a time, test end-to-end before flipping `passes`, then commit and update the progress file.
  - Start each session with this routine: pwd, git log, progress file, `init.sh`, then a smoke test.
  - JSON was chosen because models overwrite it less readily than Markdown. "Compaction alone wasn't enough." No benchmark numbers.
  - V/A / pattern.
- **https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/claude-prompting-best-practices** (current). For multi-window work:
  - Use a different first-window prompt.
  - Keep `tests.json`, `progress.txt` and `init.sh`.
  - "Consider starting with a brand new context window rather than using compaction… latest models are extremely effective at discovering state from the local filesystem."
  - Tell the model that its context will be compacted so it does not stop early (sample prompt provided).
  - V / pattern (works through the prompt).
- **https://www.anthropic.com/engineering/effective-context-engineering-for-ai-agents** (29 Sep 2025).
  - Context rot: recall degrades as the window fills. Aim for "the smallest possible set of high-signal tokens". Prefer just-in-time retrieval (paths and grep over preloading).
  - Claude Code's compaction keeps decisions, unresolved bugs and implementation details, plus the 5 most recent files. "Tool result clearing" is the "lightest-touch form of compaction".
  - Use structured notes (NOTES.md). Subagents return summaries of "often 1,000-2,000 tokens".
  - V / pattern.
- **https://www.claude.com/blog/context-management** (29 Sep 2025). Context editing plus the memory tool gave **+39%**; context editing alone **+29%** on an internal agentic-search eval; **84% fewer tokens** on a 100-turn web-search eval. M (vendor-internal) / API.
- **https://platform.claude.com/docs/en/build-with-claude/context-editing** (current).
  - `clear_tool_uses_20250919`: defaults are trigger 100k input tokens and keep 3; also `clear_at_least`, `exclude_tools`, `clear_tool_inputs`.
  - `clear_thinking_20251015`. Beta header `context-management-2025-06-27`.
  - Clearing invalidates the cache, so clear enough at once to be worth it.
  - V / **API**. The installed `claude` binary contains `clear_tool_uses_20250919` and `toolResultClearingHeldStateless`, and the costs page counts "clearing old tool results" as an "expected rebuild". So Claude Code appears to clear tool results itself, but no user setting is documented (CLI-bin, ?).
- **https://platform.claude.com/docs/en/build-with-claude/compaction** (current). Server compaction is "on demand" (beta `compact-2026-09-04`, top-level `compaction`) or "at a token threshold" (`compact_20260112` edit). It can keep recent turns verbatim, run in the background, and take a custom summary prompt. V / **API** (the CLI uses its own client-side compaction).
- **https://www.anthropic.com/engineering/multi-agent-research-system** (13 Jun 2025).
  - Multi-agent beat single-agent Opus 4 by **90.2%** on an internal research eval. Agents use about **4x** the tokens of chat; multi-agent about **15x**.
  - Token use alone explains **80%** of the variance on BrowseComp. Parallelism cut research time "up to 90%".
  - Not a good fit for most coding: there are few truly parallel parts and contexts must be shared.
  - Subagent briefs need an objective, an output format, tools and boundaries.
  - M (internal) / pattern.

### OpenAI

- **developers.openai.com/codex/config-file/config-reference** (current). Relevant keys:
  - `model_auto_compact_token_limit` ("unset uses model defaults"), `compact_prompt`, `experimental_compact_prompt_file`, `model_context_window`.
  - `tool_output_token_limit` ("token budget for storing individual tool/function outputs in history").
  - `project_doc_max_bytes` (the AGENTS.md cap).
  - `features.memories` (off by default) and the `memories.*` family.
  - `features.multi_agent` (`spawn_agent`, `send_input`, `resume_agent`, `wait_agent`, `close_agent`) and `agents.max_concurrent_threads_per_session`.
  - `hooks` and `history.persistence`.
  - V / **CLI-bin**: all compaction keys are present as strings in the 0.162.1 binary and settable with `-c key=value` (**CLI-help** shows `-c`). The binary also has an undocumented `model_post_turn_compact_threshold_percent` (probably PR #46541, "opt-in compaction after final responses"), plus `auto_compact_fallback_prompt` and `auto_compact_fallback_buffer_tokens`. ?
- **developers.openai.com/codex/app-server** (current).
  - `thread/compact/start` returns `{}` and streams a `contextCompaction` item. The installed schema also has a `thread/compacted` notification.
  - `thread/fork` takes `lastTurnId` (inclusive), `excludeTurns` and `ephemeral`.
  - `thread/resume` takes `excludeTurns`. Resuming on a different model warns and injects a one-time model-switch note.
  - `thread/inject_items` "appends raw Responses API items to a loaded thread's model-visible history without starting a user turn".
  - `turn/start` `model`/`effort` overrides "become the defaults for later turns". `turn/steer` needs `expectedTurnId`.
  - `thread/goal/set` takes `objective` (≤4,000 chars) and `tokenBudget`, and reports `tokensUsed` and `timeUsedSeconds`.
  - V / **CLI-bin**: all present in the schema generated by `codex app-server generate-json-schema` (105 client methods, 84 notifications).
- **developers.openai.com/codex/customization/memories** (current). Memories are off by default (`[features] memories = true`), stored under `~/.codex/memories/` and generated in the background after a chat goes idle. "Treat memories as a helpful recall layer, not as the only source for rules that must always apply." Required rules go in AGENTS.md. V / CLI-doc (`codex features list` shows `memories stable false`).
- **https://developers.openai.com/api/docs/guides/compaction** (undated).
  - Server-side `context_management: {compact_threshold: N}` (example 200000), or the stateless `/responses/compact`.
  - State carries forward as an opaque encrypted compaction item. "Do not prune `/responses/compact` output."
  - V / **API** (Codex uses "remote compaction" internally; the 0.162.0 notes add remote-compaction capability for custom providers).
- **https://developers.openai.com/blog/run-long-horizon-tasks-with-codex** (undated, GPT-5.3-Codex era). One run took "about 25 hours", "about 13M tokens" and produced "about 30k lines". Continuity came from `prompt.md` (spec plus "Done when"), `plans.md` (milestones plus validation commands plus a stop-and-fix rule), `implement.md` (runbook) and `documentation.md` (status log). A (one run) / pattern.
- **https://developers.openai.com/cookbook/articles/codex_exec_plans** (undated, about Oct 2025). A PLANS.md "ExecPlan" is a living spec (progress, decision log), triggered by an AGENTS.md rule; it is said to have enabled "more than seven hours from a single prompt". Search snippet only; not opened. A / pattern.
- **https://developers.openai.com/blog/skills-shell-tips** (undated). Treat compaction as "a default long-run primitive, not an emergency fallback". Write skill descriptions as routing logic, with negative examples. One customer case went from 73% to 85% accuracy with 18.1% faster time to first token. A/M (customer) / API-oriented.
- **https://openai.com/index/gpt-5-1-codex-max/** (Nov 2025, search snippet only). The model is "natively trained to operate across multiple context windows" through compaction. V / ?.

---

## 2. Prompt caching

### Anthropic

- **https://code.claude.com/docs/en/prompt-caching** (current; the most directly useful page).
  - **TTL by request bucket.**
    - On a subscription within plan usage, the *main conversation* gets **1 hour**. Everything else gets **5 min**: subagents, workflows, forks, compaction and titles, apart from a few server-controlled helpers.
    - Once you draw on usage credits, or use an API key or cloud provider, everything gets 5 min.
  - **TTL overrides** (v2.1.242+):
    - `promptCacheTtl` / `CLAUDE_CODE_PROMPT_CACHE_TTL` for the main conversation.
    - `subagentPromptCacheTtl` / `CLAUDE_CODE_SUBAGENT_PROMPT_CACHE_TTL` for everything else.
    - Subagent frontmatter `experimental.cacheTtl`.
    - `ENABLE_PROMPT_CACHING_1H=1` (both buckets) and `FORCE_PROMPT_CACHING_5M=1` (highest precedence).
    - To verify, run `claude -p "hello" --output-format json` and read `usage.cache_creation.ephemeral_1h_input_tokens`.
  - **Invalidates the cache:**
    - Model switch, including `opusplan` toggles, fallback models and skills with a `model:` field.
    - An effort change on most models. The exception is Opus 5.5, Sonnet 5.5, Haiku 5.5 and Fable 5.1 on an API key or subscription, where the "cache stays intact", but not on Bedrock, Vertex, gateways or with `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS`.
    - Turning fast mode on, once per conversation.
    - An MCP server connecting or being removed, **only when tool search is not deferring tools**. With deferral (the default), the tool list from the first request is kept.
    - Enabling a plugin that adds MCP servers, or a bare-name tool deny without tool search.
    - Compaction (the conversation layer only).
    - Image batch eviction.
    - A CLI upgrade.
  - **Keeps the cache:** file edits (a reminder is appended), CLAUDE.md edits (which do *not* apply until `/clear`, `/compact` or restart), permission-mode changes, output-style changes, skills and commands, `/recap`, `/rewind` and spawning subagents.
  - Compaction reuses the parent prefix and appends the instruction, so a warm `/compact` is cheap and a cold one reprocesses everything.
  - On resume, the system prompt is the one recorded at the start, until the next compaction.
  - Cache scope is one machine plus one directory. Parallel sessions in the same directory share it. Sequential sessions share it only if the git-status snapshot matches.
  - `/usage` shows a "Prompt cache (main)" line with hit %, misses, "likely cause" (v2.1.260+), expected rebuilds and warm/cold state.
  - V / CLI-doc. The env vars are CLI-bin: `ENABLE_PROMPT_CACHING_1H`, `FORCE_PROMPT_CACHING_5M` and `DISABLE_PROMPT_CACHING*` were all found.
- **https://claude.dev/blog/lessons-from-building-claude-code-prompt-caching-is-everything** (30 Apr 2026, Thariq Shihipar).
  - The team alerts on cache hit rate and declares SEVs when it drops.
  - Never change tools mid-session: plan mode is implemented *as tools*, and MCP tools are `defer_loading` stubs.
  - Don't switch models mid-session. Switching Opus to Haiku at 100k tokens costs more than staying; hand off to a subagent instead.
  - Put updates in `<system-reminder>` messages, not system-prompt edits.
  - Order the prompt static-first.
  - Cache-safe compaction costs "one tenth the price", producing a summary of "about 20k tokens".
  - V/M (directional) / explains CLI behaviour.
- **https://platform.claude.com/docs/en/build-with-claude/prompt-caching** (current).
  - TTL is 5 min (default) or `"ttl":"1h"`. Writes cost 1.25x (5m) or 2x (1h). Reads cost 0.1x, or 0.05x on Opus 5.5/Sonnet 5.5.
  - Minimum length is 512 tokens on Opus 5.5, Sonnet 5.5 and Haiku 5.5.
  - Hierarchy is tools, then system, then messages. A tool-definition change invalidates everything.
  - Thinking config and `output_config.effort` "always invalidate message blocks", and tools/system too on some models. Setting effort explicitly to the model default does not invalidate.
  - Appending a `role:"system"` message, or a per-message effort change, keeps the prefix on 5.x models.
  - The cache lookback is 20 blocks. Caches are isolated per workspace.
  - V / API (it explains why the CLI behaves as it does).
- **https://platform.claude.com/docs/en/build-with-claude/prompt-engineering/prompting-claude-opus-5-5** (current). "Changing the top-level `effort` value between requests invalidates the prompt cache… use a per-message effort change (beta) instead." Adding a tool or system text mid-session "invalidates earlier thinking blocks". V / API, which the CLI evidently uses on 5.5 models (see above).

### OpenAI

- **https://developers.openai.com/api/docs/guides/prompt-caching** (current).
  - On GPT-5.6+, the minimum is 1,024 visible tokens and the TTL is a minimum of `"30m"` (`prompt_cache_options.ttl`), with optional `prewarm`. Earlier models use `prompt_cache_retention` of `in_memory` (5–10 min, up to 1 h) or `24h`.
  - The whole rendered prefix must match. It depends on `model`, `tools`, `parallel_tool_calls`, `text.format`, `reasoning.effort`, `text.verbosity` and `context_management`.
  - "On GPT-6 models, append a `configuration_update` input item to change reasoning effort" and keep the cache.
  - Reads are up to 95% off (0.1x; 0.05x on GPT-6.1 Sol). Writes cost 1.25x.
  - `prompt_cache_key` only influences routing (about 15 requests/min per key).
  - Cached tokens still count against TPM.
  - V / API. `configuration_update` appears in the codex binary (CLI-bin), but whether Codex uses it for mid-thread effort changes is unverified.
- **https://openai.com/index/unrolling-the-codex-agent-loop/** (early 2026; **could not open, 403**; search snippet only). Each new prompt is an exact prefix extension of the last. An MCP tool-ordering bug caused cache misses. Codex appends rather than edits. V / unverified.
- **Codex 0.156.0 release notes** (22 Sep 2026): "#44862 Preserve parent cache affinity for ephemeral forks". 0.158.0: "#47709 Route resume prewarm through the cached WebSocket session". V / CLI.

---

## 3. Hallucination and grounding

- **https://platform.claude.com/docs/en/test-and-evaluate/strengthen-guardrails/reduce-hallucinations** (current). Techniques:
  - Explicitly allow "I don't know".
  - For documents over 20k tokens, extract word-for-word quotes first.
  - Cite a quote for every claim, and retract any claim without a supporting quote.
  - Review the summarized thinking, run best-of-N and compare, iterate, and restrict answers to the supplied documents.
  - "Don't eliminate them entirely."
  - V / works through the prompt (CLI-compatible).
- **Claude prompting best practices** (URL in §1).
  - The `<investigate_before_answering>` block: "Never speculate about code you have not opened. If the user references a specific file, you MUST read the file before answering."
  - Self-check: "Before you finish, verify your answer against [test criteria]." The page notes Opus 5 verifies well without being asked.
  - V / prompt.
- **Prompting Claude Opus 5.5** (URL in §1).
  - "much less likely to state an incorrect figure or cite the wrong source."
  - For unattended runs, treat a text-only end of turn as a report, not as done. Keep a checklist file, optionally checked by a smaller model. Cap at 2–3 auto-continuations.
  - Wrap pasted or untrusted text in `<pasted_content id=…>` tags.
  - V (internal testing) / prompt plus harness.
- **harness-design-long-running-apps** (24 Mar 2026). A separate skeptical evaluator beats self-grading. A, with costs / pattern.
- **https://openai.com/index/why-language-models-hallucinate/** (5 Sep 2025; search snippet). Hallucinations come from training and evaluation rewarding guessing. Abstaining should beat a wrong answer. gpt-5-thinking-mini abstained 52% with 26% errors, versus o4-mini at 1% abstention and 75% errors. M / informs prompt design (reward "I don't know").
- **Codex prompting guide** — https://developers.openai.com/cookbook/examples/gpt-5/codex_prompting_guide (undated).
  - "Changes should always pass build and type-check." If something could not be verified, say so and suggest steps.
  - Search for existing patterns before adding code.
  - Treat `Lxxx:` line prefixes as metadata.
  - No explicit file:line citation rule was found in either vendor's docs.
  - V / prompt.

---

## 4. Latency and small tasks

- **https://platform.claude.com/docs/en/test-and-evaluate/strengthen-guardrails/reduce-latency** (current). Pick the fastest adequate model: Haiku 5.5 at `effort: low`. Shorten both prompt and output, cap `max_tokens`, and stream. V / CLI via `--model haiku --effort low` (**CLI-help**).
- **Prompting Claude Opus 5.5** (current).
  - Medium is the default effort. Opus 5.5 at medium is at least as good as Opus 5 at high, and "`low` comes close … at much lower cost" on several coding evals. "Lowering effort reduces thinking… more reliably than prompt instructions."
  - Removing "think carefully" lines made replies start sooner.
  - "Treat earlier answers as settled" cuts follow-up thinking.
  - Time budgets such as `elapsed 340s / 1200s` make agent teams finish sooner.
  - Output is ">30 percent faster" than Opus 5.
  - V/M (internal) / effort is **CLI-help** (`--effort low…max`). The "settled answers" and time-budget patterns work through the prompt.
- **https://code.claude.com/docs/en/model-config**. Effort defaults are `medium` on Opus/Sonnet/Haiku 5.5 and `high` elsewhere. Precedence is `CLAUDE_CODE_EFFORT_LEVEL`, then `--effort`/`/effort`, then `modelSettings`/`effortLevel`, then the model default. Subagent or skill frontmatter `effort` overrides the session level. Since 2.1.292 the Agent tool takes an `effort` parameter. V / CLI-help + CLI-doc.
- **Claude Code costs page**: use Sonnet for most coding and `model: haiku` for simple subagents. **Lessons post**: a model switch costs more than it saves late in a session; use a subagent with its own small model instead. V.
- **https://developers.openai.com/api/docs/guides/latency-optimization** (undated). Output tokens dominate: "cutting 50% of your output tokens may cut ~50% of your latency", whereas cutting 50% of input gives "1–5%". Make fewer requests, parallelize, stream, and don't default to an LLM. V (rule of thumb with numbers).
- **Codex prompting guide**. `medium` is "a good all-around interactive coding model"; use `high`/`xhigh` for the hardest tasks. Maximize parallel tool calls ("Never read files one-by-one"). V / `-c model_reasoning_effort=…` (CLI-bin), or `effort` in `turn/start` (CLI-bin).
- **Multi-agent research post**: multi-agent is a poor fit for low-value tasks, which pay roughly 15x tokens. M.

---

## 5. Changelogs

### Claude Code 2.1.283 → 2.1.299 (github.com/anthropics/claude-code CHANGELOG.md)

**2.1.299**
- Added idle compaction: 200K+ tokens idle about 54 min; `CLAUDE_CODE_IDLE_COMPACT_MIN_TOKENS`; `idleCompaction:false`.
- `/usage` warns about max effort, workflows, always-on context and large tool results.
- `UserPromptSubmit` hooks now run *before* a skill or slash command expands.
- `/reload-skills` re-sends the skill list ("how to get skills back after compaction").
- "Prompt is too long · automatic compaction failed" near 200K is now reported instead of the request being sent.

**2.1.296**
- `autoCompactWindow` in subagent frontmatter and `--agents`.
- `CLAUDE_CODE_WORKFLOW_SUBAGENT_MODEL`.
- Fixed managed PostToolUse `updatedMCPToolOutput` not applying.
- Fixed resumed subagents being told the user rejected an interrupted call.
- Token counts fixed for adaptive-only models.

**2.1.295**
- `onFailure: "block"` for hooks.
- Fixed `-p` text output dropping earlier responses.
- Fixed async SessionStart context being re-added on every resume.
- Fixed Claude not being told it can resume background subagents.

**2.1.293**
- Haiku 5.5 is the default Haiku (1M context).
- Fixed Claude treating its last actions before compaction as done after it.

**2.1.292**
- Agent tool `effort` parameter.
- Hook output containing `<system-reminder>` is escaped.
- Plan mode is restored on resume.
- Fixed `-p` killing background commands 5 s after the result.

**2.1.290**
- Fixed resumed subagents losing thinking and **prompt cache** after a mid-run message.
- Fixed compaction "null is not an object".
- Fixed effort changing on a fallback-model retry.
- Fixed permission checks skipped after a PreToolUse rewrite.
- Faster resume of large sessions.

**2.1.288**
- Fixed long conversations failing with "Prompt is too long" instead of auto-compacting.
- Fixed `--resume` dropping context restored by compaction.
- Fixed resume losing the last response, and losing thinking for sessions from 2.1.286 or earlier.
- `/autocompact` window saved per model.
- `InstructionsLoaded` hook gains `agent_id`.

**2.1.287**
- Fixed CLAUDE.md attached twice after resume or compaction.
- Fixed stream-json not streaming `context: fork` skill turns.
- Large MCP results use less memory.
- Effort is kept on automatic model switches.

**2.1.286**
- Fixed `--resume`/`--continue` losing turns after parallel tool calls.
- Fixed API 400 when a hook returns a non-string.
- `--bare` sends no system reminders and connects only the named MCP servers.
- The autocompact-thrashing error names a dropped window.

**2.1.285**
- Fixed compacting or resuming failing or opening without history.
- Fixed `CLAUDE_CODE_RESUME_INTERRUPTED_TURN` re-running a max-turns turn.
- Auto-memory can no longer be turned on from a background session.

**2.1.284**
- Sonnet 5.5 is the default Sonnet (1M).
- Fixed "Prompt is too long" persisting after compacting.
- Fixed MCP "No such tool" on resume.
- Ultracode becomes a separate toggle in `/effort`.

**2.1.283**
- `/doctor prompt-audit` for CLAUDE.md, skills and agents.
- `/context` counts MCP server instructions.
- `DISABLE_PROMPT_CACHING_HAIKU` now covers the main model.
- Fixed SDK sessions losing a deferred tool result.
- `--system-prompt` and `--system-prompt-file` can be combined.
- The compaction spinner counts summary tokens.

Related earlier items, outside the requested range:
- 2.1.281: fixed the cache being lost when an MCP server disconnects.
- 2.1.275: fixed a cache miss on a host-app model switch.

### Codex CLI 0.155.0 (17 Sep) → 0.162.1 (9 Oct 2026) (github.com/openai/codex releases)

**0.155.0**
- Effort is preserved through compaction and pinned during config overrides (#43795, #43796).
- Memory v2 (#43797–#43827).
- Tool-output truncation budgets are preserved across resume and fork (#44248).
- Streamed remote compaction is always used (#44255).
- Forked sessions are distinguished in session-start hooks (#44349).
- Goals block after 3 empty auto-continuations (#44320).

**0.156.0** (22 Sep)
- Removed the deprecated `thread/rollback` (#44915).
- Parent cache affinity for ephemeral forks (#44862).
- History tokens are estimated from content (#45094).
- Opt-in compaction after final responses (#46541).
- Thread instruction providers can share updates with subagents (#46577).

**0.157.0** (25 Sep)
- Resume metadata in compaction checkpoints (#47323).
- **Resume model context from the latest compaction boundary** (#47365).
- Agent resume centralized (#47244).

**0.158.0** (28 Sep)
- Unchanged-model compaction shortcut for all session sources (#47934).
- User text parts are preserved during local compaction (#48115).
- Extension hooks for model requests and streams (#47679).
- In-memory agent message board for ephemeral sessions (#47946).
- Effort shortcuts reach Max (#48116).

**0.159.0** (29 Sep)
- Item anchors in `thread/items/list` (#48151).
- Model and access-program pairs preserved during compaction (#48224).

**0.160.0** (1 Oct)
- Unsent queued messages resume after reconnect (#49105).
- Subagents keep starting environments (#49075).
- Lifecycle extensions are told about compaction usage limits (#49097).

**0.161.0** (7 Oct)
- "Thread resume includes the latest committed history" (#49599).
- Explicit goal edits persist in model history (#49598).
- Hook matchers are compiled at discovery (#49379).
- Fork shortcut in the TUI command center.

**0.162.0** (8 Oct)
- Remote compaction for custom providers (#50459).
- **Install full context in compaction replacement history** (#51117).
- **Removed partial-history subagent forks** (#51329).
- Turn attribution preserved across recovery and compaction (#51402).
- Resolved model and effort recorded in sub-agent activity (#51463).
- **Tool declaration mode preserved across resumed context windows** (#51480).
- Dynamic tool inheritance for V2 subagents (#50082).

### Codex hooks (developers.openai.com/codex/hooks, current)

- **Events:** PreToolUse, PermissionRequest, PostToolUse, PreCompact, PostCompact, UserPromptSubmit, SubagentStart/Stop, Stop, Interrupt, SessionStart and SessionEnd. The binary confirms `pre_compact`/`post_compact`.
- **PostToolUse output replacement:** `updatedMCPToolOutput` is "parsed but not supported yet", and the hook run is marked failed. The only way to replace output is `decision:"block"` + `reason`, which substitutes feedback text.
- **additionalContext** works on SessionStart, PreToolUse, PostToolUse, UserPromptSubmit and SubagentStart. Values over the threshold spill to disk (`additionalContextLimit`, default 2,500 tokens).
- **PreCompact/PostCompact** receive `trigger` (manual or auto). `continue:false` stops compaction. Neither can add instructions.
- **SessionStart** sources: startup, resume, clear, compact and fork.
- **Stop** with `decision:"block"` turns `reason` into a continuation prompt.

### Claude Code hooks (code.claude.com/docs/en/hooks, current)

- **PostToolUse** `updatedToolOutput` replaces output for **all tools**. It must match the tool's output shape, and for built-ins a non-matching value is ignored; Bash, for example, uses `{stdout, stderr, interrupted, isImage}`. `updatedMCPToolOutput` is MCP-only and legacy.
- **additionalContext** over 10,000 characters is written to a file, and Claude gets a 2,000-character preview.
- **SessionStart** input on resume or fork includes `prompt_cache_likely_expired`, `context_tokens`, `seconds_since_last_response` and `estimated_cache_write_usd`.
- **PreCompact** (matcher manual|auto) can block compaction. **PostCompact** has side effects only.
- **CLI-bin:** `updatedToolOutput`, `PreCompact`, `PostCompact`, `SubagentStart` and `SubagentStop` are all present. `--include-hook-events` is in **CLI-help**.

### Token reporting

- **Claude:** stream-json `usage` has `cache_read_input_tokens`, `cache_creation_input_tokens` and `cache_creation.ephemeral_{1h,5m}_input_tokens`. The status line exposes `current_usage` and `prompt_cache` (v2.1.251+).
- **Codex:** `thread/tokenUsage/updated` → `ThreadTokenUsage{last,total,modelContextWindow}`, each a `TokenUsageBreakdown{inputTokens, cachedInputTokens, cacheWriteInputTokens, outputTokens, reasoningOutputTokens, totalTokens}`. The installed schema has `account/usage/read` and `account/rateLimits/read`. CLI-bin.

---

## 6. Local CLI verification (help commands only; no session started)

- **`claude --help` (2.1.299)**:
  - Context and sessions: `--autocompact`, `--effort`, `--fork-session`, `--resume`/`--continue`, `--session-id`, `--agents`, `--forward-subagent-text`.
  - Prompt and caching: `--system-prompt-snapshot <on|off>` (records the prompt once and reuses it "verbatim on every request and resume… until the conversation is compacted") and `--exclude-dynamic-system-prompt-sections` (moves cwd, env, memory paths and git status out of the system prompt "for cross-user prompt-cache reuse").
  - Output and limits: `--include-hook-events`, stream-json in and out, `--json-schema`, `--max-budget-usd`.
  - `--bare` is **not usable on subscription**: "OAuth and keychain are never read".
  - Not in help (binary or docs only): `idleCompaction`, `autoCompactWindow`, the cache-TTL env vars, `updatedToolOutput`.
- **`codex` 0.162.1 help**:
  - Flags: `-c key=value`, `--enable/--disable`, `--strict-config`.
  - Commands: `exec resume|fork`, `--ephemeral`, `--json`, `--output-schema`, `resume`, `fork`, `queue`.
  - `app-server` with `--listen stdio|unix|ws` and `generate-json-schema`.
- **`codex features list`**:
  - Stable and on: `multi_agent`, `goals`, `hooks`.
  - Stable but off: `memories`, `multi_agent_v2`.
  - Under development and off: `context_management`, `token_budget`, `reasoning_effort_override`.
  - Compaction keys and `configuration_update` appear only in the binary.

---

## Candidate bounded probes (about 5 minutes each; the docs can't settle these)

1. **Claude worker TTL bucket.** Does a Brigadier-spawned `claude -p` / stream-json session on subscription get the 1 h TTL (it is a "main conversation") or 5 min? Check with one tiny `--output-format json` call and read `usage.cache_creation.ephemeral_1h_input_tokens`.
2. **Claude effort change keeps the cache.** On Opus 5.5 with subscription: turn 1 at `--effort medium`; resume the same session at `--effort low`. Compare `cache_read_input_tokens` on turn 2.
3. **Codex effort change mid-thread.** Over app-server: `turn/start effort=medium`, then `turn/start effort=low`. Compare `cachedInputTokens` in `thread/tokenUsage/updated`. This shows whether Codex uses `configuration_update` or rewrites the prefix.
4. **Codex `thread/inject_items` and the cache.** Inject a developer item between turns, then check `cachedInputTokens` on the next turn. Does it append cleanly?
5. **Claude PostToolUse `updatedToolOutput` on Bash.** Return a trimmed `{stdout,…}` and check whether the transcript and next request carry the trimmed version, and that the cache stays warm. This is a candidate cheap token-saver; Codex cannot do this yet.
6. **What survives Codex compaction.** Run `thread/compact/start` on a seeded thread, then read the `contextCompaction` item and `thread/items/list`. Are user messages kept verbatim (as the API docs say) and is AGENTS.md re-sent?
7. **Codex `model_auto_compact_token_limit` and `compact_prompt` with `--strict-config`.** Are both accepted on 0.162.1 with the default provider, and what does the undocumented `model_post_turn_compact_threshold_percent` do?
8. **Claude subagent and fork cache sharing.** `--resume <id> --fork-session` on a warm session: does the first request read the parent cache (the docs say yes for forks)?
9. **Claude `--system-prompt-snapshot on`** with a changed `--append-system-prompt` on resume. Confirm the old prompt is reused (no cache miss) and learn when new instructions take effect, which matters for Brigadier changing a worker's brief mid-run.
10. **Idle compaction and Brigadier.** `idleCompaction:false` is already set. Confirm that a stream-json (non-terminal) session is exempt anyway; the docs say "interactive terminal session" only.

Sources I could not open (treated as unverified): openai.com "Unrolling the Codex agent loop" (403); anthropic.com managed-agents and long-running-Claude (search snippet only); the OpenAI ExecPlans cookbook and the GPT-5.1-Codex-Max page (search snippet only).
