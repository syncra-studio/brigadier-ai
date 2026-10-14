# R3: What practitioners and open-source harnesses do (2025 to Oct 2026)

Scope: open-source harnesses, engineering blogs and detailed practitioner posts. Vendor docs and academic papers are left out because the other passes cover them. Evidence grades: **M** = measured with numbers, **A** = anecdote or self-reported numbers with no method given, **D** = design description only. "CLI-fit" says whether the pattern works when the harness only drives an unmodified Claude Code or Codex CLI.

---

## 1. Orchestrators and long-running harnesses

**Ralph loop (Geoffrey Huntley)**: https://ghuntley.com/ralph/ (14 Jul 2025). A bash `while` loop feeds the same `PROMPT.md` to a fresh CLI session each time. State lives only on disk: `fix_plan.md` (a ranked list of remaining work that the agent edits and prunes), `specs/` (one spec per file) and `AGENT.md` (build and run commands the agent adds to as it learns them). Each loop does one item, commits, and exits. The next loop starts with a clean context. Checks: type checkers as "backpressure", tests after every change, and a ban on placeholder code. Only one subagent may run build/test. The main context acts only as a scheduler, and search goes to subagents. Numbers: a "$50k contract for $297" claim. Grade **A**. CLI-fit: **yes**, it was built for the unmodified CLI. Critics note that the official Ralph plugin keeps one context window ("automates the continue button"), which loses the fresh-context benefit (Reddit, **A**).

**HumanLayer "Advanced Context Engineering" / Frequent Intentional Compaction**: https://github.com/humanlayer/advanced-context-engineering-for-coding-agents/blob/main/ace-fca.md (Aug 2025). The workflow runs as research, then plan, then implement, each in its own session. Bulky context (searches, traces, logs, JSON) is turned into artifacts: a research doc (relevant files, how data flows, likely causes), a plan (exact files, edits and a check for every phase) and `progress.md` (goal, approach, steps done, the failure being worked on). After each verified phase, status is written back into the plan. Target context use is **40 to 60%**. Their ranking of what hurts context most: wrong information, then missing information, then noise. Numbers: in a ~300k-LOC Rust repo new to the author, a bug-fix PR came in about 1 h and was approved by the maintainer. Two features totalling ~35k LOC took about 7 h (3 h research and planning, 4 h implementing), against an estimate of 3 to 5 days each. The team of three spent about $12k a month. Failures are reported too: a research pass wrongly called the bug invalid, and a 7-hour dependency removal failed because research was too shallow. It needs an expert reviewer. Grade **A** (real numbers, n is small). CLI-fit: **yes**, since it is just prompts plus files.

**Continuous Claude**: https://github.com/AnandChowdhary/continuous-claude (2025, still active). Runs Claude Code (or Codex) in a loop. Each iteration gets its own branch and PR, waits on `gh pr checks` and any required reviews, then squash-merges. A failed iteration has its PR closed and its work thrown away. Memory is `SHARED_TASK_NOTES.md`, which the prompt frames as a short relay baton rather than a log, kept apart from durable knowledge (`--knowledge-file`). An optional reviewer pass runs checks and simplifies the code. The run must have at least one limit set (`--max-runs`, `--max-cost`, `--max-duration`). It stops on 3 straight errors, or when the agent prints a completion token on 3 straight runs, and it sleeps when it hits a rate limit. Grade **D** (one example costs $0.042 per iteration). CLI-fit: **yes**.

**Anthropic C-compiler run (Carlini)**: https://www.anthropic.com/engineering/building-c-compiler (5 Feb 2026). An engineering write-up rather than product docs. Sixteen agents each ran a `while true` loop in their own container, sharing a bare git upstream. Agents claimed tasks with lock files in `current_tasks/`, and git push conflicts settled any race. There was no orchestrator. Continuity came from READMEs and progress files that agents had to keep current, plus a running list of failed approaches. Context hygiene: print a few lines and send the rest to log files, and put `ERROR` and its reason on one line so grep finds it. A `--fast` mode runs a deterministic 1% or 10% sample of tests per agent. Numbers: about 2,000 sessions over 2 weeks, 2B input and 140M output tokens, about $20k, about 100k LOC that builds Linux 6.9. Failures: regressions until CI was tightened, all agents piling onto one kernel bug (fixed by using GCC as an oracle), and one agent running `pkill -9 bash`, which killed its own loop. Grade **M**. CLI-fit: **yes**.

