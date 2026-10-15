#!/bin/bash
# startd.sh <data-dir> : runs a dev brigadierd in the foreground with a clean, GUI-like
# environment and its own data dir. BRIGD: the daemon binary (required). BRIG_SRC: the source
# tree it was built from (default: this checkout), whose IPC protocol version the clients use.
# BRIG_CLAUDE_HOME, when set, is passed on as CLAUDE_CONFIG_DIR: the
# arm's Claude login is that home's (the same path string, so the CLI finds its Keychain entry).
# BRIG_DEBUG_ENV, when set, is a space-separated list of BRIGADIER_*=value development settings passed
# on to the daemon (the V2 battery's seams: BRIGADIER_REBIRTH_TOKENS, BRIGADIER_WORKER_HANDOFF_TOKENS).
# Never point this at the installed app's daemon or its data.
set -euo pipefail
D=${1:?data dir}; : "${BRIGD:?set BRIGD to a dev brigadierd}"
HERE=$(cd "$(dirname "$0")" && pwd)
SRC=${BRIG_SRC:-$HERE/../..}
case "$D" in *"Application Support/Brigadier"*) echo "startd: refusing the installed app's data" >&2; exit 2 ;; esac
case "$BRIGD" in /Applications/*) echo "startd: refusing the installed app's daemon" >&2; exit 2 ;; esac
mkdir -p "$D"
python3 -c 'import sys; sys.path.insert(0, sys.argv[1]); import bipc; print(bipc.protocol_of(sys.argv[2]))' "$HERE" "$SRC" > "$D/ab-protocol"
CLEANPATH=$(echo "$PATH" | tr ':' '\n' | grep -v cmux | paste -sd: -)
DEBUG_ENV=()
for kv in ${BRIG_DEBUG_ENV:-}; do
  case "$kv" in BRIGADIER_[A-Z_]*=*) DEBUG_ENV+=("$kv") ;; *) echo "startd: ignoring $kv" >&2 ;; esac
done
exec env -i HOME="$HOME" USER="$USER" LOGNAME="$LOGNAME" SHELL=/bin/zsh TMPDIR="$TMPDIR" LANG=en_US.UTF-8 PATH="$CLEANPATH" \
  ${BRIG_CLAUDE_HOME:+CLAUDE_CONFIG_DIR="$BRIG_CLAUDE_HOME"} ${DEBUG_ENV[@]+"${DEBUG_ENV[@]}"} \
  BRIGADIER_DATA_DIR="$D" "$BRIGD" --foreground --data-dir "$D"
