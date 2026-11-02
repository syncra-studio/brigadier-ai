#!/usr/bin/env bash
# The Linux computer-use live test. On the host it builds the image and reruns itself inside a
# container; inside, it builds brigadier-cu, starts Xvfb, GNOME's window manager (mutter) and the session and
# accessibility buses, and runs live_test.py, which drives the helper over stdio MCP.
# Exits non-zero on any failure.
#
#   helpers/computer-use/tests/linux-live/run.sh            (host: Docker needed)
#   LINUX_LIVE_CACHE=<dir>   where the cargo registry and build output persist (default: target/)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
repo="$(cd "$here/../../../.." && pwd)"

if [ "${1:-}" != --inside ]; then
  cache="${LINUX_LIVE_CACHE:-$repo/target/linux-live}"
  mkdir -p "$cache/cargo-registry" "$cache/target" "$cache/out"
  image=brigadier-cu-linux-live
  docker build -q -t "$image" "$here" >/dev/null
  exec docker run --rm --init --user "$(id -u):$(id -g)" \
    -e HOME=/tmp/home -e CARGO_TARGET_DIR=/cache/target -e LINUX_LIVE_OUT=/cache/out \
    -v "$repo:/src:ro" -v "$cache:/cache" -v "$cache/cargo-registry:/opt/cargo/registry" \
    "$image" /src/helpers/computer-use/tests/linux-live/run.sh --inside
fi

mkdir -p "$HOME" "${LINUX_LIVE_OUT:=/tmp/linux-live-out}"
export LINUX_LIVE_OUT
getent passwd "$(id -u)" >/dev/null || echo "tester:x:$(id -u):$(id -g)::$HOME:/bin/bash" >>/etc/passwd
cd /src/helpers/computer-use
cargo build --locked --release -p brigadier-cu
export BRIGADIER_CU_BIN="$CARGO_TARGET_DIR/release/brigadier-cu"

export DISPLAY=:99
Xvfb "$DISPLAY" -screen 0 1280x800x24 -nolisten tcp >"$LINUX_LIVE_OUT/xvfb.log" 2>&1 &
xvfb=$!
trap 'kill "$xvfb" 2>/dev/null || true' EXIT
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
# One session bus for the window manager, the apps and the helper; GTK starts the accessibility
# bus on it when an app asks.
dbus-run-session -- bash -c \
  'mutter --x11 --replace --sm-disable >"$LINUX_LIVE_OUT/mutter.log" 2>&1 & exec python3 -u "$0/live_test.py"' "$here"