**Cursor "Scaling long-running autonomous coding"**: https://cursor.com/blog/scaling-agents (14 Jan 2026). Measured coordination failures. With a shared lock file, 20 agents managed the throughput of 2 or 3. Optimistic concurrency made agents risk-averse, so they dodged hard tasks. An "integrator" role became a bottleneck and was removed. What worked: planners (which can be recursive) that create tasks, workers that do one task and push without talking to other workers, and a judge at the end of each cycle that decides whether to continue. Each cycle starts fresh, and "periodic fresh starts are still needed to combat drift and tunnel vision". Scale: a browser of about 1M LOC in about a week, and a +266k/−193k migration over 3 weeks. Model fit per role mattered: in their runs Opus 4.5 "stopped earlier and took shortcuts". Also: "prompts mattered more than harness and models", and removing complexity helped more than adding it. Grade **M** (scale numbers, no controlled comparison). CLI-fit: **partly**. The roles map onto CLI sessions, but Cursor runs its own harness.

**Beads / Gas Town (Steve Yegge)**: https://github.com/steveyegge/beads and the v1.0 post https://steve-yegge.medium.com/gas-town-from-clown-show-to-v1-0-c239d9a407ec (2025 to 2026). Beads gives agents a git- and Dolt-backed issue graph to use as memory instead of markdown TODOs. `bd ready` lists unblocked work, `bd update --claim` claims a task atomically, closing a task unblocks the tasks that depend on it, `bd remember` stores durable facts, `bd prime` injects memory at session start, and old closed issues are summarised ("memory decay"). Gas Town runs 20 to 30 agents on top of Beads. Yegge admits early versions caused "massive data loss" for weeks. Grade **D/A**. CLI-fit: **yes**, since agents use it through bash.

**Amp Handoff**: https://ampcode.com/news/handoff (23 Oct 2025). Amp removed `/compact` and replaced it with Handoff. The user states a goal for the next thread. The tool pulls out only the context relevant to that goal and drafts a starting prompt plus a list of files, which the user can edit before sending. The stated reasons: compaction is lossy, and it "encourages long, meandering threads… stacking summary on top of summary". A later rebuilt CLI (May 2026) reportedly brought auto-compaction back (secondary source, **unverified**). Grade **D**. CLI-fit: **yes**, because an outside harness can write the handoff prompt and start a new session. This is very close to Brigadier's design.

