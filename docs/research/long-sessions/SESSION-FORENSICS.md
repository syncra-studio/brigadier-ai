# Session forensics: the Providers-redesign session, worktree removal, and the fixed thread start (2026-10-15)

Read-only forensics of one real session in the installed app. It covers three things:
- what went wrong in that session;
- whether a thread deletes its own worktree;
- what the thread's fixed start is made of, against the benchmark gap.

Sources (all read-only):
- `brigadier.db`, opened with `sqlite3 -readonly`;
- a /tmp copy of `routing.sqlite`, since it won't open read-only in place;
- the daemon logs;
- the session's worktrees, read with `git --no-optional-locks`;
- the user's two screenshots (run dir `msgs/shots/`).

Code references are to `d669bca0`; the `launch` branch is `50695633`. Paths: `M/` = `crates/core/src/manager/`,
`D/` = `apps/desktop/src/`. Times are local (UTC+3); daemon log lines are UTC. `seq N` is an event row in
`brigadier.db`.

## 0. The build the user ran

- No commit is recorded in the app: `Info.plist` says only 0.1.0, and neither the about page nor the logs name a commit.
- The binary dates from 2026-10-14 19:16. That is after `38e92c53` (18:47) and before `60f916f5`/`d669bca0` (19:47).
- `brigadierd` contains "Brigadier sends it back to", text that comes from `585d07f1`.
- The commits after `38e92c53` touch only tests, fixtures and smoke scripts.
- So the installed product code is `d669bca0`. None of the `launch` changes are in it.

## 1. The session

- **Conversation:** `01a13c27-242d-729b-99a1-1e32ce9ecf18`, in project brigadier-ai.
- **Worktree:** `session-ce9ecf18`, on branch `brigadier/ce9ecf18/session` at `d669bca0`.
- **Thread:** Claude (Opus 5.5), Full access.
- **Workers:**
  - **task-1, "Mock up Providers redesign"** (`01a13c27-2438-…-469d3ee008e3`): a mockup task on Codex gpt-6-astra, effort high.
  - **task-2, "Write designer brief prompt"** (`01a13c3e-865e-…-c4d50b1c47da`): a read-only scout on Codex gpt-6-luna, effort high.
  - **Pre-warm** (`01a13c42-fb83-…-88257a56b89b`): worktree `task-next-7a56b89b`, which is the detached sibling.

### 1.1 Timeline

Thread turns and their usage come from `routing.sqlite turn_usage`. Events come from the `conversation:`, `orch:`, `task:` and `cleanup` streams.

