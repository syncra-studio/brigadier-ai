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
  *As run (see Results):* the user narrowed the accounts while the before arm ran, so the two arms
  ran under different Claude accounts.
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
*As run (see Results):* they were not validated before the arms. The computer-use helper was down
for the whole run, so the probes have not run on the base, the solution or either arm. The arms
never see them either way.

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

**Phase 3's measured gates are not met.** After used 0.81× before's worker tokens (gate ≤ 0.6×), 0.83×
the total (gate ≤ 0.75×), and 1.67× the `run_check` calls (gate ≤ 0.5×). The Brain gates, the
provider gate, elapsed time and fix tasks pass. This is one run per arm. The blind reviewer also
found both results short of the whole request, which by t3's rule voids the comparison until it is
rerun.

### How the arms ran

| | before | after |
|---|---|---|
| Daemon | `4920bbde` (phase 2 tip), `before-src` build | `ba0fe1eb` (sf-p3 tip), its own build |
| Claude account | acct-5896e1e5 (`BRIG_CLAUDE_HOME`) | the default `~/.claude` login (no `CLAUDE_CONFIG_DIR`) |
| Brain | seed copied in after setup (sha checked); the restart's skeleton added 6 module nodes: 288 nodes | the same: 288 nodes, module 16 → 22 |
| t0 (request sent) | 1791709039179 (11:57:19 EEST) | 1791710835443 (12:27:15 EEST) |
| Request answered | +1425.8 s | +1487.1 s |
| Settled (last event: landing review over) | 1791710541879 | 1791712400974 |
| Tasks | 1 implement task, landed; plan review 4 findings, code review 1, landing review clean | 1 implement task, landed; plan review 3 findings, code review 2, landing review clean |
| Result (session branch, not merged: its merge card stays unanswered by policy) | `788037fb` | `3a3318a5` |
| Cards answered | 0 | 0 |

- **Different accounts.** The user ruled during the before arm that only the default login is used
  from then on. The before arm finished on acct-5896e1e5 and the after arm ran on the default
  login. Model and effort were the same (Opus 5.5 high). The arms ran one after the other, so they
  shared no prompt cache.
- **Load.** No build ran on the machine between the arms. The machine is shared with another run
  and was not watched for the whole of each arm.
- **Transcripts folders,** to delete later:
  - before: `~/Library/Application Support/Brigadier/accounts/acct-5896e1e5/projects/-private-tmp-term-docs-fixes-w24-ab-before-data-orch-01a12a2e-4a4b-76ff-9928-fed2dc711eba`
    (the thread). The worker's folder was removed at its landing, as the before build does.
  - after: `~/.claude/projects/-private-tmp-term-docs-fixes-w24-ab-after-data-orch-01a12a49-b2f2-7355-8103-7c1aededd26b`
    and `~/.claude/projects/-private-tmp-term-docs-fixes-w24-ab-after-data-worker-home-01a12a49-b2f2-7355-8103-7c1aededd26b`.
  - Each arm's recorder also hard-linked every transcript into the scratch dir.

### Tokens (t0 to settled)

Workers are counted two ways:
- **Transcripts** (`armtokens.sh`): every message of the worker's sessions, including the one
  Explore subagent each worker started. The daemon doesn't meter subagents, about 3M in each arm.
  This is the fair figure.
- **Daemon** (`worker_tokens.py`): the worker usage events.

| | before | after | after ÷ before | gate |
|---|---|---|---|---|
| Worker tokens, transcripts, raw | 25,331,223 | 20,460,162 | **0.81** | ≤ 0.6: **miss** |
| Worker tokens, transcripts, without cache reads | 476,057 | 535,988 | 1.13 | |
| Worker tokens, daemon, raw | 22,214,578 | 17,532,881 | 0.79 | ≤ 0.6: miss |
| Worker tokens, daemon, without cache reads | 357,687 | 422,545 | 1.18 | |
| Claude, all sessions, raw | 25,926,774 | 21,365,425 | 0.82 | ≤ 1: pass |
| Codex, all sessions, raw | 1,785,602 | 1,516,412 | 0.85 | ≤ 1: pass |
| Sum, raw | 27,712,376 | 22,881,837 | **0.83** | ≤ 0.75: **miss** |
| Elapsed to the answer | 1425.8 s | 1487.1 s | 1.04 | ≤ 1.25: pass |

`armtokens.sh` exits 3 ("incomplete") in both arms, for two reasons:
- **The Brain's skeleton job has no Codex rollout.** In both arms it ended before t0, so it is
  outside the window.
- **The daemon's `turn_usage` is below the transcripts:** +3,116,645 before and +2,980,390 after.
  - before: the worker's subagent, 3,116,645.
  - after: the worker's subagent, 2,927,281, plus one thread research call on Codex
    (gpt-6-luna), 53,109.
  - The daemon meters neither.

