# tools/computer-suite: the computer-use suite with a model in the loop

COMPUTER-USE-PLAN §7 and §8 Phase 4. Python 3 stdlib plus `tools/ab`'s daemon tools. Nothing here
touches the installed app, its daemon or its data. Run it from a terminal that holds the
Accessibility and Screen Recording grants: the release helper, started by the dev daemon, inherits
them as the responsible process.

## Setup

    cargo build --release -p brigadier-computer --bins
    (cd apps/desktop && pnpm tauri:debug-app)                      # "Brigadier Dev", for the dev-* tasks
    BRIGD="target/debug/bundle/macos/Brigadier Dev.app/Contents/MacOS/brigadierd" BRIG_SRC=$PWD \
      BRIGADIER_COMPUTER_HELPER=$PWD/target/release/brigadier-computer tools/ab/startd.sh $ROOT/data &
    BRIGD=... tools/ab/startd.sh $ROOT/target/data &                # the dev-build target's own daemon
    tools/ab/setup.py $ROOT/data $ROOT/repo --permission fullAccess # $ROOT/repo: a scratch git repo
    tools/ab/recorder.py $ROOT/data $ROOT/rec &                     # events; transcripts into $ROOT/transcripts
    tools/computer-suite/target.py start $ROOT/target "<Brigadier Dev.app>"

## A run

    tools/computer-suite/run.py $ROOT <claude|codex> <run> [task ...]
    tools/computer-suite/summarize.py <scripted.json> $ROOT/<provider>-<run> ... [--json out]

`scripted.json` is `brigadier-computer suite scripted <out>`'s result: the reference batches.

Each trial is set up by `brigadier-computer suite setup` (or `target.py prepare`). The thread (Sonnet, low) is
asked to call `delegate_task` with kind `operate`, the provider and these exact arguments. The
trial ends when the task has ended and the thread is idle. Then the runner collects:
- the broker's records (`listComputerActions`);
- the worker's report;
- `suite check` (and `target.py check` for the dev build);
- `suite teardown`;
- the attempts' provider, model and effort;
- `turn_usage` of the worker and the thread;
- the model calls and the shortcut audit from the transcripts (`calls.py`).

A shortcut, such as a direct write to the trial's file, a read of the fixture's log, or a call
into Brigadier's socket, fails the trial. The focus monitor (`suite watch`) runs from the first
setup to the last teardown.