| Time | What happened | Evidence |
|---|---|---|
| 23:42:40 | User: "See the Providers settings page… redesign it fully, propose me some variants… open to move the routing to its own page under Agents" | conversation stream |
| 23:42:43–23:43:20 (T1, 37 s) | The thread reads the code and asks on a card: which structure (4 variants as text), Usage on or off the page, mockups or not | T1 usage: cache read 268,239, write 52,208, output 3,567. First call's context 31,414 (orch seq 161136) |
| 23:46:36 | User answers: Providers + Routing; keep Usage separate; "Mock up my pick" (the recommended options) | waited 3 m 16 s on the card |
| 23:46:37–23:47:00 (T2) | `remember`, then `delegate_task` for the mockup (task-1) | The router's reason was Claude Code's weekly window "projected at 459% by its reset", while the weekly window showed 23% used (not investigated further; §2.7) |
| 23:46:59–23:53:10 | task-1 builds fixtures, commits `d0ccfc22` and takes 12 screenshots | the task's `outputs` (mockup-1..12.png, stored as blobs) |
| 23:53:12–23:53:25 (T3) | The thread reads 2 of the PNGs and asks whether to build it and whether to rename "In use" | |
| 00:07:42 | User: "No, I'll need you to give me a prompt I can give to an LLM designer…"; keep "In use" | waited 14 m 17 s on the card. **The user's own choice, not a bug** |
| 00:07:44–00:08:13 (T4) | `stop_worker` task-1 ("Not landing…"), `remember`, then `delegate_task` for the scout (task-2) | |
| 00:07:51–00:07:55 | task-1's cleanup removes worktree `task-next-3ee008e3` and its scratch | `cleanup` stream. Branch `brigadier/ce9ecf18/task-1-mock-up-providers-redesign` is kept and still exists |
| 00:08:13–00:11:16 | task-2 writes `scratch/01a13c3e…/outputs/designer-prompt.md` (36,516 bytes), then calls `submit_report` at 00:11:16 | Codex `sessionStarted` cwd = the scratch dir; worktree `task-2-0b1c47da`, read-only |
| 00:11:18 | The worker's closing text (469 bytes) is kept as an addendum. **The ledger disposes task-2 and removes its scratch, `outputs/` included** | `cleanup.removed`, seq 162269: **2 s after the report** |
| 00:11:17–00:11:34 (T5) | The thread reads the report's stored artifact (blob `2094e4c7…`, 2 reads, 6.2k tokens). At 00:11:26 and 00:11:28 it runs `find` for designer-prompt.md and finds nothing. Its reasoning: "not on disk since the scratch was emptied, and I don't have a way to write outside the workspace" (wrong: the session has Full access). Its answer: "save designer-prompt.md from the … task card", plus "To check: … Codex version 2.1.0 … Fix it before you send the prompt if it matters." | orch stream |
| 00:11:35–00:11:36 (T6, 2.4 s) | The addendum arrives ("Created the prompt at [designer-prompt.md](…/scratch/01a13c3e…/outputs/designer-prompt.md)"). The thread writes "Update: the prompt is also saved at [designer-prompt.md](scratch path). That folder is temporary scratch space, so copy the file somewhere else soon." | **A dead link, 8 s after it checked the file was gone** |
| 00:13:05 | User: "where the fuck is my prompt, i cant open the fucking file!" | A pre-warm `task-next-7a56b89b` is created for this message too |
| T7 (29.6 s) | The thread runs `grep -rl` over Application Support and finds the blob. It runs `cp` to `~/Desktop/designer-prompt.md`, then `pbcopy`. It repeats the "fix it before you send" note | orch stream |
| 00:14:03 | User: "fix it yourself lol" | |
| T8 (7.4 s) | The thread runs `codex --version` and `claude --version`, `sed -i` on lines 137 and 140 of the Desktop file, then `pbcopy`, and says it's fixed | The sed output confirms the fix |
| 00:18:13 | User quits the app | Log: "quit requested by a client". `task-next-7a56b89b` is never dropped. Relaunches at 00:21:52 and 00:31:19 end only processes. The worktree is still on disk |

**Totals:**
- **Thread:** 8 model calls; cache read 1,813,759; cache write 82,505; output 12,851; context at most 82.6k; about $1.28 at API prices.
- **task-1 (Codex):** 25 calls; input 126,690; cached 2.10M; output 15.7k.
- **task-2 (Codex):** 24 calls; input 145,948; cached 1.82M; output 17.6k.

### 1.2 Why it showed "Worked for 28m 56s"

- `request.updated` keeps one `worked` interval `{from 23:42:40, to null}` through both `waiting` states (seq 161257, 161781).
- So the 17 m 33 s spent waiting on the user's two cards (3:16 + 14:17) counts as work.
- **Real work was about 11 m 23 s:**
  - thread turns about 2.5 min;
  - task-1 (the mockup, which the user then dropped) 6 m 11 s;
  - task-2 (the brief) 3 m 03 s.
- **This is the designed behaviour.** `docs/THREAD-PARITY-PLAN.md:197-202` decided that "a wait on an open question card keeps the span open… the final 'Worked for …' is the wall time". It has a test at `M/requests.rs:702-707` (`waiting_on_its_question_card_is_work_so_the_time_runs_on`).
- The merge card already stops the clock (THREAD-PARITY-PLAN §8.4); question cards don't.
- So this is not a code bug. It is a settled UX choice that misled the user here: they read 29 minutes of work for an 11-minute job. Changing it reopens that decision (PLAN §6).

## 2. Bugs and unintuitive moments

Each item below gives:
- what happened;
- the evidence;
- the root cause in the code;
- whether `launch` fixes it;
- the fix.

The switch from mockups to a designer brief is **not** on the list. The user chose it because they didn't like the mockup.

### U1. The deliverable was deleted 2 s after the report, and the user got a dead link (High)

