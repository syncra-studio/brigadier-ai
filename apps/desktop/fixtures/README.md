# Plan cards

Run `pnpm dev --port 1426` in `apps/desktop`, then open
`http://localhost:1426/fixtures/plan-cards.html`. All data is synthetic; commands are
recorded in `window.planFixtureCalls` and never reach a daemon. This entry is absent
from the production build. Set `window.planFixtureFailNext = true` to exercise a
recoverable command error.

The gallery renders normal approval/review/revision/rejection and all three approval
origins, overnight proposals (including bare goals, invalid restrictions and power
risks), preparation, planning, work, limits, wind-down, reporting, partial completion
and full completion. A started run reads as its progress over a timeline of its phases;
the active phase is marked on it. Expand criteria, use Show N more, open a worker,
and exercise Start/Stop/Merge/Continue/Read report against the fixture actions.

Normal plans render as the summary context card's Plan section, each in a card of its own here.
Use `?summary=1` for the actual pinned summary and its thread links. Check a wide
window and a 320px window: the plan is a section of the context card, the run's card sits
beneath it, and View plan opens the floating summary at narrow widths. Earlier revision links
open the current card, whose history folds. The summary fixture holds only normal plans; the application adapter uses real run records when present.

Check both density modes; Tab through folds, worker/report links and buttons, and use
Enter or Space to activate them. The `/overnight` menu item only prepares a draft.
Normal approval/rejection still sends `decidePlan`; automatic normal proposals have
no user Start or approval control. Stop settles after one successful fixture call;
Continue creates another proposal and requires Start.

# The night of 2026-10-03

`http://localhost:1426/fixtures/night.html` renders the real thread and side panel of the first
overnight run from its stored events (`src/fixtures/boards/overnight-2026-10-03.json`, made by
`scripts/extract-board-fixture.mjs` from a copy of the database; each decision's `short`, the words
the daemon's board gives it, comes from what the Rust test `the_nights_fixture_carries_the_boards_short_words`
prints, and the run's `reportText`, its report as the daemon renders it again, from what
`the_night_of_october_3_reads_in_twenty_seconds` prints with `--nocapture`). Requests are answered in the
page and never reach a daemon; a worker's transcript is not in the fixture. Open Phase 2's header:
it shows seven rows (the plan, four workers, one judgement call, the lead's reply), where the app
showed 53 before one row per task. Open a row's chevron for its checks round by round, and the
run card's Phase 2 for its steps.

The page has the app's window layout (rail, sidebar, top bar), so the summary sits where it does in the
app. Query switches: `density=compact`; `sidebar=0` (sidebar closed); `run=live` (the run still working)
or `run=none` (a normal session, whose plan is the context card's Plan section); `plans=N` (N more
plans: the latest shows, the rest sit under Earlier plans).
`window.night` holds the stores and `revealPlan`/`revealOvernight` for probes.

# Worker activity

`http://localhost:1426/fixtures/worker-activity.html` holds synthetic board data and serves
the rendering test in `src/app/conversation/WorkerActivity.test.ts`, run by `pnpm test`.
It renders task rows and the expanded background-workers strip with a fixed clock,
including live activity, waits, checks and a completed worker. This standalone entry
is absent from the production build.

# Phase flow

`http://localhost:1426/fixtures/flow.html` renders a session run the phase way from synthetic data
(`src/fixtures/flow.tsx`): a finished request, and a second one whose lead works while three approvals
wait in the composer's place (a command, a keychain action, "Start this plan?"). Answering an approval
removes it, as the daemon would. Query: `view=done` (the second request finished, with what waits on the
user), `approvals=0`, `sidebar=0`, `summary=0`, `density=compact`. `window.flow` holds the stores and
the recorded requests (`calls`).

# Live thinking

`/fixtures/thinking.html?approvals=0&summary=0` extends the phase flow with reasoning before
and after action rows. `view=done` shows the finished turn folded into its work header.
Open the lead's name to see the same live snippet and expandable thoughts in its own thread.
The standalone entry is excluded from the production build and never reaches a daemon.

`src/replay/thinking.ts` samples event envelopes through the app's board reducer, block builder
and row derivation. `pnpm test` verifies the timed fixture in `src/replay/thinking.test.ts`:
reasoning appears at 2 seconds, no later frame shows only Thinking, and actions separate thoughts.
