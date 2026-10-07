# Thread phase 1 results (THREAD-PLAN §3 phase 1), 2026-10-07

Phase 1's done-whens, measured on the phase-1 engine (`thread-p1`) against the baseline in
`2026-10-07-thread-baseline.md`. Measured with `tools/ab`, the same way as the baseline. Raw
artifacts are outside the repo under `/tmp/brig-ab-1007/<arm>/`.

## Summary

| Done-when | Result |
|---|---|
| T1 lands verified in ≤ 480 s, tokens ≤ 0.6× baseline (≤ 2,991,014 raw) | **Missed.** Run 2 landed in **357.2 s** (met), but used **4,091,353 raw = 0.82×** (missed). Run 1 missed both (834.7 s, 5,052,925 raw = 1.01×) |
| 0 automatic verifiers or reviewers; every landing has a `ReviewRun` whose findings reach the thread, one merged before its review ended | Met: live in both runs, plus a flow test |
| Approve for me: `pnpm test` (Chromium) and `git commit` succeed for a Claude and a Codex worker, 0 escalations | Met, after two fixes found on the way (`90f1f2c8`, `3278280c`) |
| Overnight smoke (2 small phases) completes on the old conductor with the new texts | Met: both phases verified in 130 s, 0 cards |
| An optional verifier calls `review_code`, gets the findings, triages them and reports; `rg request_review crates apps` empty | Met by flow test; the `rg` output is empty. Not run live (optional) |
| A Claude one-shot review of a Codex-authored landing runs under `dontAsk` and returns findings | Met (run 1, live) |
| `tools/full-checks.sh` passes | Met |

The user's rule is "if a phase misses its numbers twice, stop and report". T1 was run twice, and
the token number was missed both times. No third run was started.

## T1 runs

Both runs used the phase-1 daemon (`bin/brigadierd-p1`), today's new defaults (Full access, routing
with the Q7 floors), a New worktree session from `main`, and the orchestrator on Claude Opus high.
The recorder approved the merge card (`recorder.py --merge`) so that a merge made before its
review ended would be exercised live. Tokens are counted from t0 to settled (correction 2). That
includes reviews and any fix the orchestrator chose after the answer.

| | Run 1 (`t1-p1`) | Run 2 (`t1-p1b`) | Baseline (`t1-brig`) |
|---|---|---|---|
| Daemon tree | `9b5053ac` without the two fixes below | `3278280c` (both fixes) | `257bccd8` |
| Lead | Codex `gpt-6-astra` high | Claude Opus high | Claude Opus medium |
| Verified landing | **834.7 s** (first landing); 1573.7 s with the post-answer fix | **357.2 s** | 885.9 s |
| Final answer | 850.8 s; 1638.2 s with the fix | 371.5 s | 899.6 s |
| Settled | 1638.2 s | 434.6 s | — |
| Tokens raw, to settled | **5,052,925 (1.01×)** | **4,091,353 (0.82×)** | 4,985,023 |
| Tokens raw, to the first answer | 2,504,923 (0.50×) | 3,812,891 (0.76×): everything but the landing review | — |
| Checks on the result tip | `6333d009`: install, typecheck, lint, test all 0 | `e03df2d9`: all 0 | all 0 |
| Longest "only Thinking" (replay) | 3 s | 2 s | 4 s |
| Claude five-hour quota, start → end | 77 % → 88 % | 7 % → 16 % (the second account) | 14 % → 38 % |

No run hit a usage limit, so the time without a limit wait equals wall-clock time.

### Run 1

The lead was routed to Codex. It posted its outline at 100 s and got the go-ahead at 101.8 s. Its
own Claude plan and worker reviews ran in the background (387 → 448 s, 4 findings). It did its
own browser checks until about 720 s and landed at 834.7 s.

Two engine bugs stalled it, and both are fixed on `thread-p1`:
- **`90f1f2c8`, nested builds.** The machine watch treated `pnpm test`'s nested
  `pnpm --filter … test` (its own process group) as a second build and held it behind its own
  parent. That cost about 75 s twice and made the Chromium tests time out.
- **`3278280c`, `packed-refs.lock`.** A Codex worker's sandbox lacked the shared
  `packed-refs.lock`, so every commit printed a git error.

After the answer, the landing review (834.7 → 896.4 s) found 1 issue. The orchestrator delegated a
fix, which landed at 1573.7 s. That landing's review found 1 more issue, which the orchestrator
declined and said why.

