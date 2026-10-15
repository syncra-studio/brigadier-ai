#!/usr/bin/env bash
# CI's launch smoke check of a built app: two warm-up launches, then three judged pairs, judged
# by apps/desktop/scripts/judge-smoke.mjs into $RUNNER_TEMP/smoke-report.json. Run from the
# checkout's root on a GitHub runner (it reads RUNNER_OS and RUNNER_TEMP).
#
#   tools/ci/smoke.sh <path to the app's executable>
set -euo pipefail

app="$1"

smoke() {
  if [ "$RUNNER_OS" = "Linux" ]; then
    # A desktop session has a D-Bus session bus (the single-instance lock and the tray
    # use it); xvfb-run alone has none. Without DRI3, WebKitGTK needs the DMA-BUF
    # renderer off to draw at all.
    WEBKIT_DISABLE_DMABUF_RENDERER=1 dbus-run-session -- xvfb-run -a "$app" --smoke
  else
    "$app" --smoke
  fi
}
# The release build just used every core. On macOS wait for the host to settle before
# each launch, with a cap so a persistently busy runner still runs and reports the
# unchanged budgets. loadavg is normalized by the runner's actual CPU count.
wait_for_idle() {
  if [ "$RUNNER_OS" != "macOS" ]; then return; fi
  local elapsed=0 load
  while true; do
    load="$(node -p 'require("node:os").loadavg()[0] / require("node:os").cpus().length')"
    if awk -v load="$load" 'BEGIN { exit !(load < 0.75) }'; then
      echo "macOS 1-minute load/core $load (< 0.75) after ${elapsed}s"
      return
    fi
    if [ "$elapsed" -ge 120 ]; then
      echo "macOS 1-minute load/core $load still >= 0.75 after ${elapsed}s; running with diagnostics"
      return
    fi
    sleep 10
    elapsed=$((elapsed + 10))
  done
}
diagnostics() {
  echo "Runner snapshot before $1: $(date -u '+%Y-%m-%dT%H:%M:%SZ')"
  if [ "$RUNNER_OS" = "Windows" ]; then
    # $_ is PowerShell's current process, not a shell variable.
    # shellcheck disable=SC2016
    powershell.exe -NoProfile -Command 'Get-CimInstance Win32_PerfFormattedData_PerfProc_Process | Where-Object { $_.Name -ne "_Total" -and $_.Name -ne "Idle" } | Sort-Object PercentProcessorTime -Descending | Select-Object -First 12 Name,IDProcess,PercentProcessorTime,WorkingSetPrivate | Format-Table -AutoSize' || true
  elif [ "$RUNNER_OS" = "macOS" ]; then
    uptime
    ps -Ao pid,ppid,%cpu,%mem,comm -r | head -n 16 || true
  else
    uptime
    ps -Ao pid,ppid,%cpu,%mem,comm --sort=-%cpu | head -n 16 || true
  fi
}
if [ "$RUNNER_OS" = "macOS" ]; then
  # GitHub's disposable macOS VM permits sudo. Stop Spotlight indexing during the
  # timed launches; a failure is visible but does not bypass smoke validation.
  sudo -n mdutil -a -i off || echo "Spotlight indexing could not be disabled"
fi
# Two throwaway launches prime loader and webview caches, including WebView2 on
# Windows. Each uses a fresh data directory and prints its own smoke report.
for warmup in 1 2; do
  echo "::group::Warm-up launch $warmup (not judged)"
  wait_for_idle
  diagnostics "warm-up $warmup"
  if BRIGADIER_DATA_DIR="$RUNNER_TEMP/brigadier-warmup-$warmup" \
    BRIGADIER_SMOKE_REPORT="$RUNNER_TEMP/warmup-report-$warmup.json" smoke; then
    echo "Warm-up $warmup completed"
  else
    echo "Warm-up $warmup exited nonzero; judged launches will still run"
  fi
  echo "::endgroup::"
done
# Then three pairs of launches: a first launch on a new data directory, then a second
# launch on the same directory, each judged against its own startup budget. On shared
# runners the window and webview creation before the page loads swings by seconds
# between identical launches, so each startup budget is judged by the median of its
# three launches against the same limit; every other check is judged on the first
# launch, as the app judged it.
launches=()
for pair in 1 2 3; do
  for kind in first second; do
    echo "::group::Judged launch $pair ($kind on its data directory)"
    wait_for_idle
    diagnostics "judged launch $pair $kind"
    report="$RUNNER_TEMP/smoke-launch-$pair-$kind.json"
    launches+=("$report")
    if BRIGADIER_DATA_DIR="$RUNNER_TEMP/brigadier-smoke-$pair" \
      BRIGADIER_SMOKE_REPORT="$report" smoke; then
      echo "Judged launch $pair $kind completed"
    else
      echo "Judged launch $pair $kind exited nonzero; its report will still be judged"
    fi
    echo "::endgroup::"
  done
done
node apps/desktop/scripts/judge-smoke.mjs "$RUNNER_TEMP/smoke-report.json" "${launches[@]}"
