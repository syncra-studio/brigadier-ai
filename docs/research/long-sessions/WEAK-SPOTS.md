# Brigadier weak spots for long sessions, usage and grounding (2026-10-14, second pass 2026-10-15; code at `d669bca0`)

Three sources:
- **A:** an independent Codex audit of the code (worker w02, source reading only; the full text is in the run's
  `msgs/w02-audit.md`).
- **B:** targeted checks made for this research, each testing a hypothesis the research raised.
- **C:** the second pass (2026-10-14/15): a code investigation and plan red-team (w05, Mythos 5.1, `msgs/w05-mythos-investigation.md`)
  and an internet and CLI research pass (w04, Fable 5.1, `msgs/w04-fable-research.md`). Every C item below was
  adjudicated by w06: the cited lines were reopened at `d669bca0`, the usage numbers were re-queried read-only from
  the installed app's `routing.sqlite`, and the CLI claims were checked on Claude Code 2.1.300 and codex-cli 0.162.1.
  Claims that didn't hold are listed in §7.

"Spot-checked" means the file:line evidence was re-read here and the mechanism holds. "Measured" means a number from
the installed app's own usage store. None of these was reproduced live, and no usage saving is claimed for any of
them beyond the measured rows quoted. Paths: `M/` = `crates/core/src/manager/`, `P/` = `crates/providers/src/`.

