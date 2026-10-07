# Thread phase 2 spike: "Open in terminal" for both vendors (2026-10-07)

THREAD-PLAN §2 Q5 and §5 risk 6. The question: can the daemon take a headless worker's own
session, hand it to an interactive terminal, and take it back afterwards, for Claude and for
Codex?

Checked live on the user's own unmodified binaries: Claude Code 2.1.292
(`/Users/stephen/.local/bin/claude`) and codex-cli 0.160.1 (`/Users/stephen/.local/bin/codex`).
Models: Sonnet for Claude (`--model sonnet --effort low`); Codex's default model with
`model_reasoning_effort="low"`. Scratch folder `/tmp/w07-spike` (two git repos), removed
afterwards, with the sessions it made.

The interactive side ran in a real PTY (a small Python `pty.fork` driver: 160×50, typed
keystrokes, Enter as `\r`, screen text captured).

## Verdict

| Vendor | Verdict |
|---|---|
| Claude | **Feasible with conditions.** The round trip works: same session id, same transcript file, the terminal sees the headless turns, and the headless resume sees the terminal turns. Conditions: the daemon must close the headless process first (Claude has no lock), give the PTY a clean environment, and handle the folder trust dialog. |
| Codex | **Feasible with conditions.** `codex resume <id>` opens a thread made by `app-server` and continues it in the same rollout file. Codex has a per-thread writer lock, so two writers can't happen. Conditions: close the app-server first, unarchive the thread first, and repeat the sandbox and approval flags. |

Risk 6 does not trigger. Phase 4's takeover can be built as Q5 describes.

## What was tested (exact commands)

Common to all Claude runs (`$MCP` is a stdio MCP server with one tool, `codeword`, that returns
`grant=$SPIKE_GRANT`; its config names the variable as `"env":{"SPIKE_GRANT":"${SPIKE_GRANT}"}`,
like Brigadier's `mcp_config`; `$SETTINGS` is Brigadier's Full-access worker settings:
`autoMemoryEnabled:false`, `autoContinueAtUsageLimit:false`, `permissions.allow:["mcp__spike"]`,
`sandbox.enabled:false`, the `agents-md@builtin` plugin config):

```
COMMON = --strict-mcp-config --setting-sources project --mcp-config "$MCP" --settings "$SETTINGS"
         --permission-mode bypassPermissions --add-dir /tmp/w07-spike/scratch
         --add-dir /private/tmp/w07-spike/scratch --model sonnet --effort low
HEADLESS = -p --input-format stream-json --output-format stream-json --thinking-display summarized
           --include-partial-messages --verbose --replay-user-messages --permission-prompt-tool stdio
```

- Headless start: `claude $HEADLESS $COMMON --session-id <uuid>`, one stream-json user message,
  wait for `result`, close stdin.
- Terminal: `claude $COMMON --resume <uuid>` in a PTY, cwd = the repo.
- Headless again: `claude $HEADLESS $COMMON --resume <uuid>`.

