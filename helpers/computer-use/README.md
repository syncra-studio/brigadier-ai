# Brigadier Computer Use helper

The helper that lets Brigadier's agents see and drive apps on this Mac in the
background: Accessibility-tree first, input posted to the target process (no
focus change, no pointer movement), window-scoped screenshots, a per-session
agent cursor, and a browser `page` tool. macOS only.

It is a separate Cargo workspace (the root workspace excludes it) and ships
inside Brigadier.app as `Helpers/Brigadier Computer Use.app`, signed with
Brigadier's Developer ID. At run time Brigadier copies that bundle to
`<data dir>/computer/Brigadier Computer Use.app`, because macOS attributes
Screen Recording to the outermost app bundle.

## Origin

Forked from an MIT-licensed driver at its release 0.8.12, commit
`e8a8dbfd45a5d050058e7340f97ffe6f157c3128`. The licence and copyright are in
the repository's `THIRD_PARTY_NOTICES.md`. Changes from the original:

- Renamed every product name, binary (`brigadier-cu`), bundle, environment
  variable (`BRIGADIER_CU_*`) and path to Brigadier's.
- Removed self-update and update checks, the ffmpeg installer, the skill
  installer, autostart, all telemetry, the unauthenticated HTTP transport,
  and a compatibility profile for another desktop client.
- Removed the Windows and Linux backends.
- One home directory (`--home <dir>` or `BRIGADIER_CU_HOME`) holds the
  socket, PID file, config, journal and activity store.
- Socket peers must carry the daemon's own code signature when it runs from
  the helper app, on top of the same-user check and the agent token
  (`BRIGADIER_CU_SOCKET_AUTH_TOKEN`, or `<home>/auth`).
- `supervisor.rs`: the MCP shim starts the daemon on demand under a
  cross-process lock, replaces a dead or hung daemon (checking the recorded
  PID's executable and start time before signalling it), and retries once a
  call that lost the daemon, but only one that never reached it or that only
  reads, so an action never runs twice.
- `lifecycle.rs`: the desktop build stamps a build id into the bundle; a
  healthy daemon from another build is replaced like a dead one. A `health`
  request also fails when the daemon's main loop has stopped running. While
  `<home>/off` exists (computer use turned off in Brigadier), shims refuse
  calls and never start a daemon, and the daemon refuses calls.
- `agent_apps.rs`: the apps each Brigadier agent is using (`agent_sessions`),
  and "Stop using <app>" (`stop_app`), kept in `<home>/stopped.json` so it
  holds after the helper is replaced, until the agent exits. The daemon also
  keeps the permissions it last read in `<home>/permissions.json`.
- `host.rs`: `brigadier-cu host probe|ensure|restart|stop|off|on|request`,
  how Brigadier's daemon (which the socket's signature check would refuse)
  asks things of the helper.
- Any tool accepts a `session` argument (ignored where it has no use), since
  agents are told to pass theirs on every call.
- The bundle explains its Apple Events use, so macOS can ask before browser
  page actions control Safari or Chrome.
- Windows on another Space (an app launched in the background while the
  user works in a full-screen app) are found by remote token
  (`ax/offspace.rs`), so they can be read, captured and driven without
  switching Spaces.
- Development builds of Brigadier stay targetable; the user's release
  Brigadier is not.

## Layout

| Crate | Purpose |
| --- | --- |
| `brigadier-cu` | CLI, stdio MCP shim, daemon, supervisor |
| `cu-core` | Protocol, tool registry, sessions, recording, CDP helpers |
| `platform-macos` | Accessibility, capture, input, browser, permissions |
| `cursor-overlay` | Agent cursor rendering |
| `pip-preview` | Home directory and the optional preview window |
| `cu-testkit` | Test helpers |

## Build and check

```bash
cd helpers/computer-use
cargo build --release -p brigadier-cu
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
target/release/brigadier-cu list-tools
```

The desktop build (`pnpm stage-sidecar` in `apps/desktop`) builds the release
binary, assembles the bundle from `scripts/BrigadierCuBundle`, and signs it.
