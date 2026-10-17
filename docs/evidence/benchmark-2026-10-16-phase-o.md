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

Plain re-run on the current CLI (Claude Code 2.1.301/2.1.302) in the same window as Brigadier, arms alternating, judges
on, no quiet hold (Q54). Statistic: mean of two runs; T4 the mean of the middle two of four. Gate ratios are judged
against this plain; the stored medians are informational.

### O-4 at `19ca6bdf` (T1–T6, T7L)

| Task | Plain | Brigadier gate | Gate ratio | Full ratio | vs stored median |
|---|---|---|---|---|---|
| T1 | 132,665.5 | 108,400.5 | 0.82× | 0.82× | 0.82× |
| T2 | 777,100 | 508,472.5 | 0.65× | 0.65× | 0.47× |
| T3 | 1,437,856.5 | 1,122,667 | 0.78× | 1.04× | 0.59× |
| T4 | 202,749 | 244,882.5 | **1.21×** | 1.21× | 1.21× |
| T5 | 888,767 | 543,428 | 0.61× | 1.03× | 0.50× |
| T6 | 243,504 | 243,959.5 | **1.0019×** | 1.0019× | 0.95× |
| T7L | 6,038,015.5 | 3,486,866.5 | 0.58× | 0.73× | 0.66× |

- Hidden tests 28/28; E3 16/16; T7L 24/24 features in both arms, no compaction.
- T5's one high finding (a test "fails type checking") is false: the run's own eval type-checks and passes 109/109.
- T7L's strict constraint audit flags medium/low judge findings in both arms' trees; plain's carry two highs,
  Brigadier's none.
- T4 and T6 failed on two causes, both fixed in `791fe1cd` (`c184e54b` before the rebase):
  - T6: a scratch script run through `run_check` was learned as a project check. The session's full checks ran it in
    their own checkout, failed, and woke the thread twice (+2 calls). A command naming a temporary folder is no longer
    learned, and one learned before is skipped.
  - T4: the thread put its tries by hand in the check's command (one fails on purpose), so the check failed and a
    second `run_check` committed (+1 call, about 42k). The commit reply and revision 71 now say a try is a shell call
    of its own, never part of the check's command.

### O-6b at `d6b23ff0`, rebased onto T2 (T1, T4, T6)

| Task | Plain (runs) | Brigadier gate (runs) | Gate ratio | vs O-4 plain | vs stored median |
|---|---|---|---|---|---|
| T1 | 166,423 (155.7k, 177.1k) | 93,173.5 (106.5k, 79.9k) | 0.56× | 0.70× | 0.70× |
| T4 | 228,697 (206.7k, 250.7k, 276.8k, —) | 216,604 (185.4k, 197.9k, 235.3k, 259.3k) | 0.95× | 1.07× | 1.07× |
| T6 | 288,247 (288.0k, 288.5k) | 182,364.5 (195.3k, 169.4k) | 0.63× | 0.75× | 0.71× |

- Plain's fourth T4 run stopped on the account's weekly limit. With its other three, the middle-two mean of four is at
  least 228.7k whatever the fourth costs, so 0.95× is the worst case.
- Calls per run, Brigadier against plain: T1 4–5 against 7–8, T4 7–8 against 7–9, T6 7–8 against 10. The costlier T4
  runs spend nothing beyond the task: a 17.4k first call, one check, nothing after the commit.
- Hidden tests pass in every completed run; high findings 0 against 0; E3 8/8; no full check ran a temporary-folder
  command.
- T3 and T5's full ratios (1.04×, 1.03×) are the Q48 review and its findings turn, which the gate leaves out; O4 already
  cut the findings turn from 6 calls to 3.
- The T2 rebase added two internal store records (a request's file record, a background run's output digest) that the
  live feed sends with no type. The A/B and bench readers now skip them (`ed466170`, `5151f4c0`). A first O-6 run and
  a first V2 re-run were void because of them.

### V2 and V3

- V3 at `19ca6bdf`: 12/12 asks, E3 12/12, no invalid tool call.
- V2 at `19ca6bdf`: every verdict kept but one: in a Claude lookup-size run, the thread corrected a worker's false claim
  with `remember` but without `replaces`, so the claim stayed live in its briefings. Phase O's shorter `remember` text
  had dropped "pass it as `replaces` to correct it, or a claim you find wrong"; `d6b23ff0` restores it.
- V2 lookup battery, size seam, at `5151f4c0` (with `d6b23ff0`): Claude 3/3 runs valid (3 seams each) and every item
  passes, `false_learned` and `thread_only` included; each correction of the worker's false claim passes `replaces`
  and the briefings show it replaced. Codex 2/2 valid (2 seams), every item passes but `thread_only` unknown, as in
  the reference. No probe ran a command or read a file.
