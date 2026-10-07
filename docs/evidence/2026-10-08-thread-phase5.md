# Thread phase 5: overnight on the same thread (2026-10-08)

THREAD-PLAN.md §3 phase 5. Branch `thread-p5` from `thread-build` `d3653dfa`. Claude Code 2.1.293,
codex-cli 0.160.1. Every live check ran on a dev `brigadierd` built from the branch, with a scratch
`BRIGADIER_DATA_DIR` (`/tmp/w14/live/data`) and a scratch repo (`/tmp/w14/live/repo`), driven over
IPC with `tools/ab` (`startd.sh`, `setup.py`, `recorder.py`, `bipc.py`) at Full access, the thread
on Claude Opus 5.5 high. Computer Use was down (`health_report`: "the tag-scoped cmux Computer Use
runtime is not listening"), so every check was scripted.

## Done-when

| Done when | Result |
|---|---|
| A 2-phase plan with a 20-min deadline runs on the normal thread, winds down at the deadline, writes the morning report (commits, reviews, waiting items, usage) and keeps the machine awake during the run | **Pass.** Run 2 below: stop `deadline`; wind-down due at +800.0 s, began at +828.0 s (the supervisor's 30 s tick), finished at +841.2 s, before the +1200 s deadline. The report has Phases, Commits (each with its review), Waiting on you, Details (how each phase was checked) and Usage. `caffeinate -d -i -s -w <daemon>` and SleepDisabled=1 during the run; both gone within 10 s of the end |
| No code from `conductor.rs`'s phase loop remains | **Pass.** `conductor.rs` is deleted. Of its 38 function names, the 26 of the phase loop (`advance_run`, `pick_next`, `hand_to_lead`, `nudge_lead`, `lead_turn_ended`, `phase_done`, `propose_phases`, `resume_judged_plan`, `settle_phase`, …) appear nowhere in `crates` or `apps`; the 11 that remain are the run's record and decision helpers, rewritten in `run.rs` (`record_run`, `change_run_if`, `run_decided`, `run_setting`, …), and `unsettled` is an unrelated test helper. `rg 'phase_done\|propose_phases\|advance_run\|hand_to_lead\|nudge_lead\|pick_next\|lead_phase\|lead_turn_ended\|resume_judged_plan\|retire_orchestrator\|PhaseState\|PlanningPhase\|CriterionResult\|PhaseGate' crates apps` (generated excluded): 0 lines |
| During the run, a thread tiny edit lands on the run branch and a thread preview runs from the run worktree; the session branch is unchanged; after the run the same native thread session continues (same session id) in the session checkout | **Pass.** Run 2: `314dccb Add NOTES.md` (and phase 1's `4125e0b`) on the run branch with `Brigadier-Author: thread`; the preview `python3 -m http.server 8765` (pid 90675, the daemon's child) had its cwd in `…/overnight-1f4ed60c`; the session branch stayed at `17c9807`; the preview was gone after the run. The thread CLI: before `--session-id c06d419d…` with `--add-dir …/session-44812b70`, during `--resume c06d419d…` with `--add-dir …/overnight-1f4ed60c`, after `--resume c06d419d…` with `--add-dir …/session-44812b70`; one transcript `c06d419d….jsonl`; asked its branch after the run, it answered `brigadier/44812b70/session`. Run 1 showed the same with `f43c6259` |
| Stop and restart mid-run recover | **Pass.** Run 3 (restart): the daemon killed while the run's worker ran, started again 3 s later; "resuming an overnight run after a restart"; the thread listed its tasks, delegated again, landed `3cf642c`, settled the phase and called `end_run`: stop `done`. Run 4 (Stop): `stopOvernight` 23 s after Start → `windingDown` at once, the morning answer, the report, `finished` 25.4 s later, stop `stopped`, keep-awake released |

## The runs

All four on one dev daemon (`/tmp/w14/bin/brigadierd-p5`); runs 1–2 from the tree before
`52dfbd11` (Rust identical but for the report's usage fix), runs 3–4 from `52dfbd11`.

**Run 1, `end_run`.** "Greeter night", 2 phases, "For 20 minutes", phase 2 "run `./slow-check.sh`
(about 25 minutes)". The thread made phase 1 itself as a tiny edit (`62ec155`, trailer
`Brigadier-Author: thread`, on the run branch), judged that a 25-minute check can't fit a 20-minute
run, settled phase 2 partial and called `end_run` (done) 22.6 s after Start; finished 5.4 s later,
stop `done`, "Merge takes phase 1 (`62ec155`)", the commit's review "still running" at the report.
It didn't test the deadline, so run 2 waits on a signal instead.

**Run 2, the deadline.** "Greeter release", 2 phases, "For 20 minutes", rules "keep checking
`./status.sh` until it prints READY or the run's time is up; don't settle phase 2 or end the run
before READY" (READY never came). Start `1791410631507`, wind-down due `1791411431506`.

| | ms | from Start |
|---|---|---|
| running | 1791410631594 | +0.1 s |
| phase 1's tiny edit `4125e0b` on the run branch (first look) | 1791410649000 | by +17.5 s |
| mid-run message (NOTES.md + preview) | 1791410653775 | +22.3 s |
| NOTES.md `314dccb` and the preview up, thread's answer | 1791410664502 | +32.9 s |
| `windingDown`, stop `deadline` | 1791411459549 | +828.0 s |
| the thread's morning answer | 1791411468811 | +837.3 s |
| `reporting` | 1791411472602 | +841.1 s |
| `finished` | 1791411472675 | +841.2 s |

The report (Details trimmed):

```
**Greeter release**: stopped at its 01:23 deadline. 1 of 2 phases done.

Merge takes phase 1 (`4125e0b`). 1 later commit isn't in the merge.

1 thing waits on you.

### Phases
- ✓ Phase 1 · Greet: done.
- ◐ Phase 2 · Release note: partial, Write the READY line ./status.sh prints into RELEASE.md and commit it, once the release s….

### Commits
- `314dccb` Add NOTES.md · review: no findings (not in the merge)
- `4125e0b` Add greet.sh · review: no findings

### Waiting on you
- Phase 2 needs the release signal: ./status.sh printed WAIT all night, so RELEASE.md is still unwritten.
Then say “continue” to pick the run up on the same branch.

### Details
How each phase was checked:
- Phase 1:
  - Settled: Added executable greet.sh at the repo root (commit 4125e0b). p1-c1 checked: …
- Phase 2:
  - Settled: Ran ./status.sh once a minute from 01:04 to 01:17. Every check printed WAIT, …
  - Left: Write the READY line ./status.sh prints into RELEASE.md and commit it, once …
Branch `overnight/2026-10-08-greeter-release-1f4ed60c` from `brigadier/44812b70/session`. …
Usage: Claude 276.4k tokens.
```

Its usage line left out the two Codex reviews (54k and 29k in `turn_usage`, under
`review:<id>`): fixed in `52dfbd11` (counted with the session's turns, unit-tested); runs 3 and 4
read "Claude 278.4k · Codex 170k" and "Claude 142.5k · Codex 88.7k".

Keep-awake, sampled every 10 s (`/tmp/w14/live/sampler.log`): the first sample after Start (+6 s)
had none yet; from +16 s to the end `caffeinate -d -i -s -w 44836` and SleepDisabled=1 (the lid
rule, "sleep disabled for the closed lid" in the daemon log); 4 s after `finished` neither, and
"sleep restored". SleepDisabled was 0 before, between and after the runs.

**Run 3, restart.** "Farewell", 1 phase, rules "delegate phase 1 to a worker". Start
`1791411774805`; the worker `01a11876-7c23` running; daemon killed at `1791411794` and started
again at `1791411797`. The worker went `stopped`; the thread, told "Brigadier restarted", listed
its tasks, delegated again (`01a11877-1c4c`: landed `3cf642c`), settled phase 1 done and called
`end_run`; finished `1791411922` (+147 s), stop `done`, "1 task landed", the stopped worker
under Problems. Its report also said "Brigadier was unavailable 01:17–01:23", from before the run
began: the gap took its words from the last heartbeat on record, an earlier run's. Fixed in
`5dc2df63` (each run's gap from the later of that beat and its start, over three missed beats,
in its own words); the restart flow test now seeds such a beat and asserts no gap, and fails
without the fix.

**Run 4, Stop.** "Release wait", 1 phase waiting on the signal. Start `1791412052004`, Stop
`1791412075090`, `windingDown` at once, the morning answer ("You stopped the run at about 01:28 …
I left the phase unsettled"), finished `1791412100532`, stop `stopped`: "0 of 1 phase done",
"◐ Phase 1 · Release note: unfinished, you stopped the run".

## T1 (one run, per D5)

Arm `/tmp/brig-ab-1007/t1-p5`: a clone warmed with `warm.sh` from `t1-p2`, daemon `5dc2df63`
(`bin/brigadierd-p5`), Full access, new worktree, thread Claude Opus 5.5 high, as phase 2's run.

| | Phase 5 | Phase 2 |
|---|---|---|
| Landed (`times.py`) | **332.8 s** | 328.9 s |
| Answer / settled | 343.9 s / 343.9 s | 339.7 s / 339.7 s |
| Tasks | 1 (implement) | 1 |
| Reviews | 1, Codex, on the worker's range | 1 |
| Frozen checks on `2bfd30da` | install, typecheck, lint, test: all exit 0 | all exit 0 |
| Raw tokens (`armtokens.sh`, t0 → settled) | **2,779,508** | 2,862,725 |
| Claude raw | 2,587,151 (thread 341,640, worker 2,245,511) | 2,723,037 |
| Codex raw | 192,357 (the review) | 139,688 |
| Without cache reads | **187,878** (Claude 148,097, Codex 39,781) | 168,970 |

`turn_usage` and the transcripts agree exactly (difference +0); no Codex child thread went
unmetered. Quota at start: Claude five-hour 3%, seven-day 19%; Codex 5%. Phase 5 touches only the
overnight path, so T1 is the same within one run's noise (+3.9 s, −3% raw).

## Checks

- `tools/full-checks.sh` on `5dc2df63`: passed (fmt, gen-ts up to date, pnpm build, stage-sidecar,
  clippy `-D warnings`, `cargo test --workspace` (core 275 passed, 1 ignored), pnpm typecheck, lint,
  test 142 passed, `git diff --check`).
