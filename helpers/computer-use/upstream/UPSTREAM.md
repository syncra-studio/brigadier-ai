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

- `patches/0145-windows-classic-dialogs-outside-xaml-shield.diff`: Exempt classic #32770 dialogs owned by XAML apps from the EnableWindow activation shield: disabling them made UIA report their buttons as not enabled (0x80040200) and restoring the lease re-enabled dialogs beneath a newer modal.

- `patches/0146-windows-press-key-to-control-window-without-focus.diff`: Post modifier-free background press_key input straight to an addressed control that has its own child window instead of calling UIA SetFocus, which activated hosts such as Notepad's editor before the hand-back.

- `patches/0147-windows-refuse-self-elevating-task-manager.diff`: Refuse launching the system Task Manager before any process starts: even a direct non-elevated start under the activation lock is raised to administrator, leaving a window over the user's work that the helper cannot read, move or close.

- `patches/0148-windows-windowless-ffmpeg-tools.diff`: Start ffmpeg, ffprobe and the Windows where probe through one helper that sets CREATE_NO_WINDOW on Windows, so recording, rendering and probing never open an activated console window; other platforms keep plain Command::new.

- `patches/0149-windows-contain-task-browsers-and-refuse-orphans.diff`: Start task-owned Chrome/Edge suspended inside a kill-on-close job before resuming, end that job when its endpoint proof fails or its lifecycle retires, and refuse typing into any browser running a marked task profile that no live lifecycle owns before any generic endpoint discovery.

- `patches/0150-windows-recheck-input-after-endpoint-revalidation.diff`: Re-read the physical-input generation immediately after the awaited task-endpoint revalidation and before Input.insertText, so user input arriving during that await stops background insertion before any text is sent.

- `patches/0151-windows-task-manager-refusal-reason.diff`: Correct the Task Manager refusal reason with the user-approved RunAsInvoker evidence: it relaunches itself elevated even from a non-elevated start, so the helper could not read or move its window and that window would cover the user's work.

- `patches/0160-windows-enable-window-outside-shared-lock.diff`: Call EnableWindow outside the shared disable-lease lock (reading the initial state with IsWindowEnabled, which sends no message), so one hung target window blocks only its own action instead of every later guarded action and every guard's release.

- `patches/0161-windows-chromium-insert-needs-input-monitoring.diff`: Refuse background Chromium text insertion before any input when the user's own keyboard and mouse input cannot be monitored (the interruption checks would otherwise pass silently), and say in the reply when the active window changed during the insertion.

- `patches/0162-windows-bounded-browser-powershell-probes.diff`: Bound the browser signature check (15 s, the child killed on expiry) and the browser command-line query (10 s, kill_on_drop) so a slow revocation check or WMI query cannot stall launch_app or leave a PowerShell process behind.

- `patches/0163-windows-reuse-task-browser-without-second-start.diff`: When a live task-owned browser is re-proven for reuse, open the requested pages through its endpoint without starting the executable again; a second start hands the command line to the running browser, which opens and activates a new window over the user's.

- `patches/0164-windows-foreground-lock-refusal-reason.diff`: State the real reason a start_minimized launch is refused when the activation-prevention lock is not held: Windows may have refused it, or an overlapping action released it to hand the user's window back, in which case Windows was never asked.

- `patches/0165-windows-typing-stops-on-user-input.diff`: Background per-character typing on Windows (the WM_CHAR route native and XAML editors such as Notepad use) checks for the user's own mouse and keyboard input, pointer motion included, before every character and stops at once with a reply saying how much was typed; its deadline is sized to the text, and reaching it says how far typing got instead of reporting a provider timeout.

- `patches/0166-windows-launch-hands-front-back-and-stays-behind.diff`: Windows refuses SetForegroundWindow to a helper without a foreground right even when attached to the front thread, so a launched app that took the front kept it, and a window shown without activation still opened on top of the user's window. Keep the launched app's new windows under the user's window, hand the front back by minimizing the launched window once (Windows then activates the user's window; the app is shown again behind it), and say plainly when Windows refused.