## 1. Continuity: what a fresh session gets

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| C1 | **A quota, error or outage reroute hands over far less than a size hand-off.** The successor gets the spec, the progress (newest 2,000 events, cut to 24 kB), assistant prose cut to 800 characters, commands with their exit code and only the last 300 characters of their output, and other tool calls without their outputs. There is no transcript, no note and no `checks.md`. Read-only research can lose everything it found. **The same thin path is taken more often than the first pass said:** a usage limit that moves the task to **another account of the same provider and model**, a task that **waited for its own quota to reset and continues on the same model**, a sign-in error, and a **"prompt is too long"** error (`ContextWindow`) all start a new session (`Origin::New`) from this successor. Nothing in `fallback.rs` resumes the session that was running, though Claude's `--resume` takes a session id or the absolute path of its transcript file | `M/fallback.rs:70-91`, `:136-160`, `:214-293` (`Origin::New` at `:285`), `:438`, `:528`, `:553`, `:595`, `:625`; `P/claude/parse.rs:1042`; against `M/worker_handoff.rs:244-275` | High | A, spot-checked; C (w05), spot-checked |
| C2 | **Restart recovery stops workers that were only waiting.** A worker that was Blocked on a question, outline or review, or Paused by the user, falls through to "Brigadier quit while this task was running" and is disposed as Stopped. Only Running/Starting tasks with a native session resume | `M/lifecycle.rs:113-190` (Blocked kept only when `deaths >= 2`, Paused only with `quota_wait`) | High | A, spot-checked |
| C3 | **A restart with missing CLI session files stops the worker,** although a live death in the same state uses the fresh-successor path | `M/worker_resume.rs:103`, `:216-218` | Med/High | A |
| C4 | **The thread's rebirth note asks for no negative knowledge.** Its headings are goal, decisions not kept yet, promises and next steps. It asks for nothing on approaches tried and ruled out, checks that failed and why, or exact identifiers, numbers and error strings. The worker hand-off asks for "anything you found or ruled out" and the checks that passed. `search_transcript` is named in the briefing, but the reborn thread has to know what to look for | `M/rebirth.rs:80-88` against `M/worker_handoff.rs:48-60`; `M/rebirth.rs:478` | Med | B |
| C5 | **Rebirth cuts live state by bytes and carries no exact pointers.** Waiting and queued texts are cut at 300 bytes, the live board at 8 kB in ascending task order (the newest tasks go first), and recent user messages at 16 kB. A cut says "search_transcript finds the rest" with no message id | `M/rebirth.rs:63-65`, `:729`, `:775`, `:825`, `:848`, `:1024`, `:1094` | Med | A |
| C6 | **The decision ledger in a rebirth briefing has no bound** relative to the new session's window (up to 10,000 decisions, every body whole, past the nominal 25k cap). Over days it can eat the rebirth's saving | `M/brains.rs:85`, `M/rebirth.rs:1172`, `:1204-1223` | Med now, High over days | A |
| C7 | **Cold-cache checkpoints live only in memory.** After a daemon restart (overnight, an update) the paid-for note is gone, and the morning message resumes and re-writes the whole history. Since C11 the lesser problem: until the checkpoint fork shares the thread's prefix, keeping it saves little | `M/cold.rs:140-148`, `:179`, `:210`; `M/conversation.rs:377` | Med | A |
| C8 | **Hand-off notes are accepted by prose matching.** A worker note must contain "next steps". The thread note's decisions are read from exact English headings. The injected tail holds only assistant messages (six, up to 8 kB each) | `M/worker_handoff.rs:40-42`, `:249`, `:485`; `M/rebirth.rs:1116` | Med | A |
| C9 | **The worker "full transcript" is already clipped,** because normalized tool outputs are capped at 8 KiB and workers' own CLI tool output (Bash, file reads, MCP results) has no output store. Only their `run_check` output is stored whole (`M/checks.rs:763-807`, with a copy of a long output in the worker's scratch `checks/`). The transcript's header admits the clipping but says the CLI clipped it, though the clip is Brigadier's | `P/claude/parse.rs:18`, `:506`; `P/codex/parse.rs:17`, `:418`; `M/worker_handoff.rs:480`; `M/workers.rs:1874` | Med/High | A |
| C10 | **The hand-off transcript is rebuilt whole in memory on every hand-off,** and a page read error becomes an empty page in a "full" transcript | `M/worker_handoff.rs:256`, `:445-478`, `:461` | Med | A |
| C11 | **The thread's checkpoint and hand-off fork shares no cache prefix with the thread, and is then counted as a cache refresh (Claude).** On Claude the fork that writes the rebirth note launches with no built-in tools (`--tools ""`) and no MCP servers, while the thread has `Read,Grep,Glob,Bash,Edit,Write,WebSearch,WebFetch` and Brigadier's MCP catalog. Tool definitions are the first level of the cached prefix, and "modifying tool definitions… invalidates the entire cache" (Anthropic prompt-caching page, re-fetched), so the fork rewrites the whole thread. The code's comment ("the fork shares its cached start") and the 2.1.285 measurement behind `refreshed` date from before the thread got its own tools (`72f41c86`, 2026-10-07). Then `cold_rebirth_plan` treats the fork's end as a refresh of the thread's cache lifetime: the thread's real entry expires 60 minutes after its last request, but Brigadier believes it lives until about 50 minutes after the fork, so a message in that gap resumes the old CLI against a cold cache. **Measured** (installed app, read-only, re-queried here): the only `handoff` row, 2026-10-14 03:46:29, Opus, read 544 and wrote **216,616**; the thread's previous turn (02:55:03, 51 min earlier, the 5/6 checkpoint point) stood at 252,313; the cold rebirth's first turn at 09:34:30 wrote another 77,108. So that checkpoint cost about 294k of writes where a plain cold resume would have written about 252k. Whether the fork wrote at the 1 h or 5 min price isn't recorded (P0.1). On Codex the same `ToolSet::None` only disables web search: the fork keeps Codex's own shell and patch tools (`P/codex/mod.rs:1187-1194`) and runs after the old CLI closes (`M/rebirth.rs:259-266`); whether it shares the thread's prefix is unknown, and Codex threads aren't checkpointed anyway (N5) | `M/rebirth.rs:329-366`; `P/claude/mod.rs:298-309`, `:76`; `M/cold.rs:61-64`, `:159-172`; `routing.sqlite` `turn_usage` | High (usage on every idle long thread) | C (w05), spot-checked and re-measured |
| C12 | **A reborn thread can't search its predecessor's own tool output,** though its briefing says `search_transcript` covers "this conversation's whole transcript". The index takes messages, reported task reports, answered questions and plan decisions; the thread's own commands and reads live in the orchestrator log and aren't indexed | `M/brains.rs:966-1010`; `M/rebirth.rs:478`; `crates/core/src/work.rs:2739` | Med | C (w05), spot-checked |

