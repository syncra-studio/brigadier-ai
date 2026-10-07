#!/bin/bash
# cmx.sh <arm-dir> <cmux subcommand> [args...] : runs a cmux command against the arm's own
# /delegator tab only (the surface UUID dlg_start.sh saved in <arm-dir>/surface), so a typo can
# never reach another tab, the calling Delegator's included.
set -euo pipefail
arm="${1:?arm dir}"; shift
sid=$(cat "$arm/surface" 2>/dev/null || true)
[[ "$sid" =~ ^[0-9A-F]{8}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{4}-[0-9A-F]{12}$ ]] || { echo "cmx: no surface for $arm" >&2; exit 2; }
sub="${1:?subcommand}"; shift
exec cmux "$sub" --surface "$sid" "$@"
