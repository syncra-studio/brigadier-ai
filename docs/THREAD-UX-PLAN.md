# Thread UX plan: one calm thread for the lead and its workers

> Status: approved 2026-10-08 ("Do as you recommend": §8 records the rulings). Built on branch `thread-ux2` from
> `main` at `246761fd` (the thread build, merge in words and the trust dialog), in the order of §7.
> Evidence (§2) is from `fef7b14a`, all six phases of docs/THREAD-PLAN.md.
> Paths: `C/` = `apps/desktop/src/app/conversation/`, `E/` = `apps/desktop/src/components/assistant-ui/elements/`,
> `T/` = `apps/desktop/src/components/transcript/`. File:line references are to `246761fd`.

## 1. What the user asked for

After watching a session, the user said the thread's actions and rows "look scattered". They want the main thread and
each worker's thread to read as one calm, ordered story, and the lead to start, show, manage and collect its workers
cleanly.

This plan changes how the thread *shows* work, and the few prompt lines that decide how work is *split*. It doesn't
reopen THREAD-PLAN §2 (Q1–Q14). In particular it keeps:
- the voice rules and the `[quiet]` filter (Q1);
- delegate by default, with tiny edits by the thread itself (Q4);
- headless workers with "Open in terminal" (Q5);
- the live line at every busy moment and the "Worked for" timer (Q3).

## 2. Why it looks scattered today

Evidence: a finished T1 session in a `fef7b14a` dev build. The folded turn already looks right: "Worked for 8m 35s",
the answer, the action bar and the summary card. The trouble starts once the work is unfolded or a worker is opened.

1. **"Thought" rows split every group.** Unfolded, the turn reads: Thought / Searched project memory, searched code,
   ran commands / Thought / a commentary line / Sidebar hide mode started working / Sidebar hide mode finished /
   Thought / Read Diff of task-1 / Thought / Landed 1 commit on … / Thought / Finished the session.
   - Each thinking segment is its own row (`C/ThinkingRow.tsx:31-36`; it says just "Thought" when under 1 s, `:33`).
   - So each group holds one tool and can't summarise anything (`foldWork`, `C/RequestBlock.tsx:394-411`).
2. **At least five grey-row styles.** They differ in colour (/50, /60, /65, muted, /80), height (1.25rem and 1.75rem)
   and gap (1, 1.5, 2): `ACTIVITY_ROW` (`E/activity-row.tsx:21`), `STEP_ROW` (`C/OrchestratorSteps.tsx:102`),
   `TaskRow` (`C/TaskRow.tsx:35`) and the thinking rows. `isWorkerKind` flips a row from one style to another
   (`C/OrchestratorSteps.tsx:299-300`, `:351-352`), so rows next to each other don't line up.
3. **A worker has three ways to be named:** `WorkerName` (`C/OrchestratorSteps.tsx:120-139`), `WorkerLink`
   (`C/TaskRow.tsx:14-21`) and `WorkerChip` (`C/WorkerChip.tsx:93-153`).
4. **Live and folded group differently.** Live uses `WorkGroup` runs (`C/OrchestratorSteps.tsx:310-323`). The fold
   uses `foldWork` plus nested groups (`C/RequestBlock.tsx:394-441`). So the rows rearrange the moment the turn ends.
5. **Main-thread rows can't open.** Tool steps have no detail (`C/OrchestratorSteps.tsx:253`): no command output, no
   diff, no search results, and generic words ("Read a file"). A worker's thread *can* open its rows
   (`C/WorkerThread.tsx:113-219`), using a second word list (`T/activity.ts:188-249` against `C/toolWords.ts`) with
   different plurals ("searched files" against "searched").
6. **Internal tool names leak** as rows: "Read Diff of task-1", "Finished the session", "Read {worker}'s report".
7. **Worker progress lives in three places at once.** It shows as lifecycle rows in the thread, as up to three lines
   under the live status (`C/ThreadStatus.tsx:50-67`), and in the Workers tab and the summary card. "Waiting on you"
   shows in the block (`C/RequestBlock.tsx:366`), the summary (`C/PinnedSummary.tsx:288`) and the status head
   (`C/liveStatus.ts:52-54`).
8. **Worker lifecycle rows only half merge.** Adjacent "started" rows merge ("A, B and C started working",
   `C/blocks.ts:640-673`), but "finished", "stopped" and "is waiting" never do. Messages, answers and stops sent to
   workers get no row at all (`toolHasRow`, `C/blocks.ts:169`; `ON_TASK_ROW`, `:228-230`), so you can't see the lead
   manage its team.