- **What happened:**
  - task-2 wrote the brief into its scratch `outputs/` folder.
  - The ledger removed that folder at 00:11:18 (seq 162269), 2 s after `submit_report`.
  - The worker's closing text, the thread's "Update:" line, and the UI's file chip all point at that deleted path.
  - The user clicked it at 00:13 and got nothing.
- **Root cause:**
  - A read-only task is disposed as soon as its post-report turn ends: `M/workers.rs:3017-3025` → `finish_read_task` → `dispose_task` (`M/workers.rs:4638`; disposal `:4677-4679`).
  - The scratch dir, `outputs/` included, goes through the ledger (`M/ledger.rs:910-927`).
  - Outputs are stored as blobs first: `report_files` at `M/outputs.rs:201`, called from `M/workers.rs:3553` (comment at `:3542`); `final_outputs` at `M/outputs.rs:248`.
  - The task card can open and save them: `D/app/conversation/TaskDetails.tsx:210-212` and `D/app/conversation/WorkerThread.tsx:218`, calling `save_artifact`/`artifact_copy` (`M/outputs.rs:668`, `:681`).
  - So nothing was lost, and the **intended place for a deliverable is the task card**. The thread's first sentence ("save it from the task card") was right.
  - What breaks is the links:
    - Markdown links to scratch paths are never rewritten.
    - Every non-http link becomes a file chip (`D/components/assistant-ui/markdown-text.tsx:127-158`).
    - A chip outside the checkout calls `revealPath` (`apps/desktop/src-tauri/src/main.rs:379`), which hits a deleted path: a dead link and an error toast.
  - The worker prompt invites those links. It calls outputs "files meant for … the user" (`M/prompts.rs:120`), and says "Brigadier attaches them to your report and the user saves them from the task card" (`:1262`). The worker then linked its scratch path in its closing text anyway.
- **launch:**
  - It adds "Outputs (the user saves them from the task card; read_artifact reads them)" to **long** report envelopes. Main already has the same line at `M/prompts.rs:1600`.
  - It does nothing about links.
- **Fix:**
  1. Brigadier rewrites any link into a task's scratch or `outputs/` to that task's stored output, which opens from the blob as the task card does. This applies in worker text, addenda and thread messages.
  2. The thread's rule says: "a worker's outputs are opened and saved from its task card; never give a scratch path".
  3. Optionally, a "Save…" chip for each stored output under the final answer.

### U2. The thread repeated a link it had just found dead (High: a false claim)

- **What happened:**
  - At 00:11:28 `find` returned nothing, and the thread's reasoning said the scratch was emptied.
  - At 00:11:36, prompted by the addendum, it told the user the file "is also saved at" that path.
- **Root cause:**
  - The late-event rule `LATE_EVENT` (`M/prompts.rs:190`) makes any addendum that "changes what the user must know" worth an "Update:" line.
  - Nothing in the addendum or the rules tells the thread the scratch is gone, or that outputs live on the card.
  - The thread trusted the worker's words over its own check from 8 s before.
- **launch `3755744e`:**
  - "A worker's words after its report ride with the report while the thread hasn't read it".
  - Here the thread had already read the report, so the addendum would still have come as a separate event. So launch fixes this only partly.
  - The bench report (`runs/20261014-1232-oss-readme/workers/w10/report.md`, issue 1) notes the same limit.
- **Fix:**
  - U1's link rewriting removes the dead link at its source.
  - The addendum envelope also says when the task's scratch has been removed.
  - The honesty rule gains one line: "never point the user at a file you haven't just seen exist".

### U3. A known defect was handed to the user instead of fixed (High: the user had to repeat the request)

- **What happened:**
  - The final answer said "To check: … Codex version 2.1.0 … Fix it before you send the prompt if it matters". It said the same again at T7.
  - The user had to write "fix it yourself lol". The thread then fixed it in 7.4 s.
- **Root cause:**
  - The final-answer rule (`M/prompts.rs:129`) allows "one line 'To check: …' for what nobody could check". The thread used it for something it could both check and fix itself.
  - It also wrongly believed it couldn't write outside the workspace (reasoning at 00:11:34), although the session has Full access.
  - And the scout that wrote the file was read-only and already disposed, so "send it back" wasn't possible.
  - This defect was not a review finding passed through. No review ran on a read-only scout. The thread found it itself while reading the artifact.
