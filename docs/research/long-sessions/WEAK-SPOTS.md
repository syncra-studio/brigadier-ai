# Brigadier weak spots for long sessions, usage and grounding (2026-10-14, code at `d669bca0`)

Two sources:
- **A:** an independent Codex audit of the code (worker w02, source reading only; the full text is in the run's
  `msgs/w02-audit.md`).
- **B:** targeted checks made for this research, each testing a hypothesis the research raised.

"Spot-checked" means the file:line evidence was re-read here and the mechanism holds. None of these was reproduced
live, and no usage saving is claimed for any of them. Paths: `M/` = `crates/core/src/manager/`, `P/` =
`crates/providers/src/`.

## 1. Continuity: what a fresh session gets

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| C1 | **A quota, error or outage reroute hands over far less than a size hand-off.** The successor gets the spec, the progress (newest 2,000 events, cut to 24 kB), assistant prose cut to 800 characters and tool calls without their outputs. There is no transcript, no note and no `checks.md`. Read-only research can lose everything it found | `M/fallback.rs:438`, `:528`, `:553`, `:595`, `:625`; against `M/worker_handoff.rs:244-275` | High | A, spot-checked |
| C2 | **Restart recovery stops workers that were only waiting.** A worker that was Blocked on a question, outline or review, or Paused by the user, falls through to "Brigadier quit while this task was running" and is disposed as Stopped. Only Running/Starting tasks with a native session resume | `M/lifecycle.rs:113-190` (Blocked kept only when `deaths >= 2`, Paused only with `quota_wait`) | High | A, spot-checked |
| C3 | **A restart with missing CLI session files stops the worker,** although a live death in the same state uses the fresh-successor path | `M/worker_resume.rs:103`, `:216-218` | Med/High | A |
| C4 | **The thread's rebirth note asks for no negative knowledge.** Its headings are goal, decisions not kept yet, promises and next steps. It asks for nothing on approaches tried and ruled out, checks that failed and why, or exact identifiers, numbers and error strings. The worker hand-off asks for "anything you found or ruled out" and the checks that passed. `search_transcript` is named in the briefing, but the reborn thread has to know what to look for | `M/rebirth.rs:80-88` against `M/worker_handoff.rs:48-60`; `M/rebirth.rs:478` | Med | B |
| C5 | **Rebirth cuts live state by bytes and carries no exact pointers.** Waiting and queued texts are cut at 300 bytes, the live board at 8 kB in ascending task order (the newest tasks go first), and recent user messages at 16 kB. A cut says "search_transcript finds the rest" with no message id | `M/rebirth.rs:63-65`, `:729`, `:775`, `:825`, `:848`, `:1024`, `:1094` | Med | A |
| C6 | **The decision ledger in a rebirth briefing has no bound** relative to the new session's window (up to 10,000 decisions, every body whole, past the nominal 25k cap). Over days it can eat the rebirth's saving | `M/brains.rs:85`, `M/rebirth.rs:1172`, `:1204-1223` | Med now, High over days | A |
| C7 | **Cold-cache checkpoints live only in memory.** After a daemon restart (overnight, an update) the paid-for note is gone, and the morning message resumes and re-writes the whole history | `M/cold.rs:140-148`, `:179`, `:210`; `M/conversation.rs:377` | Med | A |
| C8 | **Hand-off notes are accepted by prose matching.** A worker note must contain "next steps". The thread note's decisions are read from exact English headings. The injected tail holds only assistant messages (six, up to 8 kB each) | `M/worker_handoff.rs:40-42`, `:249`, `:485`; `M/rebirth.rs:1116` | Med | A |
| C9 | **The worker "full transcript" is already clipped,** because normalized tool outputs are capped at 8 KiB and workers have no output store. The hand-off code says so itself | `P/claude/parse.rs:18`, `:506`; `P/codex/parse.rs:17`, `:418`; `M/worker_handoff.rs:480`; `M/workers.rs:1874` | Med/High | A |
| C10 | **The hand-off transcript is rebuilt whole in memory on every hand-off,** and a page read error becomes an empty page in a "full" transcript | `M/worker_handoff.rs:256`, `:445-478`, `:461` | Med | A |

## 2. Context limits inside one turn

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| L1 | **Thread rebirth is decided only after a turn ends.** One long turn can cross the window: Codex compacts at its own ceiling anyway, and the compaction is only logged as a contract breach. On a 1M Claude window with rebirth at 400–450k this needs one turn to add over 500k, so it is mainly a risk on smaller windows and for Codex | `M/conversation.rs:4243`, `:4260`, `:4404`, `:4706-4712`, `:4755`; `P/codex/mod.rs:1180` | Med (High on small windows) | A, spot-checked; severity lowered here |
| L2 | **The worker hand-off guard can be beyond the window,** and workers keep CLI auto-compaction on. The guard is `max(at, start + at/2)`. On a 200k window `at` = 140k, so a successor that starts at 140k would wait for 210k. The audit's "300k" is wrong: the cap is 200k (`knowledge.rs:222`). Successors usually start far smaller, so the case is rare | `crates/core/src/knowledge.rs:222-247`; `M/workers.rs:1870` | Med | A, spot-checked; corrected and lowered |

