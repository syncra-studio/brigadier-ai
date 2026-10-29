# Vendored upstream: the computer-use driver

This folder is a copy of the macOS, Windows and Linux backends of the MIT-licensed driver at
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
  `../patches/`: `platform-macos`, `platform-windows` and `platform-linux`, what
  they share, and `cua-driver-uia`, the Windows uiAccess worker (vendored but not
  built or shipped by Brigadier yet).
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
autostart and HTTP transport), the SDK and bindings, the optional perception
add-on and the test and tooling crates.

## Release

<!-- sync:begin (written by scripts/sync-upstream.sh) -->
| | |
|---|---|
| Repository | https://github.com/trycua/cua |
| Tag | `cua-driver-rs-v0.36.0` |
| Commit | `6d6c734c127025fb39ad5d585a10382808b39dc1` (2026-10-16) |
| Path in the repository | `libs/cua-driver/rust` |
| Crates | `cua-driver-core` `cua-driver-contract` `platform-macos` `platform-windows` `platform-linux` `cua-driver-uia` `cursor-overlay` `cua-cursor-motion` `pip-preview` |
| Also copied | `Skills/cua-driver/` `LICENSE.md` `THIRD_PARTY_NOTICES.md` |
| Files | 493 (sha256 of PRISTINE.sha256: `81b9371a20caaacd253d99df34da50feffb141146ff2217a696eb8a3d8bb3979`) |

Patches applied, in order:

- `patches/0001-state-dir.diff`: one state folder for the driver's own config (CUA_DRIVER_STATE_DIR), so an embedding app keeps it beside its other state instead of ~/.cua-driver.
- `patches/0002-registry-wrap.diff`: ToolRegistry::wrap lets an embedding app put its own checks around a tool in place, so direct calls, run_steps steps and the registry's own re-routes all pass them.
- `patches/0003-restore-policy.diff`: a host restore policy (focus_steal::set_restore_policy) asked before every restore the driver makes, so an embedding app can leave the user's own switches and system prompts in front.
- `patches/0004-host-authorized-wrap.diff`: ToolRegistry::wrap_host_authorized lets an embedding app that replaces a tool with its own implementation authorize the calls the driver admits only in unrestricted mode, with its own check, so unrestricted mode can stay off.
- `patches/0005-off-space-window-resolver.diff`: a host off-Space window resolver (ax::bindings::set_off_space_window_resolver) used in place of the built-in remote-token probe, so an embedding app can reach windows on another Space with element ids it remembers and a scan that resumes where it stopped.
- `patches/0006-window-change-report.diff`: a host setting (CUA_DRIVER_WINDOW_CHANGE_REPORT_MS) for how long an action's result waits for a window change, so an embedding app whose agents read the window again after each action gets results back sooner while the focus-steal protection keeps its whole timeout.
- `patches/0007-in-process-info-plist.diff`: Info.plist values are read in process instead of by a plutil process per key, so list_apps and the running-app snapshot the registry takes before each launch_app take tens of milliseconds instead of most of a second.
- `patches/0008-web-content-off-space.diff`: the Chromium/Electron accessibility wait also looks for web content in the app's windows on another Space, so the first read of such an app whose windows are all off-Space returns at once instead of waiting out the 4 s timeout (up to three times per app lifetime).
- `patches/0009-space-lookups-where-needed.diff`: Space lookups only where they are read: a one-window Space check asks about that window only, and the exact-target check and the running-app list skip them, so typing into an app takes about 0.1 s less with a hundred windows open.
- `patches/0010-key-edit-after-insert.diff`: a host switch (document_edit::set_key_edit_after_insert) for the net-zero key edit after an AXSelectedText insert into a document window, so an embedding app whose inserts already mark the document edited can keep ⌘Z undoing the insert itself.
- `patches/0011-menu-window-confirmation.diff`: v0.36.0 treated app-menu membership as window ancestry, so a command addressed to one window could act on a sibling key window; require the addressed window to be the app's focused window before admitting an app-menu element.
- `patches/0012-semantic-insert-before-keyboard-refusal.diff`: v0.36.0 refused a retained-element AX text insert when key-window priming failed; keep the semantic insert available while refusing implicit targets, terminals, synthesized-key fallback and document key edits without keyboard permission.
- `patches/0013-native-accessibility-without-web-wait.diff`: v0.36.0 waited up to four seconds for a web tree in native apps accepting AXEnhancedUserInterface; require a Chromium-family framework in the addressed process's bundle for that wait, preserving Chromium and Electron first-read readiness.
- `patches/0014-popup-without-speculative-keys.diff`: v0.36.0 selected Chromium popup labels with speculative pid-routed typeahead, which can reach a sibling without confirmed key routing and commit intermediate or invalid prefix selections; use the existing exact-match semantic AX popup route instead.
- `patches/0015-preserve-original-key-target.diff`: v0.36.0 key-window cleanup could pick a sibling after priming a target that was already AXFocusedWindow but lacked a focused element; when the original window is the target, restore nothing and leave that window focused.
- `patches/0016-helper-windowserver-identity.diff`: the WindowServer diagnostic recognizes Brigadier's helper bundles (Brigadier Computer Use.app and BrigadierCu.app) and BRIGADIER_CU_EMBEDDED=1, and uses the helper's own launch setup instead of recommending the upstream app daemon.
- `patches/0020-windows-launch-foreground.diff`: watch every Windows launch before shell activation, hand the foreground back only for the target without intervening user input, and report unconfirmed restoration honestly.
- `patches/0021-windows-exact-browser-launch.diff`: pass URLs only to the named browser (or the associated default executable), quote each argument, and return only a window whose visible UIA document confirms the requested URL.
- `patches/0022-windows-targeted-page-reads.diff`: route Windows page URL hints to a unique visible UIA document, refuse missing or ambiguous targets and unbound CDP ports, and discard reads when the tab changes.
- `patches/0023-windows-user-protocol-default.diff`: resolve protocol executables through the current user's default association instead of a plain shell class lookup.
- `patches/0024-windows-launch-running-evidence.diff`: do not report a shell launcher stub or a zero process id as a confirmed running app; return an explicit unconfirmed-launch error instead.
- `patches/0025-windows-hosted-app-termination.diff`: return the verified hosted Store app pid from launch, retain runtime ownership on that process, protect ApplicationFrameHost at termination, and require confirmed process exit instead of treating wait timeouts as success.
- `patches/0030-windows-semantic-click-readback.diff`: prefer UI Automation patterns and WindowPattern.Close for Windows clicks, with bounded window/control readback and honest unconfirmed outcomes.
- `patches/0031-windows-winui-menu-path.diff`: resolve WinUI/XAML menu bars and detached owned popups in the background, invoke native patterns, and collapse menus opened by a failed call.
- `patches/0032-windows-key-delivery-honesty.diff`: report Windows press_key and hotkey dispatch as unconfirmed on PostMessage, SendInput and UIA accelerator routes, including structured results.
- `patches/0033-windows-input-deadlines.diff`: bound Windows semantic input with the existing UIA single-flight deadline, prevent late mutations and duplicate fallback, and test portable routing and close readback decisions.
- `patches/0040-diff-returned-element-count.diff`: refresh returned_element_count after projecting diff elements so typed observations accept changed and unchanged reads.
- `patches/0041-batch-token-window-inheritance.diff`: run_steps resolves token snapshot windows before planning so observe:{} and subsequent steps inherit the addressed window; reject conflicting explicit targets.
- `patches/0042-windows-hosted-window-identity.diff`: supplement Windows enumeration with the current desktop and attribute each UWP frame to its own hosted app pid; accept the app/frame pair for reads and actions.
- `patches/0043-windows-elevated-window-read-refusal.diff`: refuse reads before UIA silently returns a title-bar-only tree for elevated windows; reads and input explain the privilege boundary and remedy plainly.
- `patches/0044-single-line-query-rows.diff`: flatten multiline display rows before filtering on all platforms, keep native values intact, and project matches through actual parent rows without stale sibling ancestors or duplicate sections.
- `patches/0050-off-space-picture-and-window-menu.diff`: reject black off-Space window bodies instead of using them as visual grounding, and refuse Window menu Space switches with an explicit user step; accessibility reads and semantic actions remain available.
- `patches/0051-window-scoped-pixel-press.diff`: resolve background pixel presses within the exact AX window, including off-Space controls and actionable ancestors; only advertised AXPress is attempted, and acceptance remains explicitly unconfirmed.
- `patches/0052-remote-open-save-panel.diff`: resolve unlisted Open/Save AX windows through client descendants and the AppKit panel service by exact CGWindowID on every Space; route panel key equivalents to their AX server without Chromium authentication and exclude client app menus.
- `patches/0053-preserve-off-space-window-commands.diff`: limit the off-Space Window menu refusal to selecting a named application window; preserve background window commands such as Zoom and tab navigation.
- `patches/0070-windows-browser-read-deadlines.diff`: bound browser verification and targeted page reads, share the UIA single-flight gate, and balance COM apartments.
- `patches/0071-windows-input-timeout-and-focus.diff`: bound click readback, suppress late menu cleanup, preserve safe WPF routing and launch activation warnings.
- `patches/0072-windows-background-pointer-refusal.diff`: refuse MSAA real-mouse clicks in background mode and keep the Chromium fallback off SendInput.
- `patches/0080-windows-foreground-observation.diff`: Record brief and delegated foreground activations through bounded WinEvents, preserve late-window hand-back, and remove synthetic-key UWP restoration hidden from launch reports.
- `patches/0081-windows-background-input-guard.diff`: Guard all posted key/text actions and dialog-owner activation, with overlapping style leases and late hand-back for run_steps and Save As dismissal.
- `patches/0082-windows-launch-focus-evidence.diff`: Bind broker and resolved app pids to launch observations, sample active and focus wording together, and keep confirmed opening distinct from unconfirmed foreground preservation.
- `patches/0090-windows-caret-insertion.diff`: type_text uses caret insertion into a real editor; whole-document replacement remains exclusive to set_value.
- `patches/0091-windows-elevated-kill-refusal.diff`: kill_app checks process token integrity before termination and shares the administrator refusal used by reads and actions.
- `patches/0092-windows-common-dialog-save.diff`: Native common-dialog Save sends the exact IDOK WM_COMMAND before UIA Invoke and confirms only observed dialog closure.
- `patches/0093-windows-winui-caption-close.diff`: Recognize WinUI caption Close outside TitleBar ancestry and send top-level WM_CLOSE with bounded closure readback.
- `patches/0094-windows-chromium-text-insertion.diff`: Route Chromium caret text to its unique visible renderer child after guarded exact-field focus, without a whole-value write.
- `patches/0095-windows-input-delivery-validation.diff`: Keep activation shielding through synchronous character delivery and readback; cover native edit selection and dirty state in Windows tests.
- `patches/0096-windows-input-target-bounds.diff`: Validate UIA field rectangles before choosing editor or renderer children, and preserve exact administrator refusal wording.
- `patches/0097-windows-resolved-input-guard.diff`: Arm the shared background guard after resolving the actual text HWND and retain click target/owner protection through command readback.
- `patches/0098-windows-send-message-transport.diff`: Distinguish synchronous Windows SendMessageTimeout delivery from PostMessage in action transport identities and normalization, without upgrading unconfirmed effects.
- `patches/0099-windows-caption-identity-and-focus-refusal.diff`: Recognize both WinUI caption Close ids while excluding tab/document ancestry, and explain failed Chromium background focus plainly.
- `patches/0100-windows-settings-protocol-window.diff`: resolve pidless Settings protocol launches through the exact SystemSettings-owned native frame without repeated global UIA waits.
- `patches/0101-windows-edge-omnibox-url.diff`: confirm Edge targets from browser-chrome address values, bind only one visible document, and revalidate the same URL after each targeted read.
- `patches/0102-windows-page-text-run-boundaries.diff`: preserve whitespace within UIA text runs and word boundaries across inline nodes rather than trusting Chromium aggregate TextPattern concatenation.
- `patches/0103-windows-explorer-folder-launch.diff`: resolve File Explorer by name to explorer.exe with quoted folder arguments and describe unsupported app registrations without browser-specific wording.
- `patches/0104-windows-fresh-launch-documents.diff`: request fresh browser windows, exclude pre-existing URL matches, open a unique blank Notepad file and confirm its selected tab, and disclose additional/restored tabs.
- `patches/0105-windows-launch-read-evidence-hardening.diff`: reject conflicting document/address evidence, avoid returning prior browser windows, and distinguish the visible confirmed URL from other requested tabs.
- `patches/0106-windows-browser-launch-confirmation-facts.diff`: scan native browser windows once per poll and report an opened window separately from its unconfirmed requested URL without reading unrelated content.
- `patches/0107-windows-notepad-file-launch-route.diff`: pass fresh Notepad files through its executable alias and avoid claiming URL confirmation when only the app window was verified.
- `patches/0108-windows-page-and-tab-read-fidelity.diff`: read editable values rather than labels, count only browser-chrome tabs, and report unavailable tab counts as unknown rather than implying no restoration.
- `patches/0109-windows-fresh-document-launch-cleanup.diff`: keep the native UIA call explicitly unsafe and remove untouched fresh Notepad files when launch is refused or fails before Windows accepts it.
- `patches/0110-plain-administrator-refusal.diff`: State the administrator refusal in one plain sentence, followed by a labelled hint shared by reads and input.