9. **A worker's thread opens on a wall of cards.** The T1 worker opens at the bottom of about ten bordered artifact
   cards, one per screenshot, each with a Save button. Its answer sits above them, out of view.
10. **Chevrons only show on hover** for every row, the fold header and the groups, so nothing tells you what opens.
    Detail indents differ (`ps-6 pt-2 pb-1` against `ps-6 pt-1 pb-2`).
11. **Dead paths:**
    - the `TaskCardView` component with its live `WorkerTranscript` (`C/cards/TaskCardView.tsx:103-148`,
      `:530-546`) can't be reached: `blocks.ts` never makes the "task" card that `C/cards/CardBody.tsx:13` renders.
      The same file's `TaskDetails` (`C/Agents.tsx:6`) and `ArtifactFiles` (`C/WorkerThread.tsx:27`, the card wall
      of §2.9) are in use;
    - the created, messaged, answered, accepted and read-report step renderers never run in the main thread
      (`C/OrchestratorSteps.tsx:156-204`).

## 3. The design

### 3.1 A turn has fixed parts

A turn is built from these parts, top to bottom. Each part has exactly one home.

1. **The user's message**: the bubble, unchanged. Steers stay where they are today.
2. **The work header**: `Working for 52s` while live; `Worked for 1m 54s ›` once done.
   - Tabular numbers, tertiary colour, a 1 px rule under it.
   - The chevron is always shown when the turn is done.
   - It shows from the first event, without today's 2 s delay (`C/RequestBlock.tsx:148`, `:169`). The live line
     covers that first moment anyway.
3. **The activity**, in order. While live it is always open. Once the turn is done it folds under the header, closed
   by default. It holds only four kinds of row (§3.2–§3.4):
   - **commentary**: the lead's short lines to the user, as now;
   - **work groups**: the lead's own reading, searching, commands and edits;
   - **team sentences**: what happened to workers, and what the lead did to them;
   - **notices**: a compaction, an error, a declined action. Each is a single standalone row.
4. **The live line**: the last row while busy (§3.5).
5. **The answer**: the final reply, primary colour.
6. **Cards that need the user** stay outside the fold: questions, plans, approvals, "Waiting on you". Each shows
   once; the status head no longer repeats it (§3.5).
7. **The turn diff card**: one per turn, "Edited 3 files +61 −0 · Undo · Review" (`C/TurnDiff.tsx`, kept).
8. **The action bar**: as today (copy, rate, read aloud, fork, …).

Thinking is never a row of its own (§3.3). Lifecycle noise ("updated", "landed", "read report") is never a row.

### 3.2 Work groups: one row recipe, one vocabulary

**Grouping.** Consecutive work steps of the lead merge into one group. These count as work steps: read, search,
list, command, edit, web search or page read, code index, Brain query, checks, previews, and any other tool. A group
ends only at commentary, a team sentence, a notice or the answer. Thinking never ends a group.

**The collapsed group row** is `[icon] Read files, ran commands ›`. It is one sentence built from a fixed list, in
the order the kinds first appeared:

| Kind | Leading form | Following form |
| --- | --- | --- |
| reads | Read files / Read a file | read files |
| edits | Edited files / Edited a file | edited files |
| commands | Ran commands / Ran a command | ran commands |
| code search | Searched code | searched code |
| web | Searched the web | searched the web |
| Brain | Checked project memory | checked project memory |
| checks | Ran checks | ran checks |
| previews | Started a preview | started a preview |
| other | Used {tool name} | used {tool name} |

- Parts join as "A, B and C".
- The icon is the first kind's icon.
- A group of one step shows that step's own row instead, e.g. `Ran pnpm test in 41s`.
- The same table serves the main thread and the worker thread. It replaces the two `PLURALS` tables
  (`C/OrchestratorSteps.tsx:49-77`, `T/activity.ts:232`).

**While live**, the open group's row shows the current step, not the summary: `Reading blocks.ts`,
`Running pnpm test`, `Searching for "Worked for"`. The verb shimmers. When the next step starts, the row shows that
step. When the group closes, the row becomes the summary sentence. The rows never rearrange when the turn ends: the
fold just closes over the same groups (this fixes §2.4).

