#!/usr/bin/env bash
# x_server_alive with a fake xdpyinfo: one failed answer isn't a death, and a server whose process
# is gone is dead whatever xdpyinfo says. run.sh runs this before it starts the X server.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
source "$here/x_server.sh"

fake="$(mktemp -d)"
trap 'rm -rf "$fake"' EXIT
cat >"$fake/xdpyinfo" <<'FAKE'
#!/bin/sh
# Fails while the count in $FAILS_LEFT's file is above 0; -1 means it always fails.
left="$(cat "$FAILS_LEFT")"
[ "$left" = 0 ] && exit 0
[ "$left" = -1 ] || echo "$((left - 1))" >"$FAILS_LEFT"
exit 1
FAKE
chmod +x "$fake/xdpyinfo"
export PATH="$fake:$PATH" FAILS_LEFT="$fake/fails-left"

failed=0
check() { # <what> <expected: alive|dead> <pid> <failures before xdpyinfo answers>
  echo "$4" >"$FAILS_LEFT"
  if x_server_alive "$3"; then got=alive; else got=dead; fi
  if [ "$got" = "$2" ]; then echo "ok: $1"; else echo "FAILED: $1: expected $2, got $got"; failed=1; fi
}

sleep 60 &
running=$!
sleep 0 &
gone=$!
wait "$gone"

check "a server that answers is alive" alive "$running" 0
check "a server that misses one answer is alive" alive "$running" 1
check "a server that never answers is dead" dead "$running" -1
check "a server whose process is gone is dead, even if something answers" dead "$gone" 0

kill "$running" 2>/dev/null
exit "$failed"
