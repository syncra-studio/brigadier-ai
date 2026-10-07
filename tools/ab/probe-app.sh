#!/bin/bash
# probe-app.sh <arm-dir> <sha>      builds that SHA's desktop app and daemon in a detached worktree
#                                   of the arm's clone and starts it in the background under its own
#                                   identity (ai.brigadier.ab-<arm>) and data dir (<arm>/probe-data).
# probe-app.sh <arm-dir> --stop     stops the app it started (its own process group only).
# probe-app.sh <arm-dir> --clean    also removes the worktree, the data dir and the identity's
#                                   WebKit data, caches and defaults.
# Never the installed app, its identity (ai.brigadier.app) or its data. `tauri dev -c` merges the
# configs in order (checked: `pnpm tauri dev --help`, Tauri CLI 2).
set -euo pipefail
export PATH=$HOME/.cargo/bin:$PATH; unset CARGO_TARGET_DIR
A=$(cd "${1:?arm dir}" && pwd); what=${2:?sha|--stop|--clean}
arm=$(basename "$A" | tr -c 'A-Za-z0-9\n' '-')
id="ai.brigadier.ab-$arm"; W="$A/probe-wt"; D="$A/probe-data"
stop() {
  if [ -f "$A/probe.pid" ]; then
    pgid=$(cat "$A/probe.pid")
    kill -TERM -- "-$pgid" 2>/dev/null || true
    sleep 3; kill -KILL -- "-$pgid" 2>/dev/null || true
    rm -f "$A/probe.pid"
  fi
  if [ -S "$D/run/brigadierd.sock" ] && [ -x "$W/target/debug/brigadierd" ]; then
    "$W/target/debug/brigadierd" quit --data-dir "$D" 2>/dev/null || true
  fi
}
case "$what" in
  --stop) stop; exit 0 ;;
  --clean)
    stop
    [ -d "$W" ] && git -C "$A/repo" worktree remove --force "$W"
    rm -rf "$D" "$HOME/Library/WebKit/$id" "$HOME/Library/Caches/$id" "$HOME/Library/HTTPStorages/$id" \
      "$HOME/Library/Application Support/$id" "$HOME/Library/Saved Application State/$id.savedState"
    defaults delete "$id" >/dev/null 2>&1 || true
    exit 0 ;;
esac
sha=$what
if [ ! -d "$W" ]; then
  git -C "$A/repo" worktree add -f --detach "$W" "$sha" >/dev/null
  cp -c -R "$A/repo/target" "$W/target" 2>/dev/null || true
else
  git -C "$W" checkout -q --detach "$sha"
fi
cd "$W"
pnpm install --frozen-lockfile >/dev/null
cargo build -q -p brigadier-daemon --bin brigadierd
pnpm --filter @brigadier/desktop stage-sidecar --debug >/dev/null
mkdir -p "$D"
set -m
( exec env BRIGADIER_DATA_DIR="$D" pnpm tauri dev --config src-tauri/tauri.dev.conf.json \
    --config "{\"identifier\":\"$id\",\"productName\":\"Brigadier AB $arm\"}" > "$A/probe-app.log" 2>&1 ) &
echo $! > "$A/probe.pid"
echo "started $id at $sha (pgid $(cat "$A/probe.pid")), log $A/probe-app.log"