Common to all Codex runs (Brigadier's `app_server_args` for a worker, plus low effort):

```
FLAGS = --disable plugins --disable apps --disable hooks --disable computer_use
        --disable browser_use --disable memories -c "notify=[]" -c allow_login_shell=false
        -c model_reasoning_effort="low"
```

- Headless start: `codex app-server $FLAGS`, then JSON-RPC `initialize`, `initialized`,
  `thread/start {cwd, sandbox:"danger-full-access", approvalPolicy:"never", config:{model_reasoning_effort:"low"}}`,
  `turn/start {threadId, input:[{type:"text",text}], effort:"low"}`, wait for `turn/completed`,
  close stdin.
- Terminal: `codex resume <thread id> $FLAGS --dangerously-bypass-approvals-and-sandbox --no-alt-screen`
  in a PTY, cwd = the repo; leave with `/quit`.
- Headless again: `codex app-server $FLAGS`, then
  `thread/resume {threadId, cwd, sandbox, approvalPolicy, config, excludeTurns:true}` and `turn/start`.

All CLIs ran under `env -i HOME USER LOGNAME SHELL PATH=~/.local/bin:/usr/bin:/bin:/usr/sbin:/sbin LANG TMPDIR SPIKE_GRANT`
(see "Environment" below for why).

The prompts: turn 1 (headless) creates `alpha.txt` and states "the secret codeword is
PELICAN-42". In the terminal: "Reply with the secret codeword I gave you earlier, immediately
followed by the word CONFIRMED", then "call the codeword tool" (Claude) and "create `beta.txt`".
Headless again: "report which files you created, and which part created each".

## Claude results

### Check 1: headless session → terminal resume. **Pass.**

Session `952c861c-84d5-4723-8dda-dc50be333fe2`, transcript
`~/.claude/projects/-private-tmp-w07-spike-claude-repo/952c861c-….jsonl`.

- Headless turn: `alpha.txt` = `alpha from headless`; `system/init` showed
  `"mcp_servers":[{"name":"spike","status":"connected"}]` and `"permissionMode":"bypassPermissions"`.
- Terminal: the TUI showed the earlier turn, the status line read
  `⏵⏵ bypass permissions on … ○ low`, and the model answered `PELICAN-42 CONFIRMED`.
- The MCP server from `--mcp-config` worked in the terminal: `The spike codeword tool returned
  "grant=grant-7731", and I created beta.txt …`. So `${NAME}` expansion works from the PTY's
  environment.
- `beta.txt` = `beta from terminal`.
- On `/exit` the TUI printed `Resume this session with: claude --resume 952c861c-…`.

### Check 2: no concurrent writers. **Claude does not stop them. The daemon must.**

Session `e9ef99c4-8727-4c5f-925d-3cd3fca50652`. The headless process stayed alive (idle, stdin
open) after its first turn. `claude --resume` in a PTY opened anyway: no lock, no warning. It
answered `HERON-77 CONFIRMED` and wrote `gamma.txt`. Then the still-alive headless process got
one more turn:

> "In this conversation I've created only `delta.txt` …, and no one has created `gamma.txt`"

Both processes appended to the **same** transcript file. Its parent chain forked: the terminal's
first message and the headless process's next message both have parent `1d8b4f90` (the last
headless reply). A later `claude -p --resume` followed the newest branch (the headless one) and
said "No turn in this conversation created `gamma.txt`". **The terminal's turns were silently
lost.**

### Check 3: supervision handed back. **Pass.**

`claude $HEADLESS $COMMON --resume 952c861c-…` after the terminal exited:

> "I created `alpha.txt` ("alpha from headless") in the first turn and `beta.txt` ("beta from
> terminal") in the third turn … The spike codeword tool returned `grant=grant-7731`."

`system/init` `session_id` was `952c861c-…` again; MCP `spike` connected.

### Check 4: ids, files, cache, flags

