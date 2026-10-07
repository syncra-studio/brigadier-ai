# Thread phase 2: the thread gets tools, verified (2026-10-07)

THREAD-PLAN.md §3 phase 2. Branch `thread-p2` from `thread-build` `3fbe9e90`. Claude Code 2.1.293,
codex-cli 0.160.1. Every live check ran on a dev `brigadierd` built from the branch, with a scratch
`BRIGADIER_DATA_DIR` under `/tmp`, driven over IPC with `tools/ab` (`startd.sh`, `setup.py`,
`bipc.py`). Computer Use was down (`health_report`: "the tag-scoped cmux Computer Use runtime is not
listening"), so UI checks were scripted.

The CLI contracts of step 1 and the trimming numbers are in
[2026-10-07-thread-phase2-contracts.md](2026-10-07-thread-phase2-contracts.md). The "Open in
terminal" spike is in [2026-10-07-thread-open-in-terminal-spike.md](2026-10-07-thread-open-in-terminal-spike.md).

## Done-when

| Done when | Result |
|---|---|
| T1 lands verified in ≤ 7 min | **Pass on time**: landed at 328.9 s in one run (target 420 s); the frozen checks pass on the landed tip. The behaviour probes P1.1–P1.4 are still pending, because Computer Use is down (as in phase 1). See T1 below |
| 0 "I can't" over 5 scripted asks; "run the app" starts a preview | **Pass**: 0 of 5; the first ask started `preview-1` |
| Read/Bash/Edit under all three levels, both vendors | **Pass after a fix, with one limit**: 5 of 6 rows passed on the first run; Codex at Approve for me could not leave its sandbox, was fixed and re-checked live. Claude at Approve for me runs everything sandboxed, but its own auto mode reviewer denies every sandbox bypass |
| Preview survives hibernation, rebirth, fallback; gone after stop, archive, merge | **Pass**: it survived hibernation, a forced rebirth and a forced Claude → Codex fallback with the same pid, and was gone after stop_preview, the user's Stop, the chip's Stop, archive, quit and merge |
| Trimming: a 50 KB passing Claude Bash reaches the model as ≤ 4,096 B | **Pass**: 4,077 B digest (exit status, the 6 warning lines, head and tail, `read_artifact` id) |
| `read_artifact` returns the full 50 KB byte-identical | **Missed by one trailing newline**: 51,297 B read back for 51,298 B raw. The CLI strips the command's last newline before the hook sees it (`stdout` in `PostToolUse`). Over 150,000 characters the CLI keeps its own exact copy, and a 228,894 B output read back byte-identical in 15 pages |
| A failing Claude Bash reaches the model untrimmed, its full output stored | **Partly, a contract limit**: the model gets the CLI's own 10,040 B excerpt untrimmed; `PostToolUseFailure` carries only that excerpt, so that is what is stored (labelled as the CLI's excerpt), not the 51,154 B |
| A failing Codex `run` with 50 KB reaches the model as ≤ 4,096 B, error lines first | **Pass**: 4,077 B digest (3,993 B as the model read it inside its JSON wrapper), `exit 1` then `error[E0308]` and `FAIL` lines first; the stored output is the command's 51,154 B byte for byte |
| Old sessions gone after the first start, plain Chats kept | **Pass as decided (D2)**: every earlier conversation, Chats included, is deleted on the first start; a Chat and a session made after it survive two restarts. The plan's line still says "plain Chats are kept"; the user's decision 2 (§5) deletes them |
| Preview cwd inside the session worktree; `ps` shows it after a hibernation; provider teardown and `end_processes`/`end_in_dir` exercised | **Pass**: every preview's cwd was the session worktree (`lsof -d cwd`, 7 previews); `ps -p` showed it after the hibernation; the hibernation ended the thread's CLI (`exited code 0`) and `end_processes` removed `orch:<conv>`'s scratch processes (a planted `sleep 900` there was killed), touching no `preview:` or `session:` owner |
| `tools/full-checks.sh` passes | **Pass** on `76a7cc21`: 419 Rust and 137 app tests, 0 failed |

