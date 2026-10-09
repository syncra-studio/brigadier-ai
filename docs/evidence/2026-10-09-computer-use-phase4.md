# Computer use phase 4: the GUI specialist and the model-in-the-loop suite (2026-10-09)

COMPUTER-USE-PLAN.md §8 Phase 4. Branch `computer-use`. Claude Code 2.1.295, codex-cli 0.161.0.
Every live run went through a dev `brigadierd` built from the branch, on a scratch `BRIGADIER_DATA_DIR`. It was
driven over IPC with `tools/ab` and `tools/computer-suite/run.py` at Full access. The suite's thread only relays,
so it ran on Claude Sonnet at effort low; the live handoff's thread was the new-session default, Opus 5.5 high. The computer helper was spawned from a terminal, so it ran with **the terminal's inherited Accessibility and
Screen Recording grants**, not the helper bundle's own; no privacy setting was touched. From about 11:30 a person was
using the Mac. The suite's fixtures open in the background and behind every other window, and the suite never sends
input to a window it didn't open.

## Done-when

| Done when | Result |
|---|---|
| The suite runs end to end on Claude and Codex workers, 3 runs per provider | **Partly.** Claude 20/20, 20/20, 17/18; Codex 20/20, 18/18, 18/18. Six dev-build trials didn't run (the dev-build tasks of Codex runs 2–3 and Claude run 3): the person closed the dev build's window, it was still closed at the end, and every way for us to reopen it takes the front |
| E1 meets its gate (as amended by the user: model calls ÷ (scripted tool calls + 2), median ≤ 1.3) | **Claude passes (1.0); Codex misses (2.25).** Pooled medians. Codex's causes and fixes are under E1 |
| P3 meets its gate (≥ 98% at ≥ 12 pt, ≥ 95% at 8 pt) | **Pass.** 50/50 at each of 8, 12, 16 and 24 pt (Opus, effort medium) |
| Total provider usage per completed task is reported | **Pass.** Table below |
| A GUI-heavy request in a dev session is handed to an `Operate` worker | **Pass.** Delegated as `kind: "operate"`, done in 114 s, the whole request's end state checked |
| The macOS comparison table is in the evidence, or the grants weren't given | **Grants not given.** Tools A–C couldn't run (below) |

## The suite