- **launch:** no change.
- **Fix:**
  - Change the rule's text: "To check is only for what nobody here can check or fix. A known defect in a deliverable you fix yourself, or send back, before you answer".
  - Give deliverables a home the thread can write to (U4), so "I can't edit it" never holds.

### U4. Recovery to ~/Desktop and the clipboard, without asking (Medium)

- **What happened:**
  - At T7 and T8 the thread copied the brief to `~/Desktop/designer-prompt.md` and ran `pbcopy` twice. The user hadn't asked for either.
- **Permission model:**
  - Full access allows it, and the user had asked where the file was. So this was not a permission breach.
  - It is not ledger litter either. The cleanup ledger covers only Brigadier's own artifacts (`docs/PLAN.md:38`, `:133`), and a file the user asked for isn't litter.
- **The problem:**
  - No rule says where a deliverable belongs, so the thread invented a location.
  - It also overwrote the user's clipboard twice.
- **Fix:**
  - One rule: deliverables are opened and saved from the task card. When the thread hands over a file itself, it uses the card's Save, or asks where to put it.
  - No silent writes to the Desktop or the clipboard.

### U5. "2 done" in the workers panel (Low/Medium)

- **task-1 (the mockup):**
  - Stopped at 00:07:44 and never landed. The user rejected it, which was their call.
  - Its commit `d0ccfc22` survives on branch `brigadier/ce9ecf18/task-1-mock-up-providers-redesign`.
  - Its 12 screenshots are stored as outputs. The thread looked at 2 of them.
  - It shows "done", but its state went reported → stopped.
- **task-2 (the brief):**
  - Its work was used.
  - The file now exists only as blob `blobs/20/2094e4c7…` and the Desktop copy.
- **Fix:** show "stopped" (or "not used") for a task stopped without landing, and keep "done" for work that landed or was used.

### U6. The pre-warm worktree leaks when the app quits (Low/Medium)

- **What happened:**
  - `task-next-7a56b89b` was created at 00:13:05 for a message that only asked a question.
  - The app quit at 00:18:13, before the 10-minute unused timer. The worktree is still on disk after two relaunches.
- **Root cause:**
  - A pre-warm is made on every user message (`M/conversation.rs:1121`; `M/prewarm.rs:124-133`).
  - Its expiry is an in-memory timer. Its owner is never marked as being disposed, so the launch sweep `ledger.sweep` skips it (`M/ledger.rs:604`).
  - It surfaces only as "Orphaned" in Settings › Storage (`crates/core/src/disk/mod.rs:497`, `:1665ff`).
- **Fix:**
  - Drop pre-warms on quit.
  - At launch, sweep `task-next-*` owners that have no task.
  - And PLAN B6: pre-warm only when a writing task is likely.

### U7. The 29-minute timer (Medium; settled by design)

See §1.2. Card waits count as work by decision (THREAD-PARITY-PLAN). Proposed:
- either stop the clock on question cards, as the merge card already does;
- or show both: "Worked 11 m · waited for you 18 m".

Either one reopens that decision, so it goes to PLAN §6.

### U8. Minor: `~` is joined onto the workspace in the thread's read log (Low)

- **What happened:**
  - `thread.looked` recorded `…/session-ce9ecf18/~/Desktop/designer-prompt.md`, with `outside: false` (seq 162376, 162405).
  - So a Desktop file was logged as a workspace file at a path that doesn't exist.
- **Root cause:**
  - `absolute()` in `M/reads.rs:358-363` joins any non-absolute path onto the command's directory, and a leading `~` isn't absolute.
  - The read log feeds context packs (WEAK-SPOTS S4).
- **Fix:** expand a leading `~/` to the home directory before joining.

### U9. The merge card ends the workspace without saying so

See §3. This is what the user means by "the thread deletes its worktree".

### Observation, not a bug: the route to Codex

- The router sent the mockup to Codex because "Claude Code's weekly window is projected at 459% by its reset". The weekly usage shown was 23%.
- The projection code wasn't checked in this pass.
- It's worth one look, because a wrong projection silently moves work to the other vendor.

## 3. Does the thread delete its own worktree? (U9)

### 3.1 This session

- **No session worktree was removed in this session.**
- What disappeared was:
  - task-2's scratch, with the deliverable (U1);
  - task-1's worktree when it was stopped. Its branch was kept.

