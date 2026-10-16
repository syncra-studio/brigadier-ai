# Phase O: never more tokens than plain (2026-10-16)

The user's Q57: every bench task at or under plain Claude Code's tokens. This records the per-call measurement that
found where Brigadier spent more, the cuts, and the diagnostic runs between them. The acceptance bench (plain re-run on
the current CLI, both arms alternating) is in the last section.

- Bench: branch `bench` at `4db70aef`, the TypeScript fixture with hidden tests. Models Opus 5.5 and gpt-6.1-sol at
  high. Claude Code 2.1.301, codex-cli 0.163.0, node 26.9.0.
- Brigadier runs in a new-worktree session at Full access. Diagnostic runs have no judges.
- Tokens are raw (uncached input + cache reads + cache writes + output), Claude and Codex summed. "Gate" leaves out the
  other vendor's review and the turn that handles its findings (Q52); "full" keeps them.
- Plain medians stored from Claude Code 2.1.300 (`workers/w35/plain-summary.json` of the run): T1 132,347;
  T2 1,072,833; T3 1,905,810.5; T4 201,944.5; T5 1,086,460; T6 256,807; T7L 5,260,538.5.

## The first call's fixed start

One-word `claude -p` turns with the bench's plain flags, adding one part of the thread's start at a time (the tool
schemas appended as text, an estimate):

| Context of the first call (tokens) | `79dda573` | `9865bf7e` (O2) | `14274a43` (O5) |
|---|---|---|---|
| Plain Claude Code, every built-in tool | 16,089 | 16,089 | 16,089 |
| The thread's built-in tools and Claude Code's prompt | 7,486 | 7,486 | 7,486 |
| + Brigadier's instructions | 13,180 | 11,576 | 11,602 |
| + its always-loaded Brigadier tools | 17,520 | 15,721 | 14,885 |

- WebSearch and WebFetch cost 766 of the thread's 7,486; ToolSearch costs nothing until a tool waits behind it.
- The instructions went from 17,351 to 12,469 bytes for a Claude thread on a worktree.

## Where the tokens went (chore O-1, `79dda573`)

Per call, Brigadier's context at the same step was 1.9–3.6k above plain's: its start. It made as many calls as plain or
fewer:

| Task | Plain calls, raw | Brigadier calls, raw |
|---|---|---|
| T1 | 6, 130.8k · 6, 129.8k | 6, 147.8k · 5, 117.1k |
| T4 | 10, 338.7k · 11, 361.0k | 8, 249.1k · 6, 187.9k |
| T6 | 10, 317.2k · 10, 407.3k | 7, 190.9k · 11, 329.8k |
| T5 | — | 20 calls; 1,139.6k in all: gate 604.0k, Codex review 145.0k, findings turn 390.6k |

- Calls Brigadier spent and plain didn't: the last edit and the check in separate calls; a try by hand after the
  committing check (T4, 37k); in T6, a `$OLDPWD` that the shell's reset to its scratch folder broke (34k).
- T5's findings turn was 6 calls at 58–68k of context: a fix committed through git for its Fixes trailer took a call,
  and recording the findings another. A 56.8k call only replied [quiet] while the review ran.
- No wake turn that only said [quiet] happened in any run.

## The cuts

| Commit | What | Moves |
|---|---|---|
| `9865bf7e` | Instructions say each rule once; a rule's procedure moves to the message or tool that needs it; revision 70 | every call, about −1.6k |
| `2f0899c7` | Last edit, a try by hand and the run_check in one message; `remember`'s field docs shorter | calls on T1, T4 |
| `1e047a43` | `run_check`'s `fixes`: the fix commit carries the Fixes trailer and records the findings fixed, checked by it | T5's findings turn |
| `497de92b`, `14274a43` | `run` behind ToolSearch; `note_for_user` in the same message as the last check; revision 71 | every call, about −0.8k |
| `d471b5ac` | A commit's reply says nothing more runs once the work is done | the try after the commit |
| `75647610` | record_findings is looked up only when the thread records findings itself | T5 |

## Diagnostic runs at the candidates (Brigadier only; gate statistic as the acceptance bench takes it)

| Task | `79dda573` (O-1) | `1e047a43` (O-2) | `14274a43` (O-3) |
|---|---|---|---|
| T1 (n=2 mean) | 132.5k | 133.6k (161.9k, 105.3k) | 104.9k (79.1k, 130.7k) |
| T4 (O-2/O-3: n=4 middle-two mean) | 218.5k (n=2) | 210.5k (161.7k, 201.6k, 219.4k, 231.3k) | 205.5k (183.2k, 197.9k, 213.1k, 217.3k) |
| T6 (n=2 mean) | 260.4k | 169.0k (143.7k, 194.4k) | 245.2k (287.2k, 203.2k) |
| T5 gate / full (n=1) | 604.0k / 1,139.6k | 488.4k / 783.6k | 501.2k / 825.5k |
| T5 findings turn: calls, tokens | 6, 390.6k | 3, 180.0k | 3, 181.4k |

- Against the stored plain medians at `14274a43`: T1 0.79×, T4 1.02×, T5 0.46× (full 0.76×), T6 0.95×. Against
  plain on the current CLI (O-1, n=2): T1 0.81×, T4 0.59×, T6 0.68×.
- `run` behind ToolSearch (O5), on its own: in the 9 runs at `14274a43` the thread called ToolSearch 0 times, `run` 0
  times, and made no invalid tool call. A first call's context was 17.4k against plain's 17.6k (17.8k at `1e047a43`,
  19.6k at `79dda573`).
- T6's 287k run spent 10 calls testing the old and new code against each other; plain's T6 runs take 10 calls too.
- Every diagnostic run passed its hidden tests.

## Acceptance bench

(to come: chore O-4)
