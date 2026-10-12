# Session fixes phase 3: worker tokens, before and after (SESSION-FIXES-PLAN.md §3 Q6.7)

The fair A/B of phase 3's done-when. This document records the frozen task, checks and Brain seed,
written before either arm ran, and then both arms' results.
The rerun with 3 runs per arm (2026-10-12) is the last section; it replaces the n = 1 results
for the gates.

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

## Rerun: 3 runs per arm (2026-10-12)

**Phase 3's measured token gates are missed again, clearly.** Over three runs per arm, after
used 0.87× before's worker tokens (gate ≤ 0.6×) and 0.89× the sum (gate ≤ 0.75×), with 0.75× the
`run_check` calls (gate ≤ 0.5×). Even the best after run against the worst before run is 0.71×
for worker tokens. No rerun of an incomplete run could change that, so none was made (the
Delegator's rule for this rerun). The per-provider, elapsed-time and fix-task gates pass on the
means; the Brain gates hold in every after run but one worker (r6's fix task).

### How it ran

- **Arms.**
  - before: the daemon built at `4920bbde` (the phase 2 tip), as defined above.
  - after: the daemon built at `8014bf30`: phase 3's final tip, with fixes 1a–1c (`e0653432`,
    `63b077d5`, `abcd1e95`) and the phase 3 verifier's fix `5a31d274`.
  - The user asked for the after arm to be the session-fixes tip `b1e78213`. It wasn't used: it
    also carries phases 4–6, whose prompts make a UI task offer a mockup first (`MOCKUPS`) and
    have its worker build `pnpm tauri:debug-app` and drive it with the computer tools
    (`WORKER_COMPUTER`). At the base `fbd607aa` that build's identity is `ai.brigadier.dev`, the
    user's own running Brigadier Dev, and the before arm does none of it. The Delegator approved
    `8014bf30`, which matches the definition above ("that tip plus phase 3's commits only").
- **The same in both arms.** t3 at `fbd607aa`; a fresh clone, data dir and daemon per run, warmed
  from one warm clone (`cp -c`); `setup.py --permission fullAccess --claude-workers opus`; the
  Brain seed (sha `0a1e2173…6140`, checked every run) copied in after setup, then a restart;
  `recorder.py`, `brainwait.py`, `send.py`, `reqdone.sh`. Settled = the last event once the request
  had ended, no task, review or Brain job was open and the thread was idle, held for 2 quiet
  minutes. Measured with `worker_tokens.py` and `armtokens.sh` at that time.
- **One Claude account, the default login, in every run.** No `BRIG_CLAUDE_HOME`, so `startd.sh`
  set no `CLAUDE_CONFIG_DIR` (its environment starts from `env -i`). Checked every 90 s during each
  run: the daemon and every `claude` process under it had no
  `CLAUDE_CONFIG_DIR` (`ps eww`); no `accounts/` folder in the data dir; the daemon's
  `accountsChecked` event listed only the default account; transcripts landed in
  `~/.claude/projects`. (The two Brigadier-stored accounts' `projects` folders are symlinks to that
  same folder, so the run folders show there too.)
- **Order.** Three blocks, each one before run and one after run, ordered by an `os.urandom` bit
  recorded before the first run (02:06): all three bits came up 0, so before ran first in every
  block. r4 was stopped at 04:39 when the account's 5-hour window reached 95%, before its first
  task had finished; it isn't counted and ran again as r4b after the window reset.

| run | arm | daemon up | t0 | settled | result |
|---|---|---|---|---|---|
| r1 | before | 02:08:28 | 02:09:35 | 02:40:29 | `9de2e74a` |
| r2 | after | 02:47:40 | 02:48:39 | 03:26:29 | `baac614e` |
| r3 | before | 03:34:41 | 03:35:29 | 04:17:19 | `478335ae` |
| r4 | after | 04:25:11 | 04:25:59 | stopped 04:39 (quota) | — |
| r4b | after | 06:44:15 | 06:45:02 | 07:13:03 | `83247801` |
| r5 | before | 07:22:11 | 07:23:19 | 07:55:31 | `c1024e98` |
| r6 | after | 08:02:21 | 08:03:29 | 08:49:09 | `aaef60f0` |

(Times EEST, 2026-10-12. Each result is the session branch, not merged: its merge card stays
unanswered by policy. Cards answered: 0 in every run.)

### Per run

| | r1 before | r3 before | r5 before | r2 after | r4b after | r6 after |
|---|---|---|---|---|---|---|
| Worker tokens, transcripts, raw | 33.06M | 37.44M | 28.77M | 26.62M | 29.96M | 30.12M |
| Worker tokens, transcripts, without cache reads | 0.52M | 0.87M | 0.56M | 0.98M | 0.94M | 0.73M |
| Worker tokens, daemon, raw | 28.92M | 31.46M | 26.08M | 26.52M | 29.76M | 30.10M |
| Claude, all sessions, raw | 33.68M | 38.86M | 29.57M | 28.77M | 30.97M | 32.30M |
| Codex, all sessions, raw | 1.69M | 2.64M | 2.65M | 1.14M | 2.43M | 1.89M |
| Sum, raw | 35.37M | 41.50M | 32.22M | 29.90M | 33.40M | 34.19M |
| Elapsed to the answer (s) | 1809 | 2490 | 1904 | 2270 | 1632 | 2711 |
| Worker calls | 166 | 244 | 191 | 303 | 199 | 234 |
| Average context per call | 176.0k | 128.9k | 138.0k | 78.4k | 95.7k | 112.8k |
| `run_check` calls | 26 | 42 | 27 | 26 | 16 | 29 |
| Review-fix tasks | 0 | 1 | 1 | 0 | 0 | 1 |
| Workers stopped without a report | 0 | 1 | 0 | 2 | 1 | 0 |
| Operate tasks | 0 | 1 | 0 | 0 | 0 | 0 |
| Brain nodes at t0 | 307 | 300 | 307 | 299 | 302 | 307 |
| Brain queries per worker | 1 | 1, 1, 0, 0, 0 | 2, 0 | 1, 1, 1, 1, 2 | 3, 2 | 1, 0 |
| Brain facts in each brief | 0 | 0, 0, 0, 0, 0 | 0, 0 | 15, 15, 15, 15, 15 | 15, 15 | 15, 0 |
| Brain facts written back | 37 | 0, 38, 9, 8, 3 | 26, 6 | 0, 0, 17, 20, 21 | 0, 22 | 36, 2 |
| Peak context per worker session | 288k | 95k, 285k, 67k, 54k, 28k | 266k, 80k | 120k, 105k, 77k, 142k, 45k, 144k, 106k | 100k, 204k, 116k | 203k, 204k, 88k |

- Worker tokens by transcripts count every worker session and its subagents; this is the fair
  figure. The before daemon doesn't meter subagents (fix 1c is in after only), so the daemon
  figure under-counts before by one or two subagents per run.
- Average context is per worker call, weighted by calls.

### The arms

| | before mean (range) | after mean (range) | after ÷ before | gate | worst (highest after ÷ lowest before) |
|---|---|---|---|---|---|
| Worker tokens, transcripts, raw | 33.09M (28.77M–37.44M) | 28.90M (26.62M–30.12M) | 0.87 | ≤ 0.6: miss | 1.05 |
| Worker tokens, daemon, raw | 28.82M (26.08M–31.46M) | 28.80M (26.52M–30.10M) | 1.00 | ≤ 0.6: miss | 1.15 |
| Claude raw | 34.04M (29.57M–38.86M) | 30.68M (28.77M–32.30M) | 0.90 | ≤ 1.0: pass | 1.09 |
| Codex raw | 2.33M (1.69M–2.65M) | 1.82M (1.14M–2.43M) | 0.78 | ≤ 1.0: pass | 1.44 |
| Sum raw | 36.36M (32.22M–41.50M) | 32.50M (29.90M–34.19M) | 0.89 | ≤ 0.75: miss | 1.06 |
| Elapsed (s) | 2068 (1809–2490) | 2204 (1632–2711) | 1.07 | ≤ 1.25: pass | 1.50 |
| `run_check` calls | 31.7 (26.0–42.0) | 23.7 (16.0–29.0) | 0.75 | ≤ 0.5: miss | 1.12 |

- "worst" is the highest after run over the lowest before run. The other way round (best after
  over worst before) worker tokens are 0.71× and the sum 0.72×: also a miss on worker tokens.
- **Brain gates** (after arm): ≥ 1 query per worker and ≥ 1 fact in each brief hold for every
  worker of r2 and r4b, and for r6's lead. r6's second task (a review fix, "Commit missing new
  files") started with no Brain section in its brief and made no query.
- **Fix tasks:** before 0, 1, 1; after 0, 0, 1. No extra fix tasks: pass.
- **Where after's tokens go:** each call carries less context (78–113k against 129–176k), but
  there are more calls and sessions, and a lead reached the 200k hand-off in two of three runs
  (r4b, r6).

### Things that happened in the runs

- **"The worker stopped without a report" (a bug, both arms).** A worker started a background
  subagent and ended its turn to wait for it ("Its findings will start my next turn"). Brigadier
  read that as the end, nudged once, ended the CLI and failed the task; the thread then sent the
  work again. r2 twice, r3 once, r4b once. Each cost a few million tokens; the totals include
  them, as the product spent them.
- **An operate task met the user's Brigadier Dev (r3, before).** After the work landed, r3's
  thread sent an operate task to check the page in a dev build. Its worker built the arm's
  `Brigadier Dev.app` (identity `ai.brigadier.dev`, the base's dev identity) and launched it twice;
  both launches exited at once, handed by the single-instance guard to the user's running Brigadier
  Dev (PID 63195). The worker didn't act on that app, and it kept running; the handover may have
  brought its window forward. The computer helper also failed to start ("No such file or
  directory"). The worker's scratch repo and data folder were deleted. Phase 4's "computer use
  refuses a Brigadier app the session didn't launch" (`e6d9d013`) isn't in either arm.
- **The Brain drifts between runs.** After the restart, the Brain's skeleton job (Claude haiku,
  before t0) adds a varying set of nodes to the seed's 282: 299–307 nodes at t0 (n = 1 had 288).
  The seed is the same each run; the skeleton's own output is not.
- **Machine load (elapsed time only).** Nothing of this run built during a window, but no window
  was free of other work: w42 drove probe apps 02:08–02:20 (r1) and 02:48–02:50 (r2); another
  session's `cargo test --workspace` ran during r3; from 06:44 (r4b, r5, r6) other workers built
  and tested by agreement (`/tmp/term-docs-fixes-ab/overlaps-extra.txt`), with load averages of
  20–39 during r5 and r6, against 3–17 in r1–r3. The elapsed gate passes on the means (1.07×) but
  carries this caveat; token counts don't depend on load.

### Checks, probes and review

- **Frozen checks** (`check.sh` at each result, after its window): all six pass `pnpm install`,
  typecheck, lint, test, the gen-ts diff, `hidden.sh` and the touched crates' `cargo test`.
  - The frozen `cargo clippy` fails in all six (exit 101) for the missing
    `binaries/brigadierd-…` sidecar of a fresh worktree; with the empty stand-in it passes in all
    six. Recorded "As run", as at n = 1.
  - r1's `cargo test -p brigadier-core` failed 3 preview tests that need their data under `/tmp`;
    rerun with `TMPDIR=/tmp` (as `tools/full-checks.sh` runs them) it passes. The later checks ran
    with `TMPDIR=/tmp`.
- **Behaviour probes** (run by w42, w45 and w47 on every result, each with its own identity and
  data dir; `/tmp/term-docs-fixes-ab/probes.txt`): P3.1 and P3.2 pass on all
  six results, and fail on the base and pass on the solution (validated). P3.3 has no verdict on
  this Mac for any build: the Command Line Tools are already installed, so the frozen message
  needs no access even on the solution, and the substitute the Delegator approved ("Install the
  cowsay formula with Homebrew on this Mac for me.", at Ask for approval) gets that level's own
  approval card, with no notice, on the solution too. P3.3 rests on `hidden.sh` (the tool in every
  thread role) and the review.
- **Blind review.** Codex (gpt-6.1-sol, effort high, read-only), one call per result, each
  given only the request, the frozen checklist C1–C13 (the whole request, accurate explanations
  included) and that result's diff against a read-only base. The labels R1–R6 came from a
  `random.SystemRandom` shuffle at 07:57:47, written and hashed (sha256 `700130c8…9e51`) before any
  review, and read only after all six verdicts were saved. The same prompt on B's own solution
  `524c337e` (R0) calibrated the reviewer.

  | | C2 explanations | C3 default | C5 one session only | C6/C7 tool and notice | C13 tests | other parts | whole request |
  |---|---|---|---|---|---|---|---|
  | `524c337e` (R0) | partly | done | partly | partly | partly | done | no |
  | r1 before (R1) | partly | done | partly | done | partly | done | no |
  | r3 before (R3) | partly | done | partly | partly | partly | done | no |
  | r5 before (R5) | partly | done | partly | partly | partly | done | no |
  | r2 after (R2) | partly | done | partly | done | partly | done | no |
  | r4b after (R6) | partly | partly | partly | done | partly | done | no |
  | r6 after (R4) | partly | done | partly | partly | partly | done | no |

  - **No result does the whole request, and neither does B's solution**, for the same reasons:
    explanations that don't match the real sandbox or approval behaviour (C2); a level picked for
    one session that can stay in the draft when creation ends after the user navigates away, a
    race in the base's draft clearing (C5, C3); a notice withheld in untrusted folders, an
    exception the request didn't make (C6/C7 in r3, r5, r6 and the solution); tests that don't
    catch those (C13).
  - The reviewer is consistent: the same parts fall short in every verdict, the solution's
    included.
  - By the rule set for this rerun every run is incomplete. Reruns weren't made because none could
    change the verdict: the token gates miss on all six runs, complete or not.

### Reading

The gates are missed by a margin the noise doesn't cover. After's `run_check` calls fell to 0.75×, and
the missing sidecar (fix 1b) comes up in the transcripts of every before run but only one after
run. But the per-call savings are spent on more calls and sessions. The bug above, and the
Brain's skeleton drift, add noise to both arms; neither explains a 0.87× against a 0.6× gate.
