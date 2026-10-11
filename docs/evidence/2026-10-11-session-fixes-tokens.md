# Session fixes phase 3: worker tokens, before and after (SESSION-FIXES-PLAN.md §3 Q6.7)

The fair A/B of phase 3's done-when. This document records the frozen task, checks and Brain seed,
written before either arm ran, and then both arms' results.

## Frozen before either arm

### The task

`tools/ab/tasks/t3.md`: session B's task 7 ("Add access configuration"), as one self-contained
request with the decisions B's thread had settled with the user.
- **Base:** `fbd607aa`, the parent of B's landed solution `524c337e`.
- **Request:** verbatim in `t3.md`, followed by `tools/ab`'s safety sentence (as in every A/B since
  2026-10-08).
- **Models:** the thread on Claude Opus high. Workers run on Claude Opus, through a routing rule
  `only` for every kind of work except reviews (`setup.py --claude-workers opus`). Reviews route as
  usual.
- **Conditions:** Full access, the same Claude account for both arms (the run's own, given as
  `BRIG_CLAUDE_HOME`), and one arm at a time with nothing else building. Answers follow `POLICY.md`.
- **Builds:** "before" is the verified phase 2 tip, and "after" is that tip plus phase 3's commits
  only. Both are frozen once phase 2 is verified, and the baseline is rerun if that foundation
  changes.

### The hidden checks, validated

`tools/ab/checks/t3/hidden.sh` copies two tests into a tree, runs them and removes them. Neither arm
sees them. They were run with that SHA's own code, in a detached worktree with its own build:

| SHA | `configuration_page.test.ts` | `suggest_full_access.rs` | `hidden.sh` |
|---|---|---|---|
| `fbd607aa` (base) | fail: no page labelled Configuration | fail: no `suggest_full_access` for a thread | exit 1 |
| `524c337e` (B's solution) | pass | pass | exit 0 |

The behaviour probes (P3.1–P3.3) run on each arm's result with that commit's own app build and
data dir (`probe-app.sh`), never the driver daemon. They are validated the same way, on the base
and on the solution, before the arms run.

### The Brain seed

`tools/ab/brainseed.py` reads the project's `brain.sqlite` from the installed app through
`?mode=ro` and copies it with SQLite's backup API. Nothing in the installed app's data was written.
The cut is B's task 7 start, 2026-10-10 13:21:07.321 UTC (`1791638467321`).

The cut is conservative. Any node with something dated at or after the cut is dropped: its
creation (`created_ms`, or its UUIDv7 id's time), an update, a supersession, a stale mark, or a
source. Transcript rows from the cut on are dropped too.

| | |
|---|---|
| Nodes kept | 282 of 888 (contract 15, convention 106, decision 36, file summary 25, module 16, report 50, service 5, task 29) |
| States kept | fresh 121, stale 57, superseded 104 (4 pointers to dropped nodes cleared) |
| Newest moment anywhere in the seed | 13:20:48.516 UTC, before the cut |
| Sources | report 184, enrichment 67, orchestrator 14, user 11, skeleton 6; no node without a source |
| Sessions | session A 116 nodes, session B 85, one earlier session 8, project-level 73 |
| Tasks sourced | 19 tasks, all finished before the cut: A's tasks 1–12 and B's tasks 1–5. B's task 7 sourced nothing |
| sha256 | `0a1e21735b23e52800214aba55370ab822441a37557c51281c1bb8404fbb6140` |

The seed was searched for the task's own words:
- **No matches:** `suggest_full_access`, `ConfigurationPage`, `SandboxLimitNotice`,
  `FullAccessSuggested`, `resolvePermission` and `prefs.permission` appear nowhere in it.
- **"Settings > Configuration" and "default permission"** appear in two places:
  - B's two thread decisions, recorded with the user before task 7 began. They are the same
    decisions the request states.
  - B's task 4 study report, which describes the reference app.

### Session B for reference (`worker_tokens.py`, read-only snapshot)

| | |
|---|---|
| Workers | 34 workers made 2,835 calls with start contexts of 15–30k |
| Worker tokens | 218.9M raw, 5.40M without cache reads |
| Brain use | 32 `query_brain` calls in all; 0 Brain facts in any brief |
| Checks | 448 `run_check` calls, 3 cached |
| Task 7 | 271 calls; 18.0M raw, 280k without cache reads; 2 Brain queries; 29 `run_check` calls |
| Task 7's review fix (task 16) | 100 calls; 6.3M raw |

## Results

(After the arms.)
