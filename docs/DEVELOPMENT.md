# Developing Brigadier

How to build, run and check Brigadier from source, and the notes a code change needs. Read
[CONTRIBUTING.md](../CONTRIBUTING.md) before you open anything.

## Layout

```
crates/        Rust workspace
  core/          domain model, catalog projection, session manager (the engine)
  store/         SQLite WAL event store (single writer, read pool), blob store
  ipc/           authenticated local IPC + protocol types (exported to TypeScript)
  sandbox/       OS abstraction: paths, private files, processes, credentials, shell, sandbox
  providers/     Provider trait, normalized events, Claude and Codex adapters, replay fixtures
  git/           git engine: worktrees, snapshots, candidates, guarded landing
  mcp-server/    the Brigadier MCP server the thread and workers talk to
  router/        routing: model registry, quota forecasts, outcome learning, decide()
  review/        one-shot cross-vendor reviews
  brain/         the Project Brain: graph, full-text and embedding search
  index/         the code index (tree-sitter symbols and references)
  registry/      the MCP, plugin and skill registry
  computer/      computer use: the native helper and its engine (macOS)
  browser-data/  importing cookies and history into the built-in browser (macOS)
  daemon/        brigadierd
apps/desktop/  Tauri 2 shell (src-tauri/) and the React UI (src/)
registry/      the curated model registry, published from this repository
scripts/       uninstall.sh
tools/         full-checks.sh, dev-app helpers, the A/B harness
docs/          design plans and evidence (see docs/README.md)
```

## Requirements

