# Vendored upstream: the macOS computer-use driver

This folder is a copy of the macOS backend of the MIT-licensed driver at
https://github.com/trycua/cua (`libs/cua-driver/rust`), pinned to one release
tag. Brigadier's own code sits on top of it in `../crates/` and never edits
these files. The only differences from the release are the files in
`../patches/`, each a small hook we could offer upstream, and the generated
`Cargo.toml`.

Text inside these crates (names, messages, comments) stays as upstream wrote
it. What agents and users see goes through Brigadier's layer, which uses
Brigadier's names.

## What is here

- `crates/`: the backend crates, byte-identical to the release apart from
  `../patches/`.
- `Skills/cua-driver/`: documents `cua-driver-core` compiles in with
  `include_str!`. Brigadier never serves them to agents.
- `Cargo.toml`: upstream's workspace manifest with `members` cut to the
  crates here (generated). This folder is its own Cargo workspace; the
  helper workspace excludes it and uses the crates as path dependencies, so
  `cargo clippy --workspace` and `cargo test --workspace` in the helper cover
  only Brigadier's crates.
- `LICENSE.md` and `THIRD_PARTY_NOTICES.md`: upstream's licence and its own
  third-party notices.
- `PRISTINE.sha256`: the sha256 of every file as fetched, before patches.

Not copied: the upstream binary (with its telemetry, updater, skill installer,
autostart and HTTP transport), the SDK and bindings, the Windows and Linux
backends, the optional perception add-on and the test and tooling crates.

## Release

<!-- sync:begin (written by scripts/sync-upstream.sh) -->
| | |
|---|---|
| Repository | https://github.com/trycua/cua |
| Tag | `cua-driver-rs-v0.35.2` |
| Commit | `ad694f82aaaa12e67257e4510837fc1f6cfc9484` (2026-10-13) |
| Path in the repository | `libs/cua-driver/rust` |
| Crates | `cua-driver-core` `cua-driver-contract` `platform-macos` `cursor-overlay` `cua-cursor-motion` `pip-preview` |
| Also copied | `Skills/cua-driver/` `LICENSE.md` `THIRD_PARTY_NOTICES.md` |
| Files | 292 (sha256 of PRISTINE.sha256: `641816768fe20ea78d15e0f17ad63c15614e14299f6780c0da0de48b05de7f43`) |

Patches applied, in order:

- `patches/0001-state-dir.diff`: one state folder for the driver's own config (CUA_DRIVER_STATE_DIR), so an embedding app keeps it beside its other state instead of ~/.cua-driver.
- `patches/0002-registry-wrap.diff`: ToolRegistry::wrap lets an embedding app put its own checks around a tool in place, so direct calls, run_steps steps and the registry's own re-routes all pass them.
- `patches/0003-restore-policy.diff`: a host restore policy (focus_steal::set_restore_policy) asked before every restore the driver makes, so an embedding app can leave the user's own switches and system prompts in front.
- `patches/0004-host-authorized-wrap.diff`: ToolRegistry::wrap_host_authorized lets an embedding app that replaces a tool with its own implementation authorize the calls the driver admits only in unrestricted mode, with its own check, so unrestricted mode can stay off.
- `patches/0005-off-space-window-resolver.diff`: a host off-Space window resolver (ax::bindings::set_off_space_window_resolver) used in place of the built-in remote-token probe, so an embedding app can reach windows on another Space with element ids it remembers and a scan that resumes where it stopped.
<!-- sync:end -->

## Where the driver keeps state

Brigadier's helper sets these before it builds the driver's tool registry, so
everything the driver keeps lives in the helper's home folder:

| Variable | What moves there |
|---|---|
| `CUA_DRIVER_STATE_DIR` (patch 0001) | `config.json` (settings, preview window, cursor motion defaults, the `run_script` switch) |
| `CUA_DRIVER_CURSOR_THEME_DIR` | installed cursor themes |
| `CUA_DRIVER_BROWSER_PROFILE_ROOT` | browser profiles the driver would create |

Other uses of `HOME` in these crates are the user's own paths and stay there:
`~` in paths an agent passes (screenshots, recordings, launch arguments),
`~/Applications`, and the browsers' own profile folders. The direct-capture
evidence file is only written for the upstream app's own bundle ids, and the
upstream daemon's socket path is unused (the helper has its own daemon).

## Updating

Move to a newer release on a schedule (about every two weeks) or for a fix we
want, never continuously.

1. `scripts/sync-upstream.sh <tag>` (from `helpers/computer-use`). It fetches
   the tag, replaces this folder, re-applies `../patches/` in order,
   regenerates `Cargo.toml` and the record above. A patch that no longer
   applies stops it: refresh the patch, or drop it if the release now does
   the same.
2. Read the release notes since the previous tag for macOS lines. Drop any
   patch upstream now covers.
3. `cargo build`, `cargo clippy --workspace --all-targets -- -D warnings` and
   `cargo test --workspace` in `helpers/computer-use`. Fix Brigadier's layer
   for API changes.
4. `cargo test -p cu-layer --test driver` checks what agents see against
   the snapshots in `crates/cu-layer/tests/snapshots/`. A new driver tool
   fails `every_driver_tool_is_classified` until it is offered or withheld
   in `crates/cu-layer/src/catalog.rs`. Review each new argument and each
   new text that suggests bringing an app forward (the rewrite in
   `crates/cu-layer/src/text.rs` must cover it), then re-run with
   `UPDATE_SNAPSHOTS=1` and commit the snapshot diff with the update.
5. `scripts/sync-upstream.sh --verify`, then `tools/full-checks.sh` from the
   repository root.
6. On a signed dev build: the scripted benchmark (warm 5, cold 3) against the
   previous pinned build, the user-present focus check (no front changes
   caused by the helper), the reliability matrix (each case 5 times) and the
   agent-level run. None may get worse.
7. One commit: "Computer use updated to <version>", with the benchmark
   results kept beside the run notes.