## 2. Context limits inside one turn

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| L1 | **Thread rebirth is decided only after a turn ends.** One long turn can cross the window: Codex compacts at its own ceiling anyway, and the compaction is only logged as a contract breach. On a 1M Claude window with rebirth at 400–450k this needs one turn to add over 500k, so it is mainly a risk on smaller windows and for Codex | `M/conversation.rs:4243`, `:4260`, `:4404`, `:4706-4712`, `:4755`; `P/codex/mod.rs:1180` | Med (High on small windows) | A, spot-checked; severity lowered here |
| L2 | **Workers can be compacted by their CLI before Brigadier hands them off.** Workers launch with `auto_compact: true` (`M/workers.rs:1870`), so Claude compacts at its own ceiling and Codex at about 90% of its window. Brigadier's wrap-up steer **is sent mid-turn**, on the first context report past the threshold (Claude reports context on every `message_delta`), but **only once per turn**, and the hand-off itself happens only when the worker ends its turn. A worker that ignores the steer is never asked again in that turn and never stopped (corrected after w05; the first pass said the steer waited for the turn's end). The guard `max(at, start + at/2)` can also be beyond the window: on a 200k window `at` = 140k, so a successor that starts at 140k would wait for 210k. The audit's "300k" is wrong: the cap is 200k (`knowledge.rs:222`). Successors usually start far smaller, but a long turn after the steer can still reach the CLI's own compaction | `crates/core/src/knowledge.rs:222-247`; `M/workers.rs:421-432`, `:1870`, `:3043`; `M/worker_handoff.rs:65-100`; `P/claude/parse.rs:308-311` | Med/High | A, spot-checked; corrected twice |

## 3. Grounding: claims treated as facts

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| G1 | **Worker claims become Brain facts with no evidence gate.** A report's `learned` items become Convention nodes and its `decisions` Decision nodes at landing (a read-only task's when it reports), before the integrated checks or the review. Separately, the decision lines of the thread's own rebirth note become Decision nodes of orchestrator origin with no task behind them (`M/rebirth.rs:619`, `M/brains.rs:1382`). Brain-first then feeds them to later workers as settled. This is how memory poisoning works in long-running agents (see DIGEST) | `M/brains.rs:1185`, `:1200-1210`, `:1382`; `M/landing.rs:744` before `:778`; `M/workers.rs:3670` | High | A, spot-checked |
| G2 | **An overnight phase's "verified commit" needs only that no task of the phase is still open, the session's full checks pass at the run's tip, and the summary is non-empty.** It doesn't need evidence per criterion or a finished review | `M/overnight/run.rs:448-567`; `M/phases.rs:803` | High (overnight) | A |
| G3 | **`run_check` can reuse a pass when the runtime inputs changed:** services, ignored fixtures, installed dependency contents. Cached results have no expiry | `M/checks.rs:271`, `:522`, `:588`, `:1184`, `:1286` | High for integration/smoke commands | A |
| G4 | **A fresh verifier gets only part of the lead's report:** summary, done-when and verification. Risks, open questions and artifact references are left out | `M/phases.rs:878-895` | Med | A |
| G5 | **A worker question that times out after an hour becomes "continue with your best judgement",** whatever the question was, including consent-type questions | `M/workers.rs:3408-3433` | Med | A |
| G6 | **The thread never reads what a worker teaches the Brain, and judges each worker on at most 3,600 bytes.** The report envelope lists summary, changes, decisions, verification, done-when, open questions, risks and needs-user, but `learned` only as a count ("Brain: +N facts"), and `report_text` (what `search_transcript` and `read_report` give) leaves it out too. So a wrong `learned` line becomes a Convention node (G1) that no one in the session has read. The 3,600-byte limit (≈800 tokens) is counted over the summary and every list together, `learned` included, so a scout with many file:line findings or an implementer with several failing checks moves them into an artifact the thread may never open | `M/workers.rs:94`, `:3522-3540`; `M/prompts.rs:1540-1584` (count at `:1571`); `M/brains.rs:1971`; `M/brains.rs:1200-1210` | High (no lost detail; G1) | C (w05), spot-checked; w05's "decisions aren't listed either" is wrong, they are |
| G7 | **Brain facts in a worker's brief carry no provenance.** `brain_lines` prints "kind: title. body (node id)", with only a staleness marker, under "What earlier work in this project found and decided… Start from it". A user's decision, a worker's unchecked `learned` line and a Haiku enrichment node read alike. A4's "shown as unverified" needs this line to change | `M/context_pack.rs:170-213` | Med; a prerequisite of A4 | C (w05), spot-checked |