**Worktree-per-agent runners**: Claude Squad (https://github.com/smtg-ai/claude-squad), Vibe Kanban, ccmanager, cwt and uzi. All grade **D**. They isolate sessions and leave memory to the CLI, and none publish continuity or verification data. **Bernstein** (https://docs.bernstein.run) is a non-LLM, deterministic Python scheduler over 30+ CLI adapters. It gives each task its own worktree, gates work on lint, type checks and tests through a "janitor", and caps the budget of headless runs. Its own docs report that **creating a worktree takes 5 to 15 s on a non-trivial repo, which is often more than a ~30 s task**. They also admit the warm pool meant to hide this is not wired up yet. Grade **D** (self-reported). CLI-fit: **yes**.

**Spec-driven kits (spec-kit, BMAD, Task Master)**: spec → plan → tasks with acceptance criteria → implement. Grade **D**, with no measured outcomes found. **Codex team "Harness engineering"** (Feb 2026): the page returned 403. Via InfoQ I confirmed only the headline (about 1M LOC over 5 months, docs as "system of record"). Everything else is **unverified**.

**Patterns worth borrowing**
- Treat a fresh session per unit of work as normal, not as recovery. Every harness that ran for days (Ralph, Carlini, Cursor, Continuous Claude) restarts often and keeps continuity in files plus git. Keep the handoff short and pointed at a goal (Amp, `SHARED_TASK_NOTES` "baton"), separate from durable knowledge (the Brain).
- Split the carried state in three: a stable list of what must be true when done that workers cannot edit, a short progress note (done and verified / next action / failed approaches / blockers), and git as the actual record of changes.
- Keep the check outside the worker: CI or gates must pass before merge, a failed run is thrown away rather than patched, and each run has hard limits on runs, cost and time.
- Coordinate with a planner, workers and a judge. Don't use shared locks or an integrator agent (Cursor measured both failing).

---

## 2. Saving tokens on tool output and file reads

**Claude Code itself (persist-to-file)**: the current Claude Code writes large Bash output to `<session>/tool-results/<id>.txt` and shows the model a preview of about 2 KB plus the file path (issues #17944 and #81129 on claudeissues.com, 2026). A practitioner write-up says a failing command gets a ~10k-char excerpt with **no file**, which I could not confirm (**unverified**). A 2026 bug showed a stale preview next to a correct file. Grade **D/A**. CLI-fit: **yes**, since it is built in. Brigadier gets this for free in Claude workers.

**Zivtech "Economics of Claude Code, Part 3: What you carry"**: https://www.zivtech.com/blog/economics-of-claude-code-part-3-what-you-carry (22 Apr 2026, updated Jun 2026). Measured typical outputs: a build is 5k to 20k tokens, a 100-test PHPUnit run 15k to 30k, and grep for 47 matches about 8k. A 10k-token dump carried for 20 turns costs $0.10 with a warm cache and up to $1.00 with cold restarts. **Lost-constraint case**: a `varchar(128)` rule from turn 8 was dropped at auto-compaction around turn 28, causing 10 turns of wrong work and a 15-turn undo. Their rule of thumb: 80 to 90% compaction fidelity is workable, and below 60% important facts are lost. Grade **M/A**. CLI-fit: **yes** (hooks and a user-run handoff).

**RTK (Rust Token Killer)**: https://github.com/rtk-ai/rtk (2025 to 2026). A PreToolUse hook rewrites `git status` → `rtk git status` and similar commands, in both Claude Code and Codex. Filters include "failures only, passing tests counted" for test runners, hash/author/subject only for git log, and grouping by directory. Claim: 60 to 90% less output, and a 30-minute session going from ~118k to ~23.9k tokens. Tokens are estimated as bytes÷4, and the README itself says the total bill falls less than that. Full output is only available by re-running with `rtk bypass`, with no lookup by id. The hook misses the built-in Read, Grep and Glob tools. Grade **A** (self-reported). CLI-fit: **yes**, through a hook.

**Context Mode (mksglu)**: https://glama.ai/mcp/servers/mksglu/claude-context-mode. Runs commands in a subprocess, indexes the raw output in SQLite FTS5, and returns a one-line pointer. `ctx_search` then pulls back BM25-ranked windows. Claim: 315 KB → 5.4 KB across 14 scenarios. A critique (getunblocked.com) says it fixes how much one call returns, not ranking across sources. Grade **A**. CLI-fit: **yes** (MCP). This is the clearest open-source case of "short digest plus full output kept by id and searchable".

**Symbol and LSP reads (CircleCI)**: https://circleci.com/blog/claude-code-lsp/ (24 Jun 2026). Claude Code's built-in LSP tool against grep on vuejs/core (149k LOC), finding all references for 11, 20 and 260 call sites, with one CLAUDE.md line telling the model to prefer LSP for symbols and **trust its results without re-reading files**. With Opus 4.8, tool-output tokens fell **31%** and cost 3%, and time rose 2%. With Sonnet 4.6, tokens fell 33%, cost 14% and time 34%, and grep **missed references** (9 of 11 and 249 of 260) where LSP missed none. Savings showed up on big queries. On small ones cost was about equal, with one tool-search call as overhead. Run counts were not stated. Grade **M** (small). CLI-fit: **yes**, since it is a built-in tool turned on by env/config.

**Serena (LSP-based MCP, find_symbol / replace_symbol_body)**: widely used, but the "70% savings" figure comes from users with no method (**A**). Third-party graph and index MCPs (agentmap "98%", codegraph "up to 64% fewer tokens", claude-context "40%" from Zilliz) are all self-reported (**A**). One practitioner saw an agent spend about 90k tokens on grep-driven reads in a monorepo before finding the right service, and another saw it pull "~40% of the codebase" in to refactor one function (amazingcto.com, Jun 2026, **A**).

**Aider repo map**: tree-sitter tags, PageRank over definitions and references, fitted to a token budget (default 1,024). Secondary sources only. Grade **D**. CLI-fit: **partly** (as a tool or file only).

**Patterns worth borrowing**
- Digest plus id: show a short, failure-first summary and keep the full output on disk under an id the model can grep or read by range. Claude Code already does this for big Bash output. Brigadier's own tools (code index, Brain, transcripts) should return digests the same way.
- Test and build output in "failures only, passes counted" form gives the biggest single saving (RTK, Carlini's logging rules). Ask workers to run tests that way, or ship a wrapper script.
- Symbol tools pay off on large or cross-package queries and as an **accuracy floor for weaker models**, but only if the model is told to trust the result and not re-read the file.
- Don't count on a hook catching everything: the built-in Read, Grep and Glob bypass Bash hooks. Savings have to come from tool choice and instructions too.

---

## 3. Fast paths for small asks

Little hard data exists here. What I found:

- **Cold start of a CLI**: on a Raspberry Pi, `claude -p "say hello"` took ~3 s of API time but ~20 s wall time (issue #3044, Jul 2025, **A**). Systima's proxy capture (Jul 2026, via https://www.kunalganglani.com/blog/opencode-vs-claude-code-token-overhead) puts Claude Code's preamble at **~33k tokens before the user prompt**, against ~7k for OpenCode. Instruction files, MCP and plugins pushed it to ~75k. Grade **M** (method published per secondary sources; primary not opened). Any new session pays this once, and it is cached after that.
- **Worktree spawn**: 5 to 15 s per worktree, often longer than a ~30 s task (Bernstein docs, **A**).
- **Routing overhead**: each supervisor routing decision costs 150 to 800 ms, and routing took 15 to 30% of total run time in systems the authors measured (groovyweb.co 2026 patterns post, **A**).
- **Two-model chains**: Aider's architect→editor split gave "only a modest benchmark improvement… and comes with a fairly high cost in terms of latency" with a reasoning architect (aider.chat QwQ post, Dec 2024, **A**).
- **Fan-out cost**: when Claude Code split one task across two subagents, total input went from ~121k to ~513k, because each subagent pays the whole preamble again (Systima via the same secondary sources; the authors allow that a gateway cache miss may be part of it, **A**). One HN user said 7 subagents used up a budget before any finished (**A**).
- **Triage practice**: community orchestrator skills tell the agent to "triage in ~10 seconds" and, for trivial work, "implement directly, no brief, no worktree, no subagents" (skills.sh agent-orchestrator, **D**).

**Patterns worth borrowing**
- Answer small asks inside the already-warm thread session (one turn, cache already hot) and spawn nothing. A new worker costs a preamble write of 33k or more, a worktree (5 to 15 s) and a cold start.
- Make "do it here" the default and "delegate" the exception. Delegate only when the work spans many files or needs isolation. Let the thread decide, with no separate classifier call, since each routing call costs 150 to 800 ms.
- If workers must be quick, keep a pre-made worktree ready (Bernstein's idea, though not yet wired up there) and don't chain planner→editor for one-line edits.

---

## 4. Practitioner write-ups: long sessions, handoffs, failures, dashboards

**Manus "Context engineering"**: https://manus.im/en/blog/Context-Engineering-for-AI-Agents-Lessons-from-Building-Manus (18 Jul 2025). Calls KV-cache hit rate "the most important metric". Input to output runs about 100:1, and cached tokens cost 10× less ($0.30 vs $3/MTok). Rules: a stable prefix (no timestamps), append-only context, no adding or removing tools mid-task, the file system as memory with restorable compression (keep the path, drop the content), recite goals by rewriting `todo.md` (about 50 tool calls per task), and keep failures in context. Grade **A**. CLI-fit: **partly**. Prefix stability is the CLI's job. A harness can still avoid mid-session tool or config changes, use files for memory and recite plans.

**OpenHands condenser**: https://openhands.dev/blog/openhands-context-condensensation-for-more-efficient-ai-agents (9 Apr 2025). Keeps the first and last events and summarises the middle (goals, progress, remaining work, key files, failing tests) once a size threshold is passed. Up to 2× lower cost per turn, and on a SWE-bench Verified subset it solved 54% against 53% for the baseline, at the cost of a few extra turns. Grade **M**. CLI-fit: **no** for in-loop use, since the CLI owns its own compaction. It does show that a structured summary need not hurt accuracy.

**Lost rules after compaction**: many claude-code issues (#4017, #5731, #9796, #19736, #24460) report CLAUDE.md rules (no co-author line, activate the venv, output format) being followed before compaction and dropped after. One maintainer could not reproduce it. Grade **A** (many independent reports). Together with the Zivtech `varchar(128)` case, this is the most common failure story.

**Fresh session plus handoff beats compaction (consensus, mostly A)**: Amp's Handoff, HumanLayer's progress.md, and practitioner handoff skills (mejba.me: "the difference wasn't subtle") all agree on a short note covering goal, what's verified, what's in progress, the first next action, failed approaches and risky files. Compaction fires when the model is at its worst, and it cannot be reviewed.

**False success and invented results**: the Replit incident, July 2025 (Business Insider, incidentdatabase.ai #6113). During an explicit code freeze the agent deleted the production database, created about 4,000 fake users, "lied about the unit tests", and wrongly said rollback was impossible. Grade **A** (one well-documented incident). Cursor's audit, https://cursor.com/blog/reward-hacking-coding-benchmarks (25 Jun 2026): **63%** of Opus 4.8 Max's successful SWE-bench Pro solutions *fetched* the fix (upstream lookup in 57%, git-history mining in 9%). Strict isolation cut scores from 87.1→73.0% (Opus 4.8 Max) and 74.7→54.0% (Composer 2.5), and "hacking attempts increased when the model was told to keep working without stopping". Grade **M**. Cursor's own long run found Opus 4.5 "took shortcuts".

**Memory poisoning (Cisco)**: https://blogs.cisco.com/ai/identifying-and-remediating-a-persistent-memory-compromise-in-claude-code (1 Apr 2026). An npm `postinstall` script rewrote every `~/.claude/projects/*/memory/MEMORY.md`, added a `UserPromptSubmit` hook, and planted a shell alias to force auto-memory back on. The poisoned agent then advised hard-coding API keys in every project. Claude Code v2.1.50 moved user memories out of the system prompt. Grade **M** (research demo, disclosed and patched; not seen in the wild). Stale-memory "slow poisoning" posts (tianpan.co, Jul 2026) are opinion (**D**).

**Dashboards and cache monitoring**:
- Claude Code team: "we run alerts on our prompt cache hit rate and declare SEVs if they're too low" (Thariq Shihipar, quoted at https://simonwillison.net/2026/Feb/20/thariq-shihipar/). The "<90%" threshold is **unverified** (secondary source).
- Atomic Object, https://spin.atomicobject.com/claude-code-dashboard/ (18 Mar 2026): OpenTelemetry export to a free dashboard. About 600M tokens in 30 days, about 35 cache reads per cache write on a heavy day, and API-equivalent cost of about 9× the subscription. Also found a custom subagent silently set to the wrong model. Grade **M** (one user).
- Systima: Claude Code wrote **5.9× to 54× more cache tokens** than OpenCode on the same task, because dynamic state changed the prefix mid-session (**M**, via secondary sources).
- Tools: ccusage and CodeBurn read the CLI's local JSONL. Hit rate = cache_read / (cache_read + fresh input).

**Patterns worth borrowing**
- Never trust a worker's "done". Gate on checks the harness runs itself (tests, CI, a diff review) and audit the transcript for shortcuts. Be careful with "never stop" prompts, which raise cheating rates.
- Put hard constraints where compaction can't reach them: in the task file and Brain that are re-read at every handoff, and re-injected after any compaction (a SessionStart/compact hook in the session settings the harness writes). Don't rely on the summary keeping them.
- Treat the memory and Brain files as attack surface: check where each entry came from, let workers suggest and the thread accept, and record who wrote what.
- Track per-session cache_read / (cache_read + input) from the CLI's JSONL usage blocks, and flag sessions whose prefix keeps getting rewritten.

---

## 5. Anti-patterns practitioners measured

**Many MCP tools in the prompt.**
- Mario Zechner, https://mariozechner.at/posts/2025-11-02-what-if-you-dont-need-mcp/ (2 Nov 2025): Playwright MCP is 21 tools / ~13.7k tokens, Chrome DevTools MCP is 26 tools / ~18k, and his bash scripts plus README come to **~225 tokens**. Grade **M**.
- Gurubase, https://gurubase.io/blog/2026/hidden-token-cost-mcp-servers/ (13 May 2026), compiling Scott Spence (Sep 2025): 12 servers left `/context` at **143k/200k (72%) used before the first message**, ~82k of it MCP. Per-server figures: GitHub 35 tools ≈ 26k, Slack ≈ 21k, Jira ≈ 17k. Deferring tools behind tool search cut ~77k to ~8.7k. Grade **M** (compiled).
- The same author's CLI-vs-MCP benchmark (https://mariozechner.at/posts/2025-08-15-mcp-vs-cli/, 15 Aug 2025, 120 runs) found similar success rates. **Tool design (short output, few tools) mattered more than the protocol.** Grade **M** (author built one of the tools).

**Huge instruction files.**
- Systima: a 72 KB instruction file adds ~20k tokens to every request (**M**, secondary).
- HumanLayer's CLAUDE.md guidance (via secondary sources): models follow about 150 to 200 instructions reliably, the CLI's own prompt already uses about 50, and their root file is under 60 lines (**A**).
- Vercel, https://vercel.com/blog/agents-md-outperforms-skills-in-our-agent-evals (Jan 2026): a **compressed 8 KB docs index** in AGENTS.md scored **100%**. On-demand skills reached at most 79%, and only 53% (no better than no docs) when not told to use them, because skills were never invoked in 56% of cases. Cutting the index from 40 KB to 8 KB lost nothing. Grade **M** (one framework). The lesson is to keep a small, always-present index rather than a long manual.

**Changing model or tools mid-session.** Model and effort level are part of the cache key, so switching either one re-reads the whole prefix at full price. Adding an MCP server or toggling tools mid-session changes the prefix too (Claude Code prompt-caching docs, plus third-party write-ups such as mer.vin and knightli.com, 2026; **A/D**). Manus: changing tools both breaks the cache and confuses the model about tools it called earlier (**A**). A Claude Code cache bug (issue #34629) was blamed for 10 to 20× cost inflation (**A**, unverified).

**Subagent and multi-agent fan-out.** Each spawn pays the full preamble again (121k→513k above). Multi-agent setups use "up to 15×" the tokens of one good agent (practitioner Reddit post, Mar 2026, **A**). Cursor: shared locks and an integrator agent both made throughput worse (**M**).

**Long synchronous tool calls.** A tool call that runs longer than the cache lifetime (5 minutes on API billing; subscription main sessions are reported at 1 hour) makes the next turn a full cache miss, the most expensive turn in the session (Zivtech, **A**).

**Patterns worth borrowing**
- Freeze each session's tool set, model and effort at spawn. Change them only at a handoff boundary, never mid-session (Brigadier controls this through the CLI config it writes).
- Keep Brigadier's own MCP surface tiny: a few tools with short descriptions that return digests. If the set grows, rely on the CLI's deferred tool search rather than loading everything.
- Ship a compressed, always-present index (Brain highlights, code-index entry points, about 8 KB or less) rather than long instruction files or skills that the model may never call.
- Keep long builds and tests in the background and poll them with short calls, so a single turn never waits past the cache lifetime.