- `patches/0167-windows-hotkey-closed-menu-accelerators.diff`: Search UIA-expandable menus and owned WinUI popups for hotkey accelerators, clean up menus opened by the search, and refuse unmatched shortcuts without keyboard injection.

- `patches/0168-windows-edge-page-typing.diff`: Accept exact data document URLs and Edge's uniquely bound address-bar evidence for background caret insertion, preserving endpoint ownership and exact page/field checks; explain page mutation refusals with element-token alternatives.

- `patches/0169-windows-typing-and-page-patches-formatted.diff`: Format the Windows typing and Edge page typing code from 0165 and 0168 so cargo fmt --check passes.

- `patches/0170-windows-explorer-arguments-and-closed-window-results.diff`: Normalize Explorer drive and UNC additional arguments while preserving URLs and other apps, allow two seconds for Settings close confirmation, and use exact confirmed window-destruction evidence for final run_steps observations.

- `patches/0175-windows-owned-document-and-browser-profile-cleanup.diff`: Remove untouched fresh Notepad files after Notepad exits or runtime teardown, and retire and delete exited task-browser profiles with a proven stale-owner sweep at startup while preserving live-session reuse.

- `patches/0180-windows-input-guard-dialog-hand-back.diff`: Never hand the front back to the target app's own window: an action that starts while the app's dialog has the front inherits the user's window from the live watch the app took it from, or restores nothing. Hand back from input targets by minimizing the root owner once the window that took the front is shown (a modal dialog cannot be minimized; the next visible window must be the user's), keep input targets' windows under the user's window once the action has ended (at the bottom when placing them under it does not take), and say in click replies when the app took the front. Launch hand-back and its cadence are unchanged.

- `patches/0181-windows-launch-hand-back-next-visible-window.diff`: Check the next visible window, not the next window, when a launched app is minimized to hand the front back: hidden IME and helper windows sit right under it, Windows activates the next visible window, and placing the user's window right under the app is ignored, so a launch refused to hand the front back. The owner route of 0180 now shares the same check.

- `patches/0176-windows-validated-profile-quarantine-and-process-exit.diff`: Rename exited task profiles by validated handle into pinned tombstones before deletion, and retain launched Notepad process handles to detect exit between polls while preserving saved documents.

- `patches/0182-windows-dialog-owner-hand-back-target-only.diff`: Minimize a dialog's root owner to hand the front back only when that owner is one of the target app's own windows, by the same rule that matched the dialog; a dialog owned by another process's window, such as the user's, is never minimized through it.

- `patches/0183-windows-action-note-had-the-front.diff`: Say "had the front", not "took the front", in an input action's reply note: an action that starts while the app's dialog is already in front (a displaced watch) observes it at once, and the app did not take the front during that action.

- `patches/0184-windows-notepad-refuses-user-windows.diff`: Refuse a Notepad launch while Notepad shows a window this helper did not open. Windows Notepad keeps all its windows in one process and opens a file as a tab inside a window it already shows; it has no command-line switch or activation argument for a window of its own. The file would land in the user's window, and while Notepad is in front Windows lets it bring a new window in front of the user's work. Windows that the helper's own Notepad launches opened stay reusable.

- `patches/0185-windows-hotkey-confirms-closed-window.diff`: A Windows shortcut that closes its own window (Ctrl+Shift+W in Notepad) says the window closed, instead of an error about a menu that closed with it; a shortcut sent to a window that is gone, or disabled behind its own dialog, says so in plain words.

- `patches/0186-windows-launch-front-never-left.diff`: When Windows reports a launched app's activation but the user's window was in front at every check and at reply time, a launch reply says so instead of calling it unconfirmed.