Already good, and to keep: worker reports mark each "done when" as [met], [not met] or [not checked], and "a check
you didn't run is [not checked], never [met]" (`M/prompts.rs:1108`). The thread is told not to take a claim on trust
and never to say something compiles without checking it (`M/prompts.rs` thread prompt). An over-long report is
refused rather than cut.

## 4. Fixed cost and small-task speed

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| S1 | **A short question can become several serial model calls.** The thread is told to "delegate early: at most a quick query_brain or code_search, then delegate_task; the worker reads the code", and a Brain miss suggests a scout. So "where is the retry delay?" can go thread → scout start (15–30k fixed start) → scout turns → report → thread answer | `M/prompts.rs:101-103`; `M/brains.rs:779`; `M/workers.rs:3114` | Med (latency) | A + B (suspected; needs a probe) |
| S2 | **The thread's fixed start is large and the same for every ask:** 47 tool names, delegation, overnight, CI, previews, pages, peers, and a long rule list. Measured in the installed app: the first thread turn reads 24–25k tokens. Cached, it is cheap per call, but every call carries it | `crates/core/src/tools.rs:974`; `crates/mcp-server/src/catalog.rs:391-450`; `M/prompts.rs:91-160`; `routing.sqlite` (installed app, 10-04..10-14) | Med | A + B |
| S3 | **Repository instructions are collected eagerly for every subtree** (depth 6, up to 32 KiB) and sent to the thread and every Claude worker. Past 32 KiB collection stops, with a marker that more files were left out but not which (`M/instructions.rs:120-122`) | `M/instructions.rs:37-51`, `:65`, `:95-145`; `M/thread.rs:329` | Med | A |
| S4 | **Context packs reuse the whole conversation's reads,** not the task's, up to 16 KiB | `M/reads.rs:28`, `:94`, `:198`; `M/context_pack.rs:69`, `:326` | Med | A |
| S5 | **Prewarming starts on any user message,** including read-only asks | `M/prewarm.rs:35`, `:112` | Low/Med | A (suspected) |
| S6 | **The thread wakes once per Brigadier message, each wake a model call over the whole context.** `take_one_request` takes the envelopes of one request that are already waiting, so envelopes that queue while the thread is busy are batched; but one that arrives while it is idle (a landing, "checks passed", a review with no findings, a status) wakes it at once, and the next one a few seconds later wakes it again. **Measured, 2026-10-14** (re-queried): 75 thread turns, average context 169k, max 252k, 32.9M cache reads, 300k cache writes; 26 turns answered with under 400 output tokens and read 9.1M of those; 7 answered with under 80. Cache reads are the cheap kind, but they are Brigadier's largest token stream, and each turn adds to the context that brings the next rebirth | `M/conversation.rs:6192-6212`; `M/integrated_checks.rs:1022`; `routing.sqlite` | Med (usage, rebirth frequency); none for quality | C (w05), spot-checked and re-measured |
| S7 | **A one-line ask after a break may start a cold rebirth before it is answered.** With the thread over 150k and its cache older than an hour, the turn first rebirths (a new CLI and a briefing; the measured one wrote 77k), then answers | `M/conversation.rs:2252-2253`; `crates/core/src/knowledge.rs:203-218` | Low/Med (latency of the commonest small ask) | C (w05, suspected): mechanism read, latency unmeasured |
| S8 | **Brain enrichment is the largest Claude cache writer.** 14 days to 2026-10-15: the 14 calls tagged `brain` wrote 1.31M and read 22.5M (Haiku); 23 older Haiku calls of
10-04/05 carry no step tag and no task, and match enrichment jobs in time and project (Codex's review checked them
against `brigadier.db`; here they were confirmed as untagged Haiku rows without a task). Counted together: 37 calls,
**about 2.98M cache writes and 54.0M reads**, against the thread's 0.50M writes (99 calls), workers' 0.79M and reviews' 0.58M. It runs when a usage window resets within the hour with at most 60% used and none past 85%: leftover five-hour quota, but the weekly window pays too. How subscriptions weigh Haiku tokens is unknown, so this is a P0.1 target, not a verdict | `M/brain_jobs.rs:56-63`; `routing.sqlite` | Unknown until P0.1 | C (w05), re-measured |

