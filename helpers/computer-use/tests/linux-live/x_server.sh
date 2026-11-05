# Sourced by run.sh and x_server_test.sh.

# Whether the X server with this pid still serves its display. One failed answer isn't a death
# (a client just left), so xdpyinfo gets about 2 s; a server whose process is gone fails at once.
x_server_alive() {
  local _
  for _ in $(seq 10); do
    kill -0 "$1" 2>/dev/null || return 1
    xdpyinfo >/dev/null 2>&1 && return 0
    sleep 0.2
  done
  return 1
}
