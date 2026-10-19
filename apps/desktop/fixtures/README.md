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

# Mention refresh

`http://localhost:1426/fixtures/mentions-refresh.html` backs
`src/app/conversation/Mentions.test.ts`. The test supplies `/mention-test/*` middleware
to list a disposable checkout, create a file, and fail a refresh. Run it through
`pnpm test`; the page needs that middleware. It checks menu openings, newly created
files, failed refreshes, and plain chats. This entry is absent from the production build.

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

# Terminal sessions

`/fixtures/terminal-sessions.html` renders the production bottom terminal and its real
per-place terminal store with synthetic shell IPC. Use `?name=brigadier-ai` to set the project
folder name; shell cwd deliberately uses a different worktree folder. No requests
reach a daemon, and the entry is excluded from the production build.

Open terminal, add/select/close sessions, hide/reopen, resize, and drag below the
close threshold. Check the strip with many sessions and narrow widths. Cmd+J,
Ctrl+backquote, Cmd+T, Cmd+W, Cmd+Shift+[ / ], and arrow-key tab navigation use the
production handlers. `window.terminalFixture` exposes the place, the terminal store, live
mock shells, and recorded IPC requests for browser assertions.

# A recorded thread session

`http://localhost:1426/fixtures/thread-session.html` replays a recorded session (T1's request on a
dev daemon, 2026-10-08) through the board reducer from its stored events
(`src/fixtures/boards/thread-t1-2026-10-08.events.json`). `?at=<ms>` stops at a live moment.
Unfold "Worked for" and its groups to compare the activity rows; `activity/group.test.ts` reads the
same events.

`?session=grill` replays the user's session of 2026-10-09 instead (a grill, then the right sidebar
and tabs built; `src/fixtures/boards/thread-grill-2026-10-09.events.json`) as this build settles it
(`src/fixtures/settledSession.ts`): no "Waiting on you", its last request done with its newest ending
as the answer and the first in the fold, and the merge asked on a card. `recorded=1` replays it as
stored; `summary=1` pins the side panel. `blocks.test.ts` reads the same events.

# Session approvals

`/fixtures/approvals.html` renders the production approval card with synthetic command,
network, workspace-file, outside-file and unscoped requests. It includes 320px cards and a
long command scope to check stacking and truncation. Hover or focus the session button for
its full scope. `?drive=1` exercises the session buttons and Enter/Esc and writes JSON into
`#approvals-result`; `ApprovalRender.test.ts` checks it. IPC stays mocked and never reaches
a daemon. This entry is absent from the production build.

# Decision card keys

`/fixtures/card-keys.html` mounts the production merge question, "Implement this plan?" and a
phase plan's rail link, one fresh mount per case, and drives them with the browser's real keys
and clicks: a consequential card lights nothing at first, every question card takes Enter only
when focused (a plain question lit at once too), a digit only lights, and the phase plan takes no
Enter. It writes JSON into `#card-keys-result`;
`CardKeysRender.test.ts` checks it. IPC stays mocked. This entry is absent from the production
build.

# Composer suggestion

`/fixtures/composer-suggestion.html` opens a Chat whose finished request carries its guess
at the next message, in the app's own conversation view. It drives the real keys: the guess shows
in the empty composer with no key hint, Tab puts it in the field unsent, it comes back
when the field empties and hides while something is typed, and a working request shows none. It
writes JSON into `#composer-suggestion-result`;
`ComposerSuggestionRender.test.ts` checks it. IPC stays mocked. This entry is absent from the
production build.

# Composer polish

`/fixtures/composer-polish.html?case=<case>` opens a Chat in the app's own conversation view and
drives one of the composer's small behaviours with the real keys (Find keeping the keyboard, Send
waiting for uploads, and so on). It records what the composer sends and writes
JSON into `#composer-polish-result`; `ComposerPolishRender.test.ts` checks it. IPC stays mocked.
This entry is absent from the production build.

# Session menu

`/fixtures/session-menu.html` renders the session tab's ⋯ "Chat actions" for a worktree session,
a session in its repository and a chat: "New session" shows for both sessions, "Close
workspace…" only with a worktree, and confirming its dialog sends `closeWorkspace`. It writes
JSON into `#session-menu-result`; `SessionMenuRender.test.ts` checks it. IPC stays mocked. This
entry is absent from the production build.

# Closing while hidden

`/fixtures/hidden-exit.html` renders the app's dialog and popover with the page-hidden mark the app
sets at startup. `?drive=1` closes each one while shown, while hidden with every animation paused
(as a covered macOS window runs no frames), and while visible with animations paused (the control,
which stays stuck), and writes JSON into `#hidden-exit-result`; `pageHidden.test.ts` checks it.
This entry is absent from the production build.

# Mockup

`http://localhost:1426/fixtures/mockup.html` shows a session's mockup (SESSION-FIXES-PLAN.md Q10) in its
Mockup tab of the session tab strip, with screenshots drawn in the page. Query switches:
`stage=building|ready|compared|stopped`, `chat=1` (Chat stays in front, to see the tab itself),
`comment=1` (two notes already marked). `drive=1` runs the flow `src/app/conversation/MockupRender.test.ts`
checks: the tab busy while it builds, then ready; opening it, Comment, two markers with notes, ⌘↩ sends
one message with the marked screenshot, Working, the revision's See latest changes, closing the tab and
its return at the comparison. Requests never reach a daemon.