**Expanded**, a group lists one row per step, flush, in the same type:
- `Read RequestBlock.tsx`: the file name is a link that opens the file tab.
- `Ran git log --oneline -5 in 0.4s`, or `… — failed (exit 1)`.
- `Searched code for "sidebar"`, `Searched the web for …`, `Checked project memory for …`.
- `Edited Sidebar.tsx +12 −3`.

**A single step expands in place** to its detail. This is the same component the worker thread uses today
(`C/WorkerThread.tsx:113-219`), lifted into a shared module:
- a command opens a **Shell** box: `$ command`, the output (the head and tail with "… N lines …", plus a link to the
  full output), and a status of "✓ Success", "Exit 1" or "Stopped";
- an edit opens a small diff;
- a search lists its results; a web search shows the query and its sources;
- any other tool shows its input and output, as the worker thread does now.

**No internal names.** Steps on Brigadier's own plumbing are either hidden or worded for a person:

| Today | New |
| --- | --- |
| Read Diff of task-1 | hidden (the team sentence "Reviewed Sidebar hide mode's change" covers it, §3.4) |
| Read {worker}'s report | hidden |
| Landed 1 commit on brigadier/…/session | team sentence: "Landed Sidebar hide mode's change" with "1 commit" in its detail |
| Finished the session | hidden: the `Merged` step the user asked for in words says it ("Merged {branch} into {base}") |
| Called note_for_user / remember | hidden; their effect shows in "Waiting on you" or the Brain |

### 3.3 Thinking

- **Live**: thinking feeds the live line (§3.5). It shows "Thinking" with a shimmer, then the latest thought's first
  line once there is text. It is never a separate block in the activity.
- **Done**: thinking is not a row. It sits inside the group it came before, and shows only when that group is
  expanded, as `Thought for 4s ›`. That row opens to the text, up to 8.75rem tall with an edge fade. A segment under
  1 s with no text isn't shown at all.
- `C/ThinkingRow.tsx` keeps its non-compact form for the expanded group and the worker thread. The compact form in
  the main thread goes.

### 3.4 Team sentences: how workers appear in the thread

Workers show as **sentences**, never cards, in the same row recipe as everything else.

**Start.** `[g][g][g] Daemon storage, Thread rows and Thread phases started working`
- One glyph per worker, 16 px, 6 px apart. Every name is its own button that opens that worker (§3.6).
- With more than three workers: "A, B and 4 more". "4 more" opens the Workers list.
- Workers the lead starts in one batch always share one sentence, because the starts sit next to each other.

**Lifecycle.** Adjacent events of the same kind merge, just as starts do today:
- `Daemon storage and Thread rows finished`
- `Thread phases is waiting for an answer`
- `Fix uploads stopped`
- `Fix uploads failed`
- `Fix uploads finished with problems` (a report that names gaps)

**The lead's control actions** are rows now. Today they are dropped (§2.8). The board already has these steps
(`crates/core/src/work.rs:1294-1343`):

| Step | Sentence | Detail when expanded |
| --- | --- | --- |
| `Created` | (folded into "started working") | the brief, in a quiet box |
| `Messaged` | Messaged Thread rows | the message text |
| `Answered` | Answered Thread rows's question | question, answer and why |
| `stop_worker` | Stopped Fix uploads | the reason |
| `Accepted` / `Landed` | Landed Sidebar hide mode's change | commits and branch |
| a review result | Reviewed Sidebar hide mode's change | the findings count; opens the review |

**While a worker runs**, its start sentence doesn't change. Its live progress shows in the live line (§3.5), the
Workers strip (§3.5) and its own thread (§3.6). It no longer shows as extra rows in the main thread.

**One worker name component.** `WorkerName` (glyph + name button) is used everywhere: sentences, the live line, the
Workers strip and list, the summary card and @-mentions. `WorkerLink` and the in-thread use of `WorkerChip` go.

### 3.5 The live line and the Workers strip

**The live line** is the last row of a busy turn, and it is a single line. It shows the first of these that holds:
1. `Waiting for your answer`: only when the card that asks is out of view. Otherwise the card says it.
2. A live work step: the live group row already says it, so the line shows nothing extra.
3. `Thinking` with a shimmer, or the latest thought's first line.
4. `Waiting for 2 workers` (shimmer), when the lead is idle and workers run.
5. `Landing the changes`, or a quota wait.

This keeps Q3's "a live line at every busy moment".

