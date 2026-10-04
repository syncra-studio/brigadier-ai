#!/usr/bin/env bash
# The full checks of docs/PLAN.md §10.13, run from anywhere in the checkout.
#
#   tools/full-checks.sh            fmt, generated types, build, clippy, tests, typecheck, lint, app tests
#   tools/full-checks.sh --cross    also Linux and Windows clippy of the cross-buildable crates
#
# Safe inside a Brigadier worker: it installs dependencies only when the checkout has none
# (a worker's are already copied in), runs `nice` only when the process isn't low priority
# already (a worker is, and its sandbox refuses `nice`), and writes nothing outside the
# checkout's ignored folders.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"
export PATH="$HOME/.cargo/bin:$PATH"

cross=false
for arg in "$@"; do
  case "$arg" in
    --cross) cross=true ;;
    *) echo "usage: tools/full-checks.sh [--cross]" >&2; exit 2 ;;
  esac
done

nice_prefix=()
if [ "$(ps -o nice= -p $$ | tr -d ' ')" -lt 10 ]; then
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
run pnpm typecheck
run pnpm lint
run pnpm test
echo "+ git diff --check"
git diff --check

if $cross; then
  crates=(
    -p brigadier-core -p brigadier-store -p brigadier-ipc -p brigadier-providers
    -p brigadier-brain -p brigadier-index -p brigadier-router -p brigadier-review
    -p brigadier-git -p brigadier-registry -p brigadier-sandbox -p brigadier-mcp-server
  )
  run cargo-zigbuild clippy --locked --target x86_64-unknown-linux-gnu "${crates[@]}" \
    --all-targets -- -D warnings
  run cargo clippy --locked --target x86_64-pc-windows-gnu "${crates[@]}" \
    --all-targets -- -D warnings
fi

echo "full checks passed"
