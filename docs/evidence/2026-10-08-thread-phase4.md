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
| Longest stretch with only "Thinking" in T1 ≤ 5 s | See "T1" below |

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

A worktree with uncommitted changes keeps the worktree and branch, and the `[finished]` text says
why (second flow test).

## Open in terminal

Built on the phase 2 spike (`2026-10-07-thread-open-in-terminal-spike.md`) with the six
corrections from the outline review: the takeover reservation is taken under a lock first; teardown
records its intent and awaits the terminal's end; the takeover's owner, pid and start time are
stored, and a restart recovers it; the terminal gets a fresh worker grant, revoked at exit; the
hand-back resumes the exact native session before any size hand-off; the terminal's cwd is the
task worktree (resume found the session there for both vendors).

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

**Tests.** `flow/takeover_tests.rs` (4: `a_worker_opens_in_a_terminal_and_reports_what_was_done_there`,
`racing_opens_resumes_and_landings_leave_one_writer`,
`stop_archive_and_delete_end_the_terminal_without_handing_back`,
`a_restart_ends_the_terminal_and_hands_back_or_finishes_the_stop`) and
`terminals::tests::a_worker_terminal_runs_its_command_and_ends_with_its_tree` in the daemon.
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

To be filled in after the run.