## 3. Grounding: claims treated as facts

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| G1 | **Worker claims become Brain facts with no evidence gate.** A report's `learned` items become Convention nodes, and handoff-note lines become decisions, at landing, before the integrated checks or the review. Brain-first then feeds them to later workers as settled. This is how memory poisoning works in long-running agents (see DIGEST) | `M/brains.rs:1185`, `:1200-1210`, `:1382`; `M/landing.rs:744` before `:778`; `M/workers.rs:3670` | High | A, spot-checked |
| G2 | **An overnight phase's "verified commit" needs integrated checks plus a non-empty summary only.** It doesn't need evidence per criterion or a finished review | `M/overnight/run.rs:448-567`; `M/phases.rs:803` | High (overnight) | A |
| G3 | **`run_check` can reuse a pass when the runtime inputs changed:** services, ignored fixtures, installed dependency contents. Cached results have no expiry | `M/checks.rs:271`, `:522`, `:588`, `:1184`, `:1286` | High for integration/smoke commands | A |
| G4 | **A fresh verifier gets only part of the lead's report:** summary, done-when and verification. Risks, open questions and artifact references are left out | `M/phases.rs:878-895` | Med | A |
| G5 | **A worker question that times out after an hour becomes "continue with your best judgement",** whatever the question was, including consent-type questions | `M/workers.rs:3408-3433` | Med | A |

Already good, and to keep: worker reports mark each "done when" as [met], [not met] or [not checked], and "a check
you didn't run is [not checked], never [met]" (`M/prompts.rs:1108`). The thread is told not to take a claim on trust
and never to say something compiles without checking it (`M/prompts.rs` thread prompt). An over-long report is
refused rather than cut.

## 4. Fixed cost and small-task speed

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| S1 | **A short question can become several serial model calls.** The thread is told to "delegate early: at most a quick query_brain or code_search, then delegate_task; the worker reads the code", and a Brain miss suggests a scout. So "where is the retry delay?" can go thread → scout start (15–30k fixed start) → scout turns → report → thread answer | `M/prompts.rs:101-103`; `M/brains.rs:779`; `M/workers.rs:3114` | Med (latency) | A + B (suspected; needs a probe) |
| S2 | **The thread's fixed start is large and the same for every ask:** 47 tool names, delegation, overnight, CI, previews, pages, peers, and a long rule list. Measured in the installed app: the first thread turn reads 24–25k tokens. Cached, it is cheap per call, but every call carries it | `crates/core/src/tools.rs:974`; `crates/mcp-server/src/catalog.rs:391-450`; `M/prompts.rs:91-160`; `routing.sqlite` (installed app, 10-04..10-14) | Med | A + B |
| S3 | **Repository instructions are collected eagerly for every subtree** (depth 6, up to 32 KiB) and sent to the thread and every Claude worker. A large early file can stop collection silently | `M/instructions.rs:37-51`, `:65`, `:95-145`; `M/thread.rs:329` | Med | A |
| S4 | **Context packs reuse the whole conversation's reads,** not the task's, up to 16 KiB | `M/reads.rs:28`, `:94`, `:198`; `M/context_pack.rs:69`, `:326` | Med | A |
| S5 | **Prewarming starts on any user message,** including read-only asks | `M/prewarm.rs:35`, `:112` | Low/Med | A (suspected) |

## 5. Supervision

| # | Weak spot | Evidence | Sev. | Source |
|---|---|---|---|---|
| W1 | **A hung worker command can take about 50 minutes** before the first nudge (10 min silent, 40 while a tool runs, plus 10). Chatter without progress resets it | `M/watchdog.rs:73`, `:91`, `:170-192` | Med | A |
| W2 | **No watchdog covers the thread itself.** A live CLI that accepted a turn and went silent could stall the session | `M/watchdog.rs:357`; `M/conversation.rs:3358` | Possibly High | A (suspected; needs fault injection) |

## 6. Gaps in the §7 "Next" items (from BASELINE §4)

- **N1. Cache writes are not attributed to a cause.** Cache writes are about 25% of usage, and there is no split by
  fresh start, expiry, rebirth, hand-off, resume after close or tool-list change. Nothing can be tuned without it.
- **N2. No precise code reads.** `code_search`/`code_refs` return locations, and reading is still whole-file `Read` or
  `sed`. File reads were the largest tool-result cost (§7).
- **N3. Effort is set per task only.** Nothing checks whether quality holds at lower effort.

## 7. Refuted or already fixed

- A worker waiting for its own background job is no longer failed as "stopped without a report" (`e5534377`).
- w02 #9's "min(300k, 70%)" is wrong: the cap is 200k. The mechanism stands, rarer (L2).
- w02 #4's severity is lower on 1M windows (L1).
