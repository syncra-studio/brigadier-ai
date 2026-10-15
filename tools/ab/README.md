# tools/ab: the A/B against a plain cmux `/delegator` session

THREAD-PLAN §3 "Measuring". Ported from the 2026-10-05 phaseE tools. Python 3 stdlib, bash and
the app's own Node packages; nothing here touches the installed app, its daemon or its data.

An A/B root (for example `/tmp/brig-ab-1007`) holds one directory per arm:
`<arm>/repo` (the clone), `<arm>/data` (a Brigadier arm's `BRIGADIER_DATA_DIR`), `<arm>/rec`
(recorded events and cards), `<arm>/transcripts` (hard links to the arm's CLI transcripts),
`<arm>/start.json`, and the measurements.

## Tasks

`tasks/t1.md`, `tasks/t2.md`, `tasks/t3.md`: frozen before any arm runs (t3: the session-fixes token A/B, `docs/evidence/2026-10-11-session-fixes-tokens.md`). Request (verbatim), base, image (kept
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
    tools/ab/brainwait.py $AB/<arm>                           # the project's Brain jobs have ended
    tools/ab/send.py $AB/<arm>/data $AB/<arm>/repo tools/ab/tasks/t1.md $AB/<arm>   # t0
    tools/ab/reqdone.sh $AB/<arm>                             # request over (then wait for settlement)
    tools/ab/times.py brigadier $AB/<arm>
    tools/ab/armtokens.sh $AB/<arm> <settled_ms> [--also DIR] # turn_usage, cross-checked twice; exit 3 if incomplete
    ARM=$AB/<arm> APP=<apps/desktop at the daemon's SHA> node tools/ab/replay/run.mjs   # "only Thinking"

/delegator (the user's own unmodified `claude`, Opus 5.5 high, in its own cmux workspace):

    tools/ab/dlg_start.sh $AB/<arm> tools/ab/tasks/t1.md      # t0 = the Enter
    tools/ab/cmx.sh $AB/<arm> read-screen --lines 30          # watch only that tab
    tools/ab/dlg_manifest.py $AB/<arm> <end_ms> > $AB/<arm>/manifest.json
    tools/ab/times.py delegator $AB/<arm> --manifest $AB/<arm>/manifest.json --tip <sha> --branch <name>
    tools/ab/tokens.py $AB/<arm>/manifest.json <t0_ms> <settled_ms> --json $AB/<arm>/tokens.json

Both kinds, from before t0 to settlement (phase 6): `tools/ab/snap.py $AB/<arm> &` hard-links every
session file written on the machine, because a worker's own dev daemon (or the arm's daemon) deletes
its sessions at cleanup; `dlg_manifest.py`, `audit.py` and `tokens.py` (`KEEP=$AB/<arm>/snap`) read it.

Both kinds, after settlement (phase 6):

    tools/ab/audit.py $AB/<arm> <settled_ms>      # every session started in the window: counted, or explained in audit-other.json
    KEEP=$AB/<arm>/transcripts tools/ab/cant.py $AB/<arm>/manifest.json   # "I can't" candidates, judged by hand

Answers to either arm follow `POLICY.md`: `recorder.py` for Brigadier, `dlg_say.sh` for /delegator
(logged in `<arm>/interventions.jsonl`).

Then the independent checks on the result, identical for every arm:

    tools/ab/check.sh $AB/<arm> tools/ab/tasks/t1.md <sha>    # frozen check commands
    tools/ab/probe-app.sh $AB/<arm> <sha>                     # that SHA's real app, own identity
    ...behaviour probes, by the steps in the task file...
    tools/ab/probe-app.sh $AB/<arm> --clean

## V3 small asks and V1 accounting (`v3/`)

BUILD-PLAN A0 / PLAN P0.2. 12 frozen asks (4 code questions, 4 tiny edits, 2 status, 2 "explain this error") and
the deliverable cases A1, A2, A6, in `v3/asks/*.md` (request verbatim, then the exact scoring rule as JSON), on the
frozen fixture `v3/fixture/` (the bench's tally at `11a0770a:bench/fixture`, plus planted state; `build.sh` makes
the same commits every time, `EXPECTED`). Edits are judged by hidden checks (`v3/checks/`) on the session
workspace as the run left it.

    export BRIGD=<dev brigadierd> BRIG_SRC=<its source tree>
    v3/run.py template --root /tmp/v3-X --tag X --seed tools/ab/v3/seed/brain.sqlite   # frozen Brain seed
    v3/run.py ask --root /tmp/v3-X --ask q1 --regime warm --brain seeded               # one attempt
    v3/run.py set --root /tmp/v3-X --asks q1,e1,s2,x2 --regime warm --brain seeded --reruns 3
    v3/run.py ab --roots A=/tmp/v3-A,B=/tmp/v3-B --asks ... --regime cold --brain empty --reruns 3
    v3/summary.py /tmp/v3-X [/tmp/v3-Y]                # rescored; one table per regime x Brain, completion first
    v3/selftest.py /tmp/v3-X                            # scorer rules, and rescoring is byte-identical
    v3/run.py cleanup --root /tmp/v3-X [--export DIR]   # stops, deletes its Codex threads and Claude folders

- **One arm per root**, its own daemon (its IPC protocol pinned in the root's `build-src/`) and data dir, restored from the template before every attempt (same
  absolute paths, same Brain). `--brain empty` uses the template with the project's Brain emptied (one inert
  skeleton-origin node stays so the daemon doesn't rerun its skeleton pass). Brain enrichment is off.
- **V1 regimes:** `warm` runs one warm-up session (same flags) just before the attempt; `cold` waits until no
  session of these roots ended within the last hour. `ab` alternates the builds' order per ask and rerun.
- **Every attempt is kept** (`runs/<id>/`, `results.jsonl`); a harness error counts as not completed.
- **Measured** (`v3/score.py`, from the run dir alone): completion, pass and why, model calls (Claude assistant
  message ids, Codex token_count steps; per role), thread turns, worker starts, time to the final answer, usage
  through settlement (`v3/v3tokens.py`: `brig_tokens.py` + `tokens.py` over the conversation's own sessions,
  reconciled as `armtokens.sh` does; all vendors, cache reads apart), interventions and background work.

## V2 continuity battery (`v2/`)

BUILD-PLAN D1.0 / PLAN §0 V2. One frozen battery, `v2/battery/thread.md`: several requests in one session. `r1`–`r6`
plant every item:
- a standing constraint, and its exception;
- a decision with its reason;
- a commit hash, a path:line and a number with units;
- a failed check with its error line;
- a ruled-out approach;
- an item only in the thread's own tool output;
- a false `learned` line in a worker report (planted by the daemon's debug `debugPlantLearned`).

Fillers then push the plants far out of the verbatim tail. Two edits act on the constraint, its exception and the
ruled-out approach, and a probe asks for every item.

- **Fixture:** `v2/fixture/build.sh` is V3's fixture plus `scripts/import-check.mjs`. Its feed (`v2/fixture/feed/`)
  is copied into the session's workspace for `r4`–`r5` and deleted after them, so what it said can only be
  remembered.
- **Seams:** forced at least twice between the last plant and the probe (`--seam`):
  - `size`: `BRIGADIER_REBIRTH_TOKENS`, passed on by `startd.sh`'s `BRIG_DEBUG_ENV`;
  - `restart`: the daemon is SIGKILLed mid-request and started again;
  - `kill`: the thread's CLI is SIGKILLed mid-request;
  - `context`, `auth`, `billing`: the error is injected mid-turn (`debugInjectError`);
  - `limit`: a usage limit is injected (`debugInjectLimit`);
  - `none`: no seam, as a control.

    export BRIGD=<dev brigadierd> BRIG_SRC=<its source tree>
    v2/run.py template --root /tmp/v2-X --provider claude --tag X    # or --provider codex (thread and workers)
    v2/run.py battery --root /tmp/v2-X --seam size [--rebirth-tokens 45000]
    v2/run.py set --root /tmp/v2-X --seam size --reruns 3            # D1.0: three reruns, one build
    v2/summary.py /tmp/v2-X        # rescored; per item and run; exit 1 when the reruns disagree on an item
    v2/selftest.py [/tmp/v2-X]     # the scorer's rules on synthetic runs, and byte-identical rescoring
    v2/run.py cleanup --root /tmp/v2-X [--export DIR]

**Scoring** (`v2/score.py`, from the run dir alone):
- **Item verdicts:** each item is `pass`, `fail`, `unknown` (lost, and the thread said it didn't know) or
  `not_tested`.
  - An item passes when the probe recalls it exactly, the edits that depend on it act on it correctly (the hidden
    checks' `RESULT` lines), and, for the false line, no rebirth briefing or worker brief carries it unlabelled.
  - An item is `not_tested` when its source was still in the verbatim tail of the last briefing before a step
    that needs it.
- **The run:** a run is `valid` only when its seam happened at least twice.
- **Also reported:** the "unknown instead of a guess" score (unknown ÷ (unknown + guessed) over the lost items),
  every briefing's size and sections, and the probe's tool calls.

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
  daemon's usage events (`evtokens.py`). The arm's number is `turn_usage` plus the Codex child
  threads the daemon didn't meter, which `armtokens.sh` adds from their rollouts. A daemon from
  phase 2 on meters a worker's or thread's auto-review threads itself (step `guardian`, the child
  named in `child_thread`), so those are never added twice.
- Unknown usage (no transcript, no usage record, an unmatched review file) or an unexplained
  difference makes `tokens.py`/`armtokens.sh` write `"complete": false` and exit 3. Codex
  descendants (`parent_thread_id`) are followed recursively in both manifests; `--also DIR` adds a
  worker's dev-app data dir in both.
- /delegator: `dlg_manifest.py` lists the coordinator, every worker and successor, every Codex
  session started in one of the run's checkouts, and matches every saved review file to a counted
  session; `tokens.py` counts them (Claude: final usage per message; Codex: last cumulative total,
  cached inside input).

## Files

| File | What |
|---|---|
| `bipc.py` | IPC client (protocol version of the daemon under test) |
| `startd.sh`, `setup.py`, `recorder.py`, `brainwait.py`, `send.py`, `reqdone.sh` | drive a Brigadier arm |
| `dlg_start.sh`, `cmx.sh`, `dlg_say.sh` | start, watch and answer a /delegator arm's own tab |
| `POLICY.md` | the one response policy for both arms |
| `audit.py`, `cant.py` | every session in an arm's window accounted for; "I can't" candidates |
| `clone.sh`, `warm.sh`, `conditions.sh` | arm setup and conditions |
| `times.py`, `timeline.py` | boundaries; a readable timeline |
| `worker_tokens.py` | per-worker calls, contexts, tokens, Brain use and checks; per-provider totals and elapsed (SESSION-FIXES-PLAN §3 Q6.7) |
| `brig_tokens.py`, `armtokens.sh`, `brig_manifest.py`, `evtokens.py`, `dlg_manifest.py`, `tokens.py` | tokens |
| `check.sh`, `probe-app.sh` | the independent evaluator's checks and probes |
| `replay/` | "only Thinking" from recorded events, through the app's own code |
| `task.py`, `tasks/` | the frozen tasks |
| `brainseed.py` | a Brain seed cut at a moment, read-only from its source, with a provenance audit (t3) |
| `checks/t3/` | t3's hidden checks, copied into an arm's result by `check.sh` (`$AB_TOOLS`) |