### 3.2 The earlier session that matches the user's description

Session `01a1366e-0c0c-7514-9936-13ab33fbb16e` ("See the activity from the tabbar…", worktree `session-33fbb16e`):

- **10-13 21:17:43:** the first merge card came right after the first task. The user answered "Not yet".
- **10-14 09:27:** the user asked "good, are we ready to merge into main?". Over the next 1.5 hours they wrote "what is going on?", "done?" twice and "how much more?".
- **10-14 11:03:54:** the second merge card.
- **11:20:25:** the user clicked "Merge into main". The engine injected "[answer] … Call finish_session now".
- **`finish_session`:** merged 25 commits into main at `a182860a`.
- **11:20:40:** cleanup owner `session:01a1366e…` removed the worktree (seq 160929–160932), and the session's setup path became null.

So the removal followed the user's own click, and it is the **designed** behaviour:
- `docs/THREAD-PLAN.md:200`, Q9: "after merge, the session worktree and branch are removed; the next message starts fresh".
- `:202-203`: `ensure_target` recreates the worktree at the same path, from the base tip.

The code path:
1. `finish_session` (`M/landing.rs:1439`) → `:1781-1788`.
2. `release_merged_session` (`M/lifecycle.rs:687`). It is skipped if the worktree is dirty or locked (`:704-725`).
3. `forget_session_worktree` (`:751`/`:791`) deletes the merged branch, and the setup path is set to None (`:809-827`).
4. The tool result tells the thread what happened (`M/landing.rs:1789`, `M/lifecycle.rs:739-741`).

### 3.3 Why it feels like self-deletion

- **The merge card never says the workspace goes.**
  - It reads "Merge `{branch}` into `{base}`?" (`M/landing.rs:2780-2790`), with options at `M/cards.rs:884`/`:888` and "Not yet" at `M/landing.rs:1407`.
  - It doesn't say that the worktree and branch are removed and that the next message starts from a fresh one.
  - So "Merge" reads like "approve", but it ends the workspace.
- **The thread offers the merge whenever it thinks it's done.**
  - The rule is "When the work has landed, ask them once, on a card, with propose_merge" (`M/prompts.rs:293`; also `:112`).
  - In 01a1366e the first card came after a single task. The thread had decided the request was done, which the user didn't think.
  - Then the user asked about merging three times while the work went on, so merging and finishing became one blurred step.
- **What the user wanted:** to merge into main and keep going in the same workspace. The design instead gives a fresh worktree from the new main. Nothing is lost (the commits are on main), but uncommitted scratch files, open previews and the "place" of the work are gone.

### 3.4 Every code path that removes a session or task worktree

| | Path | When | What is kept |
|---|---|---|---|
| A | `dispose_task` `M/workers.rs:4638` | Landing, a read task done, stop, failure, recovery, `wind_down` | Outputs as blobs. A WIP commit for stopped or failed tasks (`:4644`). The branch is dropped only when Done (`:4683`) |
| B | `release_merged_session` `M/lifecycle.rs:687` | `finish_session` after a merge | Nothing: designed (Q9). Skipped if dirty or locked |
| C | `wind_down` `M/lifecycle.rs:607` | Archive, delete, project removal, uninstall | `keep_session_changes` `:834` runs first |
| D | `ensure_target` retry `M/workers.rs:2502-2517` | A broken worktree being recreated | |
| E | `release_hold` `M/review_hold.rs:178` | Review hold released | |
| F | Pre-warm `drop_slot` `M/prewarm.rs:277`, `adopt_prewarm` `:246` | Replaced, unused for 10 min, hibernate (`M/lifecycle.rs:464`), closed (`:611`), merged (`:727`) | Not run at quit (U6) |
| G | The `clean_up` tool `M/clean_up.rs:19` | The thread asks | It touches a `session:` owner only if disposal already started (`:61-64`). The prompt forbids manual removal (`M/prompts.rs:122`) |
| H | Launch `ledger.sweep` `M/ledger.rs:604` | At start | Only for owners already being disposed |
| I | Settings › Storage `crates/core/src/disk/mod.rs:564`, `:753`, `:629`; empty project folder `:1279` | The user clicks | |
| J | Overnight run worktrees `M/overnight/workspace.rs:136` | Run end | |
| K | Test data and thread temp at start (`M/lifecycle.rs:262`, `M/thread_scratch.rs:156`) | At start | |