**The per-worker lines move out of the thread** (`C/ThreadStatus.tsx:50-67`). They go into a **Workers strip** that
sits on top of the composer, attached like the project bar is now:
- **Collapsed**: `[g][g] 2 workers working · 1 done ⌃`.
- **Expanded**: one row per running or waiting worker:
  - `[g] Thread rows · Reading blocks.ts · 1m 2s · +12 −3`;
  - `[g] Fix uploads · is waiting for an answer`, with the question one click away.
  - A "Stop all" button on the right, with the tooltip "Stop every worker in this session".
- Clicking a row opens that worker's thread (§3.6).
- The strip shows only while some worker runs, waits, or has finished since the user's last message. It replaces the
  live-line worker list, so the same progress is never in two places in the thread column.

**The summary card** (`C/PinnedSummary.tsx`) keeps its Workers row as a count: `[g][g][g] 2 working · 1 done`
(`C/WorkerSummary.tsx:89-118`). Clicking it opens the Workers list.

### 3.6 The worker panel and a worker's own thread

**The list** (`C/Agents.tsx:67-103`):
- `Working · 2` and `Done · 5` sections; "No workers working" when empty.
- A working row is `[g] Name`, then a live preview under it (the current step, or "Working"), with the elapsed time
  on the right.
- A waiting row's preview says "Waiting for an answer".
- A done row shows its age ("4m ago"), and "failed" or "stopped" in the preview when that applies.

**The header** stays as is: `← [g] Sidebar hide mode ……… Opus 5.5 · High ⋯`.

**The worker's thread** uses the main thread's turn parts (§3.1) and rows (§3.2–§3.3). That means one component
tree, not a second renderer:
1. **The brief**, as the first bubble. It is folded to three lines, with "Show brief".
2. **The work header**: `Working for …` while live, `Worked for 3m 10s ›` once done.
3. **Its activity**: groups, commentary, thinking inside groups, and its messages with the lead as sentences
   ("Asked the lead: …", "The lead answered: …", "The lead sent a message").
4. **The report**, as the answer.
5. **Files**: the files it left in its outputs folder, as one compact list with icon, name, size and an open button.
   It shows the first three and "Show 7 more", instead of one bordered card per file (§2.9).
6. **Its action bar**: copy, rate, Open in terminal.

The view opens scrolled so that the report's top is visible, not at the very bottom. A live worker follows its live
line, like the main thread does.

### 3.7 Type and spacing (one recipe)

| Element | Size / line | Colour | Notes |
| --- | --- | --- | --- |
| Answer, commentary | 14 / 22.75 px | foreground | markdown, as now |
| Activity row, team sentence | 14 / 20 px | foreground/60; hover foreground | icon or glyph 16 px, gap 6 px |
| Live verb | 14 / 20 px | shimmer on foreground/60 | |
| Work header | 14 / 21 px | foreground/50 | tabular numbers; a 1 px rule 8 px below |
| Expanded step rows | 14 / 20 px | foreground/60 | flush; 4 px between steps |
| Detail boxes (Shell, diff, tool) | 13 / 19 px mono | foreground/80 | `bg-code-surface`, rounded-control, indented 22 px |
| Gap between activity items | 16 px | | one gap token for every activity item |
| Chevron | 14 px | foreground/40 | always visible when the row can open; rotates when open |

- One class constant, `ROW`, in `E/activity-row.tsx` replaces `ACTIVITY_ROW`, `STEP_ROW` and the `TaskRow` class.
- Compact density scales the 16 px gap token only.
- Column width stays 52rem.

### 3.8 Motion

- Groups and the fold open with a 160 ms height and opacity animation. This uses the existing collapsible.
- A live step row cross-fades its text when the step changes. No layout jump.
- Reduced motion turns both into instant changes.

## 4. How the lead runs its workers

These are prompt and tool changes in `prompts::thread` (`crates/core/src/manager/prompts.rs:50-140`) that make
§3.4 read well. They stay inside Q1, Q2 and Q4: delegate by default, voice rules, and `[quiet]` unchanged.

**When to fan out.** Split work into workers that can run at the same time when the parts are independent:
- separate questions to research or scout;
- writers on separate files;
- a check that needs no result from another part.

Start them **in one batch**: several `delegate_task` calls in one assistant message, so they show as one sentence.
Parts that depend on each other run one after another, through `plan_phases` as today. This is guidance in the
prompt, not a cap.

