#!/usr/bin/env bash
# The Linux computer-use live test. On the host it builds the image and reruns itself inside a
# container; inside, it builds brigadier-cu, starts Xvfb, the session and accessibility buses and
# a window manager, and runs live_test.py, which drives the helper over stdio MCP: every step under
# GNOME's window manager (mutter), then the launch and input steps under openbox and xfwm4.
# Exits non-zero on any failure.
#
#   helpers/computer-use/tests/linux-live/run.sh            (host: Docker needed)
#   LINUX_LIVE_CACHE=<dir>   where the cargo registry and build output persist (default: target/)
#   LINUX_LIVE_WMS="<wm>..." the window managers to run under (default: mutter openbox xfwm4)
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
    -e LINUX_LIVE_WMS="${LINUX_LIVE_WMS:-mutter openbox xfwm4}" \
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
# Per window manager: one session bus for it, the apps and the helper (GTK starts the
# accessibility bus on it when an app asks). Each run stops its window manager after itself.
failed=0
for wm in ${LINUX_LIVE_WMS:-mutter openbox xfwm4}; do
  case "$wm" in
    mutter) start="mutter --x11 --replace --sm-disable" ;;
    openbox) start="openbox" ;;
    xfwm4) start="xfwm4 --replace" ;;
    *) echo "unknown window manager $wm" >&2; exit 2 ;;
  esac
  echo "== $wm"
  LINUX_LIVE_WM="$wm" dbus-run-session -- bash -c \
    '$1 >"$LINUX_LIVE_OUT/$2.log" 2>&1 & w=$!; python3 -u "$0/live_test.py"; s=$?; kill $w; wait $w; exit $s' \
    "$here" "$start" "$wm" || failed=1
  if ! xdpyinfo >/dev/null 2>&1; then
    echo "the X server died during the $wm run" >&2
    exit 1
  fi
done
exit "$failed"
