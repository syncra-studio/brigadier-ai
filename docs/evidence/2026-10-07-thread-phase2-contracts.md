# Thread phase 2, step 1: the CLI contracts, checked live (2026-10-07)

THREAD-PLAN §3 phase 2 step 1, and §5 risk 1. Checked on Claude Code 2.1.292 and codex-cli
0.160.1, the user's own unmodified binaries, in a scratch folder (`/tmp/w07-contracts`, removed
afterwards). Claude probes ran `claude -p --output-format stream-json --verbose --tools … 
--setting-sources project --strict-mcp-config --settings <file>`, which are Brigadier's own flags
for a worker or thread. The model was Sonnet 5, because these contracts belong to the CLI, not to
the model.

Sources:
- `claude --help` (2.1.292): `--add-dir <directories...>`, `--settings <file-or-json>`,
  `--setting-sources`, `--resume`, `--system-prompt-snapshot`, `--tools`.
- Hooks reference, `code.claude.com/docs/en/hooks` ("PostToolUse input", "PostToolUse decision
  control", "PostToolUseFailure input", "PostToolUseFailure decision control").
- Tools reference, `code.claude.com/docs/en/tools-reference` ("Output limits").
- Environment variables, `code.claude.com/docs/en/env-vars` (`BASH_MAX_OUTPUT_LENGTH`).
- `codex sandbox --help` (0.160.1).
- The Codex configuration reference (`mcp_servers.<id>.tool_timeout_sec`,
  `mcp_servers.<id>.default_tools_approval_mode`, `mcp_servers.<id>.tools.<tool>.approval_mode`,
  `mcp_servers.<id>.tools.<tool>.output_token_limit`).

## 1. The Bash `tool_response` shape (`PostToolUse`)

A successful Bash call's `tool_response` is an object:

| Field | Seen |
|---|---|
| `stdout` | string; at most the read-back window (30,000 characters by default) |
| `stderr` | string |
| `interrupted` | bool |
| `isImage` | bool |
| `noOutputExpected` | bool |
| `returnCodeInterpretation` | only on a benign non-zero exit, e.g. `"No matches found"` for `grep -c zzzz /etc/hosts` |
| `persistedOutputPath`, `persistedOutputSize` | only when the output passed the inline ceiling (~30,000 characters) |

There is **no exit-code field** on success. A success is exit 0, or exit 1 from a command the CLI
treats as benign (`grep`, `rg`, `find`, `diff`, `test`, `git diff`, `git grep`, …; tools
reference, "Output limits"). In that case `returnCodeInterpretation` says so.

**Over the inline ceiling the CLI is already lossless.** With `seq 1 20000` (108,894 bytes):
- the CLI saved the whole output to
  `~/.claude/projects/<slug>/<session>/tool-results/<id>.txt` (`persistedOutputPath`,
  `persistedOutputSize` 108894);
- `stdout` held the first 30,000 characters;
- the model got a `<persisted-output>` block of 2,241 bytes: the path and a 2 KB head preview.

So for outputs over about 30 KB, the hook reads the complete output from `persistedOutputPath`.
Between 8 KB and 30 KB, `stdout` and `stderr` hold all of it.

## 2. Replacement (`updatedToolOutput`)

| Hook reply | What the model got |
|---|---|
| The same object with `stdout` replaced, `stderr` emptied (inline 13,893-byte `seq 1 3000`) | Replaced: 29 bytes |
| The same, on the persisted 108 KB output, keeping `persistedOutputPath`/`Size` | The CLI's own wrapper around the replacement: `<persisted-output>…Full output saved to: <path>…Preview (first 2KB):\n<replacement>\n</persisted-output>` (267 bytes) |
| The same, with `persistedOutputPath`/`Size` left out | Replaced exactly: 16 bytes |
| A malformed object `{"bogus": 1}` | Ignored: the original output (13,892 and 2,241 bytes) |
| A plain string | Ignored: the original output |

The docs agree: "For built-in tools, a value that doesn't match the tool's output schema is
ignored and the original output is used".

**Built:** the hook returns the response object with only `stdout` (the digest) and `stderr`
(empty) changed, and the two `persisted*` fields left out.

## 3. Failures (`PostToolUseFailure`)

A non-zero exit fires `PostToolUseFailure`, not `PostToolUse`. This was checked with `false`,
with `sh -c '…; exit 3'` and with a script that exits 1. The input carries `error` (a string)
and `is_interrupt`. It has no `tool_response`. The `error` starts with `Exit code N`. The only
reply field is `additionalContext`, so the output can't be replaced, as the plan assumed.

**The hook never gets a failing call's complete output.** Probe: a script that prints 49,951
bytes and exits 1. It has an `error[E0308]` line at about 22 KB and `FAIL: the end marker` as
its last line.
- `error` was 10,040 bytes: `Exit code 1`, then the output's first 4,988 bytes byte-identical,
  a truncation marker, then the bytes up to character 30,000.
- Both the middle `error[E0308]` line and the final `FAIL` line were missing, for the hook and
  for the model, which said "The output was truncated in the middle".
- This matches the tools reference: a failure gets "a head-and-tail excerpt of that size
  [~10,000 characters] cut from the read-back window, with no file path". The read-back window
  is the first 30,000 characters.
- The CLI's working output file is gone before the hook runs. A failure hook that listed
  `/tmp/claude-<uid>/<slug>/<session>/tasks/` found it empty. Nothing is persisted for
  failures.

With `BASH_MAX_OUTPUT_LENGTH=150000` (documented maximum 150,000; it "enlarges the read-back
window, which is also the window a failing command's excerpt is cut from"):
- `error` was still 10,040 bytes;
- its tail now held the real end: `FAIL: the end marker`, which the model quoted;
- the middle `error[E0308]` line was still missing.

**Contract limit.** The done-when "a failing Claude `Bash` call … its full output is still
stored" can't be met through any supported source for outputs over about 10 KB. Failures stay
untrimmed for the model, as planned.

What phase 2 builds:
- Brigadier stores exactly what the hook gets (the CLI's excerpt), and labels it as the CLI's
  excerpt, not the full output.
- The thread's CLI runs with `BASH_MAX_OUTPUT_LENGTH=150000`, so a failing log of up to 150,000
  characters ends with its real last lines, where build and test errors usually are. This
  doesn't raise the success ceilings: "a valid result over the inline ceiling arrives as a file
  path plus preview regardless of this variable".

Recommended fix (needs the user, since it changes Q4's "`run`, for Codex only"): offer
Brigadier's `run` tool to a Claude thread as well, and have the prompt send builds and tests
through it. `run` owns the process, so it keeps every byte of failures too.

## 4. `--resume` with a different `--add-dir`

1. A session was started with `--add-dir /tmp/w07-contracts/ws1` and read `ws1/a.txt`.
2. It was resumed with `--resume <id> --add-dir /tmp/w07-contracts/ws2` (no `ws1`), under
   `--permission-mode acceptEdits`.

Results:
- **Same session:** the init and result events carried the same `session_id`
  (`92e81868-…`).
- **Cache read:** every call of the resumed turn read 5,800–6,340 tokens from the cache and
  wrote 238–403.
- **The new `--add-dir` holds, and the old one is gone:**
  - `Edit` of `ws2/b.txt` succeeded (`beta2`);
  - `Edit` of `ws1/a.txt` was denied, because write permission isn't granted for that path.
- **The CLI tells the model by itself.** Asked for its working directories, the model answered
  "`/tmp/w07-contracts/ws2` was added, and `/tmp/w07-contracts/ws1` was removed in the latest
  update". It still knew `a.txt`'s content from before the restart.
- **Note on `--add-dir`:** it is variadic (`<directories...>`). A prompt given after it on the
  command line is taken as a directory. Brigadier sends turns on stdin (stream-json), so this
  can't happen there, but argv must not end with `--add-dir` followed by a positional argument.

So a workspace change can restart the thread's CLI between turns with `--resume` and the new
`--add-dir`, at a warm cache. Brigadier's `[workspace]` note still names the switch in its own
words.

## 5. Codex

- **Writable roots per turn:** already contract-checked and used. `turn/start` carries
  `sandbox_policy` (`P/codex/mod.rs`, `start_turn`), and it "holds for that turn and the ones
  after it". The thread's effective workspace becomes a writable root there. A workspace change
  restarts the thread the same way as for Claude (thread resume plus a note).
- **MCP tool timeout:** `mcp_servers.<id>.tool_timeout_sec` ("Override the default 60s per-tool
  timeout for an MCP server", config reference) is already passed by the adapter
  (`P/codex/mod.rs`, `mcp_config`). Workers have relied on it since the 24-hour question timeout.
  The thread's server gets 1,800 s for `run`.
- **Per-tool approval:** `mcp_servers.<id>.tools.<tool>.approval_mode`, one of
  `auto | prompt | writes | approve` ("Per-tool approval behavior override for one MCP tool on
  this server"), overrides `default_tools_approval_mode` (Brigadier sets `approve` for its own
  server).
- **`output_token_limit`** exists per tool. `run`'s digest is at most 4,096 bytes, well under
  any default budget.
- **`codex sandbox`** runs one command under Codex's own Seatbelt policy:
  `codex sandbox -P <profile> -C <dir> -c <overrides> -- <command>`. A permission profile is
  required. It also takes `--allow-unix-socket <path>` and `--log-denials`. Checked live with
  the adapter's own profile shape
  (`permissions.t = {filesystem = {":root"="read", ":workspace_roots"="write", "<secret>"="deny"}, network = {enabled=false, unix_sockets={}}}`):
  - reading the denied folder failed with "Operation not permitted";
  - writing outside the roots failed with "Operation not permitted";
  - writing in the workspace succeeded;
  - `curl` couldn't resolve a host (network off);
  - the command's exit status 7 came through as the exit status.

### How `run` keeps the thread's access exactly (Delegator correction 2)

- **Full access:** `run` spawns the command directly, as the thread's own shell would (Codex
  `never` + `danger-full-access`).
- **Approve for me and Ask:** `run` executes through `codex sandbox` with the same permission
  profile the thread's session gets. That is the adapter's `permission_profile` shape built from
  the same `Access::Scoped`: writable roots, network, denied reads and unix sockets. It runs
  with the thread's environment (`TMPDIR` etc.). So `run` is held by the same Seatbelt rules,
  with the same policy, as the thread's built-in shell. Brigadier's own sandbox crate isn't used
  for it, because it can't express denied reads or socket rules.
- **Escalation:** a second tool, `run_unsandboxed {command, workdir, timeout_secs,
  justification}`, offered only below Full access. Its per-tool `approval_mode` is `prompt`, so
  Codex's own approval flow handles the call:
  - under Approve for me, the thread's `approvals_reviewer: auto_review` (its auto-reviewer)
    decides;
  - under Ask, the request reaches Brigadier, which shows the user a card, as for the thread's
    other approval requests.

  Once approved, the command runs outside the sandbox. Plain `run` stays `approve` (no prompt)
  because it can't leave the sandbox.
- **To verify while building:**
  - that Codex's auto-reviewer settles a `prompt`-mode MCP tool call under `auto_review`, and
    how the app-server presents that request under Ask;
  - then the same matrix through `run` itself (Delegator correction 2).

  If the auto-reviewer doesn't settle MCP tool approvals, the fallback under Approve for me is
  that `run_unsandboxed` is refused with a message to use the built-in shell, whose escalation
  the auto-reviewer does settle. That fallback would be recorded.

## Side observation

`--setting-sources project` keeps the user's own hooks out. The user's `~/.claude/settings.json`
has hooks for 13 events, and none ran. Two `SessionStart` hooks still ran: they come from the
CLI's built-in plugins (`cc-plugin-agents-md@builtin`, `cc-plugin-telemetry@builtin`, listed in
the init event's `plugins`).
