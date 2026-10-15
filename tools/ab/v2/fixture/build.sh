#!/bin/bash
# build.sh <dest-repo> [node_modules-cache-dir] : the frozen V2 fixture repository, byte for byte the
# same every time: V3's fixture (tools/ab/v3/fixture/build.sh, its EXPECTED hashes) plus one commit
# on main, "Add the staging import check" (overlay/: scripts/import-check.mjs, and .gitignore keeping
# .import-feed/ out of git). The feed itself (feed/) is never committed: run.py copies it into the
# session's workspace for the battery's check and file-read steps and deletes it after them, so what
# it said can only be remembered, not read again.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
R=${1:?dest repo}; CACHE=${2:-}
"$HERE/../../v3/fixture/build.sh" "$R" $CACHE >/dev/null
(cd "$HERE/overlay" && tar -cf - .) | tar -xf - -C "$R"
git -C "$R" add -A
GIT_AUTHOR_DATE=2026-01-05T00:00:00Z GIT_COMMITTER_DATE=2026-01-05T00:00:00Z GIT_AUTHOR_NAME=tally \
  GIT_AUTHOR_EMAIL=tally@localhost GIT_COMMITTER_NAME=tally GIT_COMMITTER_EMAIL=tally@localhost \
  git -C "$R" -c commit.gpgsign=false commit -q -m "Add the staging import check"
main=$(git -C "$R" rev-parse main)
want=$(sed -n 's/^main=//p' "$HERE/EXPECTED" 2>/dev/null || true)
if [ -n "$want" ] && [ "$main" != "$want" ]; then
  echo "build.sh: main=$main, expected $want" >&2; exit 1
fi
echo "main=$main"
