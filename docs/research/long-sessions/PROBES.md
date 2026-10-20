# CLI probes run for this research (2026-10-14)

Claude Code 2.1.299, subscription sign-in (the default account), `claude -p --output-format json --model
claude-opus-5-5 --strict-mcp-config`, in an empty scratch folder. Each row is one call; the usage is from the JSON
result. n = 1 per row, so each is a direction confirmed by a control, not a distribution.

## P1. Which cache lifetime does a `-p` session get on a subscription?

| Call | cache read | cache write | of which 1 h | of which 5 min |
|---|---|---|---|---|
| fresh session, effort medium | 10,938 | 11,456 | 11,456 | 0 |

**Result:** a `-p` session is a "main conversation" and writes with the **1-hour** lifetime, as the prompt-caching
docs say for subscriptions. This confirms the 1-hour basis of Brigadier's cold-cache rebirth (§7 item 3) for
Claude. Brigadier records cache writes but not their lifetime (`ephemeral_1h_input_tokens` /
`ephemeral_5m_input_tokens` are not parsed anywhere under `crates/providers/src`).

## P2. Does an effort change on resume keep the cache (Opus 5.5)?

| Call | cache read | cache write |
|---|---|---|
| A1 fresh, medium | 10,938 | 11,456 |
| A2 resume A1, **low** | 13,031 | 9,402 |
| A3 resume A1, low again | 22,408 | 63 |
| B1 fresh, medium (control) | 22,394 | 0 |
| B2 resume B1, **medium** (control) | 22,408 | 25 |
| C1 fresh, medium, ~112k tokens of filler in the message | 12,565 | 112,313 |
| C2 resume C1, **high** | 115,515 | 9,402 |
| C3 resume C1, high again | 124,892 | 63 |

**Result:**
- A resume at the same effort rewrites nothing worth counting (B2: 25 tokens).
- A resume at a different effort rewrites about **9.4k tokens once, and the same 9.4k whatever the conversation's
  size** (A2 on a ~22k session, C2 on a ~125k session). The conversation itself stays cached (C2 read 115.5k). The
  next call at the new effort is fully cached again (A3, C3).
- So on Opus 5.5 with a subscription, changing effort between turns costs about one 9.4k cache write at the 1-hour
  price (≈ 2 × 9.4k input-equivalents), not a rewrite of the history. This matches the vendor's statement that the
  cache stays intact on 5.5 models, apart from a fixed part that changes with effort.
- Not measured: Codex (its app-server `turn/start` takes `effort`; whether the prefix survives is unknown, see PLAN
  probe list), Sonnet 5.5, Haiku 5.5, or a change of effort inside one turn.

## P3. Shared fixed start across fresh sessions

B1, a fresh session in the same folder with the same flags made within the hour, read its whole 22.4k start from
the cache and wrote 0. This is the same result §7 recorded on 2.1.285, so it still holds on 2.1.299.

Cost of all probes: about 8 small calls and one 112k-token write (the JSON reported $1.00 at API prices for C1).

## P4. Codex: effort change on resume (codex-cli 0.162.1, gpt-6.1-sol, `codex exec --json`, `exec resume`)

`codex exec resume` reports the thread's running totals, so each row is the difference from the row before. Every
turn adds about 24.8k input (the whole thread re-sent).

| Turn | effort | input added | cached of it |
|---|---|---|---|
| c1 fresh | medium | 22,809 | 13,184 |
| c2 | medium | 24,794 | 22,656 |
| c3 | **low** | 24,812 | 13,184 |
| c4 | low | 24,830 | 13,184 |
| c5 | low | 24,848 | 24,576 |
| c6 | **medium** | 24,866 | 24,576 |
| c7 | medium | 24,884 | 13,184 |

**Result: inconclusive.** Hits swing between the 13.2k shared start and the whole thread from turn to turn, with or
without an effort change: c6 switched back to medium and hit fully, and c7 at an unchanged effort missed. Codex's
cache acts best-effort, as the vendor docs describe it (routing by prefix, no guarantee). An effort effect can't be
read from 7 turns. Two things follow:
- Codex usage comparisons need many turns per arm: one session's cache luck can move uncached input by about 45% per turn.
- The effort question needs a probe of n ≥ 10 alternating turns per condition over the app-server, the path
  Brigadier uses.

## Second pass (2026-10-15): questions answered without a live call

No live CLI probe was run in the second pass. These were settled from the vendor docs (re-fetched 2026-10-14/15),
the installed binaries and help (Claude Code 2.1.300, codex-cli 0.162.1) and a read-only query of the installed
app's `routing.sqlite`.

