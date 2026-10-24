#!/usr/bin/env bash
set -euo pipefail
cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/.cargo/bin:$PATH"
export CARGO_INCREMENTAL=0
df -h .
cargo build --locked -q -p brigadier-daemon --bin brigadierd -p brigadier-cli --bin brigadier
target_dir=$(cargo metadata --locked --format-version 1 --no-deps |
  python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')
# `run.sh decisions …` runs S22b's decision-survival block; anything else, the recall battery.
script=battery.py
if [ "${1:-}" = work ] || [ "${1:-}" = endurance ]; then
  profile="$1"
  shift
  exec python3 tools/continuity-battery/experiment.py --daemon "$target_dir/debug/brigadierd" --profile "$profile" "$@"
fi
if [ "${1:-}" = p2 ] || [ "${1:-}" = p3 ]; then
  variant="$1"
  shift
  exec python3 tools/continuity-battery/budgets.py --daemon "$target_dir/debug/brigadierd" --variant "$variant" "$@"
fi
if [ "${1:-}" = decisions ] || [ "${1:-}" = updates ]; then
  script="$1.py"
  shift
fi
exec python3 "tools/continuity-battery/$script" --daemon "$target_dir/debug/brigadierd" "$@"
