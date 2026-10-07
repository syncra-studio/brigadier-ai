#!/bin/bash
# warm.sh <repo> [warm-repo] : gives a clone what the user's real checkout has: its own
# node_modules and a built target/. With a second, already warm clone, its target/ is copied
# first (APFS clone, `cp -c`) so every arm starts equally warm.
set -euo pipefail
export PATH=$HOME/.cargo/bin:$PATH
unset CARGO_TARGET_DIR
r=${1:?repo}; from=${2:-}
cd "$r"
if [ -n "$from" ]; then rm -rf target; cp -c -R "$from/target" target; fi
echo "== $r $(date +%T)"
pnpm install --frozen-lockfile >/dev/null
cargo build --workspace --all-targets 2>&1 | tail -1
cargo clippy --workspace --all-targets 2>&1 | tail -1
echo "== done $r $(date +%T)"