- **Same session id, same file.** One transcript file. The headless entries carry
  `"entrypoint":"sdk-cli"`, the terminal ones `"entrypoint":"cli"`. The parent chain is
  continuous (the terminal's first message points at the last headless reply).
- **Cache on the headless resume:** `cache_read_input_tokens` 32,523, `cache_creation_input_tokens`
  449, `input_tokens` 2. The terminal's own turns warmed the cache.
- **`--append-system-prompt` need not be repeated.** A headless session started with
  `--append-system-prompt "Always end every reply with the token ZEBRA-9."` was resumed in the
  terminal without it. The terminal reply was `Teal.\n\nZEBRA-9`. This matches
  `--system-prompt-snapshot` (on by default): "every later request and resume sends the record
  as-is".
- **Resume from another cwd** (`/tmp/w07-spike/scratch`) also found the session and appended to
  the original file. The daemon should still use the task worktree as cwd.

Flags in a terminal (each tried with `claude $COMMON <flag> --resume <id>` in a PTY):

| Flag | Interactive |
|---|---|
| `--settings`, `--mcp-config`, `--strict-mcp-config`, `--setting-sources project`, `--permission-mode bypassPermissions`, `--add-dir` (×2), `--model`, `--effort` | Accepted and in effect (status line, MCP tool call). **Repeat them.** None of these are stored in the transcript. |
| `--disallowedTools`, `--tools` | Accepted. Repeat them so the terminal has the same tools. |
| `--include-partial-messages` | **Rejected:** `Error: --include-partial-messages requires --print and --output-format=stream-json.` |
| `--replay-user-messages` | **Rejected:** `Error: --replay-user-messages requires both --input-format=stream-json and --output-format=stream-json.` |
| `--input-format stream-json` | **Rejected:** `Error: --input-format=stream-json requires output-format=stream-json.` |
| `--output-format stream-json`, `--verbose`, `--thinking-display summarized`, `--permission-prompt-tool stdio` | Parsed without error, but drop them. They are print-mode flags, and there is no stdio permission host in a terminal. |
| `--append-system-prompt` | Not needed (snapshot, above). Harmless to drop. |
| `--session-id`, `--fork-session` | Drop; use `--resume <id>`. |

### Claude: things the PTY run showed

- **Folder trust dialog.** In a folder the user never trusted, the TUI opens with "Accessing
  workspace: … Quick safety check: Is this a project you created or one you trust?" and
  **"❯ No, exit"** selected by default. Print mode skips it (per `claude --help` for `-p`). Trust
  is inherited: a subfolder of a trusted folder (`claude-repo/sub/deeper`) got no dialog; an
  untrusted sibling (`scratch/untrusted`) did. Accepting writes `hasTrustDialogAccepted: true`
  for the path in `~/.claude.json`.
- **No bypass-permissions dialog** appeared for `--permission-mode bypassPermissions`. This user
  has accepted it before; a new user may see a one-time dialog (not tested).
- **Environment.** The first terminal attempt inherited this agent's own environment
  (`CLAUDE_CODE_CHILD_SESSION=1`, `CLAUDE_CODE_SESSION_ID=…`, etc.). The TUI answered normally,
  but **wrote nothing to the transcript**, and still printed "Resume this session with: claude
  --resume …". Isolated: `CLAUDE_CODE_CHILD_SESSION=1` alone causes it (0 `"entrypoint":"cli"`
  entries); `CLAUDE_CODE_SESSION_ID=<other id>` alone does not (11 entries). Headless `-p` runs
  persisted either way. Brigadier's `NESTED_SESSION_VARS` (`crates/providers/src/cli.rs:17`)
  doesn't list `CLAUDE_CODE_CHILD_SESSION`.
- **A wrapper on PATH.** In this machine's terminal, `claude` on PATH is a cmux shim. It
  replaced the `--settings` argument with its own settings file. Run the resolved real binary,
  not a name looked up in a terminal's PATH.

## Codex results

### Check 1: app-server thread → `codex resume`. **Pass.**

Thread `01a11717-5fea-74a3-b57f-665672d7729a`. `thread/start` returned
`"path": "~/.codex/sessions/2026/10/07/rollout-2026-10-07T18-59-30-01a11717-….jsonl"`,
`"source": "vscode"`, `"historyMode": "paginated"`.

- Headless turn: `alpha.txt` = `alpha from headless`.
- `codex resume 01a11717-… …` opened straight into the thread: header `permissions: YOLO mode`,
  the earlier turn listed (`• Ran printf '%s\n' 'alpha from headless' > alpha.txt`). No folder
  trust prompt, but this folder already had a trust entry (see "Risks").
- It answered `PELICAN-42CONFIRMED`, created `beta.txt` = `beta from terminal`, replied
  `FINISHED`.
- On `/quit`: `To continue this session, run: codex resume 01a11717-…`.

### Check 2: no concurrent writers. **Pass: Codex locks the thread.**

Thread `01a11718-6842-7543-8fbb-cdae0852928c`, app-server kept alive and idle after its turn.

- While the app-server held it, `~/.codex/thread-writer-locks/01a11718-6842-….lock` existed.
- `codex resume` showed: **"🔒 This conversation is open in another app. Close it there and press
  R to continue here. r retry · f fork · esc/ctrl+c/q exit"**.