## Access matrix (both vendors × three levels)

A fresh session per row (Claude Sonnet, Codex its default model, effort low). One message asked the thread to
read README.md, run `ls`, run `ls; echo hi > /tmp/w08-live/outside-<vendor>-<level>.txt`, and append a
line to README.md; in the sandboxed rows a second message asked it to run the outside write again out
of the sandbox.

| Vendor | Level | Read / command / edit | Leaving the sandbox | Card to the user |
|---|---|---|---|---|
| Claude | Full access | ✓ / ✓ / ✓ | no sandbox; file written | none |
| Claude | Approve for me | ✓ / ✓ / ✓ (sandboxed) | the CLI's auto mode reviewer **denied** every `dangerouslyDisableSandbox` retry ("[Safety Bypass Flag]", then "[Auto-Mode Bypass]" even with the user's explicit go-ahead); file not written | none |
| Claude | Ask for approval | ✓ / ✓ / ✓ (sandboxed) | the retry raised a card (tool `Bash`, `escalation: true`); approved, the file was written | 1 |
| Codex | Full access | ✓ / ✓ (`run`) / ✓ (`apply_patch`) | no sandbox; file written | none |
| Codex | Approve for me | ✓ / ✓ / ✓ (sandboxed) | **first run: failed.** The guardian allowed `require_escalated`, but the command still got "operation not permitted". **Fixed** (below), re-checked live: `with_additional_permissions` for the folder, guardian `allow`, file written | none |
| Codex | Ask for approval | ✓ / ✓ / ✓ (sandboxed) | `run_unsandboxed` raised a card; approved, the file was written. Its native `require_escalated` also raised a card, but after approval still ran sandboxed (and its item never completed: Codex emits no `item.completed` for a sandbox-denied exec) | 2 |

**Why Codex couldn't leave the sandbox.** In codex-cli 0.160.1 any `deny` entry in a permission
profile turns escalation off: `core/src/tools/sandboxing.rs`
(`sandbox_permissions_preserving_denied_reads`, `unsandboxed_execution_allowed`) turns an approved
`require_escalated` back into an ordinary sandboxed run, because "bypassing the sandbox would
silently grant those reads". Brigadier's profile denies the daemon's run folder (its IPC token), so
the approval was asked for and ignored. Reproduced without Brigadier with `codex exec`: with the deny
entry the write fails; without it, or with the legacy `workspace-write` sandbox, it works.

**The fix** (`crates/providers/src/codex/mod.rs`): a profiled session whose approvals Codex's
auto-reviewer answers gets `features.exec_permission_approvals` ("Allow exec tools to request
additional permissions while staying sandboxed", `codex features list`: under development, off by
default). A command then asks for exactly the folders or network it needs
(`sandbox_permissions: "with_additional_permissions"`), the reviewer approves, and the grant is
merged into the profile with the denies kept; nothing runs unsandboxed. A path deny loses to a
narrower grant inside it (`protocol/src/permissions.rs`: the most specific entry wins), which an
approved read of `<run>/token` could use, so every denied folder is also denied as `<folder>/**`;
`codex sandbox` with both entries refused the narrower read, a listing and a write. The feature is
off under Ask: the app-server strips the requested paths from a client's approval request unless
the client opts into its experimental API, so a card couldn't show them; Ask keeps `run_unsandboxed`.

**Claude at Approve for me** can't leave the sandbox: its auto mode reviewer denies a sandbox bypass
categorically, whatever the user said. That matches the done-when's wording (the reviewer settles it,
no card) but leaves the thread no way out at that level. See the report's open issues.

## Five asks (Claude, Full access, Opus 5.5 effort medium)

| Ask | Tools | "can't" hits | Reply's first line |
|---|---|---|---|
| Run the app so I can see it | project_map, Bash, **start_preview** (`python3 -m http.server 8000`), curl | 0 | "The app is running at http://localhost:8000/, and a request to that address returned HTTP 200." |
| Show me the app's logs | preview_log | 0 | "The log has one line so far, and it came from my own check that the server was up:" |
| Run the tests | Bash | 0 | "One of the two tests fails, so `npm test` fails overall." |
| Check what's in package.json | none (read in ask 1) | 0 | "`package.json` sets up `w08-demo` (version 0.0.1, marked private)…" |
| Open port 8765 so I can hit it from my browser | lsof, **start_preview** (:8765), curl | 0 | "The app is now running at http://localhost:8765/…" |

