#!/usr/bin/env bash
# The live smoke (tools/live-smoke/README.md): builds this tree's daemon and runs the scenarios
# with the real Claude and Codex CLIs, headless, on a scratch data dir it removes after.
#
#   tools/live-smoke/run.sh [smoke.py options]
#
# Skipped, with a note and exit 0, when a CLI isn't installed or signed in. Never runs against
# the installed app, its daemon or its data.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_INCREMENTAL=0

for cli in claude codex; do
  if ! command -v "$cli" >/dev/null 2>&1; then
    echo "live smoke: SKIPPED: $cli isn't installed"
    exit 0
  fi
done

echo "+ cargo build --locked -p brigadier-daemon --bin brigadierd"
cargo build --locked -q -p brigadier-daemon --bin brigadierd
exec python3 tools/live-smoke/smoke.py --daemon "$PWD/target/debug/brigadierd" "$@"