- Rust 1.98.1 (pinned in `rust-toolchain.toml`; rustup installs it on first use)
- Node 24+ and pnpm 12.6.0 (`packageManager` in `package.json`)
- Tauri 2 platform prerequisites: https://v2.tauri.app/start/prerequisites/
- CMake 3.x or newer and libclang, to build whisper.cpp (dictation's engine; see below).
  macOS has libclang with the Xcode Command Line Tools; on Windows install LLVM, on Linux
  `libclang-dev`.

## Develop

```sh
pnpm install
pnpm tauri:dev          # builds and stages brigadierd, starts Vite and the app
```

`pnpm tauri:dev` and `pnpm tauri:debug-app` (a debug `Brigadier Dev.app`) run as
`ai.brigadier.dev`, so they never hand a launch to an installed Brigadier. Any debug build
keeps its data in `/tmp/brigadier-dev` unless `BRIGADIER_DATA_DIR` names another directory.

The app launches `brigadierd` detached; closing the window keeps both running in the menu bar.
Quit from the menu-bar item (or Cmd+Q) to stop the daemon too.

Data lives in `~/Library/Application Support/Brigadier` (`%LOCALAPPDATA%\Brigadier`,
`~/.local/share/brigadier`). Set `BRIGADIER_DATA_DIR` to use a throwaway directory; each data
directory gets its own daemon.

## Checks

Run all of them before you ask for a review:

```sh
tools/full-checks.sh            # fmt, generated types, build, clippy, Rust tests, typecheck, lint, app tests
tools/full-checks.sh --cross    # also Linux and Windows clippy of the cross-buildable crates
```

Or one by one:

```sh
cargo fmt --all --check
cargo run -p brigadier-ipc --bin gen-ts      # regenerate TS bindings after protocol changes
pnpm build
pnpm --filter @brigadier/desktop stage-sidecar --debug
cargo clippy --locked --workspace --all-targets -- -D warnings
TMPDIR=/tmp cargo test --locked --workspace --lib --bins --tests
pnpm typecheck && pnpm lint && pnpm test
```

CI runs formatting, the bindings check, typecheck, lint and the app tests, then builds, clippies
and smoke-launches the app on macOS, Windows and Linux. It does not run the Rust tests: run
them yourself.

`pnpm lint` includes `brigadier/no-raw-design-values`: component code may only use theme tokens
(no raw colors, arbitrary Tailwind values or numeric inline styles). Raw values live only in
`apps/desktop/src/styles/tokens.css`.

Code that is gated to one platform (`cfg(unix)`, `cfg(target_os = "macos")`) must gate its
helpers and imports the same way, or Linux and Windows clippy flags them as unused.

## Launch smoke check

The smoke check launches the app and judges it against the performance budgets (PLAN.md §4):

```sh
pnpm tauri build
BRIGADIER_DATA_DIR=$(mktemp -d) target/release/bundle/macos/Brigadier.app/Contents/MacOS/brigadier --smoke
```

For a test build, use `pnpm tauri:debug-app` and `target/debug/bundle/macos/Brigadier Dev.app`.
Never smoke-test a bundle with the release identity (`ai.brigadier.app`) on a throwaway data
directory if you also use that identity for real. The check needs the logged-in desktop
session: inside a sandbox that blocks the window server, or over SSH, it says so and exits
with code 3.

It prints a JSON report (also written to `BRIGADIER_SMOKE_REPORT` if set) and exits non-zero if
a budget for an implemented feature is missed. `BRIGADIER_BUDGET_TOLERANCE` multiplies timing
budgets only; CI uses 3 on shared runners, locally it is 1.

Startup is judged against one of two budgets. A launch on a new data folder is a first launch:
no model lists are saved yet, so the startup screen waits for the signed-in agent CLIs to report
theirs (at most 3 s once the app has drawn), and it has 3 s. Any later launch has 1 s. Missing or
signed-out CLIs never hold the startup screen. The startup note breaks the time down into
milestones (ms since process start: webview, script, connected, catalog, paint, agents,
revealed), and the frame-gap note says when the longest gap fell and which event flush cost the
most.

In CI, after warm-up launches that are never judged, the app is launched in three pairs: a first
launch on a new data directory, then a second launch on the same directory.
`apps/desktop/scripts/judge-smoke.mjs` judges them:

- first launch by the **median** of the three first launches (3 s × tolerance), and cold start
  by the median of the three second launches (1 s × tolerance);
- every other check on the first launch, exactly as the app judged it.

All six reports, with their milestones, are printed in the log and kept in the combined
report artifact. On shared runners most of a cold start passes before the page even loads,
while the platform creates the window and webview, and that part swings by seconds between
identical launches. For example, on the same code:

| Launch | webview | script | connected | catalog | paint (cold start) |
|---|---|---|---|---|---|
| macOS runner | 2299 | 2630 | 2661 | 2846 | 2909 |
| Windows runner | 3191 | 3329 | 3353 | 3590 | 3599 |
| Local Mac | 234 | 307 | 312 | 341 | 349 |

On Windows the same shell code measured anywhere from 1080 ms to 3599 ms across runs. Heavy
views (the terminal, the syntax highlighter, graphs) are lazy-loaded so the startup bundle
stays small; keep new ones lazy too.

## Providers

- Claude Code runs as one persistent `claude -p --input-format stream-json --output-format
  stream-json` process per conversation.
- Codex runs as one `codex app-server` per conversation, over stdio JSON-RPC. The Rust bindings
  in `crates/providers/src/codex/protocol.rs` are generated from the installed CLI's schema.
  After a Codex upgrade, regenerate them:

  ```sh
  cargo run -p brigadier-providers --features codegen --bin gen-codex -- "$(command -v codex)"
  ```

The Inspector's **Providers** tab (Settings › Inspector) shows each CLI's login state, version,
live model list and remaining quota. From it you can start, resume, fork, steer, interrupt,
stop and close raw sessions, answer their approval requests, simulate a usage-limit event and
replay recordings. Recordings are scrubbed of personal data as they are written. The fixtures
in `crates/providers/fixtures/` ship with the app.

## Development-only switches

None of these exist in a release build.

- `BRIGADIER_ROUTE_CHEAP=1` in the daemon's environment makes every worker use its vendor's
  cheapest model at low effort, keeping the routing reason and saying so.
- `BRIGADIER_FAULT=claude-limit[:tool-calls=N][:window=ID][:reset=MINUTES]` (or `codex-limit`)
  forces a usage limit on the first worker the daemon starts on that provider, to exercise
  fallback without spending real quota. After the session's next N finished tool calls (3 by
  default) the quota monitor holds the provider at that limit until the reset (60 minutes by
  default), the transcript notes the injection, the running turn is interrupted, and when it
  ends the session reports the limit as a CLI at its limit would. The Inspector's fault control
  does the same for a running task or conversation.