## 4b. Effort

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| E1 | **The effort the thread asks for is applied to whatever model the task gets.** Only Operate has a floor. The thread is told to pick "low" for a mechanical edit and "medium" for small work, whatever the model. In one study inside Claude Code, Sonnet 5 at low resolved 12/25 against 19/25 at high (p = 0.016), while Opus 5 at low was on the cost–quality frontier. Defaults without an ask are high for implement, review and merge | `crates/router/src/decide.rs:190-218`, `:1590-1607`; `M/prompts.rs:103`; DIGEST finding 8 | Med | B |

## 5. Supervision

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| W1 | **A hung worker command gets its first nudge only after 40 minutes** of silence (10 when no command runs), and is handed over about 10 minutes after the nudge, so about 50 minutes in. Chatter without progress resets it. For comparison, OpenAI's reference orchestrator over `codex app-server` (Symphony `SPEC.md`, re-fetched) kills and retries a run after 5 minutes without **any** agent event; that is a looser notion of activity than Brigadier's (any event counts), so the numbers aren't directly comparable, but 50 minutes per hung worker is too long for overnight runs | `M/watchdog.rs:73`, `:91-94`, `:91`, `:170-192` | **High for overnight runs** (raised after w04 and w05 both argued it) | A; C |
| W2 | **No watchdog covers the thread itself.** A live CLI that accepted a turn and went silent could stall the session | `M/watchdog.rs:357`; `M/conversation.rs:3358` | Possibly High (a stalled overnight thread is the worst multi-hour failure) | A (suspected; needs fault injection) |

## 6. Gaps in the §7 "Next" items (from BASELINE §4)