- The remover itself is `M/lifecycle.rs:46-81`: `git worktree remove --force`, then prune (`crates/git/src/repo.rs:503`).
- The ledger's `remove()` is at `M/ledger.rs:774`.
- Other removals checked in the logs:
  - 10-13 20:42: archives the user started (`session:` owners).
  - 10-14 14:02 UTC: housekeeping "Empty work folder of a project", which removes only an empty `worktrees/<project>` directory.
- Hibernate and quit never remove a session worktree.
- **The only thread tool that can remove one is `finish_session`, and only after the user's consent. There is no evidence of a removal without a user click.**

One stale doc: `docs/PLAN.md:109` still says there is no merge card. That's outside this research; it's noted here only.

### 3.5 Fix (U9)

1. **The merge card says what it does:** "Merges into `main` and closes this workspace; your next message starts fresh from `main`."
2. **Add a second option, "Merge and keep working".**
   - Option (a): it keeps the worktree and resets the session branch to the new base tip.
   - Option (b): it keeps the worktree on its branch, fast-forwarded to the merge.
   - Either way, closing becomes a separate, explicit step.
   - This changes THREAD-PLAN Q9, so it needs the user's decision (PLAN §6).
3. **Offer the merge only once the whole request's work has landed, or when the user brings up merging.** Never after each task. This changes the wording of `M/prompts.rs:293` and `:112`.

## 4. The fixed thread start, against the benchmark gap

### 4.1 What the bench saw

- On T1 (a one-line bug fix), `launch` used **429k** Claude tokens; plain Claude Code used **131k**.
- The thread made about 10 tool calls in one turn, with 382k cache reads (bench report `runs/20261014-1232-oss-readme/workers/w10/report.md:72`).
- **The "~40k prompt" is that average: 382k ÷ ~10 calls.**
  - So it is the fixed start plus the turn's growing history, not the fixed start alone.
- `launch` adds about 1.6 kB to the thread's instructions (the small-change rule, `include_request`, the review lines; `git diff d669bca0 50695633 -- M/prompts.rs`). That's about 0.4k tokens, plus about 0.4 kB on `delegate_task`'s schema.
- So the fixed start on `launch` is about the 31.4k measured at `d669bca0`, not 40k.

### 4.2 What the fixed start is made of (measured at `d669bca0`)

The first thread call's context in this session was **31,414 tokens** (orch seq 161136), before any tool result. It splits like this:

| Part | Size | How measured | Plain Claude Code pays it? |
|---|---|---|---|
| Brigadier's MCP tool catalog for the thread (45 tools; 46 with `run`) | **46,717 bytes of JSON, ≈13k tokens** (range 11.7k–15.6k at 4.0–3.0 bytes/token). Descriptions alone are 19,356 bytes | A throwaway test on a `git archive` copy of `d669bca0` (run dir `workers/w07/catalog-sizes.txt`). Not tokenized exactly | No |
| Brigadier's instructions (`--append-system-prompt`) | **19,969 bytes, ≈5.0k tokens** | The injection log, seq 161118: `tokensEstimate` 4,993. Built by `M/thread.rs:300-335` from `prompts::thread` (`M/prompts.rs:65-160`), plus the reviews-off, git-writing and browser sections, plus repo instructions (brigadier-ai has none) | No |
| Claude Code's own system prompt and the thread's 8 built-in tools (`THREAD_TOOLS`, `crates/providers/src/claude/mod.rs:76`: Read, Grep, Glob, Bash, Edit, Write, WebSearch, WebFetch) | **≈13k** (range ≈10.5–15k) | The remainder: 31.4k − 5.0k − 13k − 0.25k | Yes, more in fact: plain Claude Code ships more built-in tools |
| The user message and the first output | ≈0.25k | | Yes |

- So **Brigadier's own share is about 18k tokens on every thread call**: the catalog about 13k, the instructions about 5k.
- The tool catalog alone is about 70% of that.
- There is no briefing and no Brain block at a session's first call. Brain context reaches the thread only through tool results, and a rebirth briefing only after a rebirth.
- WEAK-SPOTS S2's earlier 24–25k was measured before computer use and the newer tools arrived.