- `patches/0111-windows-settings-window-close.diff`: Allow only an exact unmodified Settings window Close through WM_CLOSE, refuse closing the front window, and confirm only observed window destruction.


- `patches/0112-windows-walk-completeness.diff`: Report complete UIA walks truthfully and mark depth cuts, provider enumeration failures, time/node stops and partial MSAA sources as incomplete.

- `patches/0113-kept-ancestor-tree-depths.diff`: Indent rows by actual retained ancestors, observing omitted native siblings before query filtering, while keeping native depths for traversal limits.


- `patches/0114-windows-complete-observation-rows.diff`: Include read-only UIA rows in internal verification observations before claiming complete coverage, preserving public addressable rows and documenting Settings window Close.

- `patches/0115-settings-close-foreground-owner-proof.diff`: Refuse Settings window Close when the target owns the foreground popup or foreground ownership cannot be confirmed, before changing styles and again before dispatch.

- `patches/0120-windows-launch-timeout-honesty.diff`: Bound packaged-app activation and preserve submitted documents across uncertain launch outcomes; a timeout never claims cancellation.

- `patches/0121-windows-preserve-foreground-selection.diff`: Respect physical window selection after pointer injection, avoid disabling the front window, and refuse terminating the foreground app or its popup owner.

- `patches/0122-windows-caret-provider-deadline.diff`: Run caret insertion and its provider reads under one 30-second single-flight deadline; check cancellation before focus and each character so late provider returns cannot continue typing.

