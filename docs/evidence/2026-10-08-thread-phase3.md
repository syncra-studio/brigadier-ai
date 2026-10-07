# Thread phase 3: workers start warm and lean (2026-10-08)

THREAD-PLAN.md §3 phase 3, plus the user's addition: the Claude thread gets Brigadier's `run`.
Branch `thread-p3` from `thread-build` `d3653dfa`. Claude Code 2.1.293, codex-cli 0.160.1. Every
live check ran on a dev `brigadierd` built from the branch, with a scratch `BRIGADIER_DATA_DIR`
under `/tmp/brig-ab-1007`, driven over IPC with `tools/ab`. Computer Use was not used; every check
was scripted.

## Done-when

| Done when | Result |
|---|---|
| `delegate_task` → the worker's first event ≤ 2 s (was 5–7 s) | **Pass** on the last allowed attempt: 669 ms and 677 ms for two workers in a row. Attempt 1 missed (656 ms and 10,697 ms) and led to the fix in `178fc46a`. In T1: 687 ms (run 1) and 682 ms (run 2), against 6.3 s in phase 2 |
| A second worker's first call reads ≥ 50% of its prompt from cache | **Pass**: 0.700 (12,525 of 17,899 tokens read from cache) |
| In T1, no check command runs twice on the same tree | **Pass**: run 1 had 6 `run_check` runs over 6 keys; run 2 had 4 runs over 4 keys; 0 repeated keys |
| T1 lands verified in ≤ 6 min | **Missed in both runs**: 586.7 s and 403.9 s. The frozen checks pass on both landed tips |
| T1 tokens ≤ the phase 2 run (2,862,725 raw) | **Run 1 missed** (4,831,714 raw). **Run 2 passed** (2,550,273 raw, 0.89×) |
| Claude thread `run` (the user's addition) | **Pass at all three levels after one fix** (`41e94b6d`); see below |
| `tools/full-checks.sh` passes | **Pass** on the final tree (see the report). One earlier run failed a timing-sensitive test while the machine was loaded; that test is fixed in `3b46df46` |

## First event (lever 6, the pre-warm)

Measured with `tools/ab/firstevent.py`: the time from a task's `orchestratorStepped` `created` to
its worker's first event, on probe arm `p3-probe`. Each probe was a fresh Full access session: one
"hello" to make the pre-warm, then a message asking for two `delegate_task` calls in a row.

| Attempt | Probe A | Probe B | Notes |
|---|---|---|---|
| Development probe (w13, before the next pre-warm was started automatically) | 3,477 ms | 5,613 ms | not counted |
| 1 (`e4d7c2d6`) | 656 ms | **10,697 ms** | B's pre-warm was dropped: "its base moved" |
| 2 (`178fc46a`) | **669 ms** | **677 ms** | both adopted |

Why attempt 1 missed:
1. Probe A landed in the session branch, so the session's tip moved.
2. `take_over` accepted only the exact base the pre-warm was made at. It disposed of Probe B's
   pre-warm, and B started cold.
3. B's cold worktree copy ran at the same time as the copy for the next pre-warm, so it took
   about 10 s.

The fix (`178fc46a`):
- When the session still works on the same branch, the kept worktree moves to the base the
  worker would get now: `git switch --no-track -c <task branch> <new base>`. Git checks out only
  the tracked files that changed, and the copied `node_modules` and `target` stay.
- The next pre-warm starts only after the claimed worker has started.

In attempt 2 the thread delegated Probe B before Probe A landed, so the base had not moved and the
"move to the new tip" path did not run live. The flow test
`a_pre_warm_whose_base_moved_on_is_moved_to_the_new_tip_and_still_used` covers it, and so does
`worktree::tests::a_detached_checkout_moves_to_a_newer_start_keeping_its_ignored_files`.

## Cache share (lever 2)

`tools/ab/cacheshare.py` on the probe workers' transcripts. Each worker's first call:

| Workers | First call | Second worker's first call |
|---|---|---|
| Attempt 2 (`84c882ad`, `0a864c38`) | 0.699 | **0.700** |
| Attempt 1 (`cd56cb7f`, `c5b1fdd0`) | 0.699 | 0.699 |

The shared part is the tools block plus the fixed worker system prompt: 12,525 tokens. It is
shared because Claude workers start in one folder per conversation (`data/worker-home/<conv>`),
with the worktree in `--add-dir`. The task brief and the context pack come after it, in the first
message.

## Checks (lever 3)

- `run_check` was verified live: a second identical call on the same tree came back
  `cached: true` ("this command already ran on these same files 2 s ago").
- In T1, `tools/ab/checkruns.py` found no repeated (tree, workdir, command) key in either run.
  - Run 1: 6 runs, 16.9 s in all.
  - Run 2: 4 runs, 5.8 s in all.
  - Each run had one "check-like command outside run_check". Both are reads that matched on
    `test`: `sed` of a `.test.ts` file, and a `grep` of `tokens.css`. Neither is a check.
- **The shared `CARGO_TARGET_DIR` was not adopted** (THREAD-PLAN §5 risk 5). Measured: a second
  worktree rebuilt 2 of 3 crates. Worse, the first worktree then ran the second one's code, so a
  crate reported Fresh and an unchanged test failed, which would poison the check cache. Each
  worktree keeps its own copy-on-write warmed `target`.

## Claude thread `run` (the user's addition)

Three sessions on `p3-probe`, one per permission level. Each was asked to call `run` with
`seq 1 20000; exit 3` and with `seq 1 10500`, then `run_unsandboxed` where the level has it.

| Check | Full | Approve for me | Ask for approval |
|---|---|---|---|
| A failing command's full output is stored | exit 3, 108,894 B stored | same | same |
| A passing 50 KB log reads back byte-identical | 51,894 B, `cmp` with `seq 1 10500` identical | same blob | same blob |
| Plain `run` is sandboxed at the thread's level (`touch $HOME/…`) | wrote | "Operation not permitted" | "Operation not permitted" |
| `run_unsandboxed` | not offered | a benign `curl` was allowed by Brigadier's reviewer (`decidedForYou`: "Brigadier's reviewer allowed it: …"); `git push origin HEAD` went to a card | a card (`decidedBy: user`), and the command ran once |
| The reviewer's tokens are metered | n/a | 2 `turn_usage` rows with step `escalation` (claude-opus-5-5) | n/a |

Both stored blobs match `seq` by sha256: `f6351f5e…` and `63b531cf…`.

**Fixed during the check (`41e94b6d`).**
- The problem: at Ask for approval, Claude raised `mcp__brigadier__run_unsandboxed` (its `ask`
  rule) as an ordinary tool call. Brigadier's policy allows any non-file tool call inside a
  scoped sandbox, so the command ran outside the sandbox with no card.
- The fix: the thread now marks that request as an escalation, which only the user answers.
- The same commit fixes a prompt problem: at Full access the thread's prompt mentioned
  `run_unsandboxed` though the tool doesn't exist there. The line now applies "Below Full access".

## Context pack (lever 1)

- After T1 run 1, the whole pack goes in the worker's first message (`32444bb6`).
- No `context.md` is written.
- In run 1, the first message carried part of a 20,152 B pack and pointed to the rest. The worker
  read the file again in its first two calls (17.5 KB).
- The pack is now at most 16 KB, in this order:
  1. the thread's searches;
  2. the files it read, most recently read first, each cut at a whole line to what fits (8 KB
     each at most);
  3. the definitions in the files the task names, in what is left (3 KB is kept for them).
- A file with no room left is only named.
- This replaces THREAD-PLAN §2 lever 1's "writes `<scratch>/context.md`".

## T1

Arms `/tmp/brig-ab-1007/t1-p3` and `t1-p3b` were set up with `clone.sh` and warmed with `warm.sh`
from `t1-p2`. Both ran at Full access with Opus 5.5 high, as in phase 2. Run 1 used daemon
`3b46df46`; run 2 used `32444bb6`. Commands: `times.py brigadier`, `armtokens.sh <arm>
<settled_ms>`, `check.sh <arm> tasks/t1.md <tip>`. The behaviour probes P1.1–P1.4 were not run
(Computer Use), as in phases 1 and 2.

| | Phase 2 | Phase 3 run 1 | Phase 3 run 2 |
|---|---|---|---|
| Landed (s) | 328.9 | **586.7** | **403.9** |
| Final answer (s) | 339.7 | 598.0 | 419.9 |
| Landed tip; frozen checks | `b2786ff7`, pass | `1fc97278`, pass | `e63a1a98`, pass |
| Tokens raw (`turn_usage`) | 2,862,725 | **4,831,714** | **2,550,273** |
| Without cache reads | 168,970 | 275,422 | 178,735 |
| Claude thread, raw / without cache reads | 379,852 / 30,867 | 603,914 / 38,185 | 433,818 / 34,247 |
| Claude worker | 2,343,185 / 111,439 | 3,731,150 / 148,523 | 1,938,023 / 99,944 |
| Codex reviews | 139,688 / 26,664 (1) | 496,650 / 88,714 (plan + code) | 178,432 / 44,544 (1) |
| `turn_usage` against the transcripts | +0 | +134,950 more in the transcripts | +0 |

### Engine costs and worker-behaviour variance

**Wall clock.** Run 2 took 75 s longer than phase 2:

| Stage | Phase 2 | Run 2 | Change | Cause |
|---|---|---|---|---|
| Thread writes the brief and delegates | 31.2 s | 42.5 s | +11.3 s | thread model |
| `delegate_task` → worker running | 6.3 s | 0.7 s | **−5.6 s** | **engine (pre-warm)** |
| Worker running → reported | 285.2 s | 352.5 s | +67.3 s | worker (details below) |
| Reported → landed | 6.6 s | 8.6 s | +2.0 s | |

Inside the worker's +67.3 s:
- **Model time** (from tool results to the next model message): 196 s → 209 s.
- **Tool time:** 36 s → 41 s. Checks were 5.8 s of that.
- **Waiting on its own code review: 39.3 s.** Phase 2's worker started its review at 180 s and
  kept working while it ran. Run 2's worker started it last, at 307.9 s, and had nothing to do
  until it came back clean at 355.2 s.

In run 1 the worker took the outline route:
- an outline at 1:52, approved at 1:54;
- a Codex plan review from 112.7 s to 174.3 s;
- then two rounds of code review findings: findings at 352.4 s and 648.3 s, clean at 686.8 s;
- tool time was 161 s, of which 120 s were three long commands: a `sleep 6; cat` of a log,
  a Python edit, and a screenshot script.

**Tokens.** A worker's input is roughly its calls × its average context per call.

| Worker | Calls | Average context | Output |
|---|---|---|---|
| Phase 2 | 37 | 62,633 | 25,758 |
| Run 1 | 51 | 74,732 | 41,810 |
| Run 2 | 31 | 61,660 | 26,540 |

Engine costs (the same for any worker):
- **The context pack in the first message.** The first call's context grew from 20,053 tokens to
  22,356 (run 1, part of the pack) and to 26,147 (run 2, the whole pack): +2.3k and +6.1k tokens
  carried by every call. That is about +117k (run 1, 51 calls) and +189k (run 2, 31 calls) cache
  reads, plus about 6k cache writes once.
- **Run 1 only:** the pack file read again, about 4.4k tokens carried over 49 calls, about +216k.
  Fixed in `32444bb6`.
- **Brigadier's reviewer for escalations:** 0 in T1 (Full access).
- **The shared prefix and the pre-warm:** no token cost.

Worker-behaviour variance:
- **Run 1, +1.49M worker input:**
  - +14 calls at phase 2's average context: about +0.88M;
  - larger contexts: +0.62M, of which the engine costs above are about 0.33M and the worker's own
    larger reads and outputs about 0.28M;
  - outside the worker: the plan review (+277,896 Codex), larger code reviews (+79k Codex), and
    the thread judging an outline and more reports (+224k).
- **Run 2, −0.41M worker input:**
  - 6 fewer calls: about −0.38M;
  - the average context was 1k lower than phase 2 even though the pack adds 6.1k per call.
  - A pack that saved exploratory reads would show up this way, but one run can't separate that
    from variance.

What this means:
- The engine's measurable effect on T1 is about −5.6 s of wall clock and about +0.19M cache-read
  tokens per worker (run 2's pack).
- The misses on the 6-minute bound come from the thread and the worker's own choices: the outline
  route and review rounds in run 1, and in run 2 the brief taking 11 s longer and the code review
  started last and waited on.
- Phase 3's levers don't touch those. With n=2, run-to-run variance (403.9 s and 586.7 s) is far
  larger than anything phase 3 changes.

## Deviations from the plan

- **The pre-warm adopts the worktree only, not the CLI.** Go-ahead correction 1 allows this.
  - A worker's CLI session is fixed when it starts: by the routed task's model, effort, allowed
    models (from the spec's areas), plan-mode hold and access. A session started ahead would
    rarely match.
  - The gain would be about 0.6 s of CLI launch, and the worktree alone already gives about 0.7 s
    to the first event.
- **No shared `CARGO_TARGET_DIR`** (see Checks).
- **Claude workers start in `data/worker-home/<conv>`, not in their worktree**, so they share one
  cached prefix. Side effect: they no longer load the repository's `.claude/settings.json`, hooks
  or skills from the cwd, the same as the thread. The repository's instruction files are carried
  in the prompt instead.
- **The context pack lives only in the first message** (see Context pack).
