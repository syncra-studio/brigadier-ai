# tools/ab: the A/B against a plain cmux `/delegator` session

THREAD-PLAN §3 "Measuring". Ported from the 2026-10-05 phaseE tools. Python 3 stdlib, bash and
the app's own Node packages; nothing here touches the installed app, its daemon or its data.

An A/B root (for example `/tmp/brig-ab-1007`) holds one directory per arm:
`<arm>/repo` (the clone), `<arm>/data` (a Brigadier arm's `BRIGADIER_DATA_DIR`), `<arm>/rec`
(recorded events and cards), `<arm>/transcripts` (hard links to the arm's CLI transcripts),
`<arm>/start.json`, and the measurements.

## Tasks

`tasks/t1.md`, `tasks/t2.md`: frozen before any arm runs. Request (verbatim), base, image (kept
outside the repo, named by sha256), done-when, behaviour probes and check commands. `task.py`
reads them.

## One arm

Both kinds:

    tools/ab/clone.sh $AB/<arm> tools/ab/tasks/t1.md          # main pinned to the frozen base
    tools/ab/warm.sh $AB/<arm>/repo [$AB/<warm-arm>/repo]      # node_modules + target, equally warm
    tools/ab/conditions.sh $AB/meter/data > $AB/<arm>/conditions-start.txt

Brigadier (a dev daemon built from the code under test):

    BRIGD=<dev brigadierd> BRIG_SRC=<its source tree> tools/ab/startd.sh $AB/<arm>/data &
    tools/ab/setup.py $AB/<arm>/data $AB/<arm>/repo           # onboarded, Fable hidden, daemon defaults
    tools/ab/recorder.py $AB/<arm>/data $AB/<arm>/rec &       # every event; answers cards like the user
    tools/ab/send.py $AB/<arm>/data $AB/<arm>/repo tools/ab/tasks/t1.md $AB/<arm>   # t0
    tools/ab/reqdone.sh $AB/<arm>                             # request over (then wait for settlement)
    tools/ab/times.py brigadier $AB/<arm>
    tools/ab/armtokens.sh $AB/<arm> <settled_ms>              # turn_usage, cross-checked twice
    ARM=$AB/<arm> APP=<apps/desktop at the daemon's SHA> node tools/ab/replay/run.mjs   # "only Thinking"

/delegator (the user's own unmodified `claude`, Opus 5.5 high, in its own cmux workspace):

    tools/ab/dlg_start.sh $AB/<arm> tools/ab/tasks/t1.md      # t0 = the Enter
    tools/ab/cmx.sh $AB/<arm> read-screen --lines 30          # watch only that tab
    tools/ab/dlg_manifest.py $AB/<arm> <end_ms> > $AB/<arm>/manifest.json
    tools/ab/times.py delegator $AB/<arm> --manifest $AB/<arm>/manifest.json --tip <sha> --branch <name>
    tools/ab/tokens.py $AB/<arm>/manifest.json <t0_ms> <settled_ms> --json $AB/<arm>/tokens.json

Then the independent checks on the result, identical for every arm:

    tools/ab/check.sh $AB/<arm> tools/ab/tasks/t1.md <sha>    # frozen check commands
    tools/ab/probe-app.sh $AB/<arm> <sha>                     # that SHA's real app, own identity
    ...behaviour probes, by the steps in the task file...
    tools/ab/probe-app.sh $AB/<arm> --clean

## Boundaries (both arms)

- **t0**: the request is submitted.
- **Verified landing**: Brigadier, the request's last `landed` step; /delegator, the later of its
  last checking worker going `done` and the result tip reaching the result branch (reflog). Only
  counted when the frozen checks and probes pass on that tip.
- **Final answer**: Brigadier, the request's end; /delegator, the coordinator's last end-of-turn
  message.
- **Settled**: the last activity of anything the request started, reviews that finish after the
  answer included. Tokens are counted from t0 to here.
- Wall-clock time is the number reported. Time without a usage-limit wait is reported beside it,
  and an arm that hit a limit is marked quota-affected.

## Tokens

Per provider: uncached input, cache reads, cache writes and output, kept apart, plus raw and raw
without cache reads. A figure a source doesn't report is "not reported", never 0.
- Brigadier: `brig_tokens.py`, the arm's `routing.sqlite` `turn_usage` rows for its conversation;
  `armtokens.sh` cross-checks them with the transcripts (`brig_manifest.py`, `tokens.py`) and the
  daemon's usage events (`evtokens.py`).
- /delegator: `dlg_manifest.py` lists the coordinator, every worker and successor, every Codex
  session started in one of the run's checkouts, and matches every saved review file to a counted
  session; `tokens.py` counts them (Claude: final usage per message; Codex: last cumulative total,
  cached inside input).

## Files

| File | What |
|---|---|
| `bipc.py` | IPC client (protocol version of the daemon under test) |
| `startd.sh`, `setup.py`, `send.py`, `recorder.py`, `reqdone.sh` | drive a Brigadier arm |
| `dlg_start.sh`, `cmx.sh` | start and watch a /delegator arm's own tab |
| `clone.sh`, `warm.sh`, `conditions.sh` | arm setup and conditions |
| `times.py`, `timeline.py` | boundaries; a readable timeline |
| `brig_tokens.py`, `armtokens.sh`, `brig_manifest.py`, `evtokens.py`, `dlg_manifest.py`, `tokens.py` | tokens |
| `check.sh`, `probe-app.sh` | the independent evaluator's checks and probes |
| `replay/` | "only Thinking" from recorded events, through the app's own code |
| `task.py`, `tasks/` | the frozen tasks |