# Sidebar and updates

`http://localhost:1426/fixtures/updates.html` shows the expanded sidebar with Claude Code and Codex updates
waiting, for screenshots. With `?drive=1` it backs `src/app/SidebarUpdatesRender.test.ts` (run by `pnpm test`):
it clicks the Chats section's "+", runs a lone agent's update from the chip, runs one of two from the list and
watches the finished row leave the open list by itself, and has an update fail to start. `?frames=1` has the
shell offer a nightly update of Brigadier, clicks its pill and records the foot's icons on every frame until it
is the restart pill; `?fail=1` has that download fail and tries again; `?app=available|updating|failed|ready`
(with `&collapsed=1` for the strip) shows the pill so, for screenshots. Requests are answered in the page and
never reach a daemon or the shell's updater.

# A failed turn

`/fixtures/turn-error.html?view=done&approvals=0&summary=0` is the phase flow with its second request
failed on an API error, the run's error and an old notice repeating it. The turn reads "Stopped after …",
then one calm row (what happened in plain words, the error's own words under Details with Copy, Try again);
above the composer only the other notice shows. A bare row sits under the page. `drive=1` opens Details,
presses Try again (recorded, never sent) and writes JSON into `#turn-error-result`;
`TurnErrorRender.test.ts` checks it. This entry is absent from the production build.

# The Full access line

`/fixtures/full-access-notice.html` is the finished phase flow with the session at Full access: one calm
row above the composer (a muted shield, "Full access: workers run without asking." with Learn more,
Don't show again and ×). `drive=1` scrolls the thread to its bottom, measures the line and the thread's
last row, closes it with × and writes JSON into `#full-access-notice-result`;
`FullAccessNoticeRender.test.ts` checks it. This entry is absent from the production build.

# A pause inside a turn

`/fixtures/thread-gaps.html` is a session whose one finished turn paused for over two hours between its
steps. Its opened work shows the first group, a centred time line at the moment the work went on (as
above a user's message sent after an hour or more), then the second group. It writes what it saw as
JSON into `#thread-gaps-result`; `ThreadGapsRender.test.ts` checks it. `scene=quota` makes the pause
a usage limit that reset: a divider, "Claude usage limit reset · 9:39 PM", stands there instead of the
time line. `stop=1` holds the page for a screenshot. This entry is absent from the production build.

# One tool's repeats

`/fixtures/thread-repeats.html` is a session whose finished turn called a tool Brigadier has no words
for six times in a row. The group's summary counts them ("used search issues 6 times"); opened, the six
are one row with their count and time, which opens to each call with what it was given; two calls of
another tool stay two rows. It writes what it saw as JSON into `#thread-repeats-result`;
`ThreadRepeatsRender.test.ts` checks it. `stop=1` holds the page for a screenshot. This entry is absent
from the production build.

# Opening the permission menu

`/fixtures/composer-permissions.html` is a session's composer. Typing `/` lists Permissions with the
level in use and its icon; `/perm` and Enter open the permission menu above the pill and it stays open;
`/model` and Enter open the model picker the same way; ↑ on the focused pill opens the permission menu
too. It writes what it saw as JSON into `#composer-permissions-result`;
`ComposerPermissionsRender.test.ts` checks it. This entry is absent from the production build.

# Rating a plan

`/fixtures/plan-rating.html` is a session with a plan and a rated answer, the context card's Plan
section beside it with a thumbs button on its title row ("Rate this plan": Good plan, Bad plan); once rated its
menu adds Clear rating, and so does the rated answer's Feedback menu. Ratings sent come back on the
board as the daemon's events would. It writes what it saw as JSON into `#plan-rating-result`;
`PlanRatingRender.test.ts` checks it. `stop=1` holds the page for a screenshot. This entry is absent
from the production build.

# Source panel menu

`/fixtures/source-menu.html` is the Source panel on a project's folder with More Actions' Branch ›
(Merge…, Rebase Branch…, Create Branch…, Create Branch From…, Rename Branch…, Delete Branch…,
Publish Branch), Stash › and Show Git Output. It creates a branch from the name box, deletes one git
won't delete as not merged through the toast's Delete Anyway, drops a stash after asking, and opens
Git Output (a failure in red, Copy). It writes what it saw as JSON into `#source-menu-result`;
`SourceMenuRender.test.ts` checks it. This entry is absent from the production build.

# Source panel rebase

`/fixtures/source-rebase.html` is the Source panel beside a conflict tab during a rebase stopped on
conflicts (#283): the branch reads "(rebasing)", the box holds the replayed commit's message, Commit
reads Continue and the Commit menu offers Abort Rebase; the tab says "Rebasing 2 of 5: …". It
resolves the conflict in the tab, stages it, continues to the next commit's conflicts and aborts,
writing what it saw as JSON into `#source-rebase-result`; `SourceRebaseRender.test.ts` checks it.
Add `?still` to stay at the first stop. This entry is absent from the production build.
