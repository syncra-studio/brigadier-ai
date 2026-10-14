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
