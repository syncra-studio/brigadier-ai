#!/bin/bash
# build.sh <dest-repo> [node_modules-cache-dir] : the frozen V3 fixture repository, byte for byte the
# same every time (fixed authors and dates, so the commit hashes are fixed too; EXPECTED holds them).
#   main:          "tally" (base/, the bench fixture at 11a0770a:bench/fixture, tree 7750dd09)
#                  + "Add the rates draft and a returns cart" (overlay/main: docs/rates-draft.md with
#                  one planted wrong rate for A6, examples/return.json with a zero quantity for x1)
#   wip/discounts: two commits ahead of main (overlay/wip-1: src/cart.ts, overlay/wip-2:
#                  test/cart.test.ts), for s1
# No remote, so nothing can be pushed. With a cache dir, node_modules is an APFS clone of one
# `npm ci` of the committed lockfile (as the bench does), so `npm run check` works in the repo.
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
R=${1:?dest repo}; CACHE=${2:-}
[ -e "$R" ] && { echo "build.sh: $R exists" >&2; exit 1; }
mkdir -p "$R"
commit() {  # commit <iso-date> <message>
  git -C "$R" add -A
  GIT_AUTHOR_DATE="$1" GIT_COMMITTER_DATE="$1" GIT_AUTHOR_NAME=tally GIT_AUTHOR_EMAIL=tally@localhost \
    GIT_COMMITTER_NAME=tally GIT_COMMITTER_EMAIL=tally@localhost git -C "$R" -c commit.gpgsign=false commit -q -m "$2"
}
(cd "$HERE/base" && tar -cf - .) | tar -xf - -C "$R"
git -C "$R" init -q -b main
commit 2026-01-01T00:00:00Z "tally"
(cd "$HERE/overlay/main" && tar -cf - .) | tar -xf - -C "$R"
commit 2026-01-02T00:00:00Z "Add the rates draft and a returns cart"
git -C "$R" checkout -q -b wip/discounts
(cd "$HERE/overlay/wip-1" && tar -cf - .) | tar -xf - -C "$R"
commit 2026-01-03T00:00:00Z "Add percentOff for cart discounts"
(cd "$HERE/overlay/wip-2" && tar -cf - .) | tar -xf - -C "$R"
commit 2026-01-04T00:00:00Z "Test percentOff"
git -C "$R" checkout -q main
main=$(git -C "$R" rev-parse main); wip=$(git -C "$R" rev-parse wip/discounts)
want_main=$(sed -n 's/^main=//p' "$HERE/EXPECTED"); want_wip=$(sed -n 's/^wip=//p' "$HERE/EXPECTED")
if [ -n "$want_main" ] && { [ "$main" != "$want_main" ] || [ "$wip" != "$want_wip" ]; }; then
  echo "build.sh: main=$main wip=$wip, expected main=$want_main wip=$want_wip" >&2; exit 1
fi
if [ -n "$CACHE" ]; then
  if [ ! -d "$CACHE/node_modules" ]; then
    mkdir -p "$CACHE"; cp "$HERE/base/package.json" "$HERE/base/package-lock.json" "$CACHE/"
    (cd "$CACHE" && npm ci --ignore-scripts --no-audit --no-fund >/dev/null)
  fi
  cp -c -R "$CACHE/node_modules" "$R/node_modules"
fi
echo "main=$main"
echo "wip=$wip"