Counted with a grep of every assistant message of each turn for `can't|cannot|unable|I don't have`.

## Previews

**Merge (live).** Two previews (pids 37391 and 37906) ran with cwd
`…/data/worktrees/<project>/session-2610d168` (`lsof -d cwd`). The thread's `finish_session` card was
allowed; `main` moved to the session's commit, and 2 s later `ps -p 37391,37906` was empty and nothing
listened on 8000 or 8765. Both ended "stopped: the session was merged", with their logs stored, and the
ledger recorded `cleanup.removed` for both processes under `preview:<conv>`.

**Lifecycle (live).** One Claude thread at Full access started every preview with `start_preview`
(`python3 -m http.server <port>`); triggers from the code: `hibernate` IPC, `BRIGADIER_REBIRTH_TOKENS=1000`,
`debugInjectLimit` on the conversation for the fallback, `interrupt` for the user's Stop, `stopPreview`
for the chip, `archive`, and SIGTERM to the daemon for quit.

| Event | Preview pid before → after | Proof |
|---|---|---|
| Hibernation | 76252 → 76252, curl 200; the CLI 76170 and its MCP 76186 gone | `runStateChanged hibernated`; `cleanupRemoved owner=orch:<conv>` (its scratch processes, the CLI) |
| Rebirth | 81434 → 81434; the CLI 81327 → 82833 | `orchestrator reborn generation=1 trigger=Threshold` |
| Fallback | 83928 → 83928; Claude's CLI gone, `codex app-server` took over | `conversationFallback choice=codex/…` |
| stop_preview | 76252 → gone | `preview ended … "stopped by the thread"` |
| User's Stop | 79370 → gone | `… "stopped by the user"` |
| Chip's Stop | 79887 → gone | `… "stopped by the user"` |
| Archive | 80590 → gone, 0.5 s before the worktree's removal | `cleanupRemoved preview:<conv>` before `cleanupRemoved session:<conv> worktree` |
| Quit | 81434 (Claude thread) and 83928 (Codex thread) → gone | `shutting down reason=SIGTERM`, then `… "Brigadier quit"` |

## Q14

A `thread-build` (`3fbe9e90`) daemon made a session and a plain Chat on a scratch data dir with
`defaultPermission: askForApproval`. The `thread-p2` daemon's first start on it logged "the thread
engine's first start is deleting the earlier engine's conversations" (listed 2, deleting 2) and recorded
`engine.switching`, two `conversation.deleting`, `settings.changed` (defaultPermission → fullAccess)
and `engine.switched`: 0 conversations left, their `turn_usage` rows and streams gone. A new Chat and a
new session then survived two restarts, with no further deletion and still exactly two `engine.*`
events. The project's own remembered level (`askForApproval`) stays, as D1 says.

## Codex review

`codex exec review --base 3fbe9e90 -c model_reasoning_effort="high"`: five findings, all fixed in
`3538948f` with tests:
- P1, a preview's Seatbelt fallback dropped the denied reads: the sandbox policy now carries them
  (tested by running `sandbox-exec` for real);
- P1, a `run_unsandboxed` pass is bound to the approved workdir as well;
- P1, a starting preview belongs to its own task from its spawn, so a cancelled call can't orphan it;
- P2, `BRIGADIER_DATA_DIR` with `..` is refused (a symlink then `..` named the protected folder);
- P2, a multibyte file name in a Codex listing no longer panics the read tracker.

A second review of the fixes (`--base c6bbaeb1`) found three more, fixed in `76a7cc21`: the
cancelled start freed the preview numbering early (the recording task now holds it), the scripted
approval fixture now carries a prompted tool's own `workdir`, and an unused `mut`.