- `BRIGADIER_REGISTRY_URL` points the model registry update at another HTTPS address, or at
  plain HTTP on 127.0.0.1 or ::1, to try an update against a local copy.

## Dependency notes

- `wry` (the desktop shell): the Browser tab's page is a plain wry webview laid over the tab, not
  a Tauri one. Tauri gives every webview it makes the app's IPC bridge, init scripts and custom
  protocols (and child webviews need its `unstable` feature); a web page must get none of that.
  The crate is pinned to the release `tauri-runtime-wry` uses, since two copies of wry would
  register the same native classes. Linux has no embedded page: its tab opens pages in the
  system browser.
- `objc2`, `objc2-foundation`, `objc2-web-kit`, `objc2-app-kit`, `block2` (macOS, the releases
  wry is built on): the Browser tab's page gets its own WebKit UI delegate, because wry's grants
  every camera and microphone request. Ours answers them from the site's choice in Settings ›
  Browser › Site settings (Allow, Block, or Ask: WebKit's own question), sends popups to the
  system browser and leaves file uploads to wry's. The same module clears the browser's saved
  WebKit data store, counts its sites and shows the save panel for downloads, and does what a
  tab's page does natively: zoom, find in page, print, copy a screenshot and reload past the
  cache. This module, `apps/desktop/src-tauri/src/browser_ui.rs`, is the app shell's WebKit
  exception to the workspace's `unsafe_code = "deny"` (the sandbox crate's OS calls are the
  only others): calling WebKit through objc2 needs `unsafe`. The `#[allow(unsafe_code)]` sits
  on that module alone, and each `unsafe` in it says why it holds. The page also gets a script,
  before any of its own and in every frame, that takes screen capture and the
  speech-recognition APIs away for good (`NO_CAPTURE` in `browser.rs`); on Windows, whose
  WebView2 has no per-site choices here, it takes the camera and microphone APIs away too.
- Overnight run notifications use a small macOS UserNotifications adapter under the app's
  bundle identity, retaining a stable notification ID and session/run activation payload.
  `overnight_notifications.rs::mac` is the shell's other narrowly scoped unsafe exception:
  its C strings live through each call, Objective-C copies them, and callback tickets carry
  no borrowed pointers. The desktop notification plugin handles the other platforms.
- `objc2-quartz-core` (macOS, the release wry is built on) with more of `objc2-app-kit`: the
  first launch's intro holds the window clear (shown, unseen, passing the mouse through) until
  the page is ready, then shows it with no title-bar buttons, shadow or background above a
  borderless black window that dims the desktop below the menu bar, ignores the mouse and fades
  in and out (an AppKit animation with its own easing curve).
  `apps/desktop/src-tauri/src/intro.rs::mac` is the shell's third narrowly scoped unsafe
  exception: it retains the main window's NSWindow from Tauri on the main thread, makes the dim
  window there and keeps it when closed, as it holds it. Hiding the window, a page load and a
  deadline all end the intro natively, so it never depends on the page to clean up.
- `spessasynth_core` (a dev dependency of the desktop app, Apache-2.0): renders the first
  launch's welcome music offline (`apps/desktop/scripts/render-welcome-sound.mjs`) on the
  sampled instruments of the FluidR3 Mono GM SoundFont (MIT). The SoundFont is downloaded by the
  script and checked against its SHA-256, never committed; only the rendered
  `welcome-sound.m4a` ships, with its notice in `apps/desktop/src/app/onboarding/NOTICE.md`.