**How many.** Two to four in a batch is typical. More only when the parts are truly independent, such as one reader
per area. Each worker gets one job with its own "done when".

**Names.** Two or three plain words, unique in the chat ("Thread rows", "Fix uploads"). This is already the rule
(`prompts.rs:80`); the UI depends on it, since names are buttons.

**While they run.** The lead keeps doing its own small work: reads, searches, checks and tiny edits. That work shows
as its own groups between the team sentences. Its tools return at once (`prompts.rs:93`). Reports arrive as
messages.

**Messaging, answering, stopping.**
- `message_worker` and `answer_worker` now show as sentences (§3.4).
- `stop_worker` gets a required one-line `reason`. It shows when the stop row is expanded.
- The Workers strip's "Stop all" calls `stop_worker` for each running worker of the session, with the reason "Stopped
  by the user". The lead gets one note that names them.

**Collecting.** The lead judges each report as it arrives, as today. Nothing changes in the voice: it stays quiet
while work runs, and the user sees the team sentences and the live line instead of narration.

### 4.1 Speed

The user's live T1 demo ("Sidebar hide mode", a small one-file UI change) took 8m 35s, and they weren't happy with
it:

| Stage | Time |
| --- | --- |
| Submit → the lead's `delegate_task` | 47 s |
| The worker, start → report | 7m 20s |
| … of which after a clean review: more self-checking | 3m 41s |
| Report → landed → the answer | 31 s |

Four levers, all in the lead's and the workers' instructions plus one daemon message. None is a user setting (no
quality knobs):

- **(a) A worker stops once it is green.** It runs its checks while the review runs. Once its checks pass and the
  review is clean, or its findings are fixed and those checks rerun, it calls `submit_report` at once: no further
  verification, re-reading or screenshots. `LEAD_STEPS` (`crates/core/src/manager/prompts.rs:623`) says so, and the
  clean-review message (`crates/core/src/manager/review_runs.rs:906`, today "Report once your checks are done.") says
  "If your checks have passed, report now; don't verify again."
- **(b) The lead picks a lower effort for small, bounded work.** `delegate_task` already takes `effort`
  (`crates/core/src/tools.rs:129-131`). The thread's instructions tell the lead to pass `"medium"` for small,
  bounded work (one or two files, a UI tweak, copy, a bug in a known place) and to leave it out for anything larger
  or risky. It is the lead's choice per task.
- **(c) The thread delegates sooner.** For work it will delegate, the lead finds the pointers with at most a quick
  `query_brain` or `code_search` and then calls `delegate_task`; the worker reads the code. The brief's "code
  pointers" line asks only for what the lead already has.
- **(d) Fan out readily** on work that splits (§4 above).

**Measured** by re-running T1 (`tools/ab/tasks/t1.md`) on a dev daemon built from the finished branch, with the
`tools/ab` recorder and normal routing, and comparing the same four stages. One run, so the numbers carry its
variance; a miss is reported as it is.

