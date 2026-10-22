# Continuity battery v0

From the repository root, with Python 3, Rust and a signed-in Claude Code CLI:

```sh
tools/continuity-battery/run.sh
```

This builds the checkout's debug daemon and CLI, then writes
`tools/continuity-battery/reports/<date>-claude.md`. It uses a separate headless daemon,
scratch data and throwaway git project. It never connects to the installed app. The
model is `opus`, high effort. This spends real subscription tokens. The whole run is
bounded to 25 minutes, each request to 100 seconds, and filler requests to 18.

The script measures today's `manager/rebirth.rs`; it supplies no new continuity path.
Twelve items are planted before inert filler requests. The final request recalls them,
and a preceding edit applies the standing documentation rule and its correction.
The false claim is a labelled injected worker report, checked against the fixture's
plain `writeFileSync`; no worker or false knowledge node is created. The support
ticket exists only in a one-time command's output after its script is removed. A
second command fails with more than 250 KB of output and a marker in the middle.
One user item is deliberately excluded from the ledger.

`BRIGADIER_REBIRTH_TOKENS=35000` prepares the existing note fork, with its normal
swap threshold a quarter higher. `BRIGADIER_BRIEFING_TOKENS=4000` keeps the baseline
short. The script asks for at least nine filler turns and two recorded size rebirths,
then injects the existing debug context-window error, kills its own session's Claude
CLI, restarts its own daemon at a foreground-command wait point, and injects a
one-minute usage limit. It waits for that window to reset before probing Claude.
If a seam fails to happen, the report says so; a successful injection alone is not
proof of recovery. Repeated answers aren't fed back between seams.

Overrides pass through `run.sh`, for example `--tokens 60000 --max-fillers 24`.
`--report PATH` chooses the text report. `--evidence /absolute/private/path.json`
keeps detailed input/output, provider events, briefings and metering outside git.
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

This is a small Claude baseline for S21, not the nightly battery, both-provider A/B,
400k-token comparison, or weekly sample of real lineages (S25/L4). Shared-session
faults measure cumulative survival; they don't isolate each seam's causal effect.
The runtime's token counts and reported dollar costs are measurements, not prices
inferred from a model name.
