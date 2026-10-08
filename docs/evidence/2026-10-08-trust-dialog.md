# Trust dialog: "Do you trust this folder?" (2026-10-08)

Branch `trust-dialog` from `thread-build` `fef7b14a`. Follows up the phase 4 finding
(`2026-10-08-thread-phase4.md`, "Both CLIs ask whether to trust the folder the first time"):
Claude's trust dialog defaults to "No, exit", which broke "Open in terminal".

Installed CLIs: Claude Code 2.1.294 (`/Users/stephen/.local/bin/claude`), codex-cli 0.160.1
(`/Users/stephen/.local/bin/codex`). Every probe and live check ran with a scratch `HOME`
(`/private/tmp/w25/exp/home` for the probes, `/private/tmp/w25/live/home` for the live app). The
user's real `~/.claude.json` and `~/.codex/config.toml` were never read or written. In the
scratch HOMEs neither CLI is logged in: Claude says "Not logged in", and Codex has a fake API key.
So no model turn ran.

## Where each CLI keeps folder trust

| CLI | Storage | How it was checked |
|---|---|---|
| Claude Code 2.1.294 | `projects["<real path>"].hasTrustDialogAccepted: true` in `~/.claude.json` (`$CLAUDE_CONFIG_DIR/.claude.json` when set) | The binary's per-project defaults (`hasTrustDialogAccepted:!1` next to `allowedTools`, `mcpServers` …). Answering "Yes, I trust this folder" in a PTY from a linked worktree wrote the entry below. |
| codex-cli 0.160.1 | `[projects."<real path>"]` `trust_level = "trusted"` in `$CODEX_HOME/config.toml` (default `~/.codex/config.toml`) | Answering "1. Trust and continue" in a PTY from the same worktree wrote the entry below. |

What each CLI wrote when the user trusted from the linked worktree `/private/tmp/w25/exp/wt/w1`,
whose main checkout is `/private/tmp/w25/exp/R`:

```toml
# ~/.codex/config.toml
[projects."/private/tmp/w25/exp/R"]
trust_level = "trusted"
```

```json
// ~/.claude.json, under "projects"
"/private/tmp/w25/exp/R": { "allowedTools": [], …, "hasTrustDialogAccepted": true, … }
```

Both CLIs store a worktree's trust under its main checkout. So one entry for the repository's
top folder covers every worktree Brigadier makes of it.

**Claude's lock.** 2.1.294 writes `~/.claude.json` under a `proper-lockfile` lock: the directory
`<file>.lock`, stale after 10 s, its mtime refreshed while held. It re-reads the file under the
lock before each write (`saveConfigWithLock`, 10 hits in the binary's strings). Brigadier
takes the same lock (`crates/providers/src/trust.rs`).

## Which entry covers which folder (PTY probes)

Script `probes.sh`, rerunning the probes from the first trust-dialog session. Each row starts
the CLI's TUI in `cwd` with only the `trusted` entry in its file, then looks for the trust
prompt: Claude's "Quick safety check", Codex's "Trust this folder?". Codex ran with
`--no-daemon --no-alt-screen`, as Brigadier's terminal runs it. `R` is a repository, `wt/w1`
is a linked worktree of `R`, `P` is a plain folder holding the repository `P/repo`, and
`P/sub/deeper` and `R/sub` are plain subfolders.

| Case | Trusted entry | cwd | Claude | Codex |
|---|---|---|---|---|
| none | – | `R` | prompt | prompt |
| repository root, from its worktree | `R` | `wt/w1` | no prompt | no prompt |
| the worktrees' parent folder | `wt` | `wt/w1` | prompt | prompt |
| a parent of a nested repository | `P` | `P/repo` | prompt | prompt |
| the worktree itself | `wt/w1` | `wt/w1` | no prompt | no prompt |
| repository root, at the root | `R` | `R` | (not run) | no prompt |
| repository root, a subfolder | `R` | `R/sub` | no prompt | no prompt |
| a plain parent, a plain subfolder | `P` | `P/sub/deeper` | no prompt | no prompt |
| the `/tmp` spelling of `R` | `/tmp/…/R` | `wt/w1` | prompt | prompt |

So:
- A folder's trust covers its plain subfolders, but not a git repository inside it.
- Both CLIs look up the real path: `/tmp` is not `/private/tmp`. Brigadier writes the real
  path of the repository's main checkout (`git rev-parse --git-common-dir`'s parent, through
  `Git::find_repo`), also when the project's folder is itself a linked worktree.

