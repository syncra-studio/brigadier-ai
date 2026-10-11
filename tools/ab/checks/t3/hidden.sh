#!/bin/bash
# The T3 hidden checks (tasks/t3.md), run by check.sh in a detached worktree at an arm's result
# (the current directory). Neither arm sees these files: they are copied in, run and removed.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
rc=0
cp "$HERE/configuration_page.test.ts" apps/desktop/src/app/settings/t3_configuration_page.test.ts
mkdir -p crates/mcp-server/tests
cp "$HERE/suggest_full_access.rs" crates/mcp-server/tests/t3_suggest_full_access.rs
(cd apps/desktop && node --import ./scripts/test-alias.mjs --test src/app/settings/t3_configuration_page.test.ts) || rc=1
CARGO_INCREMENTAL=0 cargo test --locked -p brigadier-mcp-server --test t3_suggest_full_access || rc=1
rm -f apps/desktop/src/app/settings/t3_configuration_page.test.ts crates/mcp-server/tests/t3_suggest_full_access.rs
rmdir crates/mcp-server/tests 2>/dev/null || true
exit $rc
