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
  cli/           the `brigadier` binary: worker commands, models, brain and code search, previews
  git/           git operations: worktrees, snapshots, candidates, landing
  router/        the model registry and quota estimates `brigadier models` reads; routes no work
  brain/         the Project Brain: graph, full-text and embedding search
  index/         the code index (tree-sitter symbols and references)
  registry/      the MCP, plugin and skill registry
  browser-data/  importing cookies and history into the built-in browser (macOS)
  daemon/        brigadierd
apps/desktop/  Tauri 2 shell (src-tauri/) and the React UI (src/)
helpers/computer-use/  the computer-use helper app, its own Cargo workspace (macOS; see its README)
registry/      the curated model registry, published from this repository
scripts/       install.sh and install.ps1 (the one-command installers), uninstall.sh and uninstall.ps1
tools/         full-checks.sh, the live smoke, dev-app helpers, the A/B harness
docs/          design plans and evidence (see docs/README.md)
```

The report-only [continuity battery](../tools/continuity-battery/README.md) measures Claude recall across forced rebirth seams: `tools/continuity-battery/run.sh`.

## Requirements

- Rust 1.98.1 (pinned in `rust-toolchain.toml`; rustup installs it on first use)
- Node 24+ and pnpm 12.6.0 (`packageManager` in `package.json`)
- Tauri 2 platform prerequisites: https://v2.tauri.app/start/prerequisites/
- CMake 3.x or newer and libclang, to build whisper.cpp (dictation's engine; see below).
  macOS has libclang with the Xcode Command Line Tools; on Windows install LLVM, on Linux
  `libclang-dev`.

Windows needs more setup first: see [Windows](#windows) below.

## Develop

```sh
pnpm install
pnpm tauri:dev          # builds and stages brigadierd, starts Vite and the app
```

`pnpm tauri:dev` and `pnpm tauri:debug-app` (a debug `Brigadier Dev.app`) run as
`ai.brigadier.dev`, so they never hand a launch to an installed Brigadier. Any debug build
keeps its data in `/tmp/brigadier-dev` (`%TEMP%\brigadier-dev` on Windows) unless
`BRIGADIER_DATA_DIR` names another directory.

The app launches `brigadierd` detached; closing the window keeps both running in the menu bar
(the notification area on Windows). Quit from the menu-bar item (or Cmd+Q) to stop the daemon
too.

Release identity `ai.brigadier.app` keeps its data in `~/Library/Application Support/Brigadier`
(`%LOCALAPPDATA%\Brigadier\Data`, `~/.local/share/brigadier`). Other release builds default to a
folder named after their bundle identifier in the same parent directory. Set
`BRIGADIER_DATA_DIR` to use a throwaway directory; each data directory gets its own daemon.

## Windows

Windows 11 builds, tests and runs the dev app with these, all from `winget` (the first two need
an administrator's prompt):

```powershell
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id LLVM.LLVM -e        # libclang for whisper.cpp's bindings, in C:\Program Files\LLVM\bin
winget install --id Kitware.CMake -e
winget install --id Rustlang.Rustup -e  # the toolchain in rust-toolchain.toml comes on first use
winget install --id Schniz.fnm -e       # then: fnm install 24, and corepack enable for pnpm
winget install --id Git.Git -e          # Git Bash runs the tools/*.sh scripts
```

WebView2 comes with Windows 11. An older Node on PATH (a system Node 22, say) must come after
Node 24. From Git Bash, put the fnm Node first in its Unix spelling, as a Windows one in PATH
breaks the pnpm shim:

```sh
export PATH="$(cygpath -u "$APPDATA")/fnm/node-versions/v24.22.0/installation:$HOME/.cargo/bin:$PATH"
```

Then `pnpm install` and `pnpm tauri:dev` as above. `pnpm tauri:debug-app` makes a macOS `.app`;
on Windows build the same debug app without a bundle, and smoke-check it:

```sh
cd apps/desktop && pnpm tauri build --debug --no-bundle --config src-tauri/tauri.dev.conf.json
cd ../.. && BRIGADIER_DATA_DIR="$(cygpath -w "$(mktemp -d)")" target/debug/brigadier-app.exe --smoke
```

A debug build misses the smoke check's timing budgets (startup, runtime stalls); CI judges a
release build. The other checks are as on the Mac, with these differences:

- Run them from Git Bash. `tools/full-checks.sh` takes its niceness from `nice` there (its
  `ps` has no `-o`).
- Cargo's default job count can take a 32 GB machine past 80% of its memory. Per crate
  (`-p <crate>`), `CARGO_BUILD_JOBS=4` peaks at 35–60%. A `--workspace` build, clippy or test
  compiles `brigadier-daemon`, `brigadier-ipc` and `brigadier-desktop` side by side: 86% at
  8 jobs and 80–85% at 4, but 61% at `CARGO_BUILD_JOBS=1`. `tools/full-checks.sh` sets no job
  count, so run it as `CARGO_BUILD_JOBS=1 tools/full-checks.sh`.
- `pnpm test` runs four files at a time on Windows unless `--test-concurrency` says otherwise:
  each render test's headless browser is a dozen processes there, and a file per core takes
  the whole machine. The render tests take Playwright's headless shell from
  `%LOCALAPPDATA%\ms-playwright` before an installed Chrome (whose scrollbars take 16 px of
  every layout there), or `CHROME_BIN`.
- Some Rust tests make symlinks, which Windows allows only an administrator or with Developer
  Mode on (Settings › System › For developers). Without it they leave out just the checks
  through a symlink. Tests of what Windows doesn't do yet (the checks' commits through
  `/bin/sh`, a Codex thread's own home) are ignored there, each saying why.
- Git for Windows checks files out with CRLF line endings (`core.autocrlf`). `cargo fmt` and the
  generated bindings write LF, so `git status` may list files whose `git diff` is empty.

## Checks

Run all of them before you ask for a review:

```sh
tools/full-checks.sh            # fmt, generated types, build, clippy, Rust tests, typecheck, lint, app tests
tools/full-checks.sh --cross    # also Linux and Windows clippy of the cross-buildable crates and the desktop shell
tools/full-checks.sh --live     # then the live smoke with the real Claude and Codex CLIs (spends tokens)
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