- `whisper-rs` (whisper.cpp), `ureq`, `sha2` (brigadierd): dictation turns speech into text on
  this computer; the audio never leaves it. The composer's webview records the microphone and
  streams 16 kHz PCM to the daemon, which feeds it to a short-lived `brigadierd transcribe`
  process, so the daemon never holds the model. The model (whisper.cpp's `ggml-base-q5_1`,
  60 MB) is downloaded from Hugging Face on first use, at a pinned revision, checked against its
  SHA-256 and kept in the data directory under `models/whisper/`. `.cargo/config.toml` builds
  whisper.cpp for any CPU of the target's kind (`GGML_NATIVE=OFF`); its Rust bindings are
  generated for the target, since the crate's bundled ones don't fit Windows.
- `tree-sitter`, `tree-sitter-tags` and the language grammars, `ignore`, `notify`,
  `notify-debouncer-full`, `toml`, `yaml-rust2` (the code index, `crates/index`): no model
  involved. Symbols and references come from each grammar's tags query (C#, Swift and Kotlin
  get corrected copies under `crates/index/queries/`, and TypeScript and JavaScript the
  JavaScript query joined with the TypeScript one and our own patterns for exports, types,
  enums, class-field functions, top-level constants and JSX components; credited in
  `THIRD_PARTY_NOTICES.md`). References are matched to definitions by name, without type
  information.
- `tokenizers`, `safetensors`, `memmap2` (the Brain's embedder, `crates/brain`): the embedding
  model is `minishlab/potion-retrieval-32M` (Model2Vec static embeddings, MIT, 512 dimensions,
  about 130 MB), downloaded from Hugging Face once a project exists, at a pinned revision,
  checked against its SHA-256 and kept under `models/embeddings/`. Embedding is a token lookup
  and a mean, in the daemon; the model is loaded on first use and freed after 10 idle minutes.

## Build and sign (macOS)

```sh
pnpm tauri build --target universal-apple-darwin
```

The bundle is `ai.brigadier.app`, universal, macOS 14+, hardened runtime. Signing and
notarization are configured only through environment variables and are skipped when unset:

| Variable | Purpose |
|---|---|
| `APPLE_SIGNING_IDENTITY` | Developer ID Application identity |
| `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD` | base64 .p12 and its password (CI) |
| `APPLE_ID`, `APPLE_PASSWORD`, `APPLE_TEAM_ID` | notarization with an app-specific password |
| `APPLE_API_KEY`, `APPLE_API_ISSUER`, `APPLE_API_KEY_PATH` | notarization with an App Store Connect key |

CI reads the same names from repository secrets. An unsigned or ad-hoc-signed build runs, but
computer use needs a signed build to keep its macOS permissions across rebuilds
([COMPUTER-USE.md](COMPUTER-USE.md)).

## The uninstall script

`scripts/uninstall.sh` removes an instance whose app is already gone (the app's own **Uninstall
Brigadier…** does it otherwise). It accepts `--keep-data`, `--data-dir PATH`, `--app-id ID`,
`--app PATH`, `--dev`, `--dry-run`, and `-h`. Its default data directory is
`$BRIGADIER_DATA_DIR` or `~/Library/Application Support/Brigadier`; the default app path is
`/Applications/Brigadier.app` for the default identifier. The default identifier must be named
explicitly. For one development instance, pass `--dev --app-id ai.brigadier.<name> --data-dir
PATH` (and `--app PATH` if there is a bundle). `--dev` never sweeps other identifiers or data
directories.

The script quits that app if it still runs, asks that instance's daemon to quit, removes
recorded clean git worktrees and merged Brigadier branches, and moves its data and exact
per-app folders to the Trash. Dirty worktrees and unmerged branches stay; the summary prints a
command for each one to resolve manually. It removes only ledger-recorded Claude session files
and Codex rollouts, and reports Codex state database rows and trust entries it cannot confirm.
A remaining worktree keeps the data directory in place and makes the script exit nonzero.
