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

Patches applied: none.
<!-- sync:end -->

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
4. Review the tool-list and foreground-text snapshot tests in `cu-layer`. Each
   new tool or argument is allowed or denied in the layer's policy on
   purpose, and each new text that suggests bringing an app forward gets its
   rewrite.
5. `scripts/sync-upstream.sh --verify`, then `tools/full-checks.sh` from the
   repository root.
6. On a signed dev build: the scripted benchmark (warm 5, cold 3) against the
   previous pinned build, the user-present focus check (no front changes
   caused by the helper), the reliability matrix (each case 5 times) and the
   agent-level run. None may get worse.
7. One commit: "Computer use updated to <version>", with the benchmark
   results kept beside the run notes.