On macOS the computer-use helper is checked in its own workspace too (`cd helpers/computer-use`,
then fmt, clippy and `cargo test --locked --workspace`, with
`CARGO_TARGET_DIR=<checkout>/target/computer-use`). The live smoke (`tools/live-smoke/`) drives
the thread with the real CLIs; see its README.

CI runs formatting, the bindings check, typecheck, lint and the app tests, then builds, clippies
and smoke-launches the app on macOS, Windows and Linux. It runs the computer-use helper's tests
on macOS, but not the main workspace's Rust tests: run them yourself.

`pnpm lint` includes `brigadier/no-raw-design-values`: component code may only use theme tokens
(no raw colors, arbitrary Tailwind values or numeric inline styles). Raw values live only in
`apps/desktop/src/styles/tokens.css`.

Code that is gated to one platform (`cfg(unix)`, `cfg(target_os = "macos")`) must gate its
helpers and imports the same way, or Linux and Windows clippy flags them as unused.

## Launch smoke check

The smoke check launches the app and judges it against the performance budgets (PLAN.md §4):

```sh
pnpm tauri:debug-app
BRIGADIER_DATA_DIR=$(mktemp -d) "target/debug/bundle/macos/Brigadier Dev.app/Contents/MacOS/brigadier-app" --smoke
```