### Brain use, checks, fix tasks

| | before | after | gate |
|---|---|---|---|
| Worker calls (daemon count) | 140 | 163 | |
| Worker sessions | 1 (peak context 262k) | 2: handed to a fresh session at 201.7k, then 127k | |
| Average context per call | 158.7k | 107.6k | |
| Brain queries per worker | 1 | 4 | ≥ 1: pass |
| Brain facts in the brief | 0 | 15 (an 8.9 KB section) | ≥ 1: pass |
| Brain facts written back | 22 | 30 | |
| `run_check` calls | 12 | 20 | ≤ 6: **miss** |
| Review-fix tasks | 0 | 0 | 0 extra: pass |

- **The Brain gates pass with the steer's help.** The after worker's brief opened with
  `## What the Brain knows about this`. It held 15 hits, among them B's two Configuration
  decisions, the IPC convention and the core and desktop modules. The worker didn't query until
  the `BRAIN_FIRST` steer came at about its 10th call. It then asked three questions in a row and
  one more in the second session.
- **Where the after arm's tokens went:**
  - Cost per call fell by a third (average context 158.7k → 107.6k).
  - There were more calls (140 → 163).
  - Q6.4's 200k handoff started a second session. It cost 4.7M raw in 64 calls: it re-ran the
    whole check suite (13 `run_check` calls against 7 in the first session) and spent three
    clippy runs on the missing `binaries/brigadierd-…` sidecar resource of a fresh worktree.
  - The Explore subagent cost about 3M, the same as before's.

### Independent verification

- **Frozen checks** (`check.sh` at each result, in a detached worktree of the arm's clone):
  - Both results pass `pnpm install`, typecheck, lint and test, the gen-ts diff, `hidden.sh`, and
    `cargo test` for `brigadier-core` and `brigadier-mcp-server`.
  - The frozen `cargo clippy` failed in both with exit 101. The cause was not the code: the desktop
    build script wants `binaries/brigadierd-aarch64-apple-darwin`, which a fresh worktree lacks.
  - Rerun with the empty stand-in that `tools/full-checks.sh` also uses, clippy passes on both
    with exit 0. The logs are in each arm's `checks/`.
- **Behaviour probes P3.1–P3.3:** not run. The computer-use helper was down from before the
  before arm to the end: it is "not listening", and the recovery is in the user's cmux settings.
  Neither arm's result has been probed.
- **Blind review:**
  - Codex at effort high, read-only, given the request, a base checkout and the two diffs.
  - The coin flip was recorded before the prompt was written: 2026-10-11 12:56:18 +0300,
    `os.urandom` bit 0, so X = before and Y = after.
  - Verdict: **neither does the whole request.**
  - Both: the page, saving the default, existing sessions keeping their level, one-off levels, the
    composer path and confirmation, the link, no notice at Full access, the button giving way, old
    data loading, and tests and a fixture are done.
  - Both lose on accuracy in the explanations and on a notice withheld in untrusted folders, an
    exception the request didn't make.
  - X also: its tool description excludes a blocked push, and a notice whose event fails to store
    still reports "Shown".
  - Y also: forks keep the source session's level instead of taking the default, and Ask's
    explanation leaves out its overnight promotion.

### Reading

The changes do what they aim at, but the totals don't follow:
- the briefs carry the Brain;
- workers query it (after the steer);
- each call carries less context;
- no review produced a fix task.

The after arm's savings per call were spent on more calls, on a handoff session that re-verified
everything, and on checks the handoff repeated. With one run per arm, the call count alone varies
widely: B's own task 7 took 271 calls. The gates would need several runs per arm to separate the
change from that noise.

### What followed (the user's decision, 2026-10-11)

**The gates were missed at n = 1.** The user chose to fix the three causes the after arm showed,
without a rerun. **A rerun with 3 runs per arm is deferred until after phase 6, at the user's
option.** Until then, phase 3's measured gates stand as missed. The fixes, each tested on its own:
- `e0653432`: the handoff note lists the checks that passed and the commit each passed at. The
  fresh session's hand-off gains `checks.md`: each check of the task whose latest run passed, the
  tree it passed on and the files changed since. Its first message and the worker prompt
  (revision 8) say to rerun only the checks those changes cover. The 200k size stays.
- `63b077d5`: every worktree Brigadier makes gets an empty stand-in for each git-ignored Tauri
  `externalBin` sidecar, so `cargo clippy` runs in a fresh worktree. This covers task, session,
  overnight and review worktrees.
- `abcd1e95`: a Claude worker's subagent tokens are counted in its turn usage, so `turn_usage` and
  the worker usage events no longer miss them. The Agent tool stays.

The behaviour probes P3.1–P3.3 and the live check of the Brain counters still wait for computer
use.