- `patches/0187-windows-dialog-owner-hidden-while-closing.diff`: A click that closes a target app's background dialog (Save as Cancel or Save) hides the dialog's owner window for the action and shows it again directly under the front window: Windows activates the owner when a modal dialog ends and raises it over the user's window, even with WS_EX_NOACTIVATE set.

- `patches/0188-windows-shortcut-menu-closed-by-its-command.diff`: A Windows shortcut found in a closed menu counts the menu as closed once its flyout window is gone: a command that opens a dialog (Notepad Ctrl+W, "Save changes?") can make the menu header unreadable, and the reply no longer reports the shortcut search as failed after its command ran.

- `patches/0189-windows-settings-refuses-user-window.diff`: Refuse an ms-settings: launch while a Settings window this helper did not open is one the user can see or switch to: Settings has a single window and the launch would change its page. A Settings frame Windows keeps cloaked on no virtual desktop after the user closed it is not the user's. Settings Close waits up to 8 s for the window to go, as Settings hid its frame 2.9 s after WM_CLOSE.

- `patches/0190-windows-launch-broker-event-front-at-reply.diff`: A launch whose only activation evidence is an unattributed ApplicationFrameHost event says whether the user's window was in front at reply time.

- `patches/0191-windows-browser-window-binds-by-exact-title.diff`: Typing into a browser page binds the addressed window when a second launch opened its window at the same place and one page title contains another: the window is the only one with the native bounds holding a page with the visible URL whose title is exactly the native title before " - <browser>".

- `patches/0192-windows-retired-browser-profile-name-ends.diff`: Retiring a task-browser profile passes the new folder name with a terminating NUL: Windows read past the name, the retired folder got stray characters after its nonce, and it was never deleted.

- `patches/0193-windows-chromium-first-read-waits-for-page.diff`: Reading a Chromium window waits briefly, within the read's budget, for its page document: Chromium builds the page accessibility tree only after the first UI Automation client asks, and the first read of a new Chrome page listed only the address bar.

- `patches/0194-windows-dialog-owner-hidden-for-owner-read-token.diff`: Clicking a background dialog's button through a token read from its owner window also hides the owner while the dialog closes: the owner's read lists the dialog's controls, and the click now looks up the window the element really lives in, so the closing dialog no longer raises itself and its owner over the user's window (4 ms, Notepad Save as Cancel).

- `patches/0195-windows-common-dialog-cancel-sends-idcancel.diff`: A click on a common dialog's Cancel button sends the button's own IDCANCEL WM_COMMAND to the dialog, as Save already sends IDOK, and resolves the dialog from the element when the token came from the owner's read: UI Automation Invoke on the background dialog's Win32 button activated the dialog in the app's own thread, which raised it over the user's window for 6 ms.

- `patches/0196-cua-driver-core-clippy-1-98-lints.diff`: Lint-only rewrites in cua-driver-core so `cargo clippy -p platform-windows --all-targets -- -D warnings` passes on toolchain 1.98: `as_chunks` instead of `chunks_exact` with a constant size, an explicit `truncate(false)` (the default) on the Windows history lock file, and one needless borrow. Behaviour is unchanged on every platform.

- `patches/0197-cursor-overlay-clippy-1-98-lints.diff`: Lint-only rewrites in cursor-overlay for the same check: `as_chunks_mut` instead of `chunks_exact_mut` with a constant size, and the Windows theme-store block ends in its value instead of a needless `return`. The macOS and Linux blocks are unchanged.

- `patches/0198-platform-windows-clippy-clean.diff`: platform-windows passes its own clippy with `-D warnings`: clippy's machine-applicable suggestions, `# Safety` docs on two unsafe scroll functions, a type alias, a test-only `cfg` on a helper only tests call, a removed unused duplicate of `tools::build_registry`, and scoped allows (four upstream signatures with eight arguments, and one comparison that keeps its lossy UTF-16 decode to match how the field value was read). Behaviour is unchanged.