- **PLAN probe "Claude Bash output of 200 KB: full text or preview?" is answered by the docs** (tools-reference,
  "Timeout and output limits"). A *valid* result is inline up to roughly 30,000 characters; past that the model gets
  the path of a file saved in the session directory (cut past 64 MiB) plus a preview of the first 2,000 characters.
  A *failing* result is inline up to roughly 10,000 characters; past that it gets "a head-and-tail excerpt of that
  size cut from the read-back window, with no file path". `BASH_MAX_OUTPUT_LENGTH` sets the read-back window (30,000
  by default, hard ceiling 150,000) and doesn't raise the inline ceilings; `bashOutputMaxChars` (2.1.261+, up to
  128,000) sizes both. All three names are in the 2.1.300 binary.
- **What a hook gets for a failing command:** `PostToolUseFailure.error` "is generally the same text Claude receives
  as the failed tool's result", middle-truncated around a `... [N characters truncated] ...` marker (hooks page). So
  for a failing command a hook can store no more than the model saw. Still open, and cheap: which part of a long
  output the 30,000-character read-back window takes (start, end, or both), since that decides whether raising it
  helps a failing test log (PLAN §5).
- **`PostToolBatch` gets exactly what the model saw:** its `tool_response` "contains the same content the model
  receives in the corresponding `tool_result` block… For `Read`, that means line-number-prefixed text" (hooks page;
  `PostToolBatch` is in the 2.1.300 binary). Whether it fires in Brigadier's `-p` stream-json mode is a 1-call check.
- **Compaction can be refused on Claude:** a `PreCompact` hook exiting 2 (or `decision: "block"`) skips a compaction
  "triggered proactively before the context limit"; one "triggered to recover from a context-limit error already
  returned by the API" instead fails the current request. `PostCompact` receives `compact_summary` (hooks page). On
  Codex, `PreCompact` returning `continue: false` "stops before compacting" (Codex hooks page), but Brigadier
  switches Codex hooks off.
- **Stop hooks are bounded:** "after stop hooks have continued the turn eight times in a row, Claude Code overrides
  the next block and ends the turn", reset when Claude calls a tool; `stop_hook_active` marks a continuation.
- **P4 is explained by OpenAI's caching guide:** cache entries live on individual machines, "traffic above 15
  requests per minute can lead to overflow routing", and `reasoning.effort` is part of the cached prefix unless a
  `configuration_update` item changes it (GPT-6 and later). On GPT-5.6 and later the only `ttl` value is `30m`, a
  **minimum** retention ("at least 30 minutes", possibly longer), refreshed by reuse; writes cost 1.25× input and reads 0.05× on GPT-6.1 Sol. Whether Codex sends
  `configuration_update` is still unknown (the string appears 19 times in the 0.162.1 binary).
- **P3's scope holds for Claude workers in one conversation, more than the second pass feared.** A Claude worker's
  system prompt is the generic worker rules plus the repository's instructions; its task text is its first user
  message; it starts in the conversation's shared worker folder with its worktree added (`M/workers.rs:1601-1616`,
  `:1743-1750`). The one open question is whether the per-worker `--add-dir` path is written into the dynamic
  system-prompt sections (PLAN §5, B4 probe).
- **The checkpoint fork rewrites the thread (store, read-only):** the only `handoff` row (2026-10-14 03:46:29) read
  544 and wrote 216,616 after the thread's last turn at 252,313 (WEAK-SPOTS C11). Codex cache writes: reported as 0 in all
  725 Codex rows. The field is optional with a default of 0 in the app-server schema, so Codex comparisons lean on the
  cached share of input, keep recording the write counter, and treat Codex write charges as unknown.

## P5. Phase D2's probes (2026-10-16; claude 2.1.301 on Haiku, codex-cli 0.162.1 app-server on gpt-6.1-sol at low)

One command printed 200 KiB (204,802 bytes) with a planted marker 50 KiB in; one stdio MCP tool returned the same
text.

- **Claude, `PostToolBatch` in `-p` stream-json: it fires, once per batch, and its `tool_response` is byte-equal to
  the stream's `tool_result` content.** So the stream already carries what the model saw, and Brigadier stores it in
  the parse layer, before its 8 KiB clip, with no hook process per batch (BUILD-PLAN D2.1, change approved
  2026-10-16).
- **Claude, a big result is a preview plus a file.** `Bash`'s 200 KiB result reached the model (and the stream) as
  `<persisted-output>`: "Output too large (200KB). Full output saved to: `<config>/projects/<cwd>/<session>/tool-results/<id>.txt`"
  and a 2 KB preview. The stream line's `tool_use_result` also has `persistedOutputPath` and `persistedOutputSize`
  (204,802), and its `stdout` holds the first 30,000 characters. An MCP tool's 200 KiB result is the same idea in
  other words: "Error: result (204,802 characters across 3,414 lines) exceeds maximum allowed tokens. Output has been
  saved to `<…>/tool-results/mcp-probe-big_dump-<n>.txt`." Both files hold the whole output, marker included, so
  D2.2 copies them into the blob store when the event is seen.
