#!/bin/bash
# clone.sh <arm-dir> <task.md> [source-repo] : a throwaway clone for one arm, with `main` itself
# pinned to the task's frozen base, no other branches and no remote, so nothing can be pushed and
# both arms start from the same commit. Source defaults to the user's checkout (read only).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
A=${1:?arm dir}; taskf=${2:?task file}; src=${3:-$HOME/Development/brigadier-ai}
base=$(python3 "$HERE/task.py" "$taskf" base)
[ -e "$A/repo" ] && { echo "clone: $A/repo exists" >&2; exit 1; }
mkdir -p "$A"
git clone -q --no-checkout "$src" "$A/repo"
git -C "$A/repo" checkout -q -B main "$base"
git -C "$A/repo" remote remove origin
for b in $(git -C "$A/repo" for-each-ref --format='%(refname:short)' refs/heads | grep -vx main); do
  git -C "$A/repo" branch -q -D "$b"
done
head=$(git -C "$A/repo" rev-parse main)
[ "$head" = "$base" ] || { echo "clone: main is $head, not $base" >&2; exit 1; }
echo "$A/repo main=$head"
