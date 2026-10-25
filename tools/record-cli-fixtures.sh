#!/usr/bin/env bash
# D9(c), D25, D29: record only gaps in the installed version's index; never provoke a limit.
set -euo pipefail
case "${1:-}" in claude|codex) ;; *) echo "usage: $0 <claude|codex>" >&2; exit 2 ;; esac
cd "$(dirname "$0")/.."
cargo run --locked -p brigadier-providers --example record_fixtures -- "$1"