- Typed text goes to that screen: the prompt's letters hit `r` (retry, still locked) and then `f`
  (fork). The fork became a new thread `01a11718-e1ec-73c2-b798-8a9019bb0dac`
  ("Thread forked from 01a11718-6842-…"); `gamma.txt` was written there. The original thread was
  untouched. Its app-server then said "Only delta.txt has been created in this conversation; no
  one has created gamma.txt."
- The other way round also holds. While a `codex resume` TUI held thread `01a1171c-…`,
  app-server `thread/resume` failed with
  **`{"code": -32600, "message": "thread 01a1171c-26cd-7b33-968a-92bff3126b91 already has an active writer"}`**.
- The lock is released when its holder ends, even by SIGKILL. After `kill -9` of the TUI the
  `.lock` file was still there, but `thread/resume` succeeded at once ("OK ZEBRA-9"). The file
  went away when the next holder exited cleanly.

### Check 3: supervision handed back. **Pass.**

New `codex app-server`, `thread/resume {threadId:"01a11717-…", excludeTurns:true, …}` returned the
same thread id and the same `path`. The report turn:

> "I created alpha.txt in the headless part and beta.txt in the terminal part. After creating
> beta.txt, I replied FINISHED."

### Check 4: ids, files, cache, flags

- **Same thread id, same rollout file.** One `session_meta` (`originator: w07-spike`,
  `source: vscode`). The terminal turns were appended as normal `turn_context` + messages.
- **Cache:** the headless resume's turn had `cachedInputTokens` 14,720 of `inputTokens` 14,935.
  In the terminal: `Token usage: total=3,756 input=3,593 (+ 70,016 cached)`.
- **Developer instructions need not be repeated.** `thread/start {developerInstructions:"Always
  end every reply with the token ZEBRA-9."}` is stored as a `developer` message in the rollout. A
  terminal turn without it replied `Blue ZEBRA-9`.
- **Sandbox and approval must be repeated.** They are per process or per turn, not per thread. A
  terminal resume without sandbox flags ran its turn with
  `sandbox_policy {"type":"workspace-write","network_access":false,…}`, not the thread's
  `danger-full-access`.
- **Archived threads.** Brigadier archives a thread when it closes the session
  (`archive_thread`, `crates/providers/src/codex/mod.rs:775`). `codex resume` on an archived
  thread shows **"This conversation is archived … › 1. Unarchive and resume 2. Cancel"**.
- **The TUI names the thread.** It wrote `{"id":"01a11717-…","thread_name":"Return codeword
  confirmed"}` to `~/.codex/session_index.jsonl`.
- **No shared daemon was involved.** During a terminal run the only Codex process was the
  `codex resume` one (no `app-server daemon`).

Flags in a terminal (each tried with `codex resume <id> $FLAGS <flag> --no-alt-screen`; all
started the TUI with no error):

| Flag | Interactive |
|---|---|
| `--disable plugins/apps/hooks/computer_use/browser_use/memories`, `-c "notify=[]"`, `-c allow_login_shell=false` | Accepted. Repeat them (same isolation as the worker). |
| `-c agents.enabled=false`, `-c skills.include_instructions=false`, `--disable shell_tool` | Accepted. Repeat whatever the worker's role used. |
| `-s workspace-write -a on-request`, `--approve-for-me`, `--dangerously-bypass-approvals-and-sandbox`, `--add-dir <dir>` | Accepted. **Repeat the mapping of the worker's access level** (above). |
| `-m <model>` | Accepted. Repeat it, or the user's default model applies. |
| `--no-daemon` | Accepted. Use it so the thread lives in the PTY process itself. |
| `--no-alt-screen` | Accepted. Optional: keeps scrollback in the terminal tab. |

## Flags checked against the help text