20 tasks: 15 on the target-range fixture, 3 on the `scratch-pad` document fixture and 2 on the dev build. Each trial
is a fresh `delegate_task` with `kind: "operate"` from the thread, judged by what reached the app (the fixture's
log, its state snapshot and the broker's delivered actions), never by the worker's own expects. Model calls are
counted from the transcripts.

**Per run**

| run | model | passed | E1 median (gate ≤ 1.3) | calls per reference batch | F1 suite changes |
|---|---|---|---|---|---|
| claude-1 | Opus 5.5, medium | 20/20 | 1.0 pass | 4.0 | 0 |
| claude-2 | Opus 5.5, medium | 20/20 | 1.25 pass | 5.0 | 0 |
| claude-3 | Opus 5.5, medium | 17/18 | 1.25 pass | 4.5 | 0 |
| codex-1 | gpt-6.1-sol, medium | 20/20 | 2.5 miss | 10.0 | 0 |
| codex-2 | gpt-6.1-sol, medium | 18/18 | 2.5 miss | 10.0 | 1 (see F1) |
| codex-3 | gpt-6.1-sol, medium | 18/18 | 1.75 miss | 6.0 | 0 |

Runs 1–2 used the build before `283e7747` (argument errors that show a well-formed call, call shapes in the Codex
brief), `9c53bd87` (menu misses name what the level holds) and `11607541` (the menu task reworded). Run 3 used the
build after them, so Codex's E1 is given per run.

**Model calls per task** (`ref` = the scripted solver's batches; the dev tasks have no reference)

| task | ref | claude-1 | claude-2 | claude-3 | codex-1 | codex-2 | codex-3 |
|---|---|---|---|---|---|---|---|
| append-line | 1 | 9 | 9 | 10 | 11 | 9 | 6 |
| check-8 | 1 | 4 | 4 | 4 | 6 | 9 | 5 |
| dev-rename | – | 10 | 9 | not run | 15 | not run | not run |
| dev-settings | – | 6 | 6 | not run | 13 | not run | not run |
| find-replace | 2 | 9 | 10 | 9 | 10 | 11 | 5 |
| form | 1 | 4 | 4 | 4 | 6 | 8 | 5 |
| last-row | 1 | 10 | 5 | 5 | 10 | 12 | 8 |
| menu | 1 | 8 | 6 | 11 FAIL | 7 | 7 | 6 |
| name | 1 | 4 | 4 | 4 | 6 | 11 | 5 |
| password | 2 | 4 | 4 | 4 | 9 | 6 | 6 |
| popup | 1 | 7 | 6 | 7 | 19 | 13 | 13 |
| press-3 | 1 | 4 | 7 | 6 | 9 | 12 | 7 |
| red-dot | 1 | 5 | 5 | 5 | 10 | 10 | 8 |
| replace-text | 1 | 7 | 7 | 8 | 10 | 11 | 10 |
| row-173 | 1 | 4 | 7 | 4 | 12 | 8 | 8 |
| sheet | 2 | 5 | 5 | 5 | 10 | 11 | 11 |
| slider | 1 | 4 | 4 | 4 | 11 | 14 | 4 |
| stepper | 1 | 4 | 4 | 4 | 10 | 9 | 6 |
| tab | 1 | 4 | 4 | 4 | 14 | 15 | 7 |
| two-dots | 1 | 4 | 6 | 5 | 11 | 11 | 8 |

- **The one failure:** claude-3's menu trial. The worker read "Pick Targets › …" as the menu's name, and read the menu bar
  through an AppleScript call, which the shortcut audit fails. The goal now reads "In the app's menu bar, choose the
  item at Targets › …". A menu-only check with that wording (seed 3) passed in 1 batch, 3 calls and 12 s. It is a
  fix check, not a fourth run.
- **password** passes as `engine_refused`: the engine's block list refused the secure field, as the task expects. Each report
  repeated the password, which was in the request.
- **Where Codex's calls go:** in codex-1, 7 of slider's 11 calls guessed argument names. With call shapes in its brief
  and well-formed examples in argument errors, codex-3's slider took 4 calls.

**Pooled per provider, and usage per completed task** (tokens: uncached input / cache read / cache write / output)

| provider | runs | passed | median calls | E1 median (gate ≤ 1.3) | calls per reference batch | median wall s | worker per completed task | thread per completed task |
|---|---|---|---|---|---|---|---|---|
| Claude | 3 | 57/58 | 5 | 1.0 pass | 4.0 | 18.1 | 12 / 115,913 / 8,137 / 1,106 | 8 / 136,506 / 1,547 / 377 |
| Codex | 3 | 56/56 | 10 | 2.25 miss | 8.0 | 51.2 | 22,133 / 136,087 / 0 / 724 | 6 / 104,842 / 1,291 / 346 |

The worker usage is the Operate worker's own. The thread usage is the delegating thread's turn around the trial.

## E1

**The gate, amended by the user on 2026-10-09:** model calls ÷ (the scripted solver's tool calls + 2), median ≤ 1.3.
The 2 are the worker's look before it acts and its report after, which a script never makes. The first gate, 1.3×
the fewest `act` batches, counted them against every worker: by it, Claude was 4.0 and Codex 8.0.

- **Claude passes:** 1.0 pooled (1.0, 1.25, 1.25 per run).
- **Codex misses:** 2.25 pooled (2.5, 2.5, 1.75 per run).

**Where Codex's extra calls go.** Run 3 used the build with call shapes in the brief: 128 calls over 18 trials, read
from its transcripts.

| cause | calls | the fix |
|---|---|---|
| `submit_report` rejected: `done_when` sent as a list of objects (criterion, status, evidence), or `changes` as a string; the error names a serde type, not the call, so it guesses 2–3 times | 13 | accept a list of objects or a string wherever a list of lines is asked for, and show a well-formed call in the error, as the computer tools do |
| code-mode `exec` calls that run no tool: listing `ALL_TOOLS` to read a schema, its report tool's included, or only printing | 16 | give `submit_report`'s call shape in the Codex Operate brief beside the computer tools', and say the shapes are complete |
| computer calls rejected for their arguments | 12 | accept the argument spellings Codex used |

Without those 41 calls, run 3's median would be about 1.25, inside the gate. That is an estimate, at one model call
per tool call, which is how Codex worked here. It needs a Codex run after the fixes to count.

## P3: grounding

Four boards, 8, 12, 16 and 24 pt targets on a canvas with no structure, 10 boards of 5 targets per size: 50 trials
per size. The worker was the router's Claude pick for Operate, Opus 5.5 at effort medium. It was the cheapest
image-capable model the router would choose there.

| size | hits | wrong | misses | max error | Wilson lower bound | calls | wall s |
|---|---|---|---|---|---|---|---|
| 8 pt | 50/50 | 0 | 0 | ≤ 1.4 pt | 0.929 | 13 | 54.2 |
| 12 pt | 50/50 | 0 | 0 | ≤ 1.4 pt | 0.929 | 13 | 51.2 |
| 16 pt | 50/50 | 0 | 0 | ≤ 1.4 pt | 0.929 | 13 | 51.2 |
| 24 pt | 50/50 | 0 | 0 | ≤ 1.4 pt | 0.929 | 13 | 54.3 |

The largest error over all 200 trials was 1.4 pt. This is an empirical pass: 50 trials can't prove a 98% rate.

## The live handoff

A new dev session on the new-session default got a plain request, worded as a user would: seven GUI steps on the
fixture: type a name and notes, tick a checkbox, set the slider, choose a pop-up item, open and close the sheet,
and select a table row. The thread delegated it as `kind: "operate"`. The router picked gpt-6.1-sol at effort
medium. A later route preview showed why: Claude's 5-hour window was projected at 92%, which put Opus's score
below gpt-6.1-sol's. The task was done in 114 s, and the reply lists every
value. `handoff.py --recheck` judges the whole request's end state from the fixture's own log: pass.

## F1 during the suite

Front changes the suite caused: 0 in five runs, and 1 by the rule in codex-2. In that one, the front changed 1.0 s
after a failed type on the document fixture, with no activation sent. The person's own windows were changing just
before it, and macOS handed the front to the topmost window, which was ours. Since `d4588398` the fixtures order
their windows to the back, so they are never the topmost. The scripted suite passes 22/22 that way. Front changes
the person made (counted apart) aren't the suite's.

## Comparisons

Not run: the grants weren't given.

| tool | what it is | why it couldn't run | to run it |
|---|---|---|---|
| A | another app's background computer-use runtime | its runtime wasn't listening; turning it on is a setting only the user can change | turn its Computer Use setting off and on |
| B | the Codex CLI's built-in computer use (`computer_use` feature) | its plugin needs installing, then Screen Recording and Accessibility grants for its runtime, then per-app approval | install the plugin in Codex's settings, grant both when macOS asks, approve the target apps |
| C | another installed app's computer use | needs a login and both grants | log in, grant both |

Once granted, the method is the same suite, with the same fixtures, checkers and focus monitor, 1 run each, the tool
as the only GUI path.

## Bench

`brigadier-computer bench --no-foreground` (release). It skips P2f, the one step that raises a window, because a
person was at the Mac.

| gate | full, `deceff7c` (200 reps) | quick, `acf932a1`, the overlap (20 reps) | target |
|---|---|---|---|
| S1 observe | 6.5 / 7.8 ms | 7.9 / 21.4 ms | ≤ 15 / ≤ 40 |
| S2 observe + screenshot | **77.3 / 86.3 ms, miss** | 49.9 / 53.5 ms | ≤ 70 / ≤ 120 |
| S3 set value · menu-bar pick · press (effect) | 4.3 / 23.8 · 5.2 / 10.7 · 4.2 / 6.9 ms | 4.1 / 4.9 · 5.8 / 6.8 · 3.9 / 4.8 ms | ≤ 40 / ≤ 150 |
| S3p pop-up pick | 370.1 / 379.5 ms | 370.7 / 379.7 ms | ≤ 400 |
| S4 background pixel click (effect) | 14.8 / 39.0 ms | 15.2 / 40.1 ms | ≤ 60 / ≤ 200 |
| S5 type 100 characters, set value / keys | 3.3 / 16.7 ms | 3.4 / 15.8 ms | ≤ 20 / ≤ 250 |
| SEL, P1, P2 (worst error), P2r | 20/20, 1600/1600, 1600/1600 (0.00 pt), 200/200 | 5/5, 160/160, 160/160 (0.00 pt), 20/20 | 100% |
| P4, F1 | 0, 0 | 0, 0 | 0 |

**S2.** Phase 3's benches had S2 at 57–63 ms. A split of one observe on the fixture (window 0.4, tree 21, capture
53–60, PNG 1 ms) showed the capture unchanged and the tree read slow. Back to back, a tree read takes 6.5 ms. After a
pause of 60 ms or more, with or without a capture, it takes 23–29 ms: an app in the background, as the fixtures now
are since they open behind other windows, answers its first accessibility read slowly. `acf932a1` starts the capture
before the tree read, so the two overlap.

**The final engine** (`392f100b`) adds activation once per batch and the capture retry. Its quick bench passes
every gate but F1. F1's 187 changes are the person's: their cursor moving and one of their apps quitting. Our
events never move the cursor. S5 landed 20/20 by key events and by value.

**No full bench on the final engine.**
- The first run, on `acf932a1`, was stopped after 16 minutes at the person's request, because the fixture's window
  flickered. The bench writes its timings at the end, so that run has none.
  - The flicker: each background canvas click made the app active and then inactive, 766 times in that log. The
    window redrew its active look and back each time.
  - A worker's batch of n pixel clicks flashed n times, so `920e66dd` holds the activation for the batch: a
    two-click batch now logs one activation where it logged two.
  - In that run, S5's key-event typing never landed. It didn't happen again: S5 landed 20/20 in every quick run
    after it, with and without the hold.
- The second run was stopped after 20 minutes. Another run's test loops had taken the machine to a load average of
  about 90. The fixture's actions, 111–124 ms apart at first, were 200–800 ms apart by then.

The full bench needs a quiet Mac.

**Capture failures.** While the Mac was in use, ScreenCaptureKit sometimes failed to start a window capture
("Failed to start stream due to audio/video capture failure"): 2 of 12 grounding runs, and once for over 350 ms.
`8f7f8a3d` and `392f100b` ask again after 100, 250, 500 and 1000 ms, and the error names the system's reason. After
that, the scripted suite passed 22/22, and the grounding boards 12/12.

## The signed build

`APPLE_SIGNING_IDENTITY="Developer ID Application: …" pnpm tauri:debug-app` signed the dev app and its helper with
the user's Developer ID, with no Keychain prompt. `codesign -dv --verbose=2` on `Brigadier Dev.app` and on
`Contents/Helpers/Brigadier Computer Use.app`: the same TeamIdentifier, the hardened runtime, valid on disk, and
each satisfies a designated requirement anchored on the team (`… and certificate leaf[subject.OU] = "<team>"`).

The same-team peer check: the signed helper serving on a scratch socket (`serve --socket … --token-file …`). One
C client sends the hello, signed two ways:

| client | result | the helper's log |
|---|---|---|
| signed by the team, right token | admitted (the connection stays open) | none |
| signed ad hoc, right token | refused (closed) | `refused a connection: pid … isn't signed by team …` |
| signed by the team, wrong token | refused (closed) | `refused a connection: wrong token` |

## Reproduce

- `cargo test -p brigadier-computer --lib`, `cargo test -p brigadier-mcp-server`, `cargo test -p brigadier-core --lib
  operate`, `cargo test -p brigadier-router`.
- The scripted suite (no model): `brigadier-computer suite scripted <out>`; its result is the reference batches.
- A model run: `python3 tools/computer-suite/run.py <dir> <claude|codex> <run> <tasks…>`, then
  `python3 tools/computer-suite/summarize.py <scripted.json> <run dirs…>`.
- The live handoff: `python3 tools/computer-suite/handoff.py`, and `--recheck <out>` to judge it again.
