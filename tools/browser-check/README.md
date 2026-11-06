# Browser checks

The kept-alive dev app and what checks it, for the browser's move to Electron. Everything runs on this
Mac without taking focus or showing a dialog, except `go.mjs`, which runs only when the user says go.
Scratch data and evidence go in `$BRIGADIER_CHECK_DIR` (default `$TMPDIR/browser-check`).

| Command | What it does |
|---|---|
| `build-dev-app.mjs` | `target/browser-check/Brigadier Dev.app`, id `ai.brigadier.dev`: stock Electron, checksum verified, cookie encryption fuse on, background-only, ad-hoc signed; builds the native tools |
| `bc.mjs up\|down\|wipe\|status\|restart [kill]\|targets` | The one kept-alive instance: starts `devd.mjs`, which owns the dev server, the app and its `--remote-debugging-pipe` |
| `bc.mjs up --no-pipe` | The same instance with no debugging pipe, opening its window as the shipped app does, for sign-ins by hand. Nothing can drive it: only `status` and `down` work. `down` quits it in order |
| `bc.mjs control <op> [json]` | A fixed operation in the main process (`apps/desktop/electron/devControl.mjs`) |
| `bc.mjs host <window> <js>` | Evaluates in a window's page, over the pipe |
| `validate-instruments.mjs` | Positive and negative controls for capture, recording and the watcher. Run it before trusting them |
| `interactions.mjs` | Section 1's interactions in the real front end. Scripted input, OS pixels |
| `gate.mjs idle [minutes] [cases]`, `gate.mjs stress [minutes]` | The section 1 gate: `behind minimised space shown` run together; `hidden` alone, since it hides every window |
| `drops.mjs` | Drops and notifications in the real window on the dev daemon. Scripted drag through Chromium's input layer (trusted events, real files), OS pixels |
| `repro-blank-state.mjs`, `space-probe.mjs` | The spike's blank state: each window state with and without `capturePage`, and a probe that waits for a real Space change |
| `real-pages.mjs --package <app>`, `--kept-alive` | Real public pages in a `<webview>`, each process's sandbox read from the OS, H.264 and AAC from a local file, a cookie followed to disk. CI runs it on all three OSes (`electron-real-pages.yml`). Real engine and network, scripted guest |
| `package-report.mjs <dist folder>` | Real download and disk sizes, and packaging facts. Mounts nothing |
| `go.mjs --plan`, `--preflight`, `--go` | Real pointer, cursor and focus, all in one go. Stops on any human input and restores the pointer. Real input comes from `target/browser-check/bin/Brigadier Check Input.app`, which holds the Accessibility grant; don't rebuild it, since an ad-hoc grant is tied to its hash |

## Real and stand-in windows

`createWindow` with `real: true` is the app: it talks to the daemon through the preload's `brigadierNative` (`src/ipc/native.ts`), with data in `$BRIGADIER_CHECK_DIR/daemon-data` and `HOME` in `daemon-home` unless `BRIGADIER_DEV_DAEMON_HOME` says otherwise. Any other window is a stand-in: the shell and daemon answered in the page (`electron/renderer/shellIpc.ts`), one set of sessions per window, which is what `gate.mjs`, `interactions.mjs` and `space-probe.mjs` run on.

## What each check can claim

- **Scripted**: `sendInputEvent` into the app or page. Proves the app and page respond, not that a real pointer gets there.
- **Real unfocused**: OS pixels and OS state with the app in the background.
- **Real focused**: `go.mjs` only.

## Things that bit

- `--remote-debugging-pipe` alone makes `navigator.webdriver` true in every page, with no debugger attached. Google then refuses sign-in after the email step ("This browser or app may not be secure") and may answer a search with its "unusual traffic" page. Nothing run over the pipe can show how Google treats the app: use `bc.mjs up --no-pipe`. Its bundle must not be `LSBackgroundOnly` for typing to reach it (`LSUIElement` instead, then re-sign), and a build puts `LSBackgroundOnly` back.
- A no-pipe instance is only for a window the user opens by saying go. It opens the window as the shipped app does: `BRIGADIER_CHECK_BOUNDS` parking didn't hold, and a 1280×800 window sat on the user's screen for 9 s at 20:56:57 UTC on 2026-11-06, which their rule against windows in their way (L8) forbids.
- Building the Tauri crate overwrites `target/debug/brigadierd` with its empty sidecar placeholder. A dev run uses `target/browser-check/sidecars`, from `build-native.mjs`.
- `devd.mjs` reads `paths.mjs` once: restart it (`bc down`, `bc up`) after changing a path or its environment. A main-process edit needs `pnpm electron:main` and `bc restart`.
- `log` is a builtin in zsh: use `/usr/bin/log`, or your reads return nothing with no error you'd notice.
- A position given when a `BrowserWindow` is made gets clamped to the screen, and a position past the screen's edge snaps back. `openShell` places, shows at opacity 0 and checks. The watcher alerts on any dev window larger than a strip.
- `capturePage` doesn't follow what's on screen: it makes a hidden page paint while its guests stay hidden. Checks read OS pixels.
- A covered window's OS capture is its last frame. Pixels alone don't prove a page is alive: the gate paints a colour chosen after the return.
- SkyLight's calls that move a window to another Space are refused for other processes. A tool can't enter that state; `go.mjs` uses the real shortcut.
- Editing a file the dev server serves reloads the page, which destroys its guests: not during a run. Its client also reloads the page by itself when its socket reconnects, so the gate needs `BRIGADIER_NO_HMR=1` on both the instance and the gate.