- `patches/0123-windows-untargeted-page-deadline.diff`: Apply the shared four-second single-flight UIA deadline to page reads without URL hints as well as matched reads.

- `patches/0124-windows-termination-frame-proof.diff`: Refuse termination when a foreground shared Store frame cannot prove its hosted app identity instead of treating ApplicationFrameHost as an unrelated app.

- `patches/0125-windows-caret-worker-com-apartment.diff`: Initialize and balance COM on the dedicated caret-insertion deadline thread before using retained UIA interfaces.

- `patches/0130-windows-notepad-tab-name-confirmation.diff`: Match Notepad tab accessible names ("x.txt. Unmodified.") when confirming a fresh launch document, and skip unreadable windows or tabs instead of abandoning the whole check.



- `patches/0131-respect-physical-input-and-bound-foreground-ownership.diff`: Revoke every foreground lock holder on physical input, preserve queued activation evidence without claiming a flash, restrict broker attribution, and refuse pointer-moving background touch drags.

- `patches/0132-windows-explorer-local-folder-arguments.diff`: Normalize absolute local Explorer folder arguments to Windows separators so forward slashes cannot be parsed as switches that open the wrong folder.

- `patches/0133-windows-caption-close-without-disabling.diff`: Keep native caption Close enabled under the revocable no-activation guard so Notepad processes WM_CLOSE instead of ignoring it while disabled.

- `patches/0134-windows-uia-cached-leaf-coverage.diff`: Treat a successful null cached-child array as a leaf while preserving real COM failures, so complete Windows trees are not mislabeled as failed provider walks.