- `patches/0199-windows-hidden-owner-restored-at-click-deadline.diff`: A click that times out shows again any owner window it hid: the caller restores what the provider worker hid when the 4 s click deadline passes, and a worker that arms after the deadline hides nothing, so a hung UI Automation call can no longer leave the app's own window hidden.

- `patches/0200-macos-health-accepts-brigadier-bundle.diff`: health_report's bundle check passes for Brigadier's helper app (ai.brigadier.computer-use and the development id), so a correctly set up helper no longer always reads as degraded.
- `patches/0201-macos-finder-reads-leave-out-desktop.diff`: A window-scoped read of a Finder window leaves out Finder's desktop scroll area, so the user's desktop icons are no longer listed as part of the window.
- `patches/0202-macos-text-area-click-places-caret.diff`: A token click on a text area or text field that has no press action focuses it and puts its caret at the end of its text, confirmed by reading the caret back, instead of failing with AX error -25206.
- `patches/0203-macos-unchanged-tokens-survive-reads.diff`: On macOS a token from one of a window's last 4 reads keeps working while a later read proves its element unchanged (same AX element, role, title, value, place and parents); other platforms keep staling every older token.
- `patches/0204-macos-waits-end-on-effect.diff`: On macOS press_key ends its value readback and pre-focus pauses as soon as the change or focus is seen (same caps), batch lookups poll every 80 ms, and invoke_menu logs how long a background item took to enable.
- `patches/0205-macos-open-panel-rows-and-sheet.diff`: On macOS a background pixel click on a list or sidebar row with no press action selects that row through accessibility, confirmed by read-back (#676); panels report whether they show a sheet, so a Go to Folder shortcut that opened nothing says so (#675).
- `patches/0206-macos-caret-click-confirms-focus.diff`: On macOS a token click on a text area or text field reports the caret placed only when the app also reads back that element as focused, so the reply never claims a focus the app did not take.
- `patches/0207-macos-helper-never-active.diff`: On macOS the cursor overlay sets up AppKit with the Prohibited activation policy, so a helper started from a terminal never becomes the active app and never takes the user's key focus (#696).
- `patches/0208-macos-panel-keys-reach-the-panel.diff`: On macOS keys for an Open/Save panel go to the panel service process that draws it, so Go to Folder, Escape, arrows and Return work on a background panel, and its Go to Folder sheet counts as the panel's key window; the front guard watches the window order from the start of each action with its own restore cap and a 10 ms poll, so a sheet the panel orders in is put back under the user's window at once (#675).
- `patches/0209-macos-menu-refusal-without-blind-wait.diff`: On macOS invoke_menu refuses a menu item that is disabled in a background app at once, unless the driver acted within the last second; then it re-reads until AppKit's once-a-second menu refresh must have caught up, so an item an action just enabled is pressed, not refused (#697).
- `patches/0210-macos-text-field-click-waits-for-focus.diff`: On macOS a click on a text field waits for the field to report focus, up to the old fixed 800 ms, instead of always waiting the full 800 ms, so a click on a native field or an already focused web field replies at once (#699).
- `patches/0211-macos-script-output-drained-while-running.diff`: On macOS a script result larger than the pipe buffer (a long Safari page read through its scripting) is read while osascript runs, instead of stalling it until the 5 s timeout.

- `patches/0250-linux-launch-keeps-the-front.diff`: Linux launches keep the user's front window: launched apps get a startup id whose user time predates every user action, so window managers with focus-stealing prevention (mutter, KWin) map them behind; a window manager that focuses the new window anyway is undone by the X11 focus guard, and the reply and its active field say what moved and whether it was handed back.

- `patches/0251-linux-hotkey-through-menu-shortcuts.diff`: Linux hotkey fires a chord addressed to a window through the menu item that carries it as its shortcut (AT-SPI key binding, then the item's action under the focus guard), so Ctrl+S saves in the background without the uinput virtual keyboard that Xvfb and most sessions cannot create; a chord whose item is disabled is refused with that reason instead of a keyboard-route error.

- `patches/0252-linux-launch-restore-and-shortcut-checks.diff`: Linux launches hand the front back only when the launched app (or a window new since the launch) took it, never a window the user switched to during the wait; a hotkey keeps every modifier when it looks for a menu shortcut (an unknown one matches no item), and a hotkey with only one of x or y is refused before the menu-shortcut route.

- `patches/0253-linux-real-input-without-uinput.diff`: Linux background keys and pixel clicks work without /dev/uinput: the virtual pointer and keyboard pair is fed through its own XTEST devices (core events stay on: Xorg's software cursor keeps a no-core master's cursor without a reference, a use-after-free that aborts the server); keys reach the window under the pair's pointer (PointerRoot focus) so no window manager activates it; the focus guard stamps the user's window with a fresh user time so a dialog the app opens later can't take the front, restores an active window the window manager dropped, ignores a stacking list that was empty at capture, and the focus notes of real input stay on a line of their own.
- `patches/0254-linux-no-clipped-pointer-points.diff`: Linux background clicks and keys never land at a clipped point: a point off the screen (a window partly off it) is refused with that reason instead of reaching whatever sits at the screen edge, the background keyboard parks only on visible points, and a click reply says the focus was restored only when the focus guard found it back.

- `patches/0255-windows-chromium-page-wait-within-timeout.diff`: The Chromium page wait before a window read is bounded by the caller's timeout_ms, at most 600 ms, with each pause capped to the time left: the walk's own clock starts only at the first node, so a 100 ms read waited the full 600 ms.

- `patches/0256-windows-unconfirmed-launch-refusal-says-so.diff`: After a Notepad or Settings launch did not answer within 15 s, a later launch that finds a window of that app refuses with the true reason: the helper cannot confirm it opened that window, because the earlier launch may have opened it after its reply. Before, the refusal said the helper did not open it, which could be false. The window is still never claimed as the helper's own (Notepad and Settings each run one process that also holds the user's windows).
- `patches/0257-windows-notepad-refusal-says-it-is-a-limitation.diff`: A Notepad launch refused because the user has a Notepad window open says that this is a lasting limitation of Windows Notepad, not a temporary failure: Notepad opens every file as a tab in a window it already shows and has no way to open one in a window of its own, so a retry is refused the same way while that window is open. When that window is the user's front window, the refusal says so.
- `patches/0258-windows-uia-gate-per-app-with-worker-budget.diff`: A Windows accessibility call that hangs in one app no longer blocks calls into other apps: each blocking UI Automation call is admitted per target process (desktop-wide scans and hit-tests share one desktop gate), within a budget of four workers in all, and a stuck worker keeps its app's gate and its budget slot until its provider call really returns. Before, one process-wide gate meant a File Explorer Close that hung in its provider made a Calculator click 25 s later fail as busy. A click that times out shows again only the dialog owners it hid itself, not those of another app's click still running, and a call refused while its app is busy says that an earlier call to this app is still running.
- `patches/0259-windows-unanswered-launch-refusal-says-why-and-what-clears-it.diff`: After a Notepad or Settings launch did not answer within 15 s, the refusal of a later launch that finds a window of that app says why it stays and what ends it: Windows may have opened that window for the earlier launch afterwards, nothing ties it to that launch (one process of the app serves the user and the helper alike), so the helper never treats it as its own, and every launch of that app is refused until no window of it is open.
- `patches/0260-linux-gtk-frame-at-the-corner-under-mutter.diff`: A GTK3 window at the corner of the screen under mutter has a frame near (0,0) that differs from mutter's frame window by its invisible borders, so it was taken for Chromium's renderer-local frame and its elements came out one title bar too low; a frame that holds the client window where it really is stays a screen rectangle.
- `patches/0261-linux-gtk-frame-patch-formatted.diff`: Format the Linux window size code from 0260 so cargo fmt --check passes.

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
