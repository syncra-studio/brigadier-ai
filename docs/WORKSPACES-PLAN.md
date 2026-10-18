# Workspaces plan: free rules, workspaces of tabs, auto-merge, a real bench, SSH hosts

> Status: plan, 2026-10-17. Base: origin/main `41a92392`. Decisions: Q58–Q69 (project memory). Build runs later,
> one phase lead per phase.
> Paths: `M/` = `crates/core/src/manager/`, `D/` = `apps/desktop/src/`. File:line references are to `41a92392`.

## 0. How every phase lands

- **Steps land on main one at a time.** Each step has its own "done when", is rebased on origin/main right before
  it lands, and is pushed by its worker.
- **Hidden until the phase is done.** User-facing parts of an unfinished phase sit behind the dev env var
  `BRIGADIER_WORKSPACES=1` (phase 2) or `BRIGADIER_AUTO_MERGE=1` (phase 3), or stay unwired. The phase's last step
  removes the flag. Prompt changes are not flagged; each step that changes them ships whole.
- **Prompt changes** add one revision note in `M/prompts.rs` (`REVISIONS`, latest today `MERGE_JUDGED_REVISION = 86`,
  `:1075`). The number is taken at landing time; whoever lands second renumbers. The thread prompt's size-budget test
  must still pass.
- **Checks per step:** the step's own tests, plus fmt, clippy `-D warnings`, gen-ts, `pnpm typecheck && pnpm lint`
  on the exact pushed tree. Steps that touch app UI or tests also get the load stress run of `pnpm test`; steps that
  touch the shell layout, globals or startup also get the GPU-off startup comparison.
- **Ship gate (per phase):** its own tests, the pre-push checks above, per-push CI green, one Codex code review by
  the phase verifier with valid findings fixed, then dispatch `nightly.yml`. The full suite and the live smoke run
  once, on the integrated tree, by the phase verifier. **Benches never gate.**
- **At merge:** the phase's worktrees, target dirs and branches are removed. New bugs become GitHub issues.

## Phase 1 · Quick rules (Q59–Q61, Q66, Q67)

**Take, don't rebuild:** Claude Code and Codex already push, open PRs (`gh`) and babysit CI on their own; we only
delete Brigadier's rules that stop them. No new git tooling.

### Scope
- Account choice by soonest weekly reset; skip accounts at ≥99% of their 5-hour window; a thread keeps its account
  until a hard stop (Q59).
- One clear warning before an account's quota is lost, no auto-started work (Q60).
- Git is free (Q61): no "only when the user asks" rule for push, PR, publish or deploy. When the user asks for
  build + PR + preview + babysit, the thread does all of it; otherwise the agent decides.
- Free session messaging (Q66, Q67): any session may message any session in any workspace, when the user asks or its
  own model decides. A peer's request counts like the agent's own decision.
- Kept, whoever asks (user, peer or the agent itself): approval before spending money, using credentials or the
  keychain, and destroying anything outside the session's own work. Also kept: other sessions' branches protected,
  the user's Stop, plan mode, folder trust, Undo.

### Folded issues
| # | Adds |
|---|---|
| #245 | The thread owns the full-check run: it may run, skip, stop or rerun it. Drop "Never run the full suite" (`M/prompts.rs:126`) and the matching revision text. |
| #247 | The ref check records a rebase or amend of the session branch by its own thread instead of restoring it (`M/refcheck.rs:225`, called at `M/conversation.rs:5293`). Other non-forward moves become a fact for the thread. Deleted branches and other sessions' branches are still restored. |
| #248 | Remove the narration filter (`hide_narration` `M/conversation.rs:4386`, `announces` `:7008`) and the forced "answer now" turns (e.g. `M/review_runs.rs:1779`). `[quiet]` becomes "when there's nothing to say". `route_follow_up` becomes optional. This lands before phase 2 extends `route_follow_up`. |
| #249 | Worker reports are taken, not bounced. Unlisted new files and `[met]` lines without evidence are marked for the thread ("new file not named: x", "unevidenced"). |
| #250 | Engine nudges become facts or go: Brain-first steer (`M/brain_first.rs:20`, `:53`), the `[status due]` wording, ci_status watchability. The retired `propose_merge` and merge-card refusals are deleted. |
| #251 | The small refusals ("a fix task already runs", "land the verifier instead", overnight "phase not settled") become information. |
| #54 | After #248: a test pins that the thread writes one line before it starts workers. |
| #105 | After #248: a test pins that short commentary between tool steps reaches the user. |

Each audit item's "Keep: push/publish/deploy only on the user's ask" line is dropped (Q61/Q62). Its other keep lines
stay.