- **N1. Cache writes are not attributed to a cause.** Cache writes are about 25% of usage, and there is no split by
  fresh start, expiry, rebirth, hand-off, resume after close or tool-list change. Nothing can be tuned without it.
  Most causes can't be named by Brigadier, but a missed prefix can be estimated per call at the cache boundary: the
  previous request's prompt (input + cache read + cache write) should come back as cache read, so what doesn't is
  roughly what was rewritten (PLAN P0.1; w05's `cache_write − Δcontext` form was corrected after review, because
  Brigadier's context figure includes output tokens). C11's fork row shows it plainly: a 252k prompt came back as
  544 tokens of cache read.
- **N2. No indexed or symbol reads.** `code_search`/`code_refs` return locations. Workers are told to read line ranges
  (`M/prompts.rs:1140-1144`: Claude's `Read` with offset/limit, one `sed -n` range on Codex), but no tool reads one
  symbol or a file's outline. How often whole files are still read is unmeasured. File reads were the largest
  tool-result cost (§7).
- **N3. Effort is set per task only.** Nothing checks whether quality holds at lower effort.
- **N4. Brigadier turns Codex's hooks engine off, its own hooks with the user's.** codex-cli 0.162.1 ships hooks as
  stable and on by default (`codex features list`: `hooks stable true`; events include `PostToolUse`, `PreCompact`,
  `Stop`), and the app-server schema carries `hook/started`, `hook/completed` and `hooks/list`.
  `DISABLED_FEATURES` turns `hooks` off for every Codex session (`P/codex/mod.rs:116-123`), on purpose, to keep the
  user's personal setup out. That also gives up the only before-the-fact view of a Codex compaction and a store of
  Codex's own shell and `apply_patch` output (Brigadier's `run` tool covers only what the thread runs through it).
  Delivering Brigadier's own hooks without the user's is unverified (PLAN §5). Claude is different: Brigadier
  already passes its thread's hook in `--settings` (`P/claude/mod.rs:758-759`, `:791-811`), so the same route works
  for any Claude hook.
- **N5. A Codex thread has no cold-cache lifetime.** `cache_lifetime(Codex)` returns `None`
  (`crates/core/src/knowledge.rs:198`), so `M/cold.rs` never checkpoints a Codex thread. OpenAI documents a **minimum** retention
  of 30 minutes on GPT-5.6 and later ("at least 30 minutes after its latest write or reuse", and OpenAI may keep it
  longer), refreshed by reuse (prompt-caching guide, re-fetched). That is not an expiry time, and the guide names no
  models, so whether it covers gpt-6.1-sol is unverified. Codex's cache writes are parsed (`P/codex/parse.rs:1114`)
  and were 0 in all 725 Codex rows of the installed app; the app-server schema makes `cacheWriteInputTokens` optional
  with a default of 0, so the zeros may mean "not reported". Whether a subscription charges Codex cache writes is
  unknown.

## 7. Refuted or already fixed

- A worker waiting for its own background job is no longer failed as "stopped without a report" (`e5534377`).
- w02 #9's "min(300k, 70%)" is wrong: the cap is 200k. The mechanism stands, rarer (L2).
- w02 #4's severity is lower on 1M windows (L1).
- **w05: "a worker's `--append-system-prompt` starts with the date and the task's number, title and spec", and "each
  worker's cwd is its own worktree".** Both wrong for Claude workers. The system prompt append is the generic
  `worker_system` rules plus the repository's instructions (`M/prompts.rs:1253`, `M/workers.rs:1743`); the
  "Today is… Task task-N…" text w05 cites (`M/prompts.rs:1365-1385`) is the **first user message**
  (`M/workers.rs:1744-1750`, "A new CLI session hears its task first"). A Claude worker starts in the conversation's
  shared worker folder with its worktree added (`M/workers.rs:1601-1616`). So w05's reason to expect PLAN B4 to show
  nothing doesn't hold. What remains open is narrower: whether the per-worker `--add-dir` path appears in the
  dynamic system-prompt sections (PLAN §5, B4 probe).
- **w04: "worker reports are accepted by prose matching", so a JSON schema would make acceptance a parse.** Reports
  are already structured: `submit_report` is an MCP tool with typed lists, validated when called, and a refusal
  (for example the size check) goes back to the worker inside its turn (`M/workers.rs:3522-3540`). WEAK-SPOTS C8's
  prose matching is about **hand-off notes** and the thread's rebirth note, not reports.
