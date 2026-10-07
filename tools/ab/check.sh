#!/bin/bash
# check.sh <arm-dir> <task.md> <sha> : the independent evaluator's frozen check commands on an
# arm's result, in a detached worktree of the arm's clone at <sha> (warmed like the arm). Every
# command's output goes to <arm-dir>/checks/<n>.log; the summary to <arm-dir>/checks/summary.txt.
set -uo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
A=$(cd "${1:?arm dir}" && pwd); taskf=${2:?task}; sha=${3:?sha}
export PATH=$HOME/.cargo/bin:$PATH; unset CARGO_TARGET_DIR
W="$A/check-wt"; out="$A/checks"; mkdir -p "$out"
git -C "$A/repo" worktree add -f --detach "$W" "$sha" >/dev/null
cp -c -R "$A/repo/target" "$W/target" 2>/dev/null || true
base=$(python3 "$HERE/task.py" "$taskf" base)
echo "sha $sha base $base" > "$out/summary.txt"
git -C "$W" diff --name-only "$base" "$sha" > "$out/changed-files.txt"
n=0
python3 -c "import sys,json; sys.path.insert(0,sys.argv[1]); from task import load_task; print('\n'.join(load_task(sys.argv[2])['checks']))" "$HERE" "$taskf" > "$out/commands.txt"
crates=$(grep '^crates/' "$out/changed-files.txt" | cut -d/ -f2 | sort -u)
for c in $crates; do
  pkg=$(sed -n 's/^name = "\(.*\)"/\1/p' "$W/crates/$c/Cargo.toml" | head -1)
  echo "cargo test --locked -p $pkg" >> "$out/commands.txt"
done
while IFS= read -r cmd; do
  n=$((n+1))
  (cd "$W" && eval "$cmd") > "$out/$n.log" 2>&1; rc=$?
  echo "$rc  $cmd" | tee -a "$out/summary.txt"
done < "$out/commands.txt"
git -C "$A/repo" worktree remove --force "$W"
