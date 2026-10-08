# Thread phase 4: lifecycle, terminal takeover, live line (2026-10-08)

THREAD-PLAN.md §3 phase 4. Branch `thread-p4` from `thread-build` `763cb90e`. Claude Code
2.1.293, codex-cli 0.160.1. Every live check ran on a dev `brigadierd` built from the branch, with
a scratch `BRIGADIER_DATA_DIR` (`/tmp/w21/live/data`, scratch repo `/tmp/w21/live/repo`, Full
access), driven over IPC with `tools/ab/bipc.py`. Computer Use was not used (its `health_report`
failed: "daemon failed to start … not listening"); every check was scripted, and the terminal side
ran through the daemon's own PTY (`openWorkerTerminal`, `writeTerminal`).

## Done-when

| Done when | Result |
|---|---|
| After merge, the session worktree and branch are gone; the next message gets a fresh branch from the base tip, and the thread resumes (not reborn) | **Pass**, in flow tests and live on a real Claude thread (below) |
| "Open in terminal" round trip for a Claude worker and a Codex worker: earlier turns visible, an edit made there shows up in the headless report, no two writers | **Pass** for both vendors (below) |
| Longest stretch with only "Thinking" in T1 ≤ 5 s | **Pass**: 3 s (0:02–0:05), 4 s in all; no stretch over 30 s (`tools/ab/replay/run.mjs` on the recorded run, through the app code at this branch) |

## Merge cleanup (Q9)

**Flow tests** (fake CLI): `thread_tests::a_merge_removes_the_session_worktree_and_the_next_message_starts_fresh`
(mutation-checked) and `a_merge_keeps_a_session_worktree_with_uncommitted_changes`.

**Live**, script `merge.py` (Claude Sonnet thread, low effort). The thread was asked to commit
`MERGE.md` and call `finish_session`; the script allowed the merge card over IPC (`answerCard`).