### Run 2

The lead was Claude Opus high. Outline at about 97 s, go-ahead at 99.4 s. The background plan
review (Codex, 96.4 → 182.0 s) found 1 issue, and the orchestrator passed it to the lead 1.5 s later
(`message_worker`, 183.4 s). The lead's own Codex review was clean (190.2 → 267.3 s). It landed at
357.2 s (2 commits, head `e03df2d9`), and the merge card was approved at 364.0 s. The landing
review (Codex, 357.2 → 434.5 s) was clean.

### Run 2 tokens, per provider (t0 → settled)

| Provider | Uncached input | Cache reads | Cache writes | Output | Raw |
|---|---|---|---|---|---|
| Claude | 120 | 3,222,957 | 115,075 | 31,866 | 3,370,018 |
| Codex | 121,537 | 595,968 | not reported | 3,830 | 721,335 |
| **Total** | | | | | **4,091,353** |

By role:
- orchestrator, Claude Opus high: 259,101;
- lead, Claude Opus **high**: 3,110,917 (baseline lead, at medium: 2,948,249);
- Codex reviews: 721,335 (plan 289,449, worker 153,424, landing 278,462).

**A metering bug, found and fixed (`ae9873e3`).** The daemon's `turn_usage` held only the plan
review's Codex use, which gave 3,659,467 raw. `codex exec review --json` (codex-cli 0.160.1) runs
a range review in a child thread, and its `turn.completed` reports all zeros. You can reproduce it
with a one-commit repo and `codex exec review --commit HEAD --json`. The child's rollout names the
parent in `session_meta.parent_thread_id`, and its last `token_count` holds the real total. The
runner now reads it when the reported use is zero. Run 2's two code reviews were added from those
rollouts:
- review `01a116c2…`: rollout `01a116c2-71f5…`, 153,424 raw;
- review `01a116c4…`: rollout `01a116c4-fe36…`, 278,462 raw.

The fixed code reads the same two numbers from the real rollouts. The corrected split is in
`t1-p1b/tokens-corrected.json`. No other arm is affected:
- run 1's reviews were all Claude and fully metered;
- the baseline arms have no unmatched Codex child threads (checked against every rollout of the
  day).

### Why the token number is missed

Phase 1 removed the automatic verifier (1,151,778 raw in the baseline) and gave every landing one
review. But the lead's own session is about 3.1 M raw, and 96 % of that is cache reads over its 48
calls. That is already more than the 2,991,014 target on its own. The Q7 floor also lifts the lead
from medium to high, and it isn't any cheaper. Phase 1 doesn't change how a lead works, so the
token target can't be met by phase 1's cuts alone. Getting it down means reducing the lead's own
context or turns, for example a leaner brief, fewer self-checks, or a smaller reading footprint.
That is for the user to rule on: keep the 0.6× target for a later phase, or change it.

## Done-when 2: no automatic checkers, and every landing reviewed

- **Run 1** (`t1-p1/rec/events.jsonl`):
  - two tasks, both role `lead`, kind implement, both landed by the orchestrator, and 0 verifier or
    reviewer tasks;
  - 6 ReviewRuns, all by Claude Opus of Codex-authored work, one per landing:
    `59a60afb..13e66750` (834.7 → 896.4 s) and `13e66750..6333d009` (1573.7 → 1635.0 s);
  - both merge cards were approved (844.3 s, 1579.9 s) **before** those reviews ended;
  - both `[review task-n · …]` messages reached the orchestrator, which acted on the first and
    declined the second;
  - only the 2 merge cards.
- **Run 2:**
  - one task (role `lead`, implement) and 0 verifier or reviewer tasks;
  - 3 ReviewRuns: plan (findings 1, sent to the thread), worker (clean) and landing
    `59a60afb..e03df2d9` (clean);
  - the merge was approved at 364.0 s, while the landing review ran until 434.5 s;
  - a clean review doesn't wake the orchestrator, and the merge card shows "clean".
- **Flow test:** `a_review_still_running_at_the_merge_reports_its_findings_after_it` (`9b5053ac`).
  It checks that findings reach the orchestrator after the merge, that the review checkout is
  removed, and that there is one ReviewRun per landing.

## Done-when 3: sandbox, Approve for me

Daemon at `3278280c`, data `/tmp/brig-ab-1007/sbx/data-final`, base `ffc3c66e` (59a60afb plus the
Chromium tests at `--single-process`). `sbx/final/summary.py` prints it all.

