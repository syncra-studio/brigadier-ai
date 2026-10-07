# Thread baseline (THREAD-PLAN §3 phase 1, scope 1), 2026-10-07

T1 and T2, each once in a plain cmux `/delegator` session and once in current Brigadier, before
any phase-1 engine change was built. Measured with `tools/ab` (see its README). Raw artifacts are
outside the repo, under the A/B root `/tmp/brig-ab-1007/<arm>/`, and the run directory
`~/.claude/delegator/runs/20261007-1404-thread-build/`.

## Setup

- **Tasks:** `tools/ab/tasks/t1.md` (base `59a60afb`, image sha256 `d4794b3b…`) and `t2.md`
  (base `53c5cd32`). Frozen and committed (`0fec9b7e`) before any arm ran. The images stay
  outside the repo.
- **Clones:** one throwaway clone per arm (`clone.sh`), with `main` itself pinned to the frozen
  base, no other branches and no remote. All four were warmed the same way (`warm.sh`:
  `pnpm install`, then the Rust build from one shared `target/`).
- **Order:** T1·/delegator, T1·Brigadier, T2·/delegator, T2·Brigadier, back to back. No builds or
  tests ran on the machine while an arm ran.
- **/delegator arm:** `tools/ab/dlg_start.sh`. It opens a new cmux workspace in the clone that runs
  the user's own unmodified `claude` (2.1.292, `--model claude-opus-5-5 --effort high
  --dangerously-skip-permissions`) and accepts the folder-trust prompt. It then types `/delegator
  <verbatim request> (Image #n is the file <path>)` and Enter, which is t0. The coordinator ran
  its own `dlg` run, workers, tabs and Codex reviews, unsteered.
- **Brigadier arm:** a debug `brigadierd` built from `257bccd8`. `git diff 7cff0db9 257bccd8 --
  crates` is empty, so this is the plan's "current Brigadier". It runs through `startd.sh` with its
  own `BRIGADIER_DATA_DIR` (`<arm>/data`) and a clean environment. The installed app and its data
  were never used.
  - `setup.py`: onboarded, Fable hidden, today's defaults: Approve for me.
  - `recorder.py`: records every event and answers cards like the user. It leaves the merge card
    unanswered.
  - `brainwait.py`: the project's Brain jobs ended before t0.
  - `send.py`: a New worktree session from `main`, orchestrator Claude Opus high, the request with
    its image inline.

## Boundaries

- **t0:** the request is submitted.
- **Verified landing:**
  - Brigadier: the request's last `landed` step (`orchestratorStepped`), which comes after any
    verifier the engine ran.
  - /delegator: the later of (a) the coordinator's last checking worker (its verifier) ending
    `done`, and (b) the result tip reaching its branch (`git reflog`).
  - Either way, it counts only if the frozen checks then pass on that tip (`check.sh`, run
    independently afterwards).
- **Final answer:**
  - Brigadier: the request's end.
  - /delegator: the coordinator's last end-of-turn message.
- **Settled:** the last activity of anything the request started. That includes reviews that end
  after the answer. Tokens are counted from t0 to here.
- **Tokens:**
  - Brigadier: the arm's `routing.sqlite` `turn_usage` (`brig_tokens.py`), plus the Codex
    threads it doesn't meter. It is cross-checked against the session transcripts and the daemon's
    usage events (`armtokens.sh`).
  - **Corrected on 2026-10-07 by phase 1's verifier.** Under Approve for me, a Codex worker's
    auto-review (the "guardian") runs in a child thread of its own. Its use is in the child's
    rollout only: neither the parent thread nor `turn_usage` reports it. Both Brigadier arms' Codex
    verifiers had one: T1 118,244 raw (rollout `01a11631-3bbe…`), T2 97,814 raw (`01a11673-b17a…`).
    The first count missed them; the tables below include them. `brig_manifest.py` now lists every
    Codex child thread, so `armtokens.sh` shows such a gap as a difference.
  - /delegator: `dlg_manifest.py` → `tokens.py`. That covers the coordinator, every worker and
    every Codex session started in one of the run's checkouts. Every saved review file was matched
    to a counted session. For T2 it also covers the sessions of the workers' own dev-app daemons
    (`--also`).
  - Codex reports no cache writes: "not reported", never 0.

## Results

| Arm | Result tip (start commit) | Verified landing | Final answer | Checks on the tip |
|---|---|---|---|---|
| T1 /delegator (run `20261007-1424-sidebar-chevron`) | `d8a37673` on `main` (59a60afb) | **575.6 s** | 622.4 s | install, typecheck, lint, test: all 0 |
| T1 Brigadier (`257bccd8`) | `951dc676` on the session branch (59a60afb) | **885.9 s** | 899.6 s | all 0 |
| T2 /delegator (run `20261007-1453-thread-working`) | `7bd37c28` on `thread-working-indicator` (53c5cd32) | **3273.0 s** | 3333.0 s | all 0 |
| T2 Brigadier (`257bccd8`) | `ada353df` on the session branch (53c5cd32) | **944.1 s** | 957.4 s | all 0 |

Start commits were asserted with `git log --reverse <base>..<tip>`, and every arm's first commit
sits on its task's base. Wall-clock times are shown. No arm hit a usage limit, so time without a
limit wait is the same, and no arm is quota-affected.

### Tokens, per provider (t0 → settled)

| Arm | Provider | Uncached input | Cache reads | Cache writes | Output | Raw | Raw without cache reads |
|---|---|---|---|---|---|---|---|
| T1 /delegator | Claude | 212 | 6,629,615 | 219,160 | 53,922 | 6,902,909 | 273,294 |
| | Codex | 40,405 | 113,536 | not reported | 1,365 | 155,306 | 41,770 |
| | **Total** | | | | | **7,058,215** | 315,064 |
| T1 Brigadier | Claude | 112 | 3,011,483 | 115,722 | 30,975 | 3,158,292 | 146,809 |
| | Codex | 248,174 | 1,682,688 | not reported | 14,113 | 1,944,975 | 262,287 |
| | **Total** | | | | | **5,103,267** | 409,096 |
| T2 /delegator | Claude | 728 | 46,426,365 | 507,754 | 153,239 | 47,088,086 | 661,721 |
| | Codex | 140,392 | 1,069,824 | not reported | 6,082 | 1,216,298 | 146,474 |
| | **Total** | | | | | **48,304,384** | 808,195 |
| T2 Brigadier | Claude | 108 | 3,241,864 | 109,967 | 26,361 | 3,378,300 | 136,436 |
| | Codex | 241,938 | 2,284,416 | not reported | 17,793 | 2,544,147 | 259,731 |
| | **Total** | | | | | **5,922,447** | 396,167 |

Codex's input includes its cached input. Here "uncached input" is input minus cached.

Who used it in the Brigadier arms (`by_model_role` in `tokens.json`):
- **T1:**
  - orchestrator Claude Opus high: 210,043;
  - lead Claude Opus at **medium**: 2,948,249;
  - Codex `gpt-6-astra` (the two automatic reviews): 674,953;
  - Codex `gpt-6.1-sol` medium (the automatic verifier): 1,151,778, plus its auto-review child
    thread 118,244.
- **T2:**
  - orchestrator: 242,941;
  - lead Claude Opus medium: 3,135,359;
  - `gpt-6-astra` (the automatic outline review, which held the lead about 2 min, and the code
    review): 900,103;
  - `gpt-6.1-sol` (the verifier): 1,546,230, plus its auto-review child thread 97,814.

### "Only Thinking" (Brigadier, replayed through the app's row code at `257bccd8`)

- T1: longest 4 s (the first 4 s), 0 stretches over 30 s.
- T2: longest 8 s, 0 over 30 s.

The baseline engine already shows rows early (the first action row at 4 s and 8 s). Phase 1 is
mostly about the time to landing and the tokens.

### Conditions

Claude five-hour quota at each arm's start: 14 % (T1 /delegator), 14 % (T1 Brigadier), 38 % (T2
/delegator), 62 % (T2 Brigadier); 66 % after T2 Brigadier. Codex primary 1–2 %. No thermal or
performance warnings were recorded, and free memory was about 65 %. Per-arm files:
`<arm>/conditions-start.txt`, `conditions-end.txt`. The T1 end readings were taken before
`conditions.sh` refreshed quota (fixed in `11d91f9a`), so they repeat the start values.