Catalog bytes by group:

| Group | Bytes |
|---|---|
| Delegation and workers | 15,010 |
| Planning and approval | 11,385 |
| Memory and search | 5,721 |
| Peers | 3,791 |
| Previews | 3,255 |
| Overnight | 2,296 |
| Session end and merge | 2,187 |
| Checks | 1,657 |
| CI | 1,463 |
| Pages | 1,373 |

The largest tools, in bytes:
- `delegate_task` 5,976
- `propose_plan` 2,312
- `record_findings` 2,282
- `start_preview` 2,166
- `ask_user` 1,781
- `remember` 1,731
- `run_check` 1,624
- `propose_overnight` 1,563
- `finish_session` 1,369
- `message_session` 1,339
- `note_for_user` 1,333
- `settle_step` 1,311

### 4.3 What could be cut or loaded later

The settled rules still apply:
- no settings and no knobs;
- the tool set is fixed at session start, for the cache;
- rollout to open sessions (R).

The savings below are per call. On bench T1, multiply by about 10 calls.

| Rank | Change | Saving | Notes |
|---|---|---|---|
| 1 | **Fewer thread calls for the same work** (B1, A14, launch's self-edit). The gap is roughly (Brigadier's ≈18k) × calls, so every call saved saves the whole fixed start | ≈31k + history per call | Already the direction of `launch` (`cab67d97`) |
| 2 | **Load rarely used tool groups lazily, behind Claude Code's tool search:** overnight, CI, previews, pages, peers, the planning and approval tools beyond `ask_user`/`note_for_user`, `suggest_full_access`, `record_findings`. That is about 20–25 kB | **≈6–7k tokens per call; ≈60–70k on bench T1** (≈15% of 429k) | Reverses `533bac83`, which took tool search out of the thread so nothing waits behind a search: **the user's call** (PLAN §6). Needs a probe: deferred schemas keep the cached prefix stable, and the extra search hop is rare |
| 3 | **Trim `delegate_task`'s schema** (6 kB). Long doc comments describe rare fields (operate `target`/`end_state`, `mockup`/`matches_mockup`, `phase`/`role`, `image_generation`, `quality`). Keep one line per field and move the detail into the brief rules that use it | ≈1k per call | No rule changes |
| 4 | **Instruction sections only where they apply:** peers, previews, mockups, overnight. Send them as `[settings]`-style notes at the moment they apply, as the browser and reviews sections already are | ≈1–2k per call | Each section must still arrive before its first use, so this needs a test per section |
| 5 | **Shorter tool descriptions in general** (19 kB of descriptions; several repeat rules from the instructions) | ≈1–2k per call | Low risk; test V3 for quality |

- Cache reads are priced at about 0.1× of input on the API.
- How a subscription weighs them is unknown (PLAN P0.1). So the 3× token gap may cost far less than 3× in quota.
- Rank 1 also saves time; ranks 2–5 mostly save tokens.

## 5. The user's five points, answered

1. **Why 29 minutes?**
   - 17.5 minutes of it was the user's own time on two question cards, which the timer counts as work by design (§1.2, U7).
   - About 6 minutes went on the mockup, which the user then chose to drop, and 3 minutes on the brief.
   - Thread turns took about 2.5 minutes in all.
2. **The deliverable in scratch:**
   - The ledger cleared the scratch 2 s after the scout's report: read-task disposal (U1).
   - The file was stored first, and the task card can save it.
   - But the worker and the thread both linked the scratch path, and the UI rendered it as a live chip.
   - It is a different mechanism from the session worktree removal in §3, though both are "disposal at done".
3. **The Desktop and clipboard recovery:**
   - Full access allows it, and it isn't ledger litter.
   - But no rule says where a deliverable goes, and the clipboard was overwritten without asking (U4).
   - The intended place is the task card's Open and Save.
4. **A known error passed to the user:**
   - It came from the final answer's "To check" rule (`M/prompts.rs:129`), plus the thread's wrong belief that it couldn't write outside its workspace (U3).
   - It was not a passed-through review finding: no review ran on the scout.
5. **"2 done":**
   - task-1's mockup was stopped and thrown away (the user's call). Its commit and screenshots are kept.
   - task-2's brief was used.
   - "done" on the stopped task is misleading (U5).
