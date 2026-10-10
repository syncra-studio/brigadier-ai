#!/usr/bin/env bash
# Opens this checkout's dev build, Brigadier Dev.app, never the installed Brigadier.app.
#
#   tools/open-dev-app.sh                 its own data, /tmp/brigadier-dev
#   tools/open-dev-app.sh <data-dir>      another data folder (a scratch one, say)
#   tools/open-dev-app.sh --background …  opens it without bringing it to the front
#
# Build it first, signed so its computer-use permissions survive rebuilds:
#   cd apps/desktop && APPLE_SIGNING_IDENTITY="Developer ID Application: …" pnpm tauri:debug-app
# A data folder only takes effect when the app isn't already running; quit it first to switch.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
app="$root/target/debug/bundle/macos/Brigadier Dev.app"

flags=()
if [[ "${1:-}" == "--background" ]]; then
  flags+=(-g)
  shift
fi

if [[ ! -d "$app" ]]; then
  echo "No dev build at $app" >&2
  echo "Build it: cd apps/desktop && APPLE_SIGNING_IDENTITY=\"Developer ID Application: …\" pnpm tauri:debug-app" >&2
  exit 1
fi

if [[ $# -gt 0 ]]; then
  mkdir -p "$1"
  data="$(cd "$1" && pwd)"
  exec /usr/bin/open ${flags[@]+"${flags[@]}"} -a "$app" --args --brigadier-data-dir "$data"
fi
exec /usr/bin/open ${flags[@]+"${flags[@]}"} -a "$app"
