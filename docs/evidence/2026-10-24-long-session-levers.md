# Long-session levers on the installed CLIs (2026-10-24)

What each lever of the long-session design (`BRIGADIER_LONG_SESSIONS`, run2-D2…D15) did on the CLI versions
installed that day. Small real runs, a few cents each (run2-D15). Test transcripts and threads were deleted after.

- **Claude Code 2.1.310**, on Haiku 5.5.
- **Codex 0.164.0** (`codex app-server`), on gpt-6-luna at low effort.

## Measured

### Claude Code 2.1.310

| Lever | Result |
|---|---|
| `/compact <instructions>` sent as a stream-json user message at a turn end | **Works.** `status: compacting` → `compact_result: success` → `compact_boundary` (`trigger: "manual"`, `preTokens`, `postTokens`). The turn ends with an empty `result`. The summary held a literal line the instructions demanded. `PreCompact` receives the instructions as `custom_instructions`. |
| `compact_boundary` event | **Works**, on stdout and in the transcript (`type: system`, `subtype: compact_boundary`, with a `uuid`), `trigger` `manual` or `auto`. A new `system/init` follows each compaction. |
| `PostCompact` captures the summary | **Works:** its stdin carries `trigger` and the whole `compact_summary`. The same text is the transcript's first user message after the boundary (`isCompactSummary`), which is where Brigadier reads it (no hook needed). |
| Several `SessionStart` (matcher `compact`) hook outputs reach the model whole | **Works.** 8 hooks × 9,476 characters (≈76k in all) after a steered `/compact`: all eight reached the model whole. They arrived out of order (5,4,3,6,2,8,1,7), so each part is labelled. The transcript keeps each hook's output whole in one `hook_additional_context` attachment after the boundary: that is the receipt. A hook output of 11,995 characters became `<persisted-output>` with a 2 KB preview: the cap per hook is ≈10,000 characters. |
| Put-back after a compaction Claude starts itself | **Works:** the same hooks ran after every `trigger: "auto"` compaction (4 of 4). |
| `CLAUDE_CODE_AUTO_COMPACT_WINDOW` as the backstop | **Works**, takes 100k–1M (`k`/`M` suffixes); the environment beats the `autoCompactWindow` setting. With `100k`, auto-compaction fired at 77–83k: Claude compacts ≈20k below the window. |
| The user's own `autoCompactEnabled: false` | **Wins:** nothing auto-compacted up to 215k with the window set; a sent `/compact` still works. |

### Codex 0.164.0 (app-server)

| Lever | Result |
|---|---|
| Auto compaction at its own point, with no window override | **Works:** on a 272k-window model it compacted at 244,808 tokens (90% of 272k), mid-turn, as a `contextCompaction` item in the running turn. `thread/tokenUsage/updated` reports `modelContextWindow` 258,400. |
| `contextCompaction` item | `item/started` → `item/completed` with `{id, type}`. Automatic: inside the user's turn. Asked for: a turn of its own. |
| `thread/compact/start` | **Works:** answers `{}`, then a turn of its own (`turn/started` → the item → `turn/completed`). |
| A user turn sent right behind `thread/compact/start` | **Refused:** `-32603 failed to submit turn input: ActiveTurnNotSteerable { turn_kind: Compact }`. No user turn can slip in during a compaction turn; Brigadier holds messages until it ends. |
| `thread/inject_items` after a compaction | **Works:** a 75,995-character developer note sent right after the compaction is kept in the rollout as one `response_item`, byte for byte, its marker found once. A 52,892-character note was quoted first and last line by the next turn. It does not show in `thread/read`. |
| `thread/inject_items` during a running turn | **Accepted**; the model saw it later in the same turn. |
| An injected note through a later compaction | Not kept verbatim (the compaction's replacement history lacks it), so putting the decisions back after every compaction does not duplicate them. |
| Quoting an injected note | gpt-6-luna once declined to quote "hidden developer-note content", and quoted it in another run. The put-back now says these are the user's decisions and may be quoted. |
| `tool_output_token_limit` as a launch value (P5) | **No effect** on gpt-6-luna with code-mode `exec`: with `-c tool_output_token_limit=60000` (app-server or `thread/start` config), a 47.5k-token output was still cut to ≈10k tokens. The cut is the exec tool's own `max_output_tokens` (the model catalog's `truncation_policy`). Not tested on other models. Brigadier builds no pass-through (run2-D11 stays: revisit when a Codex version honours it). |

## Extrapolated, not measured

- Claude's backstop window for a 200k point: 250k, so that Claude compacts by itself at ≈230k (from the ≈20k gap
  measured at 100k).
- The "above half the point after compacting" fallback threshold is proposed (PROPOSALS §1, point 5) and waits for
  the test round.
- Claude's "Autocompact is thrashing" stop is matched on the message its documentation gives; it was not provoked.
- Codex's auto-compaction point on models other than the 272k one tested.

## How to reproduce

The probes are in `tools/long-session-probes/` (usage in each file's docstring):

- `python3 -I tools/long-session-probes/claude_slices.py <scratch> 8 9500` — eight `SessionStart(compact)` hooks
  after a steered `/compact`, isolated from the user's settings (`--setting-sources ""`). Delete the probe's
  `~/.claude/projects/<slug>` folder after.
- `python3 -I tools/long-session-probes/codex_order.py <scratch>` — a checkpoint turn, `thread/compact/start`
  with a user turn sent right behind it, a full-size `thread/inject_items`, and the rollout readback. Deletes its
  thread at the end.

The sanitized transcripts they produced are the receipt parsers' test fixtures:
`crates/providers/fixtures/long-sessions/claude-compact-8-slices.jsonl` and `codex-compact-inject.jsonl`.