- `claude --help` (2.1.292): `-r, --resume [value]` "Resume a conversation by session ID";
  `--session-id <uuid>`; `--fork-session`; `--settings <file-or-json>`;
  `--setting-sources <sources>`; `--mcp-config <configs...>`; `--strict-mcp-config`;
  `--permission-mode <mode>` (acceptEdits, auto, bypassPermissions, manual, dontAsk, plan);
  `--add-dir <directories...>`; `--model <model>`; `--effort <level>`;
  `--append-system-prompt <prompt>`; `--system-prompt-snapshot <on|off>`;
  `--disallowedTools`; `--tools`; `-p, --print` ("The workspace trust dialog is skipped when Claude
  is run in non-interactive mode"); `--input-format`/`--output-format` ("only works with
  --print"); `--include-partial-messages` ("only works with --print and
  --output-format=stream-json"); `--replay-user-messages`; `--verbose`; `--no-session-persistence`.
  `--thinking-display` and `--permission-prompt-tool` are not in the help (Brigadier already uses
  both).
- `codex --help` (0.160.1): `resume`, `fork`, `archive`, `unarchive`, `delete`, `app-server`;
  `-c, --config <key=value>`; `--enable/--disable <FEATURE>`.
- `codex resume --help`: `[SESSION_ID] [PROMPT]`; `-c`; `--enable/--disable`; `-m, --model`;
  `-s, --sandbox` (read-only, workspace-write, danger-full-access); `--approve-for-me`;
  `--dangerously-bypass-approvals-and-sandbox`; `-C, --cd`; `--add-dir`;
  `-a, --ask-for-approval` (on-request, never); `--no-alt-screen`; `--no-daemon` ("Run without the
  shared background server, even if it is already running"); `--remote <ADDR>`.
- `codex app-server --help`: `-c`, `--enable/--disable`, `--listen` (default `stdio://`; also
  `unix://PATH`, `ws://IP:PORT`), `--stdio`.

## What the daemon must do in phase 4

1. **End the turn, then end the headless process.** Wait for the turn to end, or interrupt it.
   Then close the CLI the normal way: Claude, close stdin and wait for exit; Codex, close the
   app-server. **Wait for the process to exit** before the PTY starts. For Claude this is the
   only guard (check 2). For Codex the lock would catch it, but the user would see a "🔒 open in
   another app" screen, and a stray `f` would fork the thread.
2. **Codex: unarchive before the PTY.** Call `thread/unarchive` (the existing
   `unarchive_thread`) on a control app-server, or skip `archive_thread` when the takeover closes
   the session. Otherwise the TUI asks "Unarchive and resume?".
3. **Build the terminal argv from the same `SessionSpec`** as the headless one, with
   `Origin::Resume`, and spawn the **resolved binary directly** (not a shell string) in the task
   worktree:
   - Claude: `claude --resume <native id>` + `--strict-mcp-config --setting-sources project
     --mcp-config <same> --settings <same> --permission-mode <same> --add-dir … --model …
     --effort … [--tools …] [--disallowedTools …]`. Drop `-p`, `--input-format`,
     `--output-format`, `--include-partial-messages`, `--replay-user-messages`, `--verbose`,
     `--thinking-display`, `--permission-prompt-tool`, `--session-id`, `--fork-session`,
     `--append-system-prompt`.
   - Codex: `codex resume <thread id> --no-daemon` + the same `--disable …` and `-c …` as
     `app_server_args`, `-m <model>`, the access mapping as flags (Full →
     `--dangerously-bypass-approvals-and-sandbox`; Approve for me → `--approve-for-me` + `-c
     sandbox_workspace_write.writable_roots=[…]`; Ask → `-s workspace-write -a on-request`), and
     `--add-dir` for extra roots. `-c model_reasoning_effort=…` for the effort.
4. **Give the PTY the CLI environment**, not the daemon's raw one. Use `CliEnv`'s variables plus
   the session's own env (MCP grants, `CLAUDE_CODE_TMPDIR`, …). Today
   `crates/daemon/src/terminals.rs:153` only drops `BRIGADIER_*`. Add `CLAUDE_CODE_CHILD_SESSION`
   to `NESTED_SESSION_VARS`; it silently stops a terminal Claude from saving its turns. Consider
   `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN` and
   `CLAUDE_PID` too: they are harmless here, but they belong to another session.
5. **Claude folder trust.** The default answer of the trust dialog is "No, exit". Pick one:
   - the card says "Claude will ask whether to trust this folder: choose Yes";
   - or the daemon lets the user trust Brigadier's worktree root once; children inherit it.

   Brigadier should not write `~/.claude.json` itself. That would be the same rule as "never
   write `~/.codex/config.toml`".
6. **Detect the end of the terminal** by the PTY child's exit (EOF on the master and `waitpid`).
   Both CLIs exit on `/exit` or `/quit`, and on SIGHUP when the tab closes. Don't parse the
   screen.
7. **Hand supervision back** with the existing resume path: Claude `-p --resume <id>`; Codex
   `thread/resume {threadId, excludeTurns:true}` (the existing `unarchive_thread` first if
   needed). Codex's `already has an active writer` error is a clean signal that something still
   holds the thread: retry after a short wait, or tell the user. Then send the report request.
8. **After the report, archive the Codex thread again** as today. The TUI gave it a name and
   updated its time, so it would otherwise show in the user's Codex Recents.

## Risks and unknowns

- **Claude has no lock.** Anything that resumes a worker's session while the daemon still runs it
  forks the transcript, and the next resume loses the terminal's turns (check 2). Examples: a
  retry racing the takeover, or the user typing `claude --resume` in their own terminal. Mitigation:
  the takeover state machine (the task is `TakenOver`, no headless spawn while the PTY lives). An
  optional guard is Claude's process registry `~/.claude/sessions/<pid>.json`, which lists live
  interactive sessions with their `sessionId`; it was not tested as a guard here.
- **Codex trust prompt.** No trust prompt showed. But this spike's `thread/start` set a writable
  sandbox, so Codex had already written `[projects."/private/tmp/w07-spike/codex-repo"]
  trust_level = "trusted"` to `~/.codex/config.toml` (removed afterwards). Brigadier doesn't set
  a thread sandbox (`codex/mod.rs:19-27`), and Codex keys a worktree's trust to the user's main
  checkout. A project the user never trusted in Codex may get a trust prompt in the terminal, and
  answering it writes `config.toml`. That is the user's own choice in their own terminal. Not
  tested.
- **First-run dialogs** for a new user (Claude's bypass-permissions confirmation, Codex
  onboarding or model nudges) were not seen on this machine and are not covered.
- **Approve for me / Ask in the terminal** were checked for flag acceptance only, not for
  behaviour (Claude's sandbox settings, Codex's `--approve-for-me` reviewer).
- **Codex lock-screen keys.** If the daemon ever opens the PTY too early, typed letters act on
  the lock screen (`r` retry, `f` fork, `q` exit). Step 1 avoids it.
- **The TUI writes small user-visible state:** Codex thread names in `session_index.jsonl`;
  Claude prompt history in `~/.claude/history.jsonl`. These are the user's own terminal history,
  which is what Q5 wants.

## Cleanup

- `/tmp/w07-spike` removed.
- Claude state removed with `claude purge -y` for the two scratch folders: transcripts, file
  history, the `~/.claude.json` entry, and the prompts in `~/.claude/history.jsonl`.
- Codex threads removed with `codex delete --force <id>`: `01a11716-…`, `01a11717-…`,
  `01a11718-6842-…`, `01a11718-e1ec-…`, `01a1171c-…`. Without a TTY, `delete` needs `--force`. It
  fails on an archived thread ("failed to delete session") until `codex unarchive`. No rollout,
  `session_index.jsonl` line or `state_5.sqlite` row is left.
- The scratch trust entry was removed from `~/.codex/config.toml`.
- Left in place: three prompt lines in `~/.codex/history.jsonl`. That shared file is
  append-only, and rewriting it could lose a running session's lines.
- Only processes this spike started were stopped, by PID.
