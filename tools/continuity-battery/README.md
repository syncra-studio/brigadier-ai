# Continuity battery v0

From the repository root, with Python 3, Rust and a signed-in Claude Code CLI:

```sh
tools/continuity-battery/run.sh
```

This builds the checkout's debug daemon and CLI, then writes
`tools/continuity-battery/reports/<date>-claude-seam.md`. It uses a separate headless daemon,
scratch data and throwaway git project. It never connects to the installed app. The
model is `opus`, high effort. This spends real subscription tokens. The whole run is
bounded to 25 minutes, each request to 100 seconds, and filler requests to 18.

The script measures the seam, which is always on for sessions; it supplies no new continuity path.
Twelve items are planted before inert filler requests. The final request recalls them,
and a preceding edit applies the standing documentation rule and its correction.
The false claim is a labelled injected worker report, checked against the fixture's
plain `writeFileSync`; no worker or false knowledge node is created. The support
ticket exists only in a one-time command's output after its script is removed. A
second command fails with more than 250 KB of output and a marker in the middle.
One user item is deliberately excluded from the ledger.

`BRIGADIER_SEAM_TOKENS=43750` (the `--tokens` default) sets the debug seam threshold. The script
asks for at least nine filler turns and two closed seams, then injects the existing debug
context-window error, kills its own session's Claude CLI, restarts its own daemon at a
foreground-command wait point, and injects a one-minute usage limit. It waits for that window to
reset before probing Claude. If a forced fault fails to happen, the report says so; a successful
injection alone is not proof of recovery. Repeated answers aren't fed back between faults.

The report reads each seam from the session's `record/seam.json`, its note from
`record/handoff/<n>.md` and what the successor started from from `<n>-start.md`. The battery plants
into, excludes from and measures the session's ledger, `record/ledger.md` (the one the standing
instructions name), not a ledger in the repo.

Overrides pass through `run.sh`, for example `--tokens 75000 --max-fillers 24`.
`--report PATH` chooses the text report. `--evidence /absolute/private/path.json`
keeps detailed input/output, provider events, hand-offs and metering outside git.
The evidence contains the planted answers and is written before scratch cleanup.
Without this option, only the text report remains. Exit 0 means the probe ran,
even when recall fails; exit 2 means the run ended before the probe. This is a report,
never a gate. A report-only formatting pass can import `report()` from `battery.py`
and pass it saved JSON evidence; it makes no model calls.

The run deletes its session and project through daemon IPC, shuts down its own
daemon, kills only process groups it created and removes its own scratch root.
Remaining folder trust keeps that root for cleanup through its daemon records and
is reported explicitly. No credentials are entered. A CLI that is missing or signed
out produces an incomplete report, with no attempt to authenticate.

## Decision survival (S22b)

```sh
tools/continuity-battery/run.sh decisions --tokens 60000
```

`decisions.py` measures `research/CONTINUITY.md` §3.7 with the same harness and writes
`reports/<date>-claude-decisions.md`. It plants ten items the way a user gives them, with no word
about the ledger: three rules (one later corrected), a principle with its defining sentence and
two examples, two decisions with reasons, one decision the thread takes itself, an open question,
an open promise and a false worker claim. It forces two seams by size and one death with no note,
then, restating nothing: T4 "where are we?" (both open items raised?), T1 a coding request the
rules and decisions bear on (scored from the file and by running its functions with `node`), T2 a
new case only the principle decides, T3 a worker brief scored by `claude -p --model sonnet` on a
fixed rubric (cited, stated, missing, contradicted, and additions not marked "proposed"), T5 the
thread's own decision told and marked "I decided", and T6 recall when asked. It is a report, never
a gate. The plain-CLI arm and Codex belong to S25 (L9). `decisions.rescore(evidence, report)` scores a
saved run again with no model calls (it reuses the T3 judge's answer).

This is a small Claude baseline for S21, not the nightly battery, both-provider A/B,
400k-token comparison, or weekly sample of real lineages (S25/L4). Shared-session
faults measure cumulative survival; they don't isolate each seam's causal effect.
The runtime's token counts and reported dollar costs are measurements, not prices
inferred from a model name.
