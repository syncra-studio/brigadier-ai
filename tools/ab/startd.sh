#!/bin/bash
# startd.sh <data-dir> : runs a dev brigadierd in the foreground with a clean, GUI-like
# environment and its own data dir. BRIGD: the daemon binary (required). BRIG_SRC: the source
# tree it was built from (default: this checkout), whose IPC protocol version the clients use.
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
exec env -i HOME="$HOME" USER="$USER" LOGNAME="$LOGNAME" SHELL=/bin/zsh TMPDIR="$TMPDIR" LANG=en_US.UTF-8 PATH="$CLEANPATH" \
  BRIGADIER_DATA_DIR="$D" "$BRIGD" --foreground --data-dir "$D"