## T1 (one run; the target was met, so the second wasn't used)

Arm `/tmp/brig-ab-1007/t1-p2`: a clone warmed with `warm.sh` from `t1-p1b`, daemon `76a7cc21`
(`bin/brigadierd-p2`), Full access, new worktree, thread Claude Opus 5.5 high, as phase 1's run 2.

| | Phase 2 | Phase 1 run 2 |
|---|---|---|
| Landed (`times.py`) | **328.9 s** | 357.2 s |
| Answer / settled | 339.7 s / 339.7 s | 371.5 s / 434.6 s |
| Tasks | 1 (lead, implement), no verifier | 1 |
| Reviews | 1: the worker's own range `59a60afb..b2786ff7` by Codex, clean at 255 s; the landing of that tip got no second review | 3 |
| Frozen checks on `b2786ff7` | install, typecheck, lint, test: all exit 0 | all exit 0 |
| Raw tokens (`armtokens.sh`, t0 → settled) | **2,862,725** | 4,091,353 |
| Claude raw | 2,723,037 (thread 379,852, worker 2,343,185) | 3,370,018 |
| Codex raw | 139,688 (the review) | 721,335 |
| Without cache reads | **168,970** (Claude 142,306, Codex 26,664) | 272,428 |

`turn_usage` and the transcripts agree exactly (difference +0), and no Codex child thread went
unmetered. Against the corrected baseline (5,103,267 raw) this run is 0.56×, against the phase-3
goal of 0.6×; it is one run, and most of the saving is the single review in place of three.

## Independent verification (after `fa756631`)

**T1, re-derived from the arm files.** `routing.sqlite` `turn_usage` for t0 → settled
(`1791399743604`–`1791400083326`): Claude 2,723,037 raw, 142,306 without cache reads (thread
379,852 / 30,867; worker 2,343,185 / 111,439); Codex 139,688 / 26,664 (the one review). Total
2,862,725 / 168,970, as reported. The Claude transcripts (deduplicated per message) and the
review's Codex rollout give the same numbers; the brain's 291,009 ended before t0. The landed
step (`orchestratorStepped` `landed`, head `b2786ff7`) is at 328.9 s, and the frozen checks
re-run on `b2786ff7` in a fresh worktree all exit 0.

**A Codex thread at Approve for me ran with the user's connectors, plugins and goals.** A
thread's config reaches Codex after the app-server's own overrides, one key path at a time
(codex-cli 0.160.1, `config/src/overrides.rs`), so the `features` table `eaa22e90` set replaced
the table the `--disable` flags built. Live on a dev daemon at `fa756631`, that thread listed
`create_goal`, `request_plugin_install` and about 300 connector tools; Full access and Ask
threads on the same daemon listed none. `codex -c features.goals=false … -c
'features={exec_permission_approvals=true}' features list` turns them back on, and a dotted key
doesn't. Fixed in `bf1ac901` (a dotted key, tested). Re-checked live: the thread widened its
sandbox (`with_additional_permissions`, guardian `allow`, file written) with the same tool list
as the other levels.

**Denied folders under a grant.** `codex sandbox` with Brigadier's profile shape (the denied
folder as a path and as `/**`) refused, besides the earlier rows: a read through a symlink into
it, a hard link out of it, a grant of the symlink's path, a write grant on its parent (listing,
writing and renaming it), a rename of its parent folder after a write grant above that, and a
write grant on the token itself. Codex adds an approved grant to the profile's entries (deny
wins a tie, and a grant can't be a glob: `sandboxing/src/policy_transforms.rs`). Nothing runs
unsandboxed except at Full access and through `run_unsandboxed`, which only Ask offers and only
for a command the user approved on its card, once, in the same folder, within 120 s.

**Codex review of the phase** (`codex exec review --base 3fbe9e90`): four findings, all fixed
with tests in `f7f28319` and `dead7fcc`. A Stop no longer misses a preview started while it
stops the others. A late log snapshot no longer undoes a preview's end. `run`'s call outlives
its longest command. A read through a symlink and `..` is recorded as the file it is.