For a test build, use `pnpm tauri:debug-app` and `target/debug/bundle/macos/Brigadier Dev.app`
(on Windows, `target/debug/brigadier-app.exe`: see [Windows](#windows)).
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

A budget miss there never fails the job: benchmarks are a report, not a gate (§5 of
`docs/STRIP-DOWN-PLAN.md`). It prints `PERF REGRESSION: <metric> <measured> vs limit <limit>`
instead, and the workflow opens or updates a tracking issue from those lines. Only a launch that
produced no report at all (the app did not launch, or crashed before writing one) fails the job.

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

On Windows, once Connect has installed Claude Code, it adds `%USERPROFILE%\.local\bin` to the
user's Path (`HKCU\Environment`), keeping the value's type and text, and announces the change
(`crates/daemon/src/user_path.rs`): Claude Code's `install.ps1` doesn't. The machine's Path is
never written. Its tests use a scratch `HKCU\Software\BrigadierTest-<random>` key, and
Connect's tests write the folder to a file instead.

To try Connect's sign-in in a test run whose `HOME` is a scratch folder, start the default
browser first. Codex (0.163.0) ignores `BROWSER` on macOS and opens its sign-in page itself, with
the sign-in's environment: a browser that isn't running yet then starts with the scratch `HOME`
and can't find its keychain. In the app that environment is the user's own, so it doesn't matter
there. Claude Code's page opens through Brigadier, with the account's real home.
`tools/sign-in-timing.py` changes only the agent's config folder, not `HOME`.

The Inspector's **Providers** tab (Settings › Inspector) shows each CLI's login state, version,
live model list and remaining quota. From it you can start, resume, fork, steer, interrupt,
stop and close raw sessions, answer their approval requests, simulate a usage-limit event and
replay recordings. Recordings are scrubbed of personal data as they are written. The fixtures
in `crates/providers/fixtures/` ship with the app.

## The MCP and skill registry

`crates/registry` reads every MCP server and skill on the machine into one catalog: Claude
Code's, Codex's (a trusted project's layers included), Claude Desktop's and Cursor's servers,
and the skills in `~/.agents/skills`, `~/.claude/skills`, `$CODEX_HOME/skills` and a trusted
project's `.claude/skills` and `.agents/skills`, and the skills and `.mcp.json` servers of the
Claude Code and Codex plugins that are on (`crates/registry/src/plugins.rs`). It only reads those
configs, and a watcher refreshes it. Ids are source-qualified (`claude:user:railway`,
`codex:project:<path>:x`, `desktop:pencil`, `codex:plugin:<plugin>@<marketplace>:skills/x`). Whether an agent has a server or skill by itself is decided per session
(agent, project) by `Catalog::server_native` and `Catalog::skill_native`.

`crates/core/src/manager/integrations.rs` uses it: a worker keeps its agent's own servers and
gets the ones its task's `integrations` name; every worker gets the switched-on skills its agent
doesn't find by itself. Settings' `mcpServersOff` and `skillsOff` switch a catalog id off for
every thread and worker. A switch takes effect at each session's next start or resume, not in
a running one. Env and header values go only into the CLI's environment under an alias; the
catalog, IPC, logs and events carry their names. `docs/research/phase7-probes.md` records the
CLI behaviour this rests on.

## Development-only switches

None of these exist in a release build.

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
- Session notifications use a small macOS UserNotifications adapter under the app's
  bundle identity, retaining a stable notification ID and session activation payload.
  `notifications.rs::mac` is the shell's other narrowly scoped unsafe exception:
  its C strings live through each call, Objective-C copies them, and callback tickets carry
  no borrowed pointers. The desktop notification plugin handles the other platforms.
- Computer use's setup window drags the helper app into a list in System Settings, which a web
  page can't do: a small AppKit drag (`computer_setup.m`) starts from under the pointer. The
  same file shows the window on the Space the user is on, also over a full-screen app, without
  activating Brigadier. `computer_setup.rs::native` is a narrowly scoped unsafe exception for
  those two calls: the window pointer is the setup window's own, on the main thread, and the
  path's C string outlives the call.
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
pnpm tauri build --bundles app --target universal-apple-darwin
```

Local test builds use `--bundles app` only; after a failed DMG step, run `hdiutil info`, detach its volume with `hdiutil detach <device>`, then delete its `rw.*.dmg` file.

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
([helpers/computer-use/README.md](../helpers/computer-use/README.md)).

Releases come from `.github/workflows/nightly.yml`. Every night it signs, notarizes and publishes
main as the `nightly` prerelease, which installed nightly builds update from, with the Windows
installer beside the macOS archive when the Windows build passed. A `v*` tag whose
version matches `tauri.conf.json` (`v0.1.0-alpha` for `0.1.0`) builds the universal app the same
way and publishes that tag's release, marked latest, with the DMG, the updater archive, its feed
(`latest.json`) and `SHA256SUMS`; installed release builds update from it. A released tag is never
moved: a fix ships as the next version. Then bump the cask in
[syncra-studio/homebrew-tap](https://github.com/syncra-studio/homebrew-tap) (`version` and the
DMG's `sha256`) and the version in `scripts/install.sh`.

## Build and install (Windows)

```sh
CARGO_BUILD_JOBS=4 CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16 \
  pnpm tauri build --bundles nsis -- -p brigadier-desktop -p brigadier-daemon
```

These are the nightly Windows job's release settings (no cross-crate LTO); on a 32 GB machine 4
jobs keep the build under 80% of memory. Tauri downloads NSIS and the WebView2 bootstrapper into
`%LOCALAPPDATA%\tauri` on the first bundle. The installer is
`target/release/bundle/nsis/Brigadier_<version>_x64-setup.exe`. It holds `brigadier-app.exe` (the app) and
`brigadierd.exe` (the daemon, which is also the agents' `BROWSER` on Windows) with the icons of
`icons/icon.ico`, and embeds the WebView2 bootstrapper, which installs the runtime only where it
is missing. The macOS-only computer-use helper isn't in it.

It installs for the current user, without an administrator's prompt, in
`%LOCALAPPDATA%\Brigadier` (or another folder on its directory page; `/S` installs silently and
`/D=<folder>`, last and unquoted, names the folder). It adds Start menu and desktop shortcuts,
the user's uninstall entry (`HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\Brigadier`)
and `HKCU\Software\SYNCRA SRL\Brigadier` with the folder; it doesn't change `Path`, and `/S`
doesn't start the app. Its uninstaller (Settings › Apps › Installed apps, or `uninstall.exe` in
that folder) removes the shortcuts, the uninstall entry and the program files only. The data in
`%LOCALAPPDATA%\Brigadier\Data` stays, as do `HKCU\Software\SYNCRA SRL\Brigadier` and the
webview's folders `%APPDATA%\ai.brigadier.app` and `%LOCALAPPDATA%\ai.brigadier.app` unless its
*Delete the application data* box is ticked (a silent `/S` run never ticks it). Updates run the
same installer (`/UPDATE`) over the installed one; a reinstall keeps the data too. Over a build
from before the app was `brigadier-app.exe`, Tauri's installer would delete the old main binary's
name, `brigadier.exe`, after copying the new files, which is now the command;
`src-tauri/installer-hooks.nsh` makes it forget that name first.

The installer isn't Authenticode-signed yet: Windows SmartScreen warns before it runs, and
`scripts/install.ps1` accepts it on its checksum alone. Signing needs a code-signing certificate
for SYNCRA SRL, named in `tauri.windows.conf.json` (`bundle.windows.certificateThumbprint` for
one in the build machine's certificate store, or `bundle.windows.signCommand` for a signing
service). The nightly's Windows job uploads the installer and its `SHA256SUMS` as the `installer-Windows`
artifact on every run. With the updater key secrets (`TAURI_SIGNING_PRIVATE_KEY`,
`UPDATER_PUBKEY`) it builds it with the nightly's or the tag's version and feed, and adds the
updater's `.sig` and a `windows-x86_64` feed entry (`latest-windows.json`). Every platform's
nightly takes its version stamp from the gate job, so the macOS and Windows builds of one run
carry the same version. The publish job uploads the installer to the `nightly` prerelease and
adds its entry to `latest.json` when its version is the macOS build's; installed Windows
nightlies then update themselves. The updater checks the installer's updater signature and runs
it in passive mode (`/UPDATE`, a progress window only) as the app exits; the download carries no
mark of the web, so SmartScreen doesn't hold it. The installer reopens the app only after
**Restart to update**. A tag's release still takes the macOS files only.

`scripts/install.ps1` is install.sh's counterpart: `irm <raw URL>/install.ps1 | iex`, or
`powershell -File scripts\install.ps1`. It downloads `SHA256SUMS` and
`Brigadier-<version>-x64-setup.exe` from the release (`BRIGADIER_RELEASE_URL` reads them from
elsewhere) and checks the checksum, refusing a missing entry or a mismatch. It refuses an
Authenticode signature that isn't valid or doesn't name SYNCRA SRL; until Windows releases are
signed it accepts an unsigned installer and says it was verified by its checksum only. It asks
before it replaces an installed Brigadier, refuses while Brigadier runs from there, and runs the
installer silently into `BRIGADIER_INSTALL_DIR`, the installed folder or
`%LOCALAPPDATA%\Brigadier`. A release bumps its version with install.sh's.

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

On Windows `scripts/uninstall.ps1` does the same in Windows PowerShell 5.1, with `-KeepData`,
`-DataDir PATH`, `-AppId ID`, `-App FOLDER`, `-Dev`, `-DryRun` and `-Help`:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\uninstall.ps1 -AppId ai.brigadier.app -DryRun
powershell -ExecutionPolicy Bypass -File scripts\uninstall.ps1 -AppId ai.brigadier.app
```

Its default data directory is `$env:BRIGADIER_DATA_DIR` or `%LOCALAPPDATA%\Brigadier\Data`, and
its default app is the installed folder (from its uninstall entry, else
`%LOCALAPPDATA%\Brigadier`); an app already removed in Settings › Apps is skipped, and the folder
it left for the data goes once that is empty. Only `ai.brigadier.app` matches the installed
app's uninstall entry. It reads the event store with Windows' own `winsqlite3.dll`, counts
junctions and other reparse points as symlinks, and moves to the Recycle Bin what uninstall.sh
moves to the Trash: the data, `%APPDATA%\<id>` and `%LOCALAPPDATA%\<id>`, and the recorded
session files. The app goes by its own uninstaller (`uninstall.exe /S`), which leaves a kept
data folder in place; then the script removes the installer's `HKCU\Software\SYNCRA SRL\Brigadier`
key, as uninstall.sh trashes the app's Preferences plist. Windows has no keep-awake rule or
microphone permission to reset, and Brigadier makes no session temp folders there.