**After the first measurement** (9m 45s, a miss: the worker ran at high effort because the lead passed none, and
it took the brief's screenshots only after its review), three fixes, then one more T1 run:

- **(b) made reliable.** `delegate_task`'s `effort` is required (`"low"`, `"medium"` or `"high"`), so the lead
  picks it for every task instead of falling back to the model's default. The instructions give the rule with
  examples: `"medium"` for small, bounded work (one or two files, a UI tweak or a small UI feature in one area,
  copy, a style fix, a bug in a known place), `"low"` for a mechanical edit, `"high"` only for cross-area, risky or
  unclear work. A daemon default from the brief's size was rejected: a guess from text length
  would misjudge short briefs for hard work.
- **No screenshots unless asked.** A brief's "done when" asks for screenshots or browser evidence only when the
  user asked for them.
- **Evidence before the review.** A lead worker does its slow done-when evidence (screenshots, manual runs) before
  `review_code`, so nothing is left once the review answers; after a clean review and green checks it reports at
  once (a).

## 5. Other thread actions

Brigadier already has several actions: fork from a message (`C/ForkMenu.tsx`), edit a message, @-mention workers
(`C/Mentions.tsx`), Undo and Reapply on the turn diff, read aloud, and copy.

This plan adds:
1. **Open any step**: commands to their output, edits to their diff, searches to their results, in the main thread
   too (§3.2).
2. **Stop all workers** from the Workers strip (§3.5).
3. **Open a worker by its name**, from every place it's named (§3.4).
4. **Show brief** in a worker's thread (§3.6).
5. **Long-thread fold**: past 30 turns, the oldest turns fold into "N earlier messages". Clicking it shows them.
   Rendering stays light.
6. **Jump to the live line**: the existing scroll-to-bottom button follows the live line while a turn runs.

## 6. Change list by file

**Contracts and the daemon**
- `crates/core/src/work.rs:1294-1343` `OrchestratorStepKind`:
  - add `Stopped { task_id, reason }` and `Reviewed { task_ids, findings }` (if the review result isn't already a
    step; check `review.updated` first);
  - add `ended_at_ms: Option<i64>` and `exit: Option<i32>` to `Tool`, so rows can say "in 41s" and "failed (exit 1)".
- A new IPC method, `getThreadItem(conversationId, itemId)` → `{ input, output, exit, ms }`:
  - it reads the matching provider `toolCall`/`command` entry from the conversation's orchestrator log
    (`crates/core/src/sessions.rs:1049`, which already holds full input and output);
  - it uses the blob store for output that was digested (`crates/core/src/digest.rs:48`);
  - wire it in `crates/daemon/src/server.rs` next to `listOrchestratorLog` (`:1017`), then run `gen-ts`.
- `crates/mcp-server/src/catalog.rs` `stop_worker`: add a required `reason`.
- `crates/core/src/manager/prompts.rs:80-93`: the fan-out, batch and "own small work while they run" lines (§4).

**The desktop: a shared activity module** (new, `C/activity/`)
- `group.ts`: grouping (§3.2) for both threads, with thinking attached to the next group. It replaces `foldWork`
  (`C/RequestBlock.tsx:394-441`), the `WorkGroup` runs (`C/OrchestratorSteps.tsx:310-339`) and the `ActionRun`
  grouping (`C/WorkerThread.tsx:222-239`).
- `words.ts`: the one vocabulary table and the live/done verbs. It merges `C/toolWords.ts`, `C/rowWords.ts` and
  `T/activity.ts:188-249`, and holds the internal-name rules.
- `ActivityRow.tsx`, `ActivityGroup.tsx`, `StepDetail.tsx` (Shell, diff, search, tool box), and `TeamSentence.tsx`
  (merged lifecycle and control sentences).
- Tests next to them: `group.test.ts` and `words.test.ts`. They absorb `C/rowWords.test.ts` and `C/toolWords.test.ts`.

**The desktop: changes to existing files**
- `C/blocks.ts`:
  - activity entries become `commentary | group | team | notice`;
  - thinking attaches to groups;
  - merge all adjacent lifecycle kinds (`:640-673`);
  - show the control steps (drop `ON_TASK_ROW` and the `toolHasRow` skips for message, answer, stop and land);
  - hide the plumbing steps (§3.2).
- `C/RequestBlock.tsx`: the turn parts of §3.1; one renderer for live and folded; the header without the 2 s delay;
  the chevron always visible when done; "Waiting on you" shown once.
- `C/ThinkingRow.tsx`: drop the compact main-thread form; the expanded group uses the regular form.
- `C/TaskRow.tsx`: becomes `TeamSentence`; `WorkerLink` goes.
- `C/OrchestratorSteps.tsx`: emptied into `C/activity/`; the dead renderers go (`:156-204`).
- `C/ThreadStatus.tsx`, `C/liveStatus.ts:86-120`: a single line in the order of §3.5; the worker lines move out.
- `C/WorkersStrip.tsx` (new): the composer-attached strip with Stop all. It is mounted with the composer, next to
  `ComposerCapsule` (`C/PaneComposer.tsx:164`).
- `C/WorkerThread.tsx`: renders through the shared turn parts and `C/activity/`. The brief is folded; files are a
  compact list; it opens at the report.
- `C/Agents.tsx`: Working and Done sections, a live preview, waiting and failed states.
- `C/WorkerSummary.tsx`, `C/PinnedSummary.tsx`: the Workers row as a count; "Waiting on you" stays only here and in
  the block.
- `C/WorkerChip.tsx`: kept for mentions and cards only. `WorkerName` moves to `C/activity/` and is used everywhere
  else.
- `E/activity-row.tsx`, `E/thread-activity.tsx`: the single `ROW` recipe (§3.7) and chevrons that are always shown.
- `C/cards/TaskCardView.tsx`: move `TaskDetails` to `C/TaskDetails.tsx`; replace `ArtifactFiles` with the compact
  file list of §3.6; then delete the `TaskCardView` component, its "task" case in `C/cards/CardBody.tsx:13`, and
  `C/WorkerTranscript.tsx`, which only it uses (§2.11). The `WorkerTranscript` store type in
  `apps/desktop/src/state/actions.ts:44` stays: `WorkerThread` loads through it.
- `apps/desktop/src/styles/tokens.css`: one activity gap token (16 px, compact 12 px) and the detail-box type.

## 7. Phases

The phases land one after another on `thread-ux2`. Each gets the usual checks (`tools/full-checks.sh`,
`cargo fmt`, `clippy`, `gen-ts`, `pnpm` checks) and one Codex review. Screenshots are taken from a dev build under
its own identity and data dir, never the installed app.

**Phase S: the speed levers (§4.1).** Instructions and the clean-review message only, so it goes first; it is
measured at the end, on the finished branch.
- Done when:
  - tests pin the new `LEAD_STEPS` and clean-review wording, and that a `delegate_task` `effort` reaches the
    worker's route;
  - the T1 re-run's stage times are in the report next to the 8m 35s breakdown of §4.1.

**Phase A: one row recipe and one vocabulary (desktop only).**
- `C/activity/` with `words.ts`, `group.ts`, `ActivityRow` and `ActivityGroup`. The main thread and the worker thread
  both use them.
- Thinking moves into groups.
- The plumbing names are hidden.
- Done when:
  - unfolding T1's turn shows at most one row per kind run, with no bare "Thought" rows (fixture test in
    `group.test.ts`, built from the T1 board events);
  - the main thread and the worker thread render the same step with the same words (test);
  - a screenshot of the same session, before and after.