| Step | Seen |
|---|---|
| Before the merge | session worktree `…/session-7fd5e9de` on `brigadier/7fd5e9de/session`; thread CLI pid 17647, `--session-id a208c123-e7c0-4ac2-87e0-bcb2ac5b47e3` |
| After the merge | `main` = `1f64299 Add merge note`; the worktree folder is gone, `git worktree list` doesn't list it, and `git branch --list brigadier/7fd5e9de/session` is empty |
| A commit lands on `main` (`12a66aa1 Later on main`), then the next message | the worktree is back at the same path, on `brigadier/7fd5e9de/session`, at `12a66aa1` (= main's tip) |
| The thread | still pid 17647 with the same session id: no restart, no rebirth. Its Bash in the new worktree answered `Later on main` / `later` |

A worktree with uncommitted changes, or one the user locked (`git worktree lock`), keeps the
worktree and branch, and the `[finished]` text says why (second flow test, both cases).

## Open in terminal

Built on the phase 2 spike (`2026-10-07-thread-open-in-terminal-spike.md`) with the six
corrections from the outline review: the takeover reservation is taken under a lock first; teardown
records its intent and awaits the terminal's end; the takeover's owner, pid and start time are
stored, and a restart recovers it; the terminal gets a fresh worker grant, revoked at exit; the
hand-back resumes the exact native session before any size hand-off; the terminal's cwd is the
task worktree for every worker that writes (resume found the session there for both vendors).

One exception to the worktree cwd: a **read-only Codex worker** (a scout, a review). Codex can't
start in a folder it may not write, so such a worker runs from its scratch folder, and its
terminal opens there too, where its sandbox was set up. The fallback in correction 6 puts the
worktree first in `--add-dir`, but `codex resume --help` (0.160.1) says `--add-dir` means
"Additional directories that should be writable alongside the primary workspace", which would
lift the worker's read-only restriction. So the worktree isn't added; the tab names it instead
("This worker only reads the code. It starts in its own folder; the code is in …"), from the
`checkout` field of `openWorkerTerminal`'s answer
(`takeover_tests::a_read_only_codex_terminal_starts_in_its_folder_and_names_the_checkout`).

Driver `drive.py <vendor>`: a session whose thread delegates one implement task to that vendor
(spec: create `notes.txt` with `from headless`, commit `Add notes`, remember codeword
`PELICAN-42`). When the task reports, the driver opens it in a terminal, answers the CLI's folder
trust prompt, asks for the codeword, an edit (`from terminal`, commit `Terminal edit`) and one
Brigadier MCP call, then quits the CLI and waits for the worker's next report.

| | Claude worker | Codex worker |
|---|---|---|
| Task / native session | `01a11918-d1ce…` / `03eb2628-aad0-4a96-af4a-d9c8150acbbc` | `01a1191d-8397…` / thread `01a1191d-8ec4-7d62-a4d9-03cd8d94cf86` |
| Terminal cwd | the task worktree `task-next-75dbebae` | the task worktree `task-next-4eda99f8` |
| Earlier turns visible | yes: the brief, the Bash commit `e0c5e4a Add notes`, its Brigadier call | yes: the earlier turn |
| Codeword from the headless turn | `PELICAN-42 CONFIRMED` | `PELICAN-42CONFIRMED` (spaces lost in the stripped PTY text) |
| Edit made in the terminal | commit `df28323 Terminal edit` | commit `9db77e4 Terminal edit` |
| Authenticated Brigadier MCP call (the terminal's grant) | `project_map` answered | `project_map` answered |
| After the CLI quit | the same native session resumed headless; report: "Then the user continued in the terminal … appended 'from terminal' … committed it as df28323" | the same thread resumed headless; report: "Terminal continuation completed: … committed as 9db77e4 …" |
| Worktree log after | `Terminal edit` / `Add notes` / `Start` | `Terminal edit` / `Add notes` / `Start` |
| One writer (ps while open and during the turn) | only pid 72443, the PTY `claude --resume`, carries `03eb2628`; the other `claude -p` processes are the threads' | only pid 81045, `codex resume`, carries the thread id; 0 `codex app-server` processes under the daemon |

**`run_check` from the terminal.** In the first Codex run the worker's `run_check` said "the
task's worker isn't running". Fixed in `f2f9ff8e` (the terminal's checks run with the terminal's
access, owned by the task, cancelled when the terminal ends). Re-run on a daemon built at
`f2f9ff8e` (`drive.py codex`, plus "call `run_check` once"): task `01a11924-b9e3…`, thread
`01a11924-c53d…`. `run_check` answered "1 changed file since your base. Check everything:
notes.txt belongs to no package." The rest of the round trip passed again: commit
`eb04673 Terminal edit`, same thread id after the quit, one writer (0 app-servers, 1 process with
the thread id).

**Tests.** `flow/takeover_tests.rs` (6 after the review fixes below; first 4: `a_worker_opens_in_a_terminal_and_reports_what_was_done_there`,
`racing_opens_resumes_and_landings_leave_one_writer`,
`stop_archive_and_delete_end_the_terminal_without_handing_back`,
`a_restart_ends_the_terminal_and_hands_back_or_finishes_the_stop`) and
`terminals::tests::a_worker_terminal_runs_its_command_and_ends_with_its_tree` and
`closing_a_worker_tab_ends_its_tree` in the daemon.
Mutation checks: removing the launch refusal, replacing Resume with `revive_worker`, and removing
the teardown fence each made a test fail.

**Findings for later.**
- Both CLIs ask whether to trust the folder the first time. Claude's dialog defaults to "No, exit":
  the first attempt's typed prompt plus Enter quit Claude at once (exit 1). That did prove the
  hand-back path: the worker resumed headless and reported. Codex preselects "1. Trust and
  continue"; Enter accepts. The terminal tab shows a hint. A better fix: trust Brigadier's
  worktree root once.
- Typing `/quit` and Enter in one write is swallowed by Codex's command popup; Enter has to come
  separately.

## T1

One run (arm `/tmp/brig-ab-1007/t1-p4`, cloned and warmed with `clone.sh` and `warm.sh`), daemon
built at `f2f9ff8e`, Full access, the thread on Opus 5.5 high as in phases 2 and 3. Commands:
`send.py`, `reqdone.sh`, `times.py brigadier`, `armtokens.sh <arm> 1791424301416`,
`check.sh <arm> tasks/t1.md 79b82f27`, `ARM=<arm> APP=apps/desktop node tools/ab/replay/run.mjs`.
Conditions at the start: Claude 5-hour window 48% used, Codex 5%; load 2.6; no thermal warning.
The behaviour probes P1.1–P1.4 were not run (Computer Use), as in phases 1 to 3.

| | Phase 3 run 2 | Phase 4 |
|---|---|---|
| Landed (s) | 403.9 | **412.0** (+8.1) |
| Final answer (s) | 419.9 | 422.7 (+2.8) |
| Settled (s) | 419.9 | 422.7 |
| Landed tip; frozen checks | `e63a1a98`, pass | `79b82f27`, pass (install, typecheck, lint, test: all exit 0) |
| Longest only "Thinking" (replay) | 5 s | **3 s** |
| Tokens raw (`turn_usage`) | 2,550,273 | **2,903,060** (1.14×) |
| Without cache reads | 178,735 | 182,300 |
| Claude thread, raw / without cache reads | 433,818 / 34,247 | 356,434 / 45,838 |
| Claude worker | 1,938,023 / 99,944 | 2,380,424 / 105,684 |
| Codex review (1) | 178,432 / 44,544 | 166,202 / 30,778 |
| `turn_usage` against the transcripts | +0 | +0 |

**Where the time went.**

| Stage | Phase 3 run 2 | Phase 4 | Change |
|---|---|---|---|
| Thread writes the brief and delegates | 42.5 s | 29.2 s | −13.3 s |
| `delegate_task` → worker running | 0.7 s | 0.7 s | 0 |
| Worker running → reported | 352.5 s | 375.2 s | +22.7 s |
| of which waiting on its own code review at the end | 39.3 s | 36.4 s (review 321.3–362.4 s) | −2.9 s |
| Reported → landed | 8.6 s | 7.3 s | −1.3 s |

**Where the tokens went** (Claude transcripts, per model call):

| | Calls | First call's context | Average context | Output |
|---|---|---|---|---|
| Worker, phase 3 run 2 | 31 | 26,147 | 61,661 | 26,540 |
| Worker, phase 4 | 35 | 25,234 | 67,318 | 24,300 |
| Thread, phase 3 run 2 | 12 | 22,571 | 35,811 | 4,082 |
| Thread, phase 4 | 11 | 22,412 | 32,099 | 3,344 |

**Engine or worker behaviour.** Phase 4 doesn't change what a worker or the thread is given:
the worker's first call is 0.9k tokens smaller than in phase 3 run 2, and the first event is the
same 0.7 s. The +8.1 s and +0.35M raw are the worker's own choices: 4 more calls (about +0.25M at
phase 3's average context) and larger reads (+5.7k average context per call, about +0.20M). The
thread was faster and used fewer raw tokens (−13.3 s, −77k raw, though +11.6k without cache reads). Phase 3's two runs on nearly the same engine were
183 s and 2.3M tokens apart, so a difference of this size between single runs is inside
worker-behaviour variance and says nothing about phase 4's code.

## Codex review

`dlg review code --base 763cb90e` on `697c6ee4`: 2 P1 and 3 P2, all valid, fixed in `33b4c30c`
with a test each (each mutation-checked: removing the fix makes its test fail or hang).

- **P1:** a hand-back whose resume failed disposed of the task while still holding the takeover's
  reservation, which the disposal takes again: a deadlock. The failure now comes after the
  reservation is released (`a_hand_back_that_cannot_resume_fails_the_task`).
- **P1:** an error or limit hand-off decided for the headless session just before the open could
  overwrite `TakenOver` and end the terminal. The open now waits for a hand-off under way and
  supersedes the headless session; hand-offs and reroutes leave a task open in a terminal alone;
  a task waiting for its usage limit can't be opened
  (`a_hand_off_decided_before_the_open_leaves_the_terminal_alone`).
- **P2:** closing a worker's tab ended only its CLI; now its whole tree
  (`closing_a_worker_tab_ends_its_tree`, with a child that ignores the hang-up).
- **P2:** messages held for the terminal were lost on a restart; they are kept with the task too
  (the restart test holds one).
- **P2:** a merge whose worktree removal failed still forgot the worktree, so the next message
  failed to make it again. The concrete case, a locked worktree, now keeps it like a dirty one.
  Other removal failures still forget it; the next launch's sweep retries the removal.

### Second review (verifier)

`dlg review code --base 763cb90e` on `73573db6`: 1 P1 and 3 P2, all valid, fixed in `ecf14d7c`
and `7f397a97` with a test each. Each fix was mutation-checked: with the fix removed, its test
fails.

- **P1:** the hand-back after a terminal waited for its overnight run's worker slot while holding
  the takeover reservation and the conversation's guard, so a Stop or an archive hung behind
  unrelated work. Now the task is marked handed back under both, then launched holding neither. A
  Stop during the wait ends it, and nothing starts when the slot frees
  (`a_stop_while_the_hand_back_waits_for_a_worker_slot_ends_it`; with the old order the Stop
  times out).
- **P2:** messages held while a terminal failed to open were dropped. They now reach the worker:
  with its resume, or as a message to a paused or reported worker
  (`messages_held_while_a_terminal_fails_to_open_reach_the_worker`).
- **P2:** the headless CLI's pending permission card stayed open after the takeover. Clicking it
  failed with "the worker has ended", and the card held the request. It now expires at the open
  (`a_takeover_expires_the_workers_pending_permission_card`).
- **P2:** a merge whose worktree removal failed for another reason (not a lock) forgot the
  worktree, and the next message failed until the next launch, whose sweep would also have
  removed a worktree made in between. The suggested fix, keeping the path, would leave that sweep
  hazard. Instead, the next message finishes the removal and the merged branch first, then starts
  fresh from the base's tip at the same path. The thread is told the removal is still to come
  (`a_merge_whose_worktree_removal_failed_retries_it_at_the_next_message`, with a folder it may
  not empty).

The verifier also found that command output that isn't UTF-8 skipped the redactor, while the
thread was shown it lossily, secret included (`printf '\377'` after `cat .env`). Fixed in
`b1b454ba` (`a_runs_output_hides_the_projects_secrets` now runs that command too;
mutation-checked).

**Live re-check by the verifier** (dev daemon at `ecf14d7c`, scratch data dir and repo, Claude
Sonnet thread). Merge: after the user allowed the card, `main` = `df83284 Add merge note`, the
session worktree folder and `brigadier/cb07d90d/session` were gone, and the recorded path was
cleared. After `e1ad3db Later on main` and the next message, the worktree was back at the same
path on the same branch at `e1ad3db`. The thread's CLI was still pid 76106 with
`--session-id 34144ab1…`, and its answer quoted `e1ad3db Later on main` / `later`. Open in
terminal, Claude worker (task told not to land): `openWorkerTerminal` showed the task
`takenOver` with cwd = the task worktree. After the trust prompt, it answered
`PELICAN-42 CONFIRMED` and committed `7567c7d Terminal edit`. `project_map` was answered (an
empty map: the scratch repo has no index), and only pid 5645, the PTY `claude`, carried the
native id `e7f37317…`. After `/exit` (code 0), the same native session reported: "In the
terminal, the user asked three things … appended the line 'from terminal' … committed it as
'Terminal edit' …". Codex was not re-run live; the evidence above stands for it.

`tools/full-checks.sh` on `33b4c30c`: exit 0 (Rust 476 passed, 1 ignored; app 148 passed). A first
run failed with "No space left on device" while the disk was full, not on a check; the re-run on
the same tree passed.