- `patches/0135-windows-task-manager-direct-launch-and-singleton-refusal.diff`: Resolve Task Manager to the trusted system executable and use direct non-activating process creation, refusing reuse of an existing singleton and refusing launch without a valid prevention lock.

- `patches/0136-windows-input-shared-frame-attribution.diff`: Require the exact guarded frame for shared or unreadable host processes instead of treating any window in ApplicationFrameHost as the input target.

- `patches/0137-windows-yield-on-physical-release-and-keep-event-evidence.diff`: Yield foreground protection on physical releases and wheel input as well as presses, preserve queued pre-input events at zero or wrapped timestamps, and report user-input interruption explicitly.

- `patches/0138-windows-refuse-pointer-moving-injection-fallback.diff`: Remove synthetic pen and touch fallbacks that cannot guarantee a stationary system pointer; retain earlier semantic UIA and native posted-message routes and return an explicit no-input refusal for the unsafe rung.

- `patches/0139-windows-share-disabled-window-leases.diff`: Share disabled-window ownership across overlapping UIA workers so only the final holder restores the original state, and ignore recycled HWND identities.

- `patches/0140-windows-window-list-platform-wording.diff`: Keep Windows list_windows descriptions and replies about Windows window metadata without claiming unavailable macOS frameworks.

- `patches/0141-windows-chromium-caret-insertion-without-native-focus.diff`: Use exactly bound CDP caret insertion for Chromium page inputs with full value/caret readback, refuse unavailable transports before any native focus or partial text, and unwind emulation on cancellation.

- `patches/0142-windows-truthful-debugging-launch-refusal.diff`: Explain Windows DevTools launch refusal without directing agents to a browser-setup action absent from the offered tools.

- `patches/0143-windows-windowless-console-probes.diff`: Start the Windows helper's PowerShell and cmd probes with CREATE_NO_WINDOW so a windowless helper never opens a new, activated console window (Windows Terminal) during launches, browser checks or diagnostics.

- `patches/0144-windows-task-owned-browser-background-transport.diff`: Give ordinary Chrome/Edge launches a lifecycle-owned isolated profile with a proven DevTools endpoint; open pages as inactive off-screen CDP windows placed behind the active window, keep the launch lock without restoring focus, disable Edge implicit account sign-in, and close the windowless browser after its last window.

<!-- sync:end -->

## Notes for v0.36.0

The latest plain SemVer driver release checked on 2026-10-18 is v0.36.0.
Upstream labels these releases as pre-releases only to keep its monorepo's
repository-wide latest pointer stable; plain SemVer versions are stable.

All ten original local patches remain necessary. Patches 0003 and 0009 were ported:
0003 checks the host restore policy before the new AX window-order restore
path, and 0009 preserves upstream's new pid/layer filters while limiting
Space queries to the windows the caller reads. The other eight original patches apply
unchanged. Upstream's Chromium first-read fix complements patch 0008; it
does not add the off-Space window probe that patch provides.

Review found five v0.36.0 regressions. Patches 0011–0013 keep menu
commands tied to the addressed key window, preserve semantic AX insertion
when keyboard priming fails, and skip the Chromium wait for native apps.
Patches 0014–0015 remove speculative popup-label keystrokes and keep an
already-focused target window focused when key priming ends. These two
additional problems were found in the follow-up review. Each new patch
includes a regression test that failed before its fix. Run these
vendored unit tests with the helper's dependency versions (Cargo prunes the
helper-only lock entries without fetching newer dependencies):

```sh
cp Cargo.lock upstream/Cargo.lock
cargo test --offline --manifest-path upstream/Cargo.toml -p platform-macos --lib regression_
```

The v0.35.4 and v0.36.0 macOS release notes were reviewed, including
background typing, menus, drags, off-Space reads, focus restoration and the
new WindowServer health check. Patch 0016 recognizes Brigadier's helper bundle
and embedded variable for that diagnostic, with remediation through its own
launch setup (issue #317). Existing tool classifications are retained;
drag remains withheld. Driver arguments and foreground wording are checked
by the driver snapshots. Per issue #270, signed warm/cold benchmarks, focus
checks and the reliability matrix belong in the nightly/release report and
do not gate this sync. Repository-wide checks run once on the integrated
tree; this change runs the helper's own build, lint, tests and verification.

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