- **Claude, `PreCompact` exit 2 blocks a proactive compaction, mid-turn included.** With
  `CLAUDE_CODE_AUTO_COMPACT_WINDOW=100000` (the binary clamps the value to at least 100,000) and five 20k-token reads in
  one turn, the CLI started compacting before each of the last three calls (`system/status` `compacting`, hook
  `trigger: "auto"`), the hook refused, and the stream went on with `status: null`, no `compact_result` and no
  `compact_boundary`; the context grew to 135k on the 200k model. A blocked manual `/compact` instead ends
  `compact_result: "failed"` with `compact_error: "skipped: Compaction blocked by PreCompact hook: …"`. So a
  `compacting` status is not yet a compaction: only `compact_boundary` is.
- **Codex, `item/completed` is whole.** The command's `aggregatedOutput` was all 204,802 bytes and the MCP call's
  `result` all of the text, marker included: no excerpt bound at this size (`tool_output_token_limit` bounds what the
  model keeps in its history, not the event). So the parse-layer copy is the whole output, more than the model saw.
- **Codex, Brigadier's own hooks without the user's: fails by the inline route.** With `--enable hooks` and
  `-c hooks.PostToolUse=[…]` / `-c hooks.PreCompact=[…]`, neither inline hook ran, while the user's
  `~/.codex/hooks.json` ran (`sessionStart`, `userPromptSubmit`; `source: "user"`). The per-session `CODEX_HOME` route
  was not tried. Codex hooks stay off (`DISABLED_FEATURES`); Codex is kept from compacting by the hand-off guard below
  its ~90% clamp alone (BUILD-PLAN D2.3, D2.3b).

## P6. Codex, Brigadier's own hooks through a per-session `CODEX_HOME` (2026-10-20; codex-cli 0.163.0 app-server on gpt-6.1-sol at low)

A scratch main home (a copy of the sign-in, a `hooks.json` of its own) and two session homes that link back to it
the way an extra account's home does (`auth.json`, `config.toml`, `sessions/`, `archived_sessions/`), each with its own
`hooks.json` (`SessionStart` and `PostToolUse`, each logging which home it came from). One turn ran `ls`.

- **Do hooks fire under app-server? Yes, but only trusted ones.** The session home's `hooks.json` is listed as
  `source: "user"`, `trustStatus: "untrusted"` (`hooks/list`), and an untrusted hook never runs, with
  `--enable hooks` or without. Codex 0.163 trusts a hook only through a `[hooks.state."<path>:<event>:0:0"]`
  `trusted_hash` in the home's own `config.toml`: the same entry given with `-c` still lists as untrusted, and
  `--dangerously-bypass-hook-trust` is a flag of the TUI and `exec` that `app-server` rejects. (This is likely why
  P5's inline `-c hooks.PostToolUse=[…]` never ran either.) With the entries written into a session home's own
  `config.toml` (hashes from `hooks/list`'s `currentHash`), both hooks fired: `hook/started`/`hook/completed` on the
  stream, `SessionStart` once, and `PostToolUse` after the command, with `tool_name: "Bash"` and the command.
- **Does sign-in survive? Yes, through a linked `auth.json`.** `account/read` gave the ChatGPT account and the turn
  ran. No token refresh fell inside the probe. A refresh writes `auth.json`, and whether that write keeps the link
  or replaces it with a file of its own (which would split the sign-in, a refresh token used up in one copy) was not
  seen.
- **Does resume still find the thread? Yes.** The rollout landed in the main home's `sessions/` through the link,
  and `thread/resume` found it from the main home and from the other session home. Each session home still grows its
  own `state_5.sqlite`, `thread_history_1.sqlite`, `goals_1.sqlite` and `memories_1.sqlite`, which aren't linked.
- **Are the user's hooks kept out? Yes.** Only the `CODEX_HOME`'s own `hooks.json` loads: the main home's never
  showed in `hooks/list` and never ran.

So the route works, but it isn't a small change. The trust entries mean the session home can't link the user's
`config.toml`. It needs a copy made at each launch, with the entries added, so the user's later config edits don't
reach a running session and what Codex writes back lands in the copy. A thread runs with the user's setup
(`ToolSet::user_setup`, hooks on), so its home would also have to carry the user's hooks, copied in and trusted
again under new keys on the user's behalf. The sign-in link's behavior on a token refresh is still unknown, and so
are the unlinked databases above. A Codex thread keeps every rule up front (issue #121 stays open).
