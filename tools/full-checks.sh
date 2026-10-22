#!/usr/bin/env bash
# The full checks of docs/PLAN.md §10.13, run from anywhere in the checkout.
#
#   tools/full-checks.sh            fmt, generated types, build, clippy, tests, typecheck, lint, app tests
#   tools/full-checks.sh --cross    also Linux and Windows clippy of the cross-buildable crates,
#                                   and Linux and Windows clippy of the desktop crate
#   tools/full-checks.sh --live     then the live smoke with the real Claude and Codex CLIs
#                                   (tools/live-smoke): once, on the integrated tree, before a merge
#
# Safe inside a Brigadier worker: it installs dependencies only when the checkout has none
# (a worker's are already copied in), runs `nice` only when the process isn't low priority
# already (a worker is, and its sandbox refuses `nice`), and writes nothing outside the
# checkout's ignored folders.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/.cargo/bin:$PATH"
# Three preview tests make a BRIGADIER_DATA_DIR in their temp folder, which a preview
# accepts only under /tmp. A session's own temp folder is there already and is the one its
# sandbox lets it write (clang's temporary files among the rest), so it is kept.
case "${TMPDIR:-}" in
  /tmp/?* | /private/tmp/?*) ;;
  *) export TMPDIR=/tmp ;;
esac

cross=false
live=false
for arg in "$@"; do
  case "$arg" in
    --cross) cross=true ;;
    --live) live=true ;;
    *) echo "usage: tools/full-checks.sh [--cross] [--live]" >&2; exit 2 ;;
  esac
done

# SwiftPM has no environment override for its manifest sandbox. Probe whether this
# process can apply one before building the Swift bridges in apple-metal/apple-cf.
# When a session's sandbox refuses it, keep that sandbox and skip only SwiftPM's.
stand_ins=()
cleanup() {
  rm -f ${stand_ins[@]+"${stand_ins[@]}"}
}
trap cleanup EXIT
# The probe denies something: inside a sandbox that allows everything, an identical profile
# still applies.
if [ "$(uname)" = Darwin ] && ! /usr/bin/sandbox-exec -p '(version 1)(allow default)(deny file-write* (literal "/private/var/empty/nested-probe"))' /usr/bin/true 2>/dev/null; then
  echo "+ SwiftPM uses the existing sandbox (--disable-sandbox)"
  BRIGADIER_SWIFT="$(command -v swift)"
  export BRIGADIER_SWIFT
  export PATH="$PWD/tools/nested-swift:$PATH"
fi

nice_prefix=()
# Git Bash's ps has no -o; its nice prints the niceness instead.
niceness="$(ps -o nice= -p $$ 2>/dev/null | tr -d ' ')" || niceness=""
if [ -z "$niceness" ]; then niceness="$(nice)"; fi
if [ "$niceness" -lt 10 ]; then
  nice_prefix=(nice -n 10)
fi

run() {
  echo "+ $*"
  ${nice_prefix[@]+"${nice_prefix[@]}"} "$@"
}

# pnpm marks the workspace's install at its root; the app's own folder holds only links.
if [ ! -f node_modules/.modules.yaml ] || [ ! -e apps/desktop/node_modules/vite ]; then
  run pnpm install --frozen-lockfile
fi

run cargo fmt --all --check
run cargo run --locked -q -p brigadier-ipc --bin gen-ts
echo "+ generated types are up to date"
git diff --exit-code -- apps/desktop/src/ipc/generated
untracked="$(git ls-files --others --exclude-standard -- apps/desktop/src/ipc/generated)"
if [ -n "$untracked" ]; then
  echo "untracked generated files:" >&2
  echo "$untracked" >&2
  exit 1
fi
run pnpm build
run pnpm --filter @brigadier/desktop stage-sidecar --debug
run cargo clippy --locked --workspace --all-targets -- -D warnings
run cargo test --locked --workspace --lib --bins --tests
if [ "$(uname)" = Darwin ]; then
  # The computer-use helper is its own workspace (macOS only); its output sits beside the main
  # build's, where stage-sidecar puts it.
  (
    export CARGO_TARGET_DIR="$PWD/target/computer-use"
    cd helpers/computer-use
    run cargo fmt --all --check
    run cargo clippy --locked --workspace --all-targets -- -D warnings
    run cargo test --locked --workspace
  )
fi
run pnpm typecheck
run pnpm lint
run pnpm test
echo "+ git diff --check"
git diff --check

if $cross; then
  crates=(
    -p brigadier-core -p brigadier-store -p brigadier-ipc -p brigadier-providers
    -p brigadier-brain -p brigadier-index -p brigadier-router
    -p brigadier-git -p brigadier-registry -p brigadier-sandbox -p brigadier-mcp-server
    -p brigadier-browser-data
  )
  run cargo-zigbuild clippy --locked --target x86_64-unknown-linux-gnu "${crates[@]}" \
    --all-targets -- -D warnings
  run cargo clippy --locked --target x86_64-pc-windows-gnu "${crates[@]}" \
    --all-targets -- -D warnings
  # The desktop crate's Windows-only code. Its build wants the daemon's Windows sidecar beside
  # it; none is built here, so an empty stand-in takes its place (binaries/ is ignored) and goes
  # again after.
  stand_in() {
    if [ ! -e "$1" ]; then
      mkdir -p "$(dirname "$1")"
      touch "$1"
      stand_ins+=("$1")
    fi
  }
  stand_in apps/desktop/src-tauri/binaries/brigadierd-x86_64-pc-windows-gnu.exe
  run cargo clippy --locked --target x86_64-pc-windows-gnu -p brigadier-desktop \
    --all-targets -- -D warnings
  # The desktop crate's Linux-only code, the same way. Its GTK and WebKit libraries aren't here,
  # and clippy links nothing, so a pkg-config that finds every library with no flags stands in.
  stand_in apps/desktop/src-tauri/binaries/brigadierd-x86_64-unknown-linux-gnu
  mkdir -p target/linux-pkg-config
  printf '#!/bin/sh\nfor a in "$@"; do [ "$a" = --modversion ] && echo 99.0; done\nexit 0\n' \
    > target/linux-pkg-config/pkg-config
  chmod +x target/linux-pkg-config/pkg-config
  run env PKG_CONFIG="$PWD/target/linux-pkg-config/pkg-config" PKG_CONFIG_ALLOW_CROSS=1 \
    cargo-zigbuild clippy --locked --target x86_64-unknown-linux-gnu -p brigadier-desktop \
    --all-targets -- -D warnings
fi

# Last, as it spends tokens: only once everything else passed. It writes only under /tmp and
# removes what it made there.
if $live; then
  run tools/live-smoke/run.sh
fi

echo "full checks passed"