**Phase B: rows that open.**
- `getThreadItem`, `StepDetail`, `Tool.ended_at_ms` and `Tool.exit`.
- Done when:
  - in a live dev session, a command row opens to its output and status;
  - an edit row opens to its diff;
  - a failing command shows "failed (exit N)".

**Phase C: team sentences and the lead's control actions.**
- Merged lifecycle sentences, the control rows, `WorkerName` everywhere, and `stop_worker` with a reason.
- The prompt lines of §4.
- Done when:
  - a request that splits into three independent parts starts three workers in one batch, shown as one sentence;
  - finishes that happen next to each other merge;
  - a message and a stop each show as a row;
  - checked on a live dev session and recorded in the report.

**Phase D: the live line, the Workers strip and the worker panel.**
- The single live line, `WorkersStrip` with Stop all, the `Agents.tsx` list, and `WorkerThread` on the shared turn
  parts with a folded brief and a compact file list.
- Done when:
  - no stretch over 5 s with only "Thinking" (the Q3 check, rerun on T2);
  - Stop all stops every running worker, and the lead gets one note;
  - a worker opens at its report.

**Phase E: polish and deletions.**
- Type and spacing tokens, motion, the long-thread fold, and deleting the `TaskCardView` component and
  `C/WorkerTranscript.tsx`.
- Done when:
  - matching screenshots of the main thread live, folded and unfolded, and of a worker live and done, are in the
    report;
  - `rg` finds no `ACTIVITY_ROW`, `STEP_ROW` or second `PLURALS` table.

Phase S comes first. Phases A and B are sequential. C and D can run in parallel after B, because they touch different files (C:
`blocks.ts`, `TeamSentence`, prompts and catalog; D: `ThreadStatus`, `liveStatus`, `WorkersStrip`, `Agents` and
`WorkerThread`). They are integrated one after the other. E comes last.

## 8. Decisions (settled 2026-10-08)

The user's ruling: "Do as you recommend".

1. **No one-line plan before delegating.** The Q1 voice rules and the `[quiet]` filter stay as they are: the team
   sentences and the live line already say what runs. Revisit after phase C only if the thread still feels opaque.
2. **Yes to the Workers strip on top of the composer.** It replaces the worker lines under the live status, so the
   same progress never shows twice in the thread column, and Stop all has a home.
3. **Yes to deleting the unreachable `TaskCardView` component and `C/WorkerTranscript.tsx`**, in phase E, after
   `TaskDetails` moves out of that file.
4. **The speed levers of §4.1 are in scope** (from the user's live demo), built as phase S and measured on T1.