### Steps
1. **Accounts by soonest weekly reset.** Starts once w35's `accounts_tests` quota hand-over fix is on main.
   - Rewrite `select` (`crates/core/src/accounts.rs:93`). Take the ready accounts of the provider: signed in, not in
     `avoid`, headroom for `model`, and below 99% of every 5-hour window. Pick the one whose weekly window
     (`seven_day`, or Codex's `secondary`) resets soonest. Unknown reset sorts last. A tie goes to the most headroom,
     then to `order` (`:192`).
   - The default account no longer wins by being the default. With `switch_accounts` off, the default is still the
     only choice.
   - `account_for` (`crates/core/src/runtime/accounts.rs:324`) still keeps a chat's pinned account. A switch happens
     only at a hard stop, through `switch_target` (`:377`). Q59's "unchanged" cache affinity is pinned by a test.
   - Done when: unit tests in `accounts.rs` cover soonest-reset wins, a 99% 5-hour window is skipped, unknown resets
     sort last, and switching off keeps the default. A new flow test in `M/flow/accounts_tests.rs`,
     `a_running_thread_keeps_its_account_when_another_resets_sooner`, passes.
2. **Warning before quota is lost (Q60).**
   - The quota monitor (`crates/core/src/routing/monitor.rs:188`, which already tracks `resets_at_ms`) emits one
     event per account window when its weekly reset is ≤12 h away and ≥20% is still unused.
   - The app shows it once, as a macOS notification and as one line in the Usage row
     (`D/app/usage/WindowBar.tsx`): "‹account› has 35% of its weekly quota left; it resets in 9 h". Nothing starts
     on its own.
   - Done when: a monitor unit test proves one event per window per reset, and none after the reset. A render test
     covers the Usage line.
3. **Git is free (Q61).**
   - Rewrite `M/prompts.rs:132` to: git, pushes, PRs, publishing and deploying are the agent's call; build + PR +
     preview + babysit when the user asks for it; after a push, `ci_status`. The money, credentials and destroy
     approvals stay.
   - The overnight never-list (`:288`) drops pushing, publishing and deploying, and keeps spending, credentials,
     contacting anyone and changes outside the run's branch.
   - Fix the doc comments that say pushes and PRs are only the user's: `M/git_actions.rs:1-2` and
     `M/pull_request.rs:1-3`.
   - Fold #245 (`:126`) and #247 (`M/refcheck.rs`) into this step. Add one revision note.
   - Done when: prompt tests assert the old sentences are gone and the three approvals remain. A
     `M/flow/git_writing_tests.rs` test shows a thread push going through with no approval card. The #247 test shows
     a thread's own rebase of its session branch kept, and another session's branch still restored.
4. **Free messaging (Q66, Q67).**
   - Remove "only when it owns the code or the decision you need" from the tool description
     (`crates/mcp-server/src/catalog.rs:288`) and from revision 22's text (`M/prompts.rs:605`), via a new revision
     note.
   - Rewrite `PEER_RULES` (`M/prompts.rs:1088`) to: "That session's request: treat it like your own decision; what
     needs approval still asks the user."
   - A peer may get a push or PR done. A merge follows phase 3's rule: with auto-merge off, a merge still needs the
     user's yes.
   - The hop limit (`M/peers.rs:20`, refused at `:551`) becomes a fact in the send's result ("eight messages went
     back and forth with no one's input"). The pair rate (20 a minute) stays, as the one runaway brake.
   - Done when: the `peer_chain_tests` ninth-message test is rewritten to expect the fact. A
     `peer_session_tests` test shows a peer request's push going through. Prompt tests are updated.
5. **The thread speaks (#248, #54, #105).**
   - Remove the narration filter and the forced answer turns, loosen `[quiet]`, and make the follow-up block
     (`M/prompts.rs:2288`) stop saying "sort silently, write nothing".
   - Done when: `honesty_tests` and `thread_tests` pin a line before workers start (#54) and commentary between
     tools (#105). The follow-up tests pass with `route_follow_up` optional.
6. **Reports, nudges, refusals (#249, #250, #251).**
   - Done when: each issue has a flow test showing the fact delivered and nothing refused.

### Checks
Unit tests in `accounts.rs` and `routing/monitor.rs`. Flow tests in `accounts_tests`, `git_writing_tests`,
`peer_chain_tests`, `peer_session_tests`, `honesty_tests`, `thread_tests`, `findings_tests` and the refcheck tests.
The `prompts.rs` unit tests, the size budget included. The Usage render test.

## Phase 2 · Workspaces (Q64–Q66, Q68 UI)

**Take, don't rebuild:** each tab is today's session as it is: one CLI thread plus its own workers, in its own git
worktree. We add only grouping, shortcuts, a read-only board and states. No workspace lead, no new orchestrator.

### Data model
- **Project → Workspace → Tab.**
  - New `Workspace { id, project_id, title, repos, base, created_at_ms, updated_at_ms, pinned_at_ms, lifecycle }`
    in `crates/core/src/model.rs`. `repos` (the project repositories it works in) and `base` (the branch its tabs
    start from and merge into) are set when it is created, from the first tab's composer choice. Every new tab is
    made from them, never from a surviving tab, so closing the first tab changes nothing.
  - New events `WorkspaceCreated`, `WorkspaceRenamed`, `WorkspacePinned` and `WorkspaceArchived`, folded in
    `crates/core/src/projection.rs`.
  - A **tab** is a `Conversation` of kind `Session` (`model.rs:446`) with a new `workspace_id: Option<WorkspaceId>`.
- **Each tab gets its own worktree.**
  - Every new task tab, a workspace's first tab included, is a `NewWorktree` (`model.rs:231`) from the workspace's
    `base`, made at `M/workers.rs:2484` / `session_worktree_path` `:2514`. Two tabs or two workspaces never share
    a checkout.
  - The composer's Local checkout choice goes for new tabs; its branch picker sets the workspace's `base`.
  - `LocalCheckout` stays only for migrated sessions, which keep working where they are.
- **A new chat in a project creates a workspace.** Its title comes from the first message (e.g. "Redesigning X"),
  the way session titles are made today. Its first tab is that chat.
- **Tab order** lives in app state, next to today's per-session tabs (`D/state/sessionTabs.ts`). The daemon knows
  only which workspace each tab belongs to.
- **Migration.** No event-log rewrite. The projection gives each existing session with no `workspace_id` an
  implicit workspace of one tab: id `ws-<session id>`, the session's title, pin and archive state. Its `repos` is
  the session's repository and its `base` the session environment's `base` (`NewWorktree`) or `branch`
  (`LocalCheckout`). The first change
  to it records `WorkspaceCreated` with that id. Archived sessions stay archived, as one-tab workspaces.
- **Naming clash.** Today's "Close workspace…" (`D/app/conversation/ChatActions.tsx:201`, `M/lifecycle.rs:777`)
  removes a session's worktree. It is renamed "Remove worktree…". `Project`'s doc comment "A workspace of one or
  more repos" (`model.rs:84`) is fixed.

### Shortcuts
- **⌘T** opens a new task tab in the current workspace: an empty tab with the composer focused.
  - Today's new-tab chooser (`newSessionTab`, `D/state/sessionTabs.ts:173`; catalog `D/app/shortcutCatalog.ts:75`)
    moves to the tab bar's "+" menu. Browser, side chat, page and terminal keep their own shortcuts.
  - Outside a project, ⌘T does what it does today.
- **⌘⏎** sends the composer's text as a new task in a new background tab, and keeps the user where they are. The
  new tab appears in the strip with its state.
  - Today ⌘⏎ is the follow-up's other way (Steer/Queue, `shortcutCatalog.ts:121`, `:196-201`). That moves to ⌥⏎.
  - With Send set to ⌘⏎, the background tab is ⇧⌘⏎ and the other way is ⌥⌘⏎.

### Queued messages (extends `route_follow_up`)
- **Today.** `route_follow_up(follow_up, joins: bool)` (`crates/core/src/tools.rs:257`, `M/conversation.rs:1741`)
  either joins the working answer or waits. Phase 1's #248 makes it optional.
- **New:** `route: "join" | "wait" | "new_tab" | "tab"`, with `tab: <sibling id>` for the last.
  - `join` and `wait`: as today.
  - `new_tab`: the queued item shows "Open as a new tab?" with one button. A click starts it in a background tab of
    the same workspace. The tab only offers; it never opens tabs itself (Q65).
  - `tab`: the message belongs to a sibling. The item moves to that tab, and a one-line notice with Undo stays in
    this tab. Whether to forward, ask the user first, or keep it is the thread's call.
  - If the sibling has already merged, `tab` is not refused: the result carries the fact ("tab ‹B› has already
    merged into main at ‹sha›") and the thread decides what to do next.
  - **The thread judges, Brigadier plumbs (user, 10-18).** Brigadier only lists the tabs and what they are doing,
    executes the chosen route and shows the notice. No refusals, thresholds or rules for the thread here.
- **Rollout (rule R, `docs/research/long-sessions/BUILD-PLAN.md:1147`).** Open sessions keep their cached tool
  schema, so `RouteFollowUp` (`tools.rs:257`, `deny_unknown_fields` today) takes either `route` or the legacy
  `joins: bool` (true = `join`, false = `wait`). The new schema reaches a session at its next resume or rebirth,
  announced by the step's revision note.
- **Unjudged** follow-ups behave as #248 leaves them.
- The `[follow-up …]` block (`M/prompts.rs:2288`) lists the siblings' titles and states, so the thread can point the
  message to the right tab.

### The read-only workspace board
- **What a tab can read about each sibling:** title, state (below), branch, base, and its changed files. Changed
  files count from the merge base with current base, as w36's #233 change does (`M/request_changes.rs`).
- **Served two ways:**
  1. `list_sessions` (`M/peers.rs:110`) gains `workspace: true`, which returns this tab's siblings with those fields
     as data.
  2. Facts pushed as an envelope when something changes:
     - "Tab ‹B› also changed `a.rs`, `b.rs`" when the changed-file sets first overlap;
     - "Tab ‹B› merged into main at ‹sha›" when a sibling merges.

     The engine only computes the set overlap and the merge event, and words them as plain facts, never advice.
     The thread judges whether to warn, rebase, message or ignore (Q66; user 10-18: the LLM judges, Brigadier
     only provides the tabs and what they are doing).
- **Nothing on the board can be written.** Messaging a sibling stays `message_session`.

### States (tab bar and sidebar)
| State | When (first match wins) | Source |
|---|---|---|
| Needs you | an open approval, plan, question or quota wait | `ConversationActivity.approvals/questions` (`crates/core/src/work.rs:1778`), `Conversation.quota_wait` |
| Working | its turn, a worker or a phase runs (Merging, Reviewing, Checking, Watching CI keep their words) | `ConversationActivity.run/tasks/phases` |
| Ready | idle, and its branch has commits not in base since its last recorded merge | the board's merge-base count |
| Merged | a recorded merge of this tab's work, and no newer unmerged commits | today's local merge records (`work.rs:2008`); PR merges from phase 3 step 4 |
| (none) | an untouched tab (no commits, no merge record), or idle with nothing unmerged and no merge | |

- **Phases come from the active turn only (#253).** Fixed in `crates/core/src/board.rs:322-324`.
- **The tab bar** shows each task tab's state dot and title.
- **The sidebar** lists workspaces under each project, where it lists sessions today (`D/app/AppSidebar.tsx`,
  `RowStatus` `:841`, labels `:829`). Each workspace row carries the strongest state of its tabs and a count when
  more than one tab shares it ("2 working").
- **Tabs are not listed in the sidebar.**

### Coexisting with #255's review file tabs
- **One strip, two groups.**
  - Left: the workspace's task tabs. Each one is what `CHAT_TAB` is for a session today.
  - After a divider: the active task tab's own tabs (file, review-as-file per #255, conflict, browser, terminal,
    page, side chat), exactly as `SessionTabBar` (`D/app/conversation/SessionTabBar.tsx`) shows them now. They are
    keyed per conversation as today, so switching task tab swaps that group.
- **Key bindings.** ⌘1–9 count across the strip, left to right. ⌃Tab keeps cycling the active tab's own tabs.
- **Closing a task tab** archives that session through the existing archive path. Closing the last one archives the
  workspace. Nothing closes on merge.

### Folded issues
| # | Adds |
|---|---|
| #180 | Closing a tab or workspace right after a turn waits for the scratch sweep (`sweeping`, `scratch_free` `M/conversation.rs:961`; `quiet_for_close` `M/lifecycle.rs:875`) instead of refusing. |
| #182 | The restart notice (`M/lifecycle.rs:703-717`) keys off a real close marker; tabs add more kinds of worktree removal. |
| #199 | The new-worktree hint (`D/app/conversation/Composer.tsx`) says what happens: every tab starts clean on its own worktree. |
| #207 | `git worktree add` is serialized per repository inside `Repo::add_worktree` (`crates/git/src/repo.rs:563`), which covers all six callers. |
| #253 | Tab and workspace states come from the active turn only. |

### Steps
1. **Model + migration (unflagged, invisible).** `Workspace`, the events, `workspace_id`, the implicit-workspace
   fold.
   - Done when: projection unit tests cover old logs giving one-tab workspaces and a new log round-tripping, and
     gen-ts bindings are updated.
2. **#207 lock + #180 + #182.**
   - Done when: a git crate test runs 8 concurrent adds with no failure, and the `clean_up_tests` /
     `spare_worktree_tests` cases for #180 and #182 pass.
3. **New tab = new session in the workspace (engine).** The create-tab and start-in-background IPC calls, each tab
   on its own worktree.
   - Done when: a flow test (new `M/flow/workspace_tests.rs`) runs two tabs at once in one workspace on two
     worktrees, with no shared files.
4. **Board + facts.** `list_sessions` `workspace: true`, the overlap fact and the sibling-merged fact.
   - Done when: `workspace_tests` show an overlap fact once per new overlap, a merged fact once, and no write path.
5. **Queued-message routes.** `route` on `route_follow_up` (legacy `joins` still accepted) and the "Open as a new
   tab?" offer. Forwarding with Undo. One revision note.
   - Done when: `workspace_tests` cover join, wait, new_tab (the offer, no auto-open), tab (forward + Undo), and tab
     to a merged sibling routed with the merged fact attached. A rule-R flow test makes a session at the previous revision, resumes it after the
     change, and checks the note arrives once and a legacy `joins` call still routes. The render test covers the
     offer.
6. **States (#253) and the Ready record.** The state function in `board.rs`, shared by tab bar and sidebar. When a
   request settles with its tab's branch ahead of base, the engine records `RequestReady { request, head }` (a
   structured fact for phase 4, not read from the answer's text).
   - Done when: `board.rs` unit tests cover each row of the table, an untouched new tab with no state (not Merged),
     #253's stale-phase case, and one `RequestReady` per settled request with unmerged commits.
7. **UI behind `BRIGADIER_WORKSPACES=1`.** The strip's two groups, ⌘T, ⌘⏎, sidebar workspace rows with states,
   the rename to "Remove worktree…", and #199.
   - Done when: render tests in `D/fixtures/tabbar.tsx` and the sidebar fixtures, and `shortcutCatalog.test.ts`,
     pass, and the load stress run and the GPU-off startup comparison are clean.
8. **Flag removed.**
   - Done when: the phase verifier drives the dev app (own identity, `focus:false`) through new workspace → ⌘T →
     ⌘⏎ → queued message offer → sibling merge fact, with screenshots in its report.

### Checks
`projection` unit tests, `board.rs` unit tests, the git crate concurrency test, `workspace_tests` (new),
`clean_up_tests`, `spare_worktree_tests`, `peer_view_tests`, render tests for the tab bar and sidebar, and
`shortcutCatalog.test.ts`.

## Phase 3 · Auto-merge (Q62, Q68)

**Take, don't rebuild:** the agent uses `gh` itself (`gh pr create`, `gh pr checks`, `gh pr merge`), as Claude
Code and Codex do. Brigadier adds the setting, the bar as guidance in the instructions, the UI that shows what was
checked (merge message, proof, Revert), and code only where the thread can't do something it wants today: a fresh
verifier for its own work, an other-vendor review it can call on any change, the #177 router fix, the session
branch reconciled after a GitHub squash or rebase merge, and `ci_status` reading PR checks. The engine never
decides, holds or gates whether a merge may happen. The LLM stays free (user, 10-18): verification is expected in
the instructions, never enforced; the merge message always shows what was verified, so a lightly verified merge is
visible, not blocked.

### Scope
- **The setting.** "Merge on its own when the work is ready", in Settings › Git (`D/app/settings/GitPage.tsx`).
  Stored as `Settings.auto_merge: bool`, default `true` (`model.rs:817`). It is the one allowed knob (Q62).
- **When on, the tab does the whole job.**
  - With a GitHub remote and a signed-in `gh`, the PR path: push the branch, open a PR, babysit CI and reviews, fix,
    merge on GitHub, then fetch and fast-forward the local base (as w36's #244/#224 does).
  - **After a PR merge, the session branch is reconciled.** The engine records `Merged { requests, head,
    merge_commit, pr, auto }`: the session branch's head before the merge and the commit the merge made on base
    (for squash and rebase merges too, read from `gh pr view --json mergeCommit`). It then moves the session branch
    onto the new base, as the local merge path already advances it. The tab reads Merged, and its next request
    starts from base, not from stale commits.
  - Without them, the local path: merge into base via `finish_session` (`M/landing.rs:1437`).
  - The merge needs no user consent and shows no consent card when the setting is on. `finish_session` records the
    origin as `auto`.
- **When off.** The tab says the work is Ready, and today's consent rule applies (b2a16dd9). A peer's request is not
  the user's yes.
- **The thread can ask the other vendor for a review of any change, small ones included.** A new thread tool,
  `review_candidate { commit }`, asks the other vendor to review the final candidate. Today the thread's own changes
  are reviewed only past the size/risk line (`review_range_if_due`, `M/unreviewed.rs:501`) and `review_code` is a
  worker's tool. `review_candidate` reuses existing coverage: a range already reviewed at that commit isn't reviewed
  again, only what came after it (#138). If no other-vendor reviewer can take it, or reviews are off
  (`reviews_on()`), the call says so as a fact; the agent decides whether to merge or wait, and the merge message
  says the review was missing. No new setting.
- **The bar lives in the instructions (below), as guidance the agent judges.** The engine does not refuse, hold or
  gate merges. The merge result still states where checks, CI and the review stand, as w36's #241/#242 leaves it.
- **Revert.** A one-click Revert on the merge message.
- **Findings beyond the request** are listed in the merge message, never silently dismissed. When the repo is on
  GitHub, each is also filed as an issue (Q58).

### The Q68 bar, as the agent instructions it becomes
New revision note and thread guidance, active only when `auto_merge` is on. It says how to ship well; the agent
judges what a change needs. No must-lists, no thresholds, no loop:

> Ship it the way a careful engineer would, then merge. You judge how much a change needs. What usually makes a
> merge safe:
> - Tests and type checks run on the final commit, after your last change.
> - The other vendor's review of the final commit (`review_candidate`, which reuses a review that already covers
>   it), with each finding fixed or answered in one line. A running review is usually worth waiting for.
> - With a PR, green CI on it.
> - For a change the user can see: run the dev app in the background (`start_preview`), use the change the way the
>   user would, and attach a screenshot or short recording to the merge message.
> - For a change you judge big or risky: a fresh verifier (`start_verifier` with `fresh: true`) that tries to break
>   it against the original request.
> When something can't be had (CI broken on main, the other vendor out of quota), decide whether to merge or wait,
> and say what was missing in the merge message.
> The merge message shows what was verified: the PR or commit, the checks on the final commit, the review and how
> each finding was settled, CI, the proof, the verifier's verdict if one ran, and anything left out. "Beyond this
> request:" lists what you found outside it (filed as issues when the repo is on GitHub); never drop a finding
> without saying so.

### The fresh verifier's exact inputs
- **API change.** `start_verifier` today takes a `TaskRef` and verifies a reported implement task (`verify_task`,
  `M/phases.rs:806-815`; `start_verifier` `:867`). It becomes `start_verifier { task?, fresh?, commit? }`:
  - `fresh: true, commit: <sha>` with no task verifies the request and that candidate commit, so work the thread
    built itself (Q35) gets a fresh verifier too;
  - `fresh: true` with a task verifies that task's candidate commit the same way;
  - without `fresh`, it behaves as today.
- With `fresh: true`, its spec holds only four things:
  1. the original request, verbatim: the user's message that opened the request, plus the follow-ups that joined it;
  2. the standing constraints: the session's `@remember` constraints;
  3. the acceptance criteria: the request's "done when", or the outline's if there was one;
  4. the candidate commit: its sha and its diff against the merge base.
- It gets no builder outline, no lead report and no context pack (`M/context_pack.rs`).
- Its routing stays as it is: Frontier floor, high effort (`M/workers.rs:1145`).
- The verifier used for phases stays as it is.

### Folded issues
| # | Adds |
|---|---|
| #177 | The other-vendor review really goes to the other vendor: compute `ordered` (`crates/router/src/decide.rs:394`) after the author block in `assessed` (`:348`). |
| #137 | The review's findings come back in the check's own reply: one round trip less per babysit loop. |
| #138 | A re-check after fixes reviews the fix (its diff since the reviewed commit), not the whole change again. |
| #142 | The UI proof's background dev launch never takes focus, relaunch included. |

### Steps
1. **#177 router fix.**
   - Done when: a router unit test where the author's model is the only listed reviewer still routes to the other
     vendor.
2. **Fresh verifier (token doc item 2).** The `start_verifier { task?, fresh?, commit? }` change, one revision
   note (rule R).
   - Done when: flow tests in `phase_acceptance_tests` assert the fresh spec holds the request, the constraints,
     the criteria and the sha, and no outline, report or pack text, both for a worker's task and for a request the
     thread built itself with no task. The old `{ task }` call still works.
3. **`review_candidate` + #137 + #138.**
   - Done when: `review_tiers_tests` show a 10-line thread change reviewed through `review_candidate`, a candidate
     already covered not reviewed again, findings in the check reply, a re-check scoped to the fix range, and with
     no other-vendor reviewer the call saying so as a fact (the thread decides what to do).
4. **PR plumbing.**
   - `ci_status` (`M/ci.rs:1039`) reports a PR's checks when the branch has one.
   - The `pull_request.rs` doc no longer says Brigadier never creates one; the agent does, with `gh`.
   - The `Merged { requests, head, merge_commit, pr, auto }` record, local and PR, and the session-branch
     reconciliation after a PR merge.
   - Done when: the `M/ci.rs` tests cover PR checks, a `merge_by_user_tests` sibling for auto merges passes, and an
     `auto_merge_tests` case squash-merges a PR (stub `gh`, local bare remote), then sends a second request in the
     same tab: the tab reads Merged, and the second request starts from the new base with none of the old commits.
5. **Setting + bar (behind `BRIGADIER_AUTO_MERGE=1`).** The setting, the revision note, and `finish_session`
   accepting `auto`.
   - Done when: flow tests show an auto merge with no consent when on, and consent still needed when off. Prompt tests
     cover the bar text (guidance, no must-lists or thresholds), and the Git page render test covers the switch.
6. **#142 + UI proof attachments.** The merge message shows an attached screenshot or recording.
   - Done when: the render test shows the media in the merge message, and a `preview_tests` case shows a relaunch
     with no focus taken.
7. **Revert.**
   - The button on the merge message sends the tab's thread a fixed request, shown as the user's: "Revert merge
     ‹sha› (PR #n) on ‹base› and push it the way it went in". The thread does it (a revert can conflict with later
     merges; the LLM judges).
   - Done when: a render test covers the button, and a flow test shows the request sent once and the thread's revert
     landing.
8. **Flag removed.**
   - Done when: the phase verifier runs one small UI task end to end in the dev app against a local bare remote
     with a stub `gh` (PR, CI, review, proof, merge and Revert), plus one real tiny PR on
     syncra-studio/brigadier-ai itself for the GitHub path (the user's own repo, Q61). No new GitHub repo is made in
     the user's name.

### Checks
Router unit tests, `phase_acceptance_tests`, `review_tiers_tests`, `M/ci.rs` tests, `merge_by_user_tests`,
`held_merge_tests`, `preview_tests`, a new `auto_merge_tests.rs`, `prompts.rs` tests, the Git page and merge-message
render tests.

## Phase 4 · Bench (Q58)

**Take, don't rebuild:** read what Brigadier already records: the event log, `turn_usage`, `quota_samples` and git
history. The scripts live in `tools/ab/` (reusing `cacheshare.py`, `brig_tokens.py`). No new recorder, and no token
trimming (no prompt or tool-list cuts).

### Metrics (all read-only over a data dir and the repo)
| Metric | Exact definition | Source |
|---|---|---|
| First-look merge rate | One population for both merge modes: every request with a `RequestReady` record (phase 2 step 6) or named in a `Merged` record's `requests` (phase 3 step 4). A candidate counts as first-look when it is merged and the tab got no user message, card answer or Source-panel git action between its opening message and the merge, the merge go-ahead aside when auto-merge was off. Rate = first-look ÷ population. | event log: `RequestReady`, `Merged`, messages, cards |
| User touches per merged change | User messages, card answers and Source-panel git actions in that tab from the opening message (excluded) to the merge. | event log |
| Escaped bugs | Merged changes followed within 7 days, in the same project, by a revert of them, or by a commit touching any of the same files whose subject matches `\bfix` or `Fixes #`. | git log of base + `Merged` records |
| Wasted quota at reset | Per account window that reset: `100 − used_percent` at the last sample before the reset, weekly and 5-hour reported apart. | `quota_samples` (`crates/core/src/routing/store.rs:27`) |
| Tokens per merged change | Report only: the sum of `turn_usage` for the tab and its tasks from the opening to the merge, by model. | `turn_usage` (`store.rs:36`) |

### Token tripwire (token doc item 3)
- **The rule.** Per bench task: tokens > 1.25 × the same task at the last ship → the phase report needs one line
  saying what bought it. Verification ships; waste gets fixed. It never blocks.
- **Next to it, the report prints:**
  - **Cache-hit share:** `cached_input ÷ (input + cached_input + cache_write)`.
  - **Opening-call cache writes and misses by `cause`.** Each session's opening call, by `cause_write` and
    `cause_missed` (`crates/core/src/routing/meter.rs:203-209`). All other writes and misses are one
    **unattributed** line, plus `unestimated_calls`.
  - **Invalid tool calls:** a counter for `Invalid arguments for …` (`crates/mcp-server/src/catalog.rs:870`).
  - **The tier behind each `brain` and `upkeep` row.** A row above standard tier is flagged.

### Two A/Bs (ship the winner; no knob)
**Deferred (user, 10-18):** both A/Bs, and the V3 decision below, come back after the MVP launch with real usage.

1. **Pace sentence in worker prompts.**
   - Arm A: today's worker prompt. Arm B: one added sentence, "Go one step at a time and check each step before the
     next; getting it right matters more than speed."
   - Same bench tasks, ≥3 runs per arm.
   - B ships if first-look merge rate is equal or better and escaped bugs are not worse. Tokens are reported.
2. **Per-subagent effort for checkers and reviewers.**
   - Arm A: today's effort. Arm B: one step lower for checker and reviewer roles (never above `high`).
   - B ships if first-look merge rate and escaped bugs hold within noise. Otherwise A stays.

### Do V3 asks stay? (decides #34 #44 #45 #46)
- **The rule.** V3 asks stay only if a V3 failure since 2026-10-15 led to a fix commit that the four main metrics
  would not have caught. The lead checks with `git log --grep 'V3'` and `docs/evidence/`.
- **If they stay:** #34 (more baseline runs), #44 (narrow the scorer regex), #45 (hide the code from the scout, or
  score the thread only) and #46 (snapshot asks per set) are done.
- **If not:** V3 is retired and the four issues are closed with that reason.

### Folded issues
| # | Adds |
|---|---|
| #29 | Worker turns are marked as turn starts (`meter.rs` `opens_turn` `:243`, `M/thread_metrics.rs`), so per-change usage includes workers. |
| #139 | The meter-vs-transcript gap is explained or fixed before tokens feed the tripwire. |
| #135 | Brain-enrichment quota use per window, collected continuously; it feeds wasted quota (pairs with #258). |
| #190 | Bench readers apply the typed-record guard (after #189). |
| #34 #44 #45 #46 | Only if V3 stays (rule above). |

### Steps
1. **#29 + #139.**
   - Done when: a meter unit test marks worker turns, and #139 has a written cause plus a test, or a fix.
2. **Metrics script.** `tools/ab/bench_metrics.py`, run with `-I`, read-only.
   - Done when: it runs on a fixture data dir with known answers for all five rows, with tests in `tools/ab/checks`.
     The fixture mixes auto and manual merges, a Ready request never merged, and an auto merge with no Ready
     record, and the first-look rate counts each exactly once.
3. **Tripwire report.** Cache share, cause lines, the unattributed line, the invalid-call counter (a small
   `catalog.rs` counter written to `turn_usage` or a sibling table), and tier flags.
   - Done when: a fixture report matches its expected text, and the counter has a unit test.
4. **#135 + #190.**
   - Done when: brain usage per window shows in the report, and the bench readers pass their guard test.
5. **A/B 1, then A/B 2.** Deferred until after the MVP launch (user, 10-18).
   - Done when: each result is written to `docs/evidence/` and the winner has shipped (a prompt revision or a routing
     change).
6. **The V3 decision.** Deferred with the A/Bs (user, 10-18); #34 #44 #45 #46 stay open.
   - Done when: the rule is applied and the four issues are done or closed.

### Checks
The meter unit tests, the `tools/ab/checks` script tests, and the catalog counter test. Benches never gate the ship.

## Phase 5 · SSH host (Q69)

**Skipped for the MVP (user, Q70).** It comes later, with a mobile app and remote control.

**Take, don't rebuild:** the user's own `ssh` (their `~/.ssh/config`, agent and keys) and the CLIs installed and
signed in on the host. No SSH client of our own, no key handling, no file sync, no remote daemon.

### Smallest form
- **Settings › Hosts:**
  - **Add host:** an ssh alias or `user@host`, plus the repo's path there.
  - **Check:** runs `ssh <host> true`, `git --version` and `claude --version` / `codex --version` there, and says in
    plain words what's missing.
- **A new tab's composer** gets "Run on: This Mac / ‹host›". This applies only to new tabs; a running tab can't move.
- **On a host tab:**
  - The thread's CLI and its workers' CLIs run as `ssh -T <host> -- cd <worktree> && <cli args>`, over the same
    stdio protocol as today (`crates/providers/src/claude/mod.rs`, `codex/`).
  - Its worktree is made on the host with `git worktree add` over ssh.
- **Brigadier's tools reach the host** over an `ssh -R` forward to the daemon. Step 1 proves this before anything
  else.
  - The thread's MCP tools today come through `brigadierd mcp`, a stdio bridge to the daemon
    (`crates/mcp-server/src/lib.rs:1-4`). On a host, the CLI's MCP config points to a forwarded loopback port served
    by the daemon (MCP over streamable HTTP, with the grant as a header).
  - Fallback: a forwarded unix socket.
- **Forwarding the tools isn't enough: the daemon's own actions must run on the host too.** Most of what a tool
  call does today runs on this Mac, on local paths and processes. A host tab carries
  `Place::Host { host, root }` on its environment, and these actions go through one host executor (`ssh <host> --
  <argv>`, run in the tab's remote worktree):
  1. **Worktree creation and removal:** `Repo::add_worktree` (`crates/git/src/repo.rs:563`) and cleanup
     (`M/clean_up.rs`), the thread's scratch included.
  2. **Checks:** `run_check` and the session's full checks (`M/integrated_checks.rs`) run their commands on the host
     and stream the output back.
  3. **Worker landing:** merging a worker's branch into the session branch (`M/landing.rs`), the ref check
     (`M/refcheck.rs`), and the merge-base counts behind changed files.
  4. **Reviews:** the reviewer is a worker CLI started on the host like any other, and the range it reviews is read
     there.
  5. **Handoffs and rebirth:** the successor CLI starts on the host (`M/rebirth.rs`).
  6. **Stop:** the remote CLI's pid is recorded at spawn (the wrapper prints it before `exec`), and Stop ends it with
     `ssh <host> kill <pid>`, never by name. A dropped ssh connection also ends it.
  7. **Resume:** `Origin::Resume` starts the CLI on the host, where its own session files live.
  8. **File reads the daemon does:** changed files, diffs, `read_session`'s file links, and the context pack's reads,
     all with `git` or `cat` over ssh.
  9. **The provider adapters' local path handling:** Claude's `spec.cwd.canonicalize()`
     (`crates/providers/src/claude/mod.rs:1093`) and Codex's matching cwd checks are skipped for a host spec. The
     remote path is passed as it is.
- **Everything else is capability-gated.** A tool or action not on that list answers on a host tab with one plain
  fact, "not on a host tab: ‹what›", so the thread can tell the user. Covered: `start_preview` and the live look,
  computer use, Undo snapshots, the Brain's indexing, mockups and page tools that read local files. The app hides
  the matching buttons on a host tab.
- **Brigadier shows it here:** the thread, the worker cards, the states, and the changed files and diff read over ssh
  (`git diff`, read-only).

### Out (says so on a host tab)
Undo snapshots and Undo, file tabs and editing, the terminal tab, previews and the live look, computer use, the
Source panel's write actions, the Brain's indexing of the host repo, auto-merge's UI proof (a host tab says Ready
for a UI change instead), phone and cloud.

### Steps
1. **Spike, on a real host.** The test host is a Linux VM or another machine, whose repo path does not exist on
   this Mac, so nothing can silently fall back to local files.
   - Done when: Claude Code and Codex there both list Brigadier's tools over the forward, call one, and run one
     `run_check` whose command runs on the host. The notes are written to `docs/evidence/`.
2. **Hosts setting + Check.**
   - Done when: a unit test parses hosts, the Check runs against a fake `ssh` script, and its page has a render test.
3. **Host executor + the nine host-aware actions + the gate.**
   - Done when: `M/flow/ssh_host_tests.rs` uses a fake local `ssh` wrapper that maps the remote root to a temp dir
     with a different path, and nothing else. It drives a whole request: worktree, check, a worker's landing, a
     review, Stop, resume and cleanup, with every path the daemon touches under the mapped root. It also shows each
     gated tool answering "not on a host tab". The adapter test covers no local canonicalize for a host spec.
4. **Here-side view and Out notice.**
   - Done when: render tests pass, and the phase verifier runs on the real host from step 1: one request with a
     check, a worker landing, Stop and resume.

### Checks
The hosts unit tests, `ssh_host_tests` (fake wrapper, flow test only), the adapter tests, render tests, the spike
notes, and the verifier's real-host run.

## Phase 6 · If time remains: token doc item 4

**Take, don't rebuild:** today's routing already starts scouts and research cheap (`crates/router/src/decide.rs:164`,
`:188`) and learns from outcomes. We add one rerun.

- **What it does.** When a scout or research task's acceptance check fails, or the thread or a checker judges that
  its answer lacks support, the task reruns once, one tier up. The reason, the cheap model and the new model are
  logged in the routing explanation and in `outcomes`.
- **What never triggers it:** the evidence reader's provenance verdict, Q38's unverified memory lines, or a broken
  citation. A broken citation is fixed in the same worker's turn.
- **Limits:** never downward, never twice, and never for implement, review or merge tasks.
- **Step 1.**
  - Done when: router or flow tests cover a rerun on failure, no rerun on a citation fix, and a cap of one.
- **Step 2.**
  - Done when: the known-answer scout bench shows no fewer facts than today, and escalations per 100 chores are in
    the report.

## Landing order with the issues run

Live workers of `20261017-0213-issues-run`, and their files (from their worktrees, 2026-10-17 ~21:30):

| Worker | Issues | Files touched or to be touched | Collides with |
|---|---|---|---|
| w35 (`issues-w30`) | `accounts_tests` hand-over fix, #209 tail | `M/flow/held_merge_tests.rs`, `M/flow/accounts_tests.rs`, maybe `accounts.rs` / `runtime/accounts.rs` | P1 step 1 (same `select` and tests) |
| w36 (`issues-w31`) | #233 (committed, unpushed), #241–#244, #223, #224 | `M/request_changes.rs`, `M/mod.rs`, `crates/git/src/repo.rs`; next `M/landing.rs`, `M/integrated_checks.rs`, `M/prompts.rs` (merge text + a revision) | P1 steps 3–6 (`prompts.rs` revisions), P2 step 2 (`repo.rs` lock), P2 board (#233 count), P3 steps 4–5 (`landing.rs` `finish_session`) |
| w37 (`issues-w27`) | #210 (4 commits), #164 | `M/conversation.rs`, `M/inbox_store.rs` (new), `M/lifecycle.rs`, `M/review_runs.rs`, `model.rs`, `work.rs` | P1 step 5 (`conversation.rs` narration, follow-up), P2 steps 1, 2, 5 (`model.rs`, `lifecycle.rs`, `route_follow_up`), P3 step 3 (`review_runs.rs`) |
| w41 (`issues-w41`) | #255 (UI commit unpushed) | `D/app/conversation/FileReview.tsx`, `FileTabView.tsx`, `TurnDiff.tsx`, `D/app/shortcutCatalog.ts`, `D/state/board.ts`, `D/state/pendingChanges.ts` | P2 step 7 (tab strip, `shortcutCatalog.ts` ⌘T/⌘⏎, `board.ts` states) |
| (`issues-w42`, uncommitted) | not in the ledger's live list | `M/git_actions.rs` | P1 step 3 (a doc-comment edit only) |

**Proposed order:**
1. **w35's `accounts_tests` fix and w36's #240–#244 set land first.** The goal requires both before phase 1 starts.
   Phase 1 step 1 rebases onto w35's fix.
2. **Phase 1's `prompts.rs` steps (3–6)** start after w36's last `prompts.rs` push, and take the next free revision
   number at landing. Step 2 (the quota warning) touches no shared file and can land any time.
3. **w37's #210 lands before phase 1 step 5 and phase 2 steps 1, 2 and 5.** All of them touch `conversation.rs`,
   `lifecycle.rs` or `model.rs`.
4. **w41's #255 lands before phase 2 step 7** (tab strip, shortcuts). Phase 2 steps 1–6 are engine-only and may run
   meanwhile.
5. **Phase 1 step 5 (#248) lands before phase 2 step 5** (the `route_follow_up` extension).
6. **Phase 3 starts after w36's `landing.rs` work is on main.**
7. **The `issues-w42` change** is asked about by the Delegator. If it is #258 (`cheapest()`), it lands as it is;
   phase 1 only edits the doc comment.

Everyone rebases on origin/main right before each landing.

## Not building

- An inbox or settle view, auto-archive, or fading.
- Closing a tab on merge.
- Phone or cloud access.
- Any proxy for the user's sign-in.
- An auto-started "Later" list for spare quota (Q60).
- A workspace lead or a workspace-level orchestrator (Q65).
- Token trimming of any kind: prompt or tool-list cuts, `$var` arguments, schema flattening, per-call model
  switches, caps, compression or summarising.
- Ratio-to-plain bars and any token gate (Q58).

## Decided on your behalf

- **Weekly window ids:** Claude's `seven_day` or Codex's `secondary`; a model-scoped weekly window counts only for
  that model. These are the ids the providers already report.
- **Unknown weekly reset sorts last; ties go to the most headroom, then add order.** That way new accounts aren't
  favoured blindly.
- **The default account loses its first place when switching is on.** Q59 replaces default-first.
- **Q60 thresholds: ≤12 h to reset and ≥20% unused, once per window.** Late enough to be true, early enough to act;
  one warning, not a stream.
- **Overnight runs also drop pushing, publishing and deploying from their never-list.** Q61 applies everywhere; the
  other never-items stay.
- **The hop limit becomes a fact; the 20-a-minute pair rate stays.** Q66 says no guardrail; the pair rate only stops
  a runaway loop.
- **With auto-merge off, a peer's request is not merge consent.** Q67 makes a peer's ask equal to the agent's own
  decision, and the agent merges on its own only with auto-merge on.
- **Every new task tab, the first included, is a new worktree; Local checkout stays only for migrated sessions.**
  Two tabs or two workspaces in one checkout would collide (Delegator correction to the first draft).
- **A workspace stores its repos and base, set at creation; migration takes them from the session.** Tabs never
  depend on a surviving tab.
- **Merged needs a recorded merge of that tab's work; an untouched tab has no state.** A new branch's tip is always
  reachable from base, so reachability alone would lie.
- **Readiness is the engine's `RequestReady` record (a request settled with unmerged commits), not the answer's
  text.** It gives the bench one structured population for both merge modes.
- **`review_candidate` is a new thread tool; with no other-vendor reviewer or with reviews off, it says so as a
  fact.** The agent decides whether to merge or wait and says the review was missing; no new setting.
- **After a PR merge the session branch moves onto the new base, with the old head and merge commit recorded.**
  Squash and rebase merges then still read Merged, and the next request starts clean.
- **Phase 3's end-to-end runs on a local bare remote with a stub `gh`, plus one tiny real PR on
  syncra-studio/brigadier-ai.** No new GitHub repo in the user's name.
- **Rejected: landing each phase whole from a phase branch (Codex review #10).** The user's standing rule (10-16)
  is land small, land often: each finished step lands on main once its own tests pass, unfinished user-facing work
  sits behind a flag or stays unwired, and the phase verifier runs the full suite, live smoke and Codex review on
  the integrated main. Phase 1's prompt steps are each complete on their own; phase 3's bar is active only with
  auto-merge on, flagged until the phase ends. §0 stays as written.
- **Tab order is app state; membership is in the daemon.** Same as today's session tabs; no new sync.
- **Existing sessions become one-tab workspaces by projection, not by rewriting the log.** No risky migration, and
  it is reversible.
- **"Close workspace…" is renamed "Remove worktree…".** The word "workspace" now means the group.
- **⌘T opens a task tab; the old new-tab chooser moves to "+".** The goal names ⌘T; the other tab kinds keep their
  own shortcuts.
- **⌘⏎ is the background tab; the follow-up's other way moves to ⌥⏎ (⇧⌘⏎ / ⌥⌘⏎ when Send is ⌘⏎).** The goal names
  ⌘⏎.
- **A forwarded queued message leaves a one-line notice with Undo; forwarding, asking or keeping is the thread's
  call; forwarding to a merged sibling goes through with the merged fact attached.** Q66's pending detail, as the
  user corrected it on 10-18: the LLM judges, Brigadier provides the facts.
- **The board is served by `list_sessions` `workspace: true` plus pushed overlap and merged facts, not by a block in
  every turn.** It costs nothing until something changes.
- **State priority: Needs you > Working > Ready > Merged.** What needs the user shows first.
- **Tabs aren't listed in the sidebar; workspace rows carry their tabs' strongest state.** The tab bar already lists
  tabs.
- **⌘1–9 count across the strip.** One rule, no mode.
- **Auto-merge picks the PR path when a GitHub remote and a signed-in `gh` exist, otherwise a local merge.** It
  works without GitHub.
- **"Big or risky" is the agent's call; no line or worker counts.** The agent decides when a fresh verifier is
  worth it (user, 10-18).
- **When a bar item can't be had (CI broken on main, reviewer out of quota), the agent decides whether to merge or
  wait, and says what was missing in the merge message.** Verification is expected, shown, never enforced (user,
  10-18).
- **Findings beyond the request are filed as GitHub issues when the repo is on GitHub, and always listed.** This is
  Q58.
- **Revert sends the thread a fixed request instead of the engine reverting.** Reverts can conflict; the LLM judges.
- **The escaped-bug match is a revert, or `\bfix` / `Fixes #` on the same files within 7 days.** Mechanical and
  repeatable for a bench.
- **Cache-hit share = `cached_input ÷ (input + cached_input + cache_write)`.** It covers every prompt token.
- **The A/B pace sentence's wording is as written above.** The lead may tune the words, not the meaning.
- **The rule for keeping V3 is "it caught something since 10-15 that the main metrics would miss".** It decides
  #34 #44 #45 #46 from evidence.
- **SSH MCP reach: a forwarded loopback HTTP port, with a forwarded unix socket as fallback.** Nothing has to be
  installed on the host; the spike proves it first.
- **SSH host tabs leave Undo, previews, the terminal, computer use and the UI proof out.** That is the smallest form;
  the tab says so.
- **Host-aware daemon actions are exactly nine (worktrees, checks, landing, reviews, handoffs, Stop, resume, file
  reads, the adapters' path handling); everything else answers "not on a host tab".** Forwarding the tools alone
  would run them on this Mac.
- **The SSH spike and the verifier use a real host whose repo path doesn't exist locally; the fake `ssh` wrapper is
  only for the flow test.** A local fake would hide local fallbacks.
- **The dev flags are `BRIGADIER_WORKSPACES=1` and `BRIGADIER_AUTO_MERGE=1`.** A debug env var for unfinished work is
  allowed; it is removed when the phase ends.
