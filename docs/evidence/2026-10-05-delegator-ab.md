# The new flow against /delegator: Q15 A/B evidence (2026-10-05)

Phase E of the 2026-10-05 flow rebuild (PLAN.md §9's 2026-10-05 row, §10.15). It measured
Brigadier's new session loop against the `/delegator` skill on the user's own requests. The
artifacts stay outside the repo, under
`~/.claude/delegator/runs/20261005-0049-takeover-3-sessions/msgs/phaseE/`. `results.md` there has
every number and the method; `acceptance.md` holds the checks, frozen before either arm ran; and
`tools/` holds the scripts. The installed app, its daemon and its data were never touched.

## Setup

- **Brigadier arm:** a dev `brigadierd` from the flow branch, run with its own `BRIGADIER_DATA_DIR`
  and driven headless over its IPC socket. It ran a session on the clone's `main`, with the
  orchestrator on Claude Opus (high) and Routing defaults for workers. A recorder subscribed to the
  daemon's events (with an EventsSince backfill) and would have answered any card the way the user
  would, logging it. None came.
- **/delegator arm:** a fresh Claude Code session (Opus 5.5, high) in its own cmux workspace, given
  `/delegator <the same request>` in its own clone, under its own run id.
- **Conditions:** each arm had a throwaway clone at the same base, warmed the same way. The arms
  ran one after the other, /delegator first, each starting on a fresh Claude 5-hour window, with
  no thermal warnings.
- **Boundaries:** both arms start when the request is submitted. They end at the verified landing
  on `main`, and separately at the coordinator's final answer.
- **Tokens:** an explicit session manifest per arm. For Claude, the final usage of each message
  is counted. For Codex, the last cumulative total, with cached input inside input and reasoning
  inside output. No session was unknown.
- **"Only Thinking":** a stretch where the only thing the thread shows is the Thinking indicator.
  It was measured by replaying the recorded events through the desktop app's own row-derivation
  code, once a second.

## Results

| Target (Q15) | Result |
|---|---|
| Speed ≤ 1.2× /delegator; a small request lands in < 5 min | Met. Inline image paste: 71:43 to landing against 66:21 (1.08×), and 72:03 to the final answer against 71:38 (1.01×). The small request: 3:52 (6:51 before the fixes below). |
| Tokens ≤ /delegator's | Met. 21.5M raw against 79.9M; 0.77M against 1.31M without cache reads. |
| Zero cards (Full access, including `cargo test`; Approve for me, a normal build+test) | Met. 0 cards in both runs. Under Full access the workers ran `cargo test --workspace` five times. |
| Rows from minute one; never more than ~30 s of only "Thinking" | Small request: 11 s. Image paste: 32 s, the orchestrator's first turn. "Working for…" showed from 3 s. At the limit, not fixed. |
| Verifiers pass; no serious unfixed review finding | Met. Both verifiers passed. One identical Codex review per arm found 1 medium finding in Brigadier's change and 2 in /delegator's, and nothing serious. |

## What the A/B found and fixed (flow branch)

- The build lease counted every `rustc` under a worker's `cargo` as a separate build, so Rust
  builds compiled one crate at a time.
- A worker's PATH had no `cargo` when the login shell doesn't add it.
- The small-request reviewer rebuilt the change and re-ran the author's checks, instead of reading
  the change.
- A worker report that said "None for implementation." under needs_user became a Waiting on you
  item, so a finished request read as waiting on the user.

## Caveats

- **/delegator stall excluded.** /delegator's lead killed processes by name. That took down every
  background shell on the Mac, including its own coordinator's wait. The 3 h 28 min until the
  coordinator was recovered are left out of its time.
- **Brigadier ran under extra load.** During Brigadier's arm the user's own follow-up work ran in
  the other clone. Brigadier's time was not adjusted for it.
- **Settings task not run.** The third task (Settings width and Refresh rankings) was not run on
  either arm, to save the user's weekly Claude quota.
- **Last fix not re-run live.** It is covered by its unit test.