## Behaviour probes (P1.x, P2.x)

**Pending: Computer Use down.** The cmux Computer Use daemon was unavailable for the whole run
("toggle Computer Use off and on in cmux Settings"). `probe-app.sh` itself works: it built and
started the T1 /delegator arm's app under its own identity. The probes need the user to re-enable
Computer Use. Until then T1.1–T1.3 and T2.1–T2.3 are judged only from each arm's own evidence and
diff, not by the frozen probes. The check commands (T1.4, T2.4) passed on all four tips.

## What each arm did

- **T1 /delegator.** Lead `w01` committed straight on `main`. Verifier `w02` made no fixes. One
  Codex review, clean.
- **T1 Brigadier.**
  - Lead: Claude Opus at medium.
  - Then the automatic verifier (Codex `gpt-6.1-sol` medium) and two automatic Codex reviews.
  - 0 cards answered. The merge card was left for the user.
- **T2 /delegator.** An outline, a Codex outline review, the lead and the verifier on their own
  branch, and a Codex code review (2 bugs, fixed). 7 commits.
- **T2 Brigadier.**
  - The automatic outline reviewer (Codex) held the lead for about 2 minutes.
  - Then the lead (Claude Opus medium), the automatic verifier (Codex sol medium) and two
    reviews.

## Incidents

- T2 /delegator's own workers drove a GUI during their checks. Lead `w01` took a full-screen
  capture, which was deleted. Verifier `w02` clicked the user's **installed** Brigadier by
  mistake, so its sidebar state may have been toggled. This is outside the measured product, but
  the user should check the installed app's sidebar. Future evaluators target their app by PID
  only.