## Live checks: the dev app

Dev app built from this branch under its own identity (`ai.brigadier.w29trust`), with
`HOME=/private/tmp/w25/live/home` and `BRIGADIER_DATA_DIR` = a scratch fixture made by a flow
test. The fixture held one project "Flow", a thread session, and two Reported workers: task-1
on Claude and task-2 on Codex, both at Full access. The Vite port was moved to 1437, because
another worker's dev server held 1420. Computer Use: `health_report` failed ("daemon failed to
start … not listening"). The window was read with `screencapture -l <window>`, clicked with a
posted `CGEvent`, and driven over IPC (`tools/ab/bipc.py` against the daemon's short socket).

| Check | Seen |
|---|---|
| The modal shows for an undecided project at start | "Do you trust this folder?", the repository path, "Agents will read the files in this folder and run commands in it. Only trust folders whose contents you know.", the hint about the project's settings, and the buttons "Don't trust" and "Trust" (screenshot). |
| "Trust" (clicked) | The project's `trust` is `[{path: <canonical repo path>, trusted: true}]`. The scratch `.claude.json` gained only `projects["<repo>"].hasTrustDialogAccepted: true`, and `config.toml` gained only `[projects."<repo>"] trust_level = "trusted"`. No `.lock` was left behind. |
| "Open in terminal" on the Codex worker, trusted | `codex resume <id> … --dangerously-bypass-approvals-and-sandbox` opened straight into the conversation (`permissions: YOLO mode`, the earlier turn shown). No trust prompt: 0 matches for `trust` or `safety check` in the PTY output. |
| "Open in terminal" on the Claude worker, trusted (first build) | No trust prompt (0 matches), but Claude's **Bypass Permissions mode warning** showed ("❯ No, exit / Yes, I accept"). Its default also exits, so it breaks "Open in terminal" just like the trust prompt. See the fix below. |
| The same after the fix | `claude … --settings {…"skipDangerousModePermissionPrompt":true…} --permission-mode bypassPermissions --resume <id>` went straight to the session lookup ("No conversation found with session ID …": the fixture's Claude session id is made up). There was no trust prompt and no warning. |
| Adding a project (`createProject` for a new scratch repository) | The modal showed for it at once (screenshot, path `/private/tmp/w29/newrepo`). |
| "Don't trust" for it | The modal closed, and the project's `trust` is `[{…, trusted: false}]`. Both scratch config files are byte-equal to their copies taken before. The screen had locked by then (`loginwindow` frontmost), so a posted click couldn't land. The answer went over the same IPC call the button makes (`setFolderTrust {id, path, trusted: false}`). |
| "Don't trust" on Flow (`setFolderTrust`, as the project settings' switch sends it) | `config.toml` is byte-equal to its copy from before the trust entry. In `.claude.json` every other key is unchanged, and `projects` is `{}`: Brigadier had created that container, and Claude creates it itself on first use. `failures: []`. |
| Sessions held to Ask, Codex worker | `codex resume … -s workspace-write -a on-request` with no bypass flag, and Codex's own trust prompt shows, since nothing was recorded for it. |
| Sessions held to Ask, Claude worker | `claude … --permission-mode acceptEdits`, with settings `"disableBypassPermissionsMode":"disable"` and the sandbox on. Claude's own trust prompt shows. |
| Start-up reconcile | With Flow trusted in the event log and the entries missing from both files, the daemon wrote them again at start. |

Fixture handling between checks: closing a takeover terminal ends the task, and its cleanup
removes the worker's worktree and its CLI session. To reuse the two workers, the scratch event
log was cut back to just after the "Trust" answer (seq ≤ 155, every stream), and the two
worktrees were re-added on their branches. Codex got a new rollout, made with `codex exec` in
the worktree, and task-2's native id was patched to it.

## Claude's bypass-mode warning (fixed here)

"Open in terminal" at Full access runs `claude --permission-mode bypassPermissions`. 2.1.294
then asks once for consent, with "No, exit" as the default. It skips the question when
`skipDangerousModePermissionPrompt` is true in the user, local, flag or policy settings. That
is function `B6` in the binary; the schema text reads "Whether the user has accepted the bypass
permissions mode dialog". A worker's `--settings` are flag settings. So Full access now adds
`"skipDangerousModePermissionPrompt": true` there: the user already chose Full access in
Brigadier. Other levels never get it (`claude::tests` asserts both).

## After the code review (commit `61fb09a6`)

A Codex review found four defects. All four are fixed, and each fix has a test that fails without it.

| Finding | Fix | Test (fails without the fix) |
|---|---|---|
| The release build failed: Don't trust called `TaskLive::cli()`, which only exists in debug builds | `TaskLive::has_cli()` | `cargo check --release --locked --workspace --exclude brigadier-desktop` and `-p brigadier-desktop` both finish |
| Overlapping Codex writes could lose entries: both passed the re-read check before either renamed | The whole read, edit and rename holds a lock: `config.toml.brigadier.lock`, made like Claude's lock and removed after. Writes in one process first wait for their turn on a mutex for each path. | `overlapping_codex_writes_keep_every_entry`: 16 threads. With the locks taken out, it failed 3 of 3 runs. |
| A re-trust kept the stale record, so removal deleted the entry instead of restoring the user's own `untrusted` | When a write succeeds, the holder's earlier records of the same CLI, file and folder are retired | `removal_puts_back_the_value_the_user_set_since` |
| A project whose folder is a linked worktree had trust written for that worktree, which the CLIs never look up from other worktrees | The key is the main checkout; a folder outside any repository uses its real path | `a_linked_worktree_project_trusts_its_main_checkout` |

Live re-check: the dev app with the new build, scratch HOME `/private/tmp/w29/home`, and a fresh
fixture with a Reported Claude worker.

| Check | Seen |
|---|---|
| "Trust" clicked in the modal | Both files gained only the repo's entry. |
| "Open in terminal" on the Claude worker | `--permission-mode bypassPermissions`, `"skipDangerousModePermissionPrompt":true`. 0 matches for trust, safety check or bypass. It went straight to "No conversation found …". |
| A project added on the linked worktree `lt/linked-a` of `lt/main`; the modal showed it; "Trust" clicked | Both files gained `/private/tmp/w29/lt/main` and nothing for `linked-a`. |
| The real CLIs started in another linked worktree, `lt/linked-b` (as a worker's checkout is) | Claude: no "Quick safety check". Codex: no "Trust this folder?", and it reached "Ask Codex". The control, an untrusted repo, showed each CLI's prompt. |
| Don't trust for that project | Codex's table for `lt/main` is gone. In Claude's entry, only `hasTrustDialogAccepted` went. The keys Claude itself had added when it started in `linked-b` (`lastGracefulShutdown`, …) stay, as they should. |

## Not checked live

- **Read-only Codex terminal folder** (a takeover's cwd outside the worktree, trusted under
  the task's owner and undone when the task ends). It can't be reached on a fixture: at start,
  recovery ends read-only tasks, and no real model runs without credentials. Covered by the
  flow test `a_read_only_codex_terminal_folder_is_trusted_until_its_task_ends` and the probes
  above.
- **The "Trusted folder" switch and the composer's locked Ask, clicked in the app.** The screen
  locked during the run. The switch sends the same `setFolderTrust` call checked above, and
  `pnpm test` covers the components.