| Worker | `pnpm test` | `git commit` | Escalations |
|---|---|---|---|
| Claude | exit 0, 95/95 | `fb040d69`, exit 0 | 0 `"dangerouslyDisableSandbox": true` in its transcript |
| Codex | exit 0, 95/95 | `17ec7b81`, exit 0, no `packed-refs` error | 0 `require_escalated` calls; rollout `sandbox_policy` workspace-write, network on, with the `packed-refs.lock` root |

The run raised 0 cards and 0 machine steps.

Before the fixes, the Codex worker failed 2/95 with "Vite startup timed out", and the Claude
worker took 36 s per Chromium test. The cause was the nested-build hold (`90f1f2c8`):
`taskpolicy -b` reproduces it and `nice` doesn't.

Chromium can't run multi-process in either vendor's sandbox. Neither CLI offers a mach-register
setting:
- Claude 2.1.292's settings schema has only `sandbox.network.allowMachLookup`;
- Codex 0.160.1's seatbelt policy has only mach-lookup rules.

So Brigadier's own Chromium tests run `--single-process`, and the worker prompt says so.

## Done-when 4: overnight smoke

The run used a daemon at `ae9873e3`, a fresh data dir (`/tmp/brig-ab-1007/ovn/data`) and the
plan `ovn/plan.json`: two tiny greeter phases, "stop after phase 2". It was driven over IPC by
`tools/ab/overnight.py` (`proposeOvernight`, then `startOvernight`), with the recorder answering
cards.

Timeline after Start:
- `preparing` at 0 s, then `running`;
- phase 1 verified at about 60 s, phase 2 verified at about 120 s;
- `finished` at 130 s, verified commit `8b567445`.

What the run did:
- **Tasks:** 2, both role `lead` and both landed. No verifier was started: the report says "No
  verifier; … checked its own work".
- **Reviews:** 2 ReviewRuns, Codex `gpt-6-astra` reviewing Claude's landings, both clean.
- **Report:** every criterion is `[met]`, with the commands as evidence.
- **Cards:** 0.

The two Codex reviews have `turn_usage` rows (`review:01a116cd…` 39,589 raw and
`review:01a116ce…` 54,637 raw). That shows `ae9873e3`'s metering fix working live.

Two cosmetic issues in the report, left for a later phase:
- its "Usage" line counts only Claude (345.1k), not the Codex reviews;
- "Waiting on you" lists the push/PR item twice, worded two ways.

## Done-when 5: an optional verifier, and no `request_review`

- Flow test `a_verifier_the_orchestrator_started_triages_its_review_and_reports` (`9b5053ac`):
  the orchestrator's `start_verifier`, then the verifier's `review_code`, the findings message, a
  fix and the report.
- `rg request_review crates apps`: no output (exit 1). The only remaining `outlineReview` is the
  serde alias on `PhaseStage::AwaitingGoAhead` (`crates/core/src/work.rs`), so old events still
  load.
- Not run live. The plan marks this path optional.

## Done-when 6: a Claude review under `dontAsk`

`/tmp/brig-ab-1007/t1-p1/evidence/review-argv.txt` holds 3 reviewer processes from run 1. Each
was started as `claude -p … --tools Read,Grep,Glob,Bash --allowedTools Read Grep Glob
Bash(git diff:*) Bash(git log:*) Bash(git show:*) … --permission-mode dontAsk --model opus
--effort high`. They reviewed Codex-authored work: the plan, the worker's own review, and the
landing. The landing review returned "findings 1".

## Behaviour probes (P1.x)

**Pending: Computer Use down.** `health_report` was retried on this run and still failed: "the
tag-scoped cmux Computer Use runtime is not listening … toggle Computer Use off and on". The
frozen check commands (T1.4) passed on both results.

## Checks

`tools/full-checks.sh` on `ae9873e3` passed (exit 0, "full checks passed"). It ran:
- `cargo fmt --all --check`;
- gen-ts ("generated types are up to date");
- `pnpm build`;
- `cargo clippy --locked --workspace --all-targets -- -D warnings`;
- `cargo test --locked --workspace --lib --bins --tests`;
- `pnpm typecheck`, `lint` and `test`;
- `git diff --check`.

The commits after it change only `tools/ab` Python and this doc. The final tree's run is in the
worker report.
