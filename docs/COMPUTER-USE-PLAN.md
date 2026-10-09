# Computer use plan: background, precise desktop control for every worker

> Status: approved 2026-10-09 by the Delegator for the user, with Codex's review of the outline folded in (§10.2).
> Base: main `ffba653f`, branch `computer-use`. Inputs: the research note `docs/research/COMPUTER-USE-2026-09.md`,
> Codex's critique of it (§10.1), measurements on this Mac (§2), the private dependencies (§12) and the checked
> third-party contracts (§11).
> Paths: `M/` = `crates/core/src/manager/`, `P/` = `crates/providers/src/`, `CU/` = `crates/computer/src/` (new).

## 1. Target

Brigadier's workers use the user's real Mac the way a careful person would: they read an app, click, type, pick
menus, scroll and drag, in any app, to check what a worker built or to do a general task. They do it **in the
background**. The user keeps their cursor, keyboard focus and frontmost app. A per-session agent cursor shows where
each worker acts.

It must be the fastest and most precise computer use available, measured by the numbers below and repeated by a
local benchmark (§7).

### Targets

All latencies are measured in the engine, at the boundary of one request, on this Mac (M-series, macOS 27, Retina
3456×2234), warm, p50 / p95 over the benchmark run. "Window" is a window of the fixture app (§7) unless stated.
Action timings are split three ways, each with its own predicate (§7): **dispatch** (the call returns after the
action is delivered), **effect** (the fixture's log or an `expect` shows the change) and **quiet** (no
accessibility notification for 50 ms after the effect).

| # | Metric | Target |
|---|---|---|
| S1 | `observe`, structure only, window ≤ 300 elements | ≤ 15 ms / ≤ 40 ms |
| S2 | `observe` with screenshot, window ≤ 1600×1000 pt | ≤ 70 ms / ≤ 120 ms |
| S3 | element action (press, set value, menu-bar pick): dispatch / effect | ≤ 10 / ≤ 40 ms p50, ≤ 150 ms p95 effect |
| S3p | pop-up pick: effect (AppKit blinks the chosen item ≈350 ms before it acts, a platform limit, §4.4) | ≤ 400 ms p50 and p95 |
| S4 | background pixel click: dispatch / effect | ≤ 15 / ≤ 60 ms p50, ≤ 200 ms p95 effect |
| S5 | type 100 characters (set-value path / key-event path) | ≤ 20 ms / ≤ 250 ms |
| S6 | tool overhead: MCP call → daemon → helper → reply, excluding the action (measured from Phase 2) | ≤ 5 ms p50 |
| S7 | helper cold start to first served request | ≤ 400 ms |
| P1 | element-path success on every target that has an element (8, 12, 16, 24 pt) | 100% |
| P2 | pixel-path mapping: the point the fixture receives vs. the point asked for, on targets that take a background click | 100% inside the target, ≤ 0.5 pt error, 1× and 2× |
| P2r | expected refusals: background pixel clicks on views that don't take one return `background_unavailable`, never a silent miss | 100% |
| P3 | model-in-the-loop pixel hit rate (canvas targets, no structure) | ≥ 98% at ≥ 12 pt, ≥ 95% at 8 pt (with zoom) |
| P4 | wrong-target actions with an effect | 0 |
| F1 | focus theft in background mode: frontmost-app changes, real-cursor moves, key-window changes of other apps | 0 across the whole suite |
| T1 | structure text of a typical window (fixture, TextEdit, Finder, a Settings pane) | median ≤ 1,500 tokens, hard page size 6,000 |
| T2 | screenshot cost | stated in every result as a labelled estimate for the worker's provider; window crop, ≤ 2,000 px per side, ≤ 4,784 Claude visual tokens |
| T3 | all computer tool definitions together | ≤ 2,500 tokens |
| E1 | model calls per eval task | median ≤ 1.3× the task's reference: the scripted solver's tool calls + 2, for one look before acting and one report (amended by the user on 2026-10-09, §9; it was 1.3× the fewest `act` batches) |

Other computer-use tools can't be measured on this machine tonight (their helpers need grants or a sign-in nobody can
give at night), so the targets are absolute. Phase 4 adds side-by-side runs on macOS as soon as the user grants
access (§9).

**Gates are mandatory.** A phase is not done while a gate it owns misses: the report states the number and the phase
stays open. Sample counts: S1–S5 and P1–P2r, 200 repetitions per target or operation; P3, 50 trials per target size;
E1, the whole suite, 3 runs per provider.

## 2. What we measured on this Mac (2026-10-09)

Everything here is **terminal-launched development evidence**: the probes ran from a terminal that holds the grants.
The shipped helper's own grants are a separate gate (§8, Phase 2).

A throwaway probe, built and run from a terminal session, against a TextEdit window it opened in the background (`open -g -n`)
on a scratch file:

- **Grants are inherited from the terminal.** `AXIsProcessTrusted()` and `CGPreflightScreenCaptureAccess()` both
  return true for an unsigned binary started from a terminal that holds both grants: the terminal is its responsible process. So
  development and the benchmark need no new grant; the shipped helper app needs a one-time grant (§8).
- **Structure is fast when attributes are fetched in one call.** One attribute per call: 12 nodes in 53.7 ms (the
  first call also pays the app's accessibility connection). `AXUIElementCopyMultipleAttributeValues` with six
  attributes per node: 12 nodes in 2.6 ms.
- **Window capture is fast once warm.** `SCShareableContent` 44 ms (cache it, refresh on window events). One
  window through `SCScreenshotManager` at 1× points: 124 ms cold, 52 ms warm. PNG encode 2.6 ms (7.7 KB),
  JPEG 1.4 ms.
- **Keys and text reach a background app.** Unicode key events posted to the app's pid (`CGEvent.postToPid`) typed
  into the background TextEdit window. Frontmost app unchanged, real cursor unchanged. 19 ms for 4 events.
- **Plain pid-posted mouse events reach the app but no window.** On a probe fixture app, AppKit received them with
  window number 0. A scan of event fields found the one AppKit reads for a mouse event's window (field 51; 103 for
  mouse-moved). With that field set to the target window and a window-local location set on the event, a
  background click landed in a view **that accepts the first click**, at the right point (±0 pt in x; y depends on
  the title-bar height, which the transform takes from the window's real content rectangle). The frontmost app and
  the real cursor didn't change.
- **Views that don't accept the first click ignore a plain background click**, as they do for a person clicking an
  inactive window (a canvas that refuses the first click, TextEdit's text view). A standard `NSButton` does take it.
  The spike (§2.1) found a background route for the rest: **synthetic activation**, below.
- **Window pairing needs care.** An app's first capturable window was a hidden 500×500 utility window, not the
  document. Windows are paired between the accessibility tree and the capture list by window id, never "the first".

### 2.1 Background feasibility spike (Phase 1, step 1)

Each case runs against a window the spike launched, in the background, with the user's app in front. "Focus kept"
means the frontmost app, the real cursor position and the other apps' key windows didn't change.

| Case | Background route tried | Lands? | Confirmed effect | Focus kept |
|---|---|---|---|---|
| Standard button (`NSButton`), inactive window | pid-posted down/up, window field + window location | yes | `pressed` logged | yes |
| Canvas that refuses the first click | same | events reach the window, not the view | none | yes |
| Canvas that refuses the first click | synthetic activation, then the same click | yes, `key=true active=true` | `down`/`up` at the asked point | yes |
| Canvas that takes the first click (no accessibility) | plain background click | yes | `down`/`up` at the asked point, ±0 pt | yes |
| Covered window (another app's window over the point) | plain background click | yes, the covered window gets it, the cover gets nothing | `down`/`up` in the covered view | yes |
| Text view already focused | unicode key events to the pid | yes | text changed `"abc"` | yes |
| Scroll view, covered | scroll-wheel event to the pid, window field + location | yes | scroll offset 0 → 50 | yes |
| Drag on a first-click canvas | down, 10 drags, up, all with the window field | yes | 10 `dragged` points along the line, `up` at the end | yes |
| Chromium page button (browser launched by the spike) | plain background click | no | none | yes |
| Chromium page button | plain click + authentication envelope | no | none | yes |
| Chromium page button | synthetic activation, then the click | yes | `onclick` ran (title `clicked 1`) | yes |
| Chromium text input, focused by an activated click | unicode key events, no envelope | yes | `input` events, value `xy` | yes |
| Chromium text input | delete key (virtual key 51), no envelope | yes | value lost one character | yes |
| Chromium scrollable `div` | plain scroll-wheel event | yes | `scrollTop` 0 → 200 | yes |

"Focus kept" was checked on every case: the frontmost app and its focused window (read through accessibility), the
real cursor (`NSEvent.mouseLocation` and the event-system cursor), and the window server's front process (read
before the action with `_SLPSGetFrontProcess`). No case changed any of them, and no application-activated
notification fired. Dispatch took 12–28 ms for a plain click, scroll or 3 keys, and about 100 ms with synthetic
activation (two 30 ms waits that the engine will tune).

**Synthetic activation.** Two event records posted to the target process (`SLPSPostEventRecordTo`, §12): a focus
record for the target window, then a make-key pair. The target app then believes it is active and its window key
(`NSApp.isActive`, `isKeyWindow`), so its views take ordinary clicks, while the window server's front process, the
user's frontmost app and its key window stay as they were. So the user's own typing still goes to their app. After
the action a matching defocus record puts the target back to inactive: the next plain click saw `key=false
active=false`. Activation is per application, so it runs under the application lease that keyboard and focus work
already take (§4.4).

**The authentication envelope** (`SLEventSetAuthenticationMessage` with an `SLSEventAuthenticationMessage`) was not
needed for any case on macOS 27: Chromium took plain pid-posted keys and scroll, and its clicks failed with or
without the envelope until the app was activated. The engine binds it (ABI pinned in §12) but doesn't attach it by
default. Phase 5 re-tests it on Electron apps.

**Launching.** Chromium made itself frontmost at launch even with `open -g`. So `launch` records the frontmost app
before it starts anything and puts it back if the launched app takes the front, and the result says so.

The engine's background rung is therefore: element actions first; then a plain background event where the target
takes it; then the same event with synthetic activation. Covered windows are **not** refused: their clicks land.
`occluded` is kept only for windows that are minimised or on another display that is asleep (Phase 5).

## 3. Where this sits in Brigadier

Brigadier runs the user's own `claude` and `codex` CLIs and owns no model loop (docs/PLAN.md: subscription terms).
So computer use is a **tool** those CLIs call:

```
worker CLI (claude / codex)
  │ MCP (stdio, via `brigadierd mcp`)        or        `brigadier-computer` CLI (scripts, bench, people)
  ▼
brigadierd ── ComputerBroker: grants, leases, stop, action log, helper lifecycle
  │ Unix socket, per-launch token
  ▼
Brigadier Computer Use.app (helper; owns the Accessibility + Screen Recording grants)
  ├─ observe: accessibility tree, window capture, zoom
  ├─ act: element actions, background events, menus, settle, verify
  └─ agent cursor overlay
```

The worker owns reasoning, history and model calls. Brigadier owns observation, execution, verification, safety and
lifecycle. From the research note we take the house rule (**code or API first, structure next, pixels last**),
batching, zoom, settle detection and harness-side verification. We drop the parts that need a model loop we don't
run: cache breakpoints, screenshot pruning, provider-side classifiers, a separate clicking model, VMs and golden
images, best-of-N rollouts and GPU grounders.

## 4. Architecture

### 4.1 Components

- **`crates/computer`** (new, pure Rust): the platform-neutral contract and logic.
  - The `Desktop` trait: apps, windows, observe, act, capabilities. One implementation per OS (`macos` now;
    `windows`, `linux` later, §6).
  - Observation types, the compact text renderer, element refs, the coordinate transform, the batch executor,
    settle and expect predicates, the block list and the action log writer. Types the UI reads are exported with
    `ts-rs` like the rest of the workspace.
  - The macOS backend uses `objc2` crates (already in the tree through Tauri) for AppKit, ScreenCaptureKit,
    CoreGraphics and the accessibility API. Every private call and undocumented event field (§12) lives in one
    module, `CU/macos/private.rs`, resolved at run time (`dlsym`). When one is missing, the action that needs it
    returns `unsupported_capability`. A public call is never passed off as an equivalent.
- **`brigadier-computer`** (new binary in the same crate): the helper. Two modes:
  - `serve --socket <path> --token-file <path>`: the long-lived helper (inside the app bundle in production, from a
    terminal in development).
  - direct one-shot commands (`apps`, `observe`, `act`, `zoom`, `bench`, `spike`): a **development and fixture
    harness only**. Workers never reach the engine this way: their CLI commands go through the daemon's broker,
    like their MCP calls (§4.6).
- **Brigadier Computer Use.app**: the helper binary in its own bundle (`ai.brigadier.computer-use`,
  `LSUIElement`), shipped inside `Brigadier.app/Contents/Helpers/`, signed with Brigadier's Developer ID. It has its
  own TCC identity, so System Settings shows "Brigadier Computer Use" and the grant survives Brigadier updates.
  brigadierd starts it through LaunchServices (`open -g -j -a`), so it is its own responsible process and the grants
  are its own, not the daemon's or a terminal's. (A process can also disclaim responsibility when it spawns a child;
  that is the fallback if LaunchServices start is too slow.) It exits after 10 minutes with no session, and when
  brigadierd goes away.
- **`ComputerBroker`** in brigadierd: starts and supervises the helper, checks each call's grant, holds target
  leases, runs the global stop, and moves screenshots of the action log into the blob store.

Why a separate process: the TCC identity; the overlay needs an AppKit main thread; and accessibility calls to a hung
app block (each call gets `AXUIElementSetMessagingTimeout`, 1 s by default), so a hang must never stall the daemon.

### 4.2 Transport

- Helper socket in a 0700 directory under `<data_dir>/computer/`, with a per-launch token in the first frame. The
  helper also checks the connecting process's code signature (same team as Brigadier) in signed builds. Only
  brigadierd connects to it.
- Frames are length-prefixed JSON. Images travel as a separate binary frame and become base64 only at the MCP edge.
- Every request carries the session and worker ids, taken by the broker from the authenticated MCP grant (never
  from the model's arguments), so the helper can draw the right cursor and the log can say who did what.
- Every request has a whole-request deadline and a cancellation generation (§4.7).

### 4.3 Observation

One `observe` call returns everything a model needs for its next batch:

```
window w2 "scratch.txt" · TextEdit pid 80153 · obs 17 · 656×422 pt · image 656×422 (1 px = 1 pt) · 1,188 tokens
e1 window "scratch.txt"                       @0,0 656x422
  e2 button "close"                           @8,6 14x14
  e3 button "minimize"                        @28,6 14x14
  e5 text-area focused value="Hi Scratch file for…" (63 chars)   @0,28 656x394
… 0 hidden
```

- **Elements** are pruned to the ones that matter: anything with a label, value, state or action; containers only
  when they name a group. Single-child chains collapse. Each line has a short ref (`e5`), a short role, the label,
  a clipped value, state flags (focused, disabled, selected, checked, expanded), non-default actions, and bounds in
  window points.
- **Refs are stable but checked.** An element keeps its ref across observations while the accessibility object is
  the same (`CFEqual`), so a re-observe doesn't renumber everything. Identity alone is not trusted, because rows get
  recycled and dialogs replace content: every observation has a generation, and every ref remembers the role, label
  and enabled state it had. Before each action the engine re-reads them. If the role or label changed, or the
  element is disabled or gone, the action fails with `stale_ref`.
- **Diffs after the first look, per consumer.** A diff is relative to a base observation the *same worker*
  received. The first `observe` of a window by a worker returns the whole (pruned) tree. Later ones return only
  what changed: `~` changed, `+` added, and removed refs as ranges (`- e9–e14`). When nothing changed, one line says
  so. A call may name its base (`since: obs 17`); if the engine doesn't hold that base for this worker (another
  worker, a fresh CLI after a handoff, an evicted cache), it returns the full state. `full: true` asks for it too.
  Every result ends with the focused element and the selected text, both clipped and paged like everything else.
- **Paging, not truncation.** Above the page size the result says what was left out and where (`… 340 rows under
  e40: observe e40`). `observe` takes `element` (a subtree) and `find` (matching lines plus their ancestors). Clipped
  values say how long they are, and `observe {element, value_page}` returns the full value a page at a time. Every
  output path (tree, diff, focused element, selected text, error text) respects the page size. What a page left
  out doesn't count as seen: the worker's next diff brings it.
- **Out of view costs nothing.** A table, outline or list that reports its visible rows has its other rows
  counted, not read; `element` and `find` read everything. Action names are read only for roles where they add
  something. An action on a ref aims at the part its scroll views show, and an element scrolled out of view can be
  pressed but never clicked (`no_such_target`, "scroll it into view first").
- **Screenshots are optional** (`screenshot: auto | always | never`). `auto` adds one when the tree is poor: a
  canvas, a web area with no children, or fewer than three labelled elements.
- **Geometry is exact.** Every image has an id and a recorded transform (window origin, content rectangle, display
  scale, output size, crop offset). The model sends coordinates in the pixels of a named image (`obs`), and the
  helper maps them through that image's transform. There is no fixed "halve it for Retina" rule. A coordinate on an
  observation older than the window's last geometry change fails with `stale_geometry`.
- **Image size.** The window crop is rendered at 1 px = 1 pt when that fits the limits: ≤ 2,000 px per side
  (Claude's limit once a request holds more than 20 images) and ≤ 4,784 Claude visual tokens. Otherwise it is
  scaled down and the header says so. We size images ourselves; Claude Code would otherwise rescale an oversized MCP
  image and the model would read coordinates off a picture the engine never saw.
- **Image cost is a labelled estimate for the worker's provider.** Claude workers: `⌈w/28⌉ × ⌈h/28⌉` visual
  tokens. Codex workers: `⌈w/32⌉ × ⌈h/32⌉` patches × the model's multiplier (1.2 for the GPT-5.4–5.6 family), at
  `original` detail. The header says "≈ 1,188 tokens (Claude estimate)". Whether each CLI passes the image through
  unchanged is tested in Phase 2 with small targets through both CLIs' real image paths.
- **Zoom** (`zoom obs region`) returns a crop at the display's full backing resolution (2× detail on Retina),
  bounded to ≤ 1,024 px per side (a larger region is scaled down to fit), as a new image with its own id and
  transform. This is **our own contract**, not the Claude computer toolset's: there, zoom coordinates stay in
  full-screenshot space; here the model may click in either image by naming its id. Both CLIs' models are tested on
  it in Phase 2.
- **Screen content is untrusted.** Results are labelled as screen content, and the worker's brief says text on
  screen is data, not instructions.
- **Redaction happens before anything leaves the engine**, on every path: tree values, the focused element, the
  selected text, screenshots and zoom crops (secure fields and blocked windows painted over), and the copies that
  are persisted to the action log. There is no unredacted original.

### 4.4 Actions and delivery

`act` runs a batch, in order, and stops at the first failure:

| Action | Fields |
|---|---|
| `click` | `ref` or `{obs, x, y}`; `button`, `count`, `modifiers` |
| `set_value` | `ref`, `text` (text fields, sliders, steppers, pop-ups) |
| `type` | `text`, optional `ref` to focus first |
| `key` | chord like `cmd+s`, `repeat` |
| `scroll` | `ref` or point; `dx`, `dy` in lines, or `pages` |
| `drag` | from and to, each `ref` or point |
| `perform` | `ref`, an accessibility action the element lists (show menu, increment, confirm…) |
| `menu` | a path in the app's menu bar, e.g. `["File", "Save As…"]` |
| `select` | `ref`, `start`, `length` in characters: sets the selected text range, so a `type` replaces it |
| `wait` | an `expect` predicate and a timeout |

Each action may carry `expect` (an element's value equals or contains, an element appears or goes away, the window
title, the focused element). The batch holds a lease on its window (§5) and re-checks the block list before each
action, because a click can open another app.

**Before every action** the engine checks, in this order, and fails the action (and skips the rest) if one doesn't
hold:
1. the batch's cancellation generation is current and its deadline hasn't passed;
2. the target is not blocked;
3. the ref's generation, role, label and enabled state still match (§4.3);
4. for keys, text and menus: the recipient is the window the batch leased, and the focused element is the expected
   responder and **not a secure field**. If the engine can't establish the recipient for a background key event, it
   refuses with `background_unavailable` rather than typing into whatever has focus.

**Keyboard, menu and focus-dependent actions are serialized per application**, not per window: two windows of one
app share its focus, first responder, menus and sheets. Pointer actions and element actions on different windows
of one app may run side by side.

**Navigation invalidates the rest of the batch.** If an action opens a new window, a sheet or a dialog, or changes
the window's title or main content (a navigation), the remaining actions' refs and points are invalid: they are
skipped with `invalidated`, and the result carries the new state.

**Delivery ladder**, best first. Each action reports the rung that ran:
1. **Accessibility action or attribute**: press, pick, set value, set the selected text range, raise a menu,
   increment. No events at all, works on hidden and other-Space windows. An app answers an action only when its
   handler returns, and a button's handler includes its highlight (≈100 ms) after the action already ran, so
   the engine waits at most 3 ms for the reply and reads the effect instead. A pop-up's items exist only while
   its menu is open: the engine opens it, presses the item and waits for the menu to close. AppKit blinks the
   chosen item for ≈350 ms before it sends the action, and no background route avoids that (Phase 1 tried the
   item's press and pick actions, Return in the open menu, and arrow keys on the closed pop-up), so a pop-up pick
   costs ≈360 ms. The menu shows on screen for that time; it takes neither focus nor the cursor.
2. **Background events to the window**: keys and text posted to the app's pid (measured working, §2). Key events
   don't need the system's authentication envelope on macOS 27 (§2.1); it is bound and kept for apps that ask. Mouse
   events are posted to the pid with the window-routing field and a window-local location. Covered windows take
   them (§2.1). A view that refuses the first click, and Chromium pages, get the same events wrapped in synthetic
   activation (§2.1), which leaves the user's frontmost app, key window and cursor alone.
3. **Foreground, the strict last resort.** Used only when the user has been idle for at least 60 s (HID idle time,
   `CGEventSourceSecondsSinceLastEventType`). The engine re-checks idle right before raising, raises the window,
   acts, and right after restores the previous frontmost app, window order and cursor position. The action is
   logged and reported as `foreground_used`. If the user becomes active at any point, it aborts and returns
   `background_unavailable`. It is never used while the user is active. It holds the desktop lease (§5). Background
   stays the default for everything.

Every action result keeps three things apart:
- **delivered**: the rung that ran;
- **settled**: the app went quiet, or timed out;
- **effect**: `confirmed` (an `expect` held, or the target's value or state changed as asked), `unverified` (no
  check was possible), `no_change` (nothing observable changed), or `background_unavailable`.

**Settle** listens for the app's accessibility notifications (value changed, focus changed, window created or
moved, menu opened, layout changed). It returns 50 ms after the last one, or as soon as `expect` holds, with a 1.5 s
default bound. A cheap window-hash check is the fallback for apps that post nothing. The text caret and spinners are
masked out of the hash. There are no fixed sleeps.

The batch returns per-action status (`done`, `failed`, `skipped`), then one closing observation **diff** (only the
elements that changed), plus a screenshot if the batch asked for one. A failed submission is never replayed
automatically. If an action made another app frontmost or opened a new window, the result says so.

**Errors teach the next step.** Every error has a stable code (`stale_ref`, `stale_geometry`, `invalidated`,
`occluded`, `background_unavailable`, `unsupported_capability`, `secure_field`, `blocked`, `busy`, `not_settable`,
`no_such_action`, `app_not_responding`, `deadline`, `permission_missing`, `stopped_by_user`…) and one line on what to do next, e.g. "observe again; refs from before a navigation are gone".

**Text** goes in by the cheapest route that can be checked: set the value or insert at the selection through
accessibility, read it back, and fall back to key events. An insert that leaves the value unchanged with the text
already in it may have replaced a selection holding that very text, so it is reported unverified, never typed a
second time. Pasting is only used for rich text, and it saves and
restores the user's clipboard and checks that the user didn't copy something in between. A paste goes through the
same secure-field check as typing.

### 4.5 Agent cursor

- The helper draws one overlay per display: a borderless, transparent, non-activating panel above all windows, on
  every Space, that mouse events go through. It never takes focus or a click, and never posts an event.
- **Never in Brigadier's own captures.** Every capture the engine makes is of one window
  (`SCContentFilter(desktopIndependentWindow:)`), so a panel of another window is never in it. If a whole display is
  ever captured, its content filter leaves the helper's own windows out. We don't rely on `sharingType = none`:
  Apple calls it legacy, and other apps' screenshots and recordings may show the cursor. That is fine: it is what
  the user is meant to see.
- Each worker's cursor has its avatar's colour (the app's `glyphFor` on the task id, the same shape-to-colour
  mapping), so it is stable and matches the worker's row. Two workers whose avatars share a colour share it here
  too; their name pills tell them apart. The pill carries the worker's name.
- The cursor glides to each target in 150 ms, beside the action rather than before it, so it never delays one
  (ruled in §10.2). It pulses on a click and outlines the element an element action names. It never moves the real
  cursor.
- It fades out 5 s after its last action and goes when the session ends; the user's Stop clears every cursor.

### 4.6 The tool surface

**MCP**: a second Brigadier server named `computer`, so the tools are `mcp__computer__<tool>`. It is not
`always_load`, so Claude keeps it behind tool search until a worker needs it, and a worker that never touches the GUI
pays almost nothing. How it plugs in:
- **Catalog selection.** `brigadierd mcp` takes a catalog argument (`--catalog computer`). The daemon serves the
  computer catalog to a grant whose role and permission level allow it, through the same `serve` path and per-call
  grant check as the Brigadier catalog.
- **Typed replies.** `ToolReply` (`crates/core/src/tools.rs`) becomes a list of content blocks (text and image)
  instead of one string. The MCP edge turns image bytes into base64 image content.
- **Provider injection.** `P/claude` and `P/codex` add the second `McpServer` for workers whose grant allows it,
  with Claude's `alwaysLoad` off and a tool timeout above the longest request deadline.
- **Cancellation.** A cancelled MCP request (the client's cancel, or the connection closing) drops the broker's
  future, which bumps the request's cancellation generation in the engine (§4.7).
- **Identity** comes from the authenticated grant, never from tool arguments.

Five tools:

| Tool | Does |
|---|---|
| `apps` | running apps and their windows (id, title, on screen, Space, frontmost); blocked ones marked |
| `launch` | open an app, a file or a URL in the background (`open -g`), returning its windows |
| `observe` | §4.3 |
| `act` | §4.4 |
| `zoom` | §4.3 |

**CLI**: `brigadierd computer apps|launch|observe|act|zoom`, same arguments as JSON or flags and the same output
text, authenticated with the worker's grant like `brigadierd mcp`, so it goes through the broker. Workers can use it
from a shell. The direct `brigadier-computer` commands are the development and fixture harness (§4.1).

The descriptions carry the house rule in two lines: use code, files and app APIs when they can do the job; use
`observe` structure before pixels; batch the steps you are sure of; put `expect` on anything that changes state.

### 4.7 Cancellation and deadlines

- Every request has a deadline for the whole request (30 s plus the time its `wait` actions ask for, together at
  most 300 s, so no batch runs past 330 s; a batch asking for more is refused with `bad_request`), separate from the
  1 s per-call accessibility timeout. The MCP tool timeout is 360 s, above that with room for the transport.
- The engine keeps a cancellation generation per session and a global one. Stop, a cancelled MCP request, a worker
  that ends, or a closed connection bumps it. Every action checks it before it starts, and long sequences (typing,
  drags, scroll runs, waits) check it between events.
- On cancellation, disconnection or a crash of a request, the engine **always** releases what it pressed: mouse
  buttons are released, and modifier keys are released with key-up events to the same pid. The release runs from a
  guard that runs on every exit path, including a panic.
- Queued requests from a cancelled generation are rejected with `stopped_by_user` or `cancelled`. Later requests
  run normally, so a stopped worker can be resumed.
- The stop control (menu-bar item and hotkey) runs on the helper's main thread and never waits on accessibility
  work. The engine part (generations, deadlines, the input-release guard) is Phase 1; the stop UI is Phase 2.

## 5. Safety

There are no per-action prompts in Full access: runs happen overnight and the user asked for no cards there. Safety
comes from limits that need nobody awake.

- **Permission levels** (docs/PLAN.md Phase 3 levels):
  - **Full access**: every tool, no cards.
  - **Approve for me** and **Ask for approval**: `apps`, `observe` and `zoom` are free. `launch` and `act` on a GUI
    app are **escalations**, because a GUI app can write anywhere and a terminal can run anything. They go through
    the existing approval path, and the approval is bound to the specific instance and windows it names (the pid and
    window ids the worker launched or was granted), not to the app in general. Launching an app doesn't make it
    confined to the workspace, and nothing claims it is.
  Built in Phase 2; the plan text is binding now.
- **Hard block list**, enforced in the helper, not the prompt: the target can't be observed or acted on.
  - Password managers: 1Password, Bitwarden, Dashlane, KeePassXC, LastPass, Apple Passwords.
  - Keychain Access, and the system's authentication and security-agent dialogs.
  - The Privacy & Security, Users & Groups, Passwords and Login Items panes of System Settings.
  - Terminal-type apps (Terminal, iTerm2, cmux, Ghostty, Warp, Alacritty, kitty, WezTerm) unless the session
    launched that window.
  - The Brigadier instance that hosts the session (its pid and bundle path) and the installed
    `/Applications/Brigadier.app`, so a worker can't approve its own cards. Brigadier **dev builds the session
    launched are allowed**: driving them is a main use case.
  - Secure text fields everywhere: their value is never returned (§4.3), and keys, text and pastes are refused while
    one is the focused responder (§4.4).
- **Grants and leases**: the MCP grant is checked on every call, as today, and revoked when the worker ends or is
  stopped. A mutating batch leases its window, and keyboard, menu and focus-dependent actions also lease the app
  (§4.4); a second worker gets `busy` with the owner's name. Foreground delivery leases the whole desktop.
- **The user comes first**: if the user is using the target window (input in the last second and the window is
  frontmost), mutating actions wait until they stop.
- **Global stop**: a Stop item in the helper's menu-bar icon and the hotkey ⌃⌥⌘. bump the global cancellation
  generation (§4.7): running sequences stop between events, pressed input is released, and all leases are revoked.
  Workers get `stopped_by_user`. Brigadier's session Stop does the same for its workers.
- **Action log, inside the existing records.** Each action is a session event in the event store (time, worker,
  action, target, rung, effect, timings), so it lives and dies with the session. Screenshots, with the predicted
  point drawn on them and redacted (§4.3), go to the blob store as artifacts owned by the session; they are
  recorded in the cleanup ledger before they are written, and deleting the session deletes them under the existing
  rules. The events double as a replayable trajectory for the bench and bug reports. The UI shows them as a
  timeline (Phase 3).
- **Bounded caches.** The engine's observation, ref and diff-base caches are capped (per worker and in total, least
  recently used first) and dropped when the worker ends.
- **Owned instances only.** Apps and windows a worker launched are recorded as its artifacts in the cleanup ledger
  (pid and start time), so cleanup quits exactly those and nothing the user had open.

## 6. Windows and Linux path

The `Desktop` trait and its capability flags are the seam. The backends slot in without changing the tool surface:

- **Windows**: UI Automation for structure and background actions (Invoke, Value, Toggle, ExpandCollapse,
  SelectionItem, Scroll patterns). Windows.Graphics.Capture for one window, which works for covered windows.
  Background input through window messages to the target window where the app accepts them; `SendInput` is the
  foreground rung. The overlay is a layered, click-through, topmost window.
- **Linux**: AT-SPI2 over D-Bus for structure and actions (Action, EditableText, Value). On X11, per-window capture
  with XComposite and input with XTEST (foreground only). On Wayland, capture and input go through the desktop portal
  (ScreenCast, RemoteDesktop) with its one-time consent. There are no background pixel events there, so AT-SPI
  actions are the only background rung, and the tool says so.

## 7. Evaluation, run locally and repeatable

- **Fixture app** (`crates/computer/fixtures/target-range/`, one Swift file built with `swiftc` by the bench, macOS
  only; AppKit written the way real apps are, so its controls behave like theirs, first-click rules included): one
  window with buttons and
  checkboxes at 8, 12, 16 and 24 pt; a text field, a secure field, a slider, a stepper, a pop-up, a table with 200
  rows, a tab view, a sheet, a menu with nested items, and a canvas of dots that has no accessibility at all. Every
  control writes what happened to it (control id, event, point) to a log the benchmark reads: the ground truth.
- **`brigadier-computer bench`** (no model; mandatory before every report): launches the fixture in the
  background, then
  - times S1–S5 and S7 over 200 repetitions each, reporting dispatch, effect and quiet separately (predicates:
    dispatch = the call returned; effect = the fixture logged the expected event; quiet = no accessibility
    notification for 50 ms after the effect). S6 is added once the broker and MCP exist (Phase 2);
  - drives every target through each path it supports, and reports four numbers separately:
    - **supported-path success** (P1, and P2 on first-click targets);
    - **expected refusals** (P2r: targets that can't take a background click must return
      `background_unavailable`);
    - **mapping accuracy** (P2: received point vs. asked point, centre and 1 pt-inset corners);
    - **coverage** (how many targets each path supports);
  - counts wrong-target actions (P4) from the fixture's log;
  - watches the frontmost app, the real cursor and other apps' key windows the whole time (F1);
  - measures the text and image size of every observation (T1, T2) and the tool definitions (T3).
  It prints a table, writes JSON under `target/computer-bench/`, and exits non-zero when a gate misses.
- **Model-in-the-loop suite** (Phase 4): 20 tasks run by real worker CLIs on the fixture, a scratch TextEdit, a
  scratch Finder folder and Brigadier's dev build on a scratch data dir. Examples: set the slider to 37; tick the
  8 pt checkbox; pick the third nested menu item; find and open row 173; click the red dot on the canvas; write and
  save a file in TextEdit; rename a file in Finder. Each task has a scripted checker. Metrics: success, wrong-target
  actions, model calls (E1: model calls ÷ (the scripted solver's tool calls + 2)), tokens per step, wall time,
  focus theft.

## 8. Phases

Each phase lands as its own commit series in the worktree, with the benchmark. Tests are written where they guard
real behaviour (mapping, generations, cancellation, redaction, the block list), following PLAN's test policy, not
for coverage. Nothing merges to main until the user says so.

### Phase 1: Engine core on macOS, CLI, fixture and benchmark

**Scope**
- **Step 1, a feasibility spike** before the engine (§2.1): standard inactive controls (a button, a text view), a
  covered window, scrolling, dragging, a canvas with no accessibility, and a Chromium window the spike launched
  (scratch profile). Each case records what works in the background, the confirmed effect and the focus
  invariants. The result table goes into §2.1, and the engine's ladder follows it.
- `crates/computer`: the `Desktop` trait, observation types and renderer (full tree and diffs), refs with
  generations, coordinate transforms, batch executor with the pre-action checks, settle and expect, cancellation
  generations, deadlines and the input-release guard, redaction, error codes with next steps, block list, action
  log records. The private bindings in one module (§12).
- The macOS backend:
  - list apps and windows, paired by window id;
  - the pruned tree, read with multi-attribute fetches;
  - window capture at the exact scale, and zoom;
  - element actions: press, set value, perform, selected range, menu-bar paths;
  - key and text events to the pid;
  - scroll;
  - background pixel click and drag where the spike shows they land, with `background_unavailable` elsewhere (the
    foreground rung comes in Phase 2 with the desktop lease; it can't be tested tonight on a machine someone else is
    using);
  - notification-based settle.
- `brigadier-computer` one-shot commands and `serve` (socket, token), as the development harness.
- The fixture app and `bench`.
- `crates/computer` added to `tools/full-checks.sh`'s cross-build package list.

**Done when** (all of it is terminal-launched development evidence)
- §2.1's table is filled from the spike.
- Tests pass for mapping (1×, 2×, scaled, zoom), generations and stale refs, cancellation and input release,
  redaction, and the block list.
- `bench` on this Mac meets S1–S5 (S3p included), P1, P2, P2r, P4 and F1, with the sample counts of §1. A miss
  keeps the phase open.
- A real run against the fixture is shown by its action records and an annotated screenshot: observe, click an 8 pt
  checkbox by ref, click a canvas dot by pixel, type into the text field, pick a menu item. The user's frontmost app
  and cursor don't change.
- `tools/full-checks.sh` passes, including Linux and Windows clippy of the new crate (non-mac backends compile as
  stubs that return `unsupported_capability`).

**Results (2026-10-09, `brigadier-computer bench`, release build, 200 repetitions, on `0d4ef581` after the Phase 1
review fixes, terminal-launched development evidence)**

| Gate | Measured | Target | |
|---|---|---|---|
| S1 observe, structure | 6.4 / 8.1 ms | ≤ 15 / ≤ 40 ms | pass |
| S2 observe with screenshot | 61.9 / 66.1 ms | ≤ 70 / ≤ 120 ms | pass |
| S3 press, all P1 targets | dispatch 7.2 · effect 4.2 / 8.2 ms | ≤ 10 · ≤ 40 / ≤ 150 ms | pass |
| S3 set value (slider) | dispatch 3.7 · effect 3.6 / 4.6 ms | ≤ 10 · ≤ 40 / ≤ 150 ms | pass |
| S3 menu-bar pick | dispatch 4.0 · effect 4.0 / 5.3 ms | ≤ 10 · ≤ 40 / ≤ 150 ms | pass |
| S3p pop-up pick | effect 368.3 / 372.9 ms (dispatch 377.0, it waits for the menu to close) | ≤ 400 / ≤ 400 ms | pass |
| S4 background pixel click | dispatch 8.6 · effect 18.7 / 43.5 ms | ≤ 15 · ≤ 60 / ≤ 200 ms | pass |
| S5 100 characters, set value / key events | 3.4 / 12.7 ms | ≤ 20 / ≤ 250 ms | pass |
| P1 element-path success | 1600/1600 | 100% | pass |
| P2 pixel mapping, 1× and 2×, centre and 1 pt-inset points | 1600/1600 inside, worst error 0.00 pt | 100%, ≤ 0.5 pt | pass |
| P2r refusals (minimised window) | 200/200 `background_unavailable` | 100% | pass |
| P4 wrong-target actions | 0 | 0 | pass |
| F1 focus theft | 0 | 0 | pass |
| T1 fixture structure text | 614 tokens | median ≤ 1,500 | (Phase 4 gate) |
| T2 fixture window image | 900×632 px, ≈759 Claude visual tokens | ≤ 4,784 | (Phase 4 gate) |

The dots are round, so P2's inset points are the four diagonals 1 pt inside the edge rather than a box's corners.
A pop-up pick can't meet S3: AppKit blinks the chosen item for ≈350 ms before it sends the action (§4.4), and no
background route avoids it. Ruled by the Delegator on 2026-10-09: pop-up picks have their own target, S3p (effect
≤ 400 ms), and S3 is gated on press, set value and menu-bar picks.

### Phase 2: Helper app, broker and the tool surface

**Scope**
- The helper bundle, LaunchServices start, socket authentication, idle exit and crash restart.
- Permission checks and onboarding: Accessibility and Screen Recording status, and the System Settings deep links.
  Plain words, one button per missing grant.
- `ComputerBroker`: grants by permission level (§5), leases, global stop, action log into the blob store.
- The `computer` MCP server for Claude and Codex workers, with image blocks; the CLI through the daemon.

**How it is built** (outline reviewed by Codex and ruled by the Delegator, 2026-10-09)
- **Crate split.** `crates/computer` gets an `engine` feature (on by default) for the backend and its objc2
  dependencies. brigadierd depends on the crate with `default-features = false`: the request types and the helper
  client only, no AppKit in the daemon.
- **The helper process.** Its main thread runs AppKit as an accessory app: the menu-bar Stop item and the ⌃⌥⌘.
  hotkey. The engine runs on its own thread and starts lazily, so the control service (permissions, permission
  requests, stop) answers before any grant exists and onboarding is reachable. Each request has an id; a separate
  control path cancels a request by id or everything, and answers while the engine is busy. A Stop from the menu or
  the hotkey is pushed to the broker, which revokes every lease. The helper accepts only its parent daemon
  (`LOCAL_PEERPID` against `--parent`, plus the per-launch token), and in signed builds the same-team signature
  check (§4.2). It exits when its parent dies, and 10 minutes after the last session ends (not the last request).
- **One helper per daemon.** The broker starts the bundle with `open -n -g -j -a` (a new instance every time, so
  two data directories never share a helper or its arguments), its socket and token under that data directory.
  A helper that crashes is started again on the next call; the call that was running fails with
  `app_not_responding` and is never replayed. Development and tonight's tests use `BRIGADIER_COMPUTER_HELPER=<binary>`,
  which spawns the binary directly so it inherits the grants of the terminal the dev build came from.
- **Bundle.** `stage-sidecar` builds `Brigadier Computer Use.app` on macOS only (Linux and Windows staging
  unchanged; universal builds both architectures before signing), with the build's identity
  (`ai.brigadier.computer-use`, `ai.brigadier.dev.computer-use` for dev), signed ad hoc locally, and Tauri's
  `bundle.macOS.files` puts it in `Contents/Helpers/`.
- **Foreground rung.** Used only after 60 s of HID idle, re-checked right before raising and between every two
  events; any user input aborts at once with `background_unavailable`. Restore puts the user's frontmost app,
  window and cursor back only if they are still what the rung left; a change the user made meanwhile is kept.
  Background mutations wait while the user is actively using the target window (§5).
- **Grants and roles.** Each worker gets a second grant with `Role::Computer`, which can call only the computer
  tools. It reaches the `computer` MCP server and, as `BRIGADIER_COMPUTER_GRANT`, the worker's shell for the CLI. This
  is a role boundary, not shell isolation: Claude puts every MCP server's environment into its own, so the worker's
  shell can also see its main grant, as before.
- **Typed replies.** `ToolReply` becomes text and image blocks; images become MCP image content, before the text.
  `act` honours `screenshot: never | auto | always` for the model; the annotated image of where each action
  aimed is kept for the log only.
- **Permission levels.** Full access runs everything. Under Approve for me and Ask for approval, `apps`, `observe`
  and `zoom` are free; `launch` asks before it runs, and `act` on an instance not yet approved asks once, as a card.
  An approval binds the instance (pid and process start time) and the windows it named or the launch opened; another
  window of that process asks again. Authorization is checked again after the card is answered, and a Stop while a
  card is pending ends the call. Only the user's Stop ends every waiting card; a worker's end ends its own. The
  card lives with its call: the answer goes to the waiting call, and a dropped call or a restart expires it.
- **Ownership.** `launch` tells a new process or window from a reused one (LaunchServices may hand back the user's
  running app). Only new instances become the worker's artifacts in the cleanup ledger, recorded before the launch
  returns (a launch runs to its end even if its caller goes), and only those are quit when the worker ends. A launch
  that opens a file reports the window showing it (the document the app reports, else its title) and lists the
  windows the app restored from the user's saved state apart. When the worker ends, the windows it opened in apps it
  started are closed before they are quit; the restored ones stay. The block list checks the app the system would
  run (looked up by bundle id, path, name, or the app for a file or URL) before anything opens. A launch that takes
  the front gives it back (§2.1).
- **Block list per request.** The broker sends each request's policy from its own state: the Brigadier instance
  that hosts the session (so a worker never drives the window with its own cards) and the terminal windows that
  session launched. One session's exceptions never reach another.
- **Action log.** Each action is a session event; the annotated image goes to the blob store, its artifact registered
  in the cleanup ledger before the blob is written, and a deleted conversation takes its images (blobs another
  conversation also refers to stay). CLI images go to a folder the worker owns, removed when it ends.

**Done when**
- A Claude worker and a Codex worker each finish a fixture task through MCP on a dev build with a scratch
  `BRIGADIER_DATA_DIR`, with screenshots visible to both models (shown in their transcripts), and each clicks the
  8 pt targets from a screenshot through its CLI's real image path.
- S6 is measured.
- Stop cancels a running batch between two events and releases pressed input.
- A blocked app is refused.
- In Approve-for me, `launch` and `act` on a GUI app ask once, bound to the instance.
- The foreground rung refuses while the user is active, aborts when they become active between events, and restores
  focus when used without overriding a change the user made (live when the machine is idle at test time; otherwise
  with the injectable idle source, and said so).
- Each model also zooms and clicks a point read off the zoomed image.
- Cancellation, ownership, approval and lifecycle cases each have an explicit result: a dropped connection, a
  queued request, menu and hotkey Stop, input release and lease revocation; a file opened in an app that was already
  running; another window of an approved process, a denial, pid reuse, Stop while a card is pending; two scratch
  daemons, parent death, idle exit, a crash without replay; two sessions' block lists kept apart.
- The §7 bench is re-run with no regression, S7 is measured, the cross-platform checks pass, and the crate builds
  with `default-features = false`.
- A `select` action sets an arbitrary selected text range, with a test and a fixture check.
- **Helper-bundle gate** (stays unverified until the user grants access): the grants are attributed to "Brigadier
  Computer Use", not Brigadier or a terminal; a missing or revoked grant gives `permission_missing` with the fix;
  the grant survives a helper restart and an app update; the daemon-launched helper works.

**Results (2026-10-09, macOS 27, Apple Silicon).** What proves what: everything below ran with the helper binary
spawned directly (`BRIGADIER_COMPUTER_HELPER`) from a terminal, so it used **the terminal's grants**, not its own.
The helper-bundle gate is still open. Live runs touched only what the test launched (fixtures, TextEdit on scratch
files); terminal windows were sent requests that had to be refused, never input.
- **Both models, through a dev build on a scratch `BRIGADIER_DATA_DIR`** (`brigadierd` from
  `pnpm tauri:debug-app`, Full access, one `scout` each, effort medium):
  - The Claude worker observed with a screenshot (its transcript shows the MCP image as `[Image: …]`). It clicked
    the 8 pt dot and the 8 pt checkbox by pixel in one `act`. It zoomed and clicked the 8 pt purple dot from the
    zoomed image (`i3`, 60,43). Then `"$BRIGADIER_COMPUTER_CLI" computer observe`, Read on the saved PNG, and a
    CLI click on the 12 pt dot.
  - The Codex worker did the same: the MCP image as content, `view_image` on the CLI's PNG, zoom click `i5` 60,54.
    Codex's sandbox allowed the CLI's socket connect.
  - Both fixture logs show `dot-8`, `check-8`, `dot-8-b` and `dot-12` hit, nothing else.
  - Eight `ComputerActed` events went in the store, each batch with its marked image as a blob in `blob_refs` (the
    predicted points sit on the targets; the password field is painted over).
  - Deleting the conversation removed its events and all 7 images. A blob another conversation shares stays: the
    store's own deletion test covers it. The CLI images went with each worker's scratch folder.
- **S6** (tool overhead, MCP → daemon → helper → reply, minus the helper's `engine_ms`): **0.32 ms p50,
  0.47 ms p95**. That's 200 `apps` calls (after 20 warm-up) through `brigadierd mcp --grant-env
  BRIGADIER_COMPUTER_GRANT`, run by a worker holding a live grant. The round trip was 12.5 ms p50, of which the
  engine took 12.1 ms.
- **S7** (helper start to its first served request): **6.6 ms p50** over 10 starts, all 10 within 6.6–6.9 ms; to
  the first engine answer (`apps`) 67.8 ms p50. The lazy engine starts on the first engine request.
- **Lifecycle:**
  - Two helpers ran side by side, each answering on its own socket. A connection with the other helper's token is
    closed without an answer.
  - Parent death: the helper exited in 0.29 s and removed its socket and token.
  - Idle exit (shortened to 3 s by the test override): the helper stayed up 6 s while a session was open and
    exited 3.08 s after the session ended.
  - A crash fails the running call, and the next call starts a new helper without replaying it. This is the broker
    unit test with a fake link, not a live kill.
- **Stop:**
  - `stop_all` mid-drag ×3 and the menu's Stop item ×3 (AXPress on our own status item) each ended the drag between
    two events: 6–12 of 31 drag events, `stopped_by_user`, the mouse-up always delivered.
  - The menu Stop was pushed to the client as `Stopped{by: "menu"}`, and a click after the stops ran normally.
  - A connection dropped mid-drag ×3 released the button (3–10 drag events, then the up) and left the helper up.
  - A queued click cancelled by id while a drag ran answered `cancelled` and never reached the fixture.
  - The hotkey's path (`hub.stop("hotkey")`) is unit-tested; pressing the real keys needs real input.
  - Lease revocation on Stop is in the broker tests.
- **Block list:**
  - The fixture as one session's host was refused for that session and allowed for another.
  - `apps` marks the terminal the test ran in "blocked (a terminal the session didn't launch)", and an
    `observe` of its window is refused with `blocked` and no image.
  - `launch` of Keychain Access is refused before anything opens.
- **Launch and ownership:**
  - TextEdit (not running) on `a.txt`: a new process and one new window. `b.txt` then reused that process with a
    new window (`new_process: false`).
  - Neither launch took the front (`open -g`), so nothing needed giving back.
  - A worker's launched TextEdit was quit by the ledger when the worker ended.
- **Approve for me, live:** a Claude worker ran `launch` TextEdit on `c.txt`, a click in that window, then two
  presses on the fixture window. That took exactly **two cards**: one for the launch, which then covered its
  window, and one for the fixture window ("only this window of this target-range process (pid 11971)"). The second
  press didn't ask. Another window of the same process, a denial, pid reuse and Stop while a card is pending are
  the flow tests (`flow::computer_tests`), not live.
- **Foreground rung:**
  - Live on an idle machine: raised, clicked and gave the front back (P2f in the quick bench; the full bench below).
  - Refusing while the user is active, aborting between events, and keeping a front the user changed are tested
    with the injectable idle source. Real hardware input wasn't possible tonight.
  - Idle is read from the combined session state; our own pid-posted events reset HID idle but not that.
- **Codex's built-ins:** `computer_use` and `browser_use` stay disabled. The adapter passes `--disable` for both,
  and `codex --disable computer_use --disable browser_use features list` (codex-cli 0.161.0) shows both `false`.
  Codex workers use ours.
- **Transcripts:** a Codex MCP result is shown as its text with `[image image/png]` for each image. Before this, the
  base64 filled the clipped output and hid the text after it.
- **Bench:** the full §7 bench, re-run on `14273968` (release build, 200 repetitions, 1052 s), passes every gate with no regression from Phase 1: S1 6.2/7.6 ms, S2 61.5/66.3 ms, S3 set value 2.3/3.1, menu-bar pick 4.1/5.7, pop-up 361.5/368.9, press 2.4/3.0 ms, S4 16.8/40.3 ms, S5 2.5/10.4 ms, SEL 20/20, P1 1600/1600, P2 1600/1600 (worst 0.00 pt), P2r 200/200, P2f 1/1 (live, idle machine), P4 0, F1 0.
- **The verification pass** (after the code review in §10.3, on the final tree):
  - One Claude scout through a dev `brigadierd` on a scratch data dir: `observe` with a screenshot and `zoom` both
    reached the model (`[Image: …]` in its transcript; it read the 8 pt red dot at 639,72 and the purple one at
    80,64 of the zoomed `i2`). The fixture log shows `dot-8` and `dot-8-b`, nothing else. Two `ComputerActed`
    events with their images; deleting the conversation removed the events, all blobs and the CLI session folders.
  - `launch` of `Keychain Access` by name and by bundle path: `blocked`, nothing opened.
  - TextEdit (not running before): each launch of a file reported just that file's window; closing the windows a
    test opened answered `closed` (once a just-opened window missed the first press, so the ones still open are
    pressed again). On this Mac TextEdit restored no windows after a kill or a clean quit with a window open, so
    telling the file's window from restored ones is proven by the engine tests, not live.
  - The quick bench passes every gate, P2f included (`bench --quick`, 20 reps, 107 s).
  - The full §7 bench on `5c4f3628` (200 reps, 1055 s) passes every gate with no regression: S1 6.3/7.9 ms, S2
    61.6/66.0 ms, S3 set value 2.4/3.1, menu-bar pick 4.2/6.5, pop-up 361.4/368.2, press 2.4/3.0 ms, S4 17.2/41.7 ms,
    S5 2.4/9.8 ms, SEL 20/20, P1 1600/1600, P2 1600/1600 (worst 0.00 pt), P2r 200/200, P2f 1/1, P4 0, F1 0.
- **Not done or open:**
  - The helper-bundle gate needs the user's one-time grants.
- **Deviation:** the action log writes the blob first and the event that mentions it second. There's no separate
  ledger artifact: the store keeps any blob an event mentions, and collects one no event mentions after its grace.
  It's the same model as stored tool output.

### Phase 3: Agent cursor and the action log in the UI

**Scope**
- The overlay (§4.5).
- The session's computer timeline in the worker's thread: actions, rungs, effects, screenshots with predicted points.
- The permission card when a grant is missing.

**Done when**
- Two workers act at the same time with two distinct cursors.
- The cursor never appears in a capture.
- The timeline replays a bench run.
- UI tests pass.

**How it is built** (outline reviewed by Codex and ruled by the Delegator, 2026-10-09; the five corrections in
`msgs/w09-reply-1.md` all accepted)
- **The action log first.** Every action of a batch gets an event, also the ones that failed before acting and the
  ones skipped after a failure. Each carries its batch (one `act` call), its index in it, the app's name and the
  target in words. Batch and index are its identity, so a history read and live events dedupe. The batch's marked
  screenshot rides on its first action.
- **The cursor contract.** The engine tells a `CursorSink` where each action aims, in global points, right before
  it is delivered, and never waits for the drawing. The broker sends the worker's name with each request. The helper
  hub labels the cursor before each job, ends it on the session's end and clears every cursor on Stop.
- **The overlay.** A pure `CursorScene` (fade, end, clear, updates still queued, tested without AppKit) drives Core
  Animation layers on the main queue: an arrow in the worker's colour, the name pill, the glide, the click pulse and
  the element outline.
- **The timeline.** A worker's computer calls (Claude's `mcp__computer__*`, Codex's `computer/*`, the CLI through
  `$BRIGADIER_COMPUTER_CLI`) fold out of its activity into one "Used the computer" disclosure after its recent
  activity. Its line says what it did, or what it does now while live. Open, it shows the shown step's batch's
  marked screenshot (full size on click), a player (Previous, Play, Next, "Step i of n", a step being one action, as
  the line counts them; ←/→, Home, End and Space on its toolbar, which keeps the focus at either end) and the steps:
  action and outcome in words on every step, the route and its time only on the shown one's. History is read in
  pages (`listComputerActions`, which reads past pages holding only other workers' actions) and kept up by live
  `computerActed` events. Screenshots are read with the existing `readAttachment`.
- **The permission item.** A missing grant raises one "Waiting on you" item per conversation. It holds up no request:
  the worker has its error, which says the user is already asked, and goes on. It has one Allow button per missing
  grant and no Done button. It closes by itself when a read finds both grants in, or on the worker's next working
  call, and is found again after a restart. The thread relaying a worker's report about the same permissions
  (`note_for_user` naming Accessibility, Screen Recording or computer use while the item is open) is told it is
  already listed, not given a second item.
- **`bench --replay <dir>`** writes the bench's action log the way the daemon keeps it (`actions.json`, one
  `ComputerAction` per action) with each batch's marked PNG beside it.

**Results** (2026-10-09; all on this Mac with the helper spawned from a terminal, so with **the terminal's
inherited grants**, not the helper bundle's own)
- **Two workers at once, two cursors: PASS (live).** `cargo run --release -p brigadier-computer --example
  cursor-proof` starts the helper as the daemon does, and two workers over its connection act together on their own
  copy of the fixture: one presses buttons through accessibility, the other clicks dots by pixel. 12/12 and 12/12
  actions done in 7.8 s. A screen capture midway shows both cursors: purple "Fix the login page" and teal "Check the
  release notes". The helper runs one engine thread, so the two workers' requests interleave; both cursors are
  shown together.
- **Never in a capture: PASS (live), byte for byte.** With a worker's cursor parked over its window (a screen
  capture at that moment shows it there), the engine's `observe` screenshot of that window and one taken after the
  worker's session ended are the same 91,973 bytes. The action log's marked image of the same click with the cursor
  on its point and after it faded: the same 91,967 bytes.
- **The cursor changes no result: PASS.** The quick bench with the cursor drawn (`bench --quick --cursor`) passed
  every gate in five runs, P4 0 and F1 0. One earlier run failed with "no app with pid" (fixed, below).
- **Bench:** the full §7 bench with the cursor drawn (`bench --cursor`, release build on `24323b5c`, 200 repetitions,
  1052 s) passes every gate. Against Phase 2's last run, pixel clicks' effect is about 4 ms slower with the cursor
  drawn (S4 p50 21.3 against 17.2 ms, p95 48.0 against 41.7 ms; the gate is 60/200 ms); the rest is level: S1 6.2/7.9 ms, S2 56.8/61.4 ms, S3 set value 2.4/5.0,
  menu-bar pick 3.7/6.0, pop-up 362.2/369.7, press 2.5/5.3 ms, S4 21.3/48.0 ms, S5 2.6/7.5 ms, SEL 20/20, P1
  1600/1600, P2 1600/1600 (worst 0.00 pt), P2r 200/200, P2f 1/1, P4 0, F1 0.
- **The timeline replays a bench run: PASS (test).** `bench --quick --replay` wrote 456 actions in 451 batches.
  One batch of each kind (9 actions in 8 batches: element press, a confirmed checkbox, a slider set, a menu pick
  with no image, a pixel click, a refusal, select and type in one batch, the foreground rung) is the fixture
  `computer-bench-run.json`. `src/replay/computer.ts` feeds it as live events through the board reducer; the test
  checks each frame's batch count, words and outcome, and that history overlapping live events shows once.
- **UI tests: PASS.** SSR tests render the timeline (one disclosure; action and outcome on every step; route only on
  the shown batch; the toolbar's buttons; a failure in red; live, looked-only and Load earlier) and the permission
  item (an Allow per missing grant, the System Settings hint after a click, closed once both are in). A headless
  Chromium test drives the real `WorkerThread`: the computer calls leave no rows of their own (an unfolded run reads
  "Used apps, used observe, used act"; folded, only "Read a file" and the timeline), ←, Home, End and Play step
  through the batches and stop at the end, and each shown batch reads its screenshot.
- **On a dev build: PASS (live).** One Claude scout (effort low) through a dev `brigadierd` on a scratch data dir
  pressed "Button 8 pt", ticked "Check 8 pt", set "Level" to 75 and clicked the red dot by pixel. The fixture's log
  shows exactly those four events, the dot at its centre (60, 30). The dev app's worker thread shows one "Used the
  computer · 4 steps in target-range" line, and open, the batch's marked screenshot, the player and the steps. The
  dev app was driven with our own engine (`brigadier-computer run`, its window only). The permission item was
  captured in the recorded-session fixture (`thread-session.html?computer=1`), because a missing grant can't be
  produced on this Mac tonight without changing privacy settings.
- **Found on the way, fixed:** with AppKit running on the main thread, a just-launched app could be missing from the
  running apps for a moment ("no app with pid", once in five runs); a live process is now waited for, up to 1 s. A
  panic in the overlay's work thread now ends the process instead of leaving it drawing forever.
- **Verified again** (2026-10-09, fresh checks on the final tree): `cursor-proof` again showed both cursors at once
  (12/12 and 12/12 actions in 7.4 s) and the same bytes with and without the cursor (91,971 for `observe`, 91,967
  for the action log's image). On a dev build, a Claude scout pressed, ticked, set the slider, typed into "Name" and
  clicked the red dot by pixel, and the fixture's log shows exactly those five events. The thread reads "Used the
  computer · 5 steps in target-range", and its player reads "Step 5 of 5". With the dev bundle's own helper, which
  has no grants (nothing in privacy settings touched), a scout's call got `permission_missing`. The session's
  summary then listed "Let workers use apps on this Mac" with an Allow per grant, and after a daemon restart the
  earlier conversation's item was still there. The quick bench and the full bench with the cursor drawn (200
  repetitions, 1057 s) pass every gate: S1 6.2/8.1, S2 56.9/61.8, S4 20.8/47.0, S5 2.5/7.0 ms, P1 and P2 1600/1600,
  P4 0, F1 0.
- **Found on the way, open, and not ours:** while one of these terminal-attributed processes (the helper, a bench)
  holds ScreenCaptureKit, every capture from another of them times out until the first exits; the system
  `screencapture` isn't affected. It isn't a leak in our capture: dropping the cached `SCShareableContent` after
  every capture changes nothing, and two plain Swift processes that only call `SCShareableContent` and
  `SCScreenshotManager.captureImage` (Apple's API, none of our code) block each other the same way: the second
  capture waits until the first process exits. All of them run under the terminal's identity, so this is likely a
  per-client limit that a helper with its own grant never hits. It bites in development: a terminal capture while
  a dev daemon's terminal-granted helper works made that worker's batch screenshot time out (the timeline then
  says "No screenshot for this step"). The case that matters is the installed app's helper and a dev build's at the
  same time. To check once the helper bundle has its own grant: start one helper, have it capture a window, then
  capture from a second helper.

### Phase 4: The GUI specialist, the model-in-the-loop suite and macOS comparisons

**Scope**
- Fit into docs/THREAD-PLAN.md's router and roles, not beside them:
  - every worker gets the `computer` server for quick checks;
  - **task kind** `Operate` in `TaskKind` (`crates/core/src/work.rs`). It writes nothing that lands (`writes()` is
    false), so it has no worktree branch to land. It runs in a scratch dir, plus the dev build or files the brief
    names;
  - **delegation**: `delegate_task` takes `kind: "operate"` with the goal, the target (an app, a dev build, a URL),
    the expected end state, and what to return (a report, the action-record range and the final screenshot
    artifact);
  - **router**: a `TaskCategory::Operate` mapped from the kind, with `default_floor` Strong and effort medium (not
    the Frontier floor of implement roles); registry scores for GUI precision per model, seeded from the suite;
    **image-capable models only**, because pixel work needs vision, decided by the model's capability, not by the
    task's first attachments; fallback to the next image-capable model on quota;
  - **access**: the session's permission level, with §5's escalation rules;
  - the thread's prompt says when to hand GUI-heavy work to an `Operate` worker (more than about five GUI steps, or
    an exploratory GUI task).
- The `Operate` brief carries the house rule and the batching and expect habits.
- The 20-task suite and its checkers, plus 50 grounding trials per target size for P3.
- Side-by-side macOS runs of the suite against other installed computer-use tools, including Codex's built-in one,
  as comparison targets only, once the user has granted them. They are never production paths.

**Done when**
- The suite runs end to end on Claude and Codex workers, 3 runs per provider.
- E1 and P3 meet their gates.
- Total provider usage per completed task is reported.
- A GUI-heavy request in a dev session is handed to an `Operate` worker.
- The macOS comparison table is in the evidence (or the report says the grants weren't given).

**How it is built** (outline reviewed by Codex and ruled by the Delegator, 2026-10-09; the five corrections in
`msgs/w12-reply-1.md` all accepted)
- **The kind.** `TaskKind::Operate` writes nothing that lands and gets `RepoAccess::None`: a scratch dir only.
  `TaskCategory::Operate` has the floor Strong, effort medium and no trials; its needs always include image input,
  on creation and on every re-route, so a quota fallback only moves to another image-capable model. A settings
  migration adds Operate to saved "no worker tasks" switches.
- **Delegation.** `delegate_task` with `kind: "operate"` requires `target` (the app, window or URL) and `end_state`,
  and refuses them on other kinds. The brief carries the house rule (code or API first, structure next, pixels
  last), batching, an `expect` on every action, reading the diff, `zoom` for small targets and checking the end
  state. The `computer` server is always loaded for Operate. The report the thread gets ends with
  `Computer actions: batches a–b · last screenshot: artifact <hash>`; the screenshot is a stored artifact that
  `read_artifact` returns as an image. The thread's prompt sends GUI work of more than about five steps, or
  exploratory GUI work, to an Operate worker.
- **The suite** (`crates/computer/src/suite.rs`, `suite_run.rs`; `brigadier-computer suite …`): 20 tasks with
  ground-truth checkers, 4 grounding boards, a scripted (no-model) solver that gives each task's reference batches,
  and an independent focus monitor. 15 tasks run on the fixture, 3 on a document-editor fixture (`scratch-pad`: an
  NSTextView whose Save is enabled only by a real edit, with a find bar), and 2 on Brigadier's dev build on its own
  scratch data dir. A trial is judged by what reached the app: the fixture's own log and a state snapshot it writes
  on SIGUSR1 (a value set through accessibility sends no notice), plus the broker's delivered actions on the trial's
  window, never the worker's own expects. Any other control touched is a wrong target.
- **The runner** (`tools/computer-suite/run.py`): per trial, the fixture is set up, the thread is asked to
  `delegate_task` with exact arguments, and the runner waits for the task's end. Then it collects the broker's
  records, the report, the check and the teardown. Model calls are counted from the transcripts: Claude's distinct
  assistant message ids, Codex's distinct `response_id`s, cross-checked by its token totals. A shortcut audit fails
  any write to the trial's files or any call into Brigadier's socket or CLI; a read is listed as a peek.
  `summarize.py` gives the tables, E1, usage per completed task and F1. A front change counts as the suite's only
  when the suite caused it: after an action, or while a fixture opened. The user may be at the Mac.
- **The live handoff check** (`handoff.py`): a plain seven-step GUI request, worded as a user would, in a new
  session on the new-session default. It is judged on the whole request's end state.
- **Found while building it, fixed in the engine:**
  - an element scrolled out of view is scrolled to (`AXScrollToVisible`, else calibrated wheel steps) before a pointer
    click;
  - a document text view is typed with real keys, because a value set through accessibility doesn't count as an
    edit;
  - chords with ⌘ or ⌃ go under synthetic activation, because inactive apps ignore menu shortcuts;
  - a menu item a background app shows as disabled is reached through its own shortcut;
  - a `checked` expect on a row, tab or cell reads its selection;
  - an `observe` that asks for a screenshot starts the capture before it reads the tree. An app in the background
    answers its first accessibility read after a pause of 60 ms or more in about 25 ms instead of 6.5 ms. That cost
    showed once the fixtures opened behind other windows, and it put S2 at 77.3 ms against ≤ 70. With the capture
    and the read overlapped, S2 is under 70 again (Results);
  - a background app is activated once per batch, not once per action. A pixel click or drag, a ⌘/⌃ shortcut, or a
    menu item reached through its shortcut runs under synthetic activation: the app is told it is active and its
    window key, then inactive again. Its window redraws its active look and back each time, so a full bench run
    (about 2,400 such clicks) made the fixture flicker without a pause, and a worker's batch of n clicks flashed n
    times. The activation is now held for the batch's window and let go when the batch ends, before its closing
    look, without the defocus when the app has become the user's own front app meanwhile. A two-click batch now
    logs one activation where it logged two;
  - a capture the system fails to start ("Failed to start stream due to audio/video capture failure", 2 of 12
    grounding runs, and once for over 350 ms while the Mac was in use) is asked for again, four times over about
    2 s. The error the worker gets names the system's reason.
- **Pulled forward from Phase 5 (deviation, ruled by the Delegator): windows on other Spaces.** With the user in a
  full-screen app, every other window is off screen, and accessibility lists only the current Space's windows. The
  engine now also reads the app's `AXMainWindow` and `AXFocusedWindow`, which are given wherever they are. Failing
  that, it scans the app's elements by remote token (§12). It captures an off-screen window from the window
  server's backing store (`SLSHWCaptureWindowList`, ~100 ms), where ScreenCaptureKit times out. The remote-token scan
  only finds elements some accessibility client already reached, so a window nobody has touched yet on another
  Space may still be missing.
- **Fixtures open through LaunchServices** (`open -n -g`, as a minimal bundle), never exec'd from a terminal: a
  fixture exec'd while the terminal was frontmost took the front.
- **Signals are checked:** before the suite or the bench signals a pid, its start time and binary must still match
  what was recorded at launch.

**Results** (2026-10-09; the helper spawned from a terminal, so with **the terminal's inherited grants**, not the
helper bundle's own; a person was using the Mac during every run from about 11:30; evidence:
`docs/evidence/2026-10-09-computer-use-phase4.md`)
- **The suite, 3 runs per provider: PARTLY DONE.** Claude (Opus, effort medium): 20/20, 20/20, 17/18. Codex
  (gpt-6.1-sol, effort medium): 20/20, 18/18, 18/18. Six dev trials didn't run (the two dev-build tasks in Codex
  runs 2 and 3 and Claude run 3), because the dev build's window had been closed and every way to reopen it brings
  the app to the front. The one failure is Claude run 3's menu trial: the worker read "Pick Targets" as the menu's
  name and read the menu bar through a script, which the shortcut audit fails. The task's wording was fixed; a
  menu-only check with the new wording passed in 1 batch and 3 calls.
- **E1, as amended (model calls ÷ (scripted tool calls + 2) ≤ 1.3): Claude PASSES, Codex MISSES.** Pooled
  medians: Claude 1.0, Codex 2.25. Per run: Claude 1.0, 1.25, 1.25; Codex 2.5, 2.5, 1.75. Run 3 had argument errors
  that show a well-formed call, and the call shapes were in the Codex brief. The gate as first written, calls per
  reference batch, missed for both: Claude 4.0, Codex 8.0.
- **Where Codex's extra calls go** (run 3: 128 calls over 18 trials):
  - 13 rejected `submit_report` calls. Codex sends `done_when` as a list of objects (criterion, status, evidence)
    or `changes` as a string. The error names a serde type ("data did not match any variant of untagged enum
    Lines") instead of showing the call, so it guesses again, 2–3 tries per report.
  - 16 calls that run no tool: code-mode `exec` calls that list `ALL_TOOLS` to read a tool's schema, its report
    tool's included, or that only print.
  - 12 computer calls rejected for their arguments.
  - Without those 41 calls, run 3's median would be about 1.25, inside the gate. That is an estimate, at one model
    call per tool call, which is how Codex worked here.
- **P3: PASS.** On the router's Claude pick (Opus, effort medium; the cheapest image-capable model it would choose
  for Operate), 50/50 trials at 8, 12, 16 and 24 pt, with no wrong targets and no misses. The largest error was
  1.4 pt, and the Wilson lower bound per size is 0.929. That is an empirical pass, not a statistical proof of ≥ 98%.
  Each size took 13 calls and about 51 s.
- **Usage per completed task: REPORTED.** Pooled per provider, worker tokens (uncached input / cache read / cache
  write / output): Claude 12 / 115,913 / 8,137 / 1,106; Codex 22,133 / 136,087 / 0 / 724. The thread that delegated
  each trial adds about 8 / 136,506 / 1,547 / 377 for Claude and 6 / 104,842 / 1,291 / 346 for Codex. Median wall
  time per trial: Claude 18.1 s, Codex 51.2 s.
- **Live handoff: PASS.** A plain seven-step GUI request, worded as a user would, in a new dev session was delegated
  as `kind: "operate"`. The router picked Codex at effort medium. The task was done in 114 s, and the whole request's
  end state checks out from the fixture's own log.
- **F1 during the suite:** 0 front changes caused by the suite in five runs. In Codex run 2, the rule counted 1: the
  front changed 1.0 s after a failed type. But the person's own window changes were happening just before it, and
  macOS gave the front to our topmost window. Fixtures now open behind every other window, which removes that path.
- **Registry:** `strengths.operate` is seeded for the two models measured: Opus 9, gpt-6.1-sol 8.5. The others stay
  unrated.
- **Comparisons: NOT RUN.** None of the other installed computer-use tools could run tonight. Tool A's runtime was
  off; turning it back on is a setting only the user can change. Tool B (Codex's built-in computer use) and tool C
  need a plugin install, a login or Screen Recording and Accessibility grants, which nobody could give.
- **Bench** (`bench --no-foreground`, which skips P2f, the one step that raises a window):
  - **Before the fix:** the full bench on `deceff7c` (200 repetitions, 1209 s) passed every gate but S2, at 77.3 / 86.3 ms
    against ≤ 70 / ≤ 120. That is the background read cost found above. The others: S1 6.5 / 7.8, S3 set value 4.3 / 23.8,
    menu-bar pick 5.2 / 10.7, pop-up 370.1 / 379.5, press 4.2 / 6.9, S4 14.8 / 39.0, S5 3.3 / 16.7 ms; SEL 20/20, P1 and
    P2 1600/1600 (worst 0.00 pt), P2r 200/200, P4 0, F1 0.
  - **After the overlap:** the quick bench (20 repetitions) passes every gate, with S2 at 49.9 / 53.5 ms.
  - **On the final engine** (`392f100b`: the overlap, activation once per batch, the capture retry), the quick
    bench passes every gate but F1. F1's 187 changes are the person's own: their cursor moving and their app
    quitting, while our events never move the cursor. S5 landed 20/20 both ways, by key events and by value.
  - **No full bench on the final engine.** Two runs were stopped. The first, at the person's request after 16
    minutes, when they saw the fixture flicker (fixed above); the bench writes its timings at the end, so it has
    none, and in that run the key-event typing of S5 never landed. That didn't happen again: S5 landed 20/20 in
    every quick run after it, with and without the activation hold. The second was stopped after 20 minutes,
    because another run's test loops had taken the machine to a load average of about 90. Its actions, 111–124 ms
    apart at first, were 200–800 ms apart by then, so any timing it gave would measure that load. The full bench
    needs a quiet Mac.

- **Signed build: PASS (live).** The dev app was built with `APPLE_SIGNING_IDENTITY` set to the user's Developer
  ID (`pnpm tauri:debug-app`; no Keychain prompt). `codesign -dv` shows the app and `Contents/Helpers/Brigadier
  Computer Use.app` both with that team's identifier and the hardened runtime. Each satisfies a designated
  requirement anchored on the team (`certificate leaf[subject.OU]`), so the helper's privacy grants survive
  rebuilds signed the same way. The helper's own signature carries no timestamp (`--timestamp=none` in
  `stage-sidecar.mjs`), which a notarized release needs. The same-team peer check, with the signed helper serving
  and one test client in two signings:
  - signed by the team, right token: admitted;
  - signed ad hoc: refused, "isn't signed by team …";
  - signed by the team, wrong token: refused, "wrong token".

**Not done or open**
- **Codex's E1.** The fixes, in order of calls saved:
  - `submit_report` accepts what Codex sends: a list of objects or a string wherever a list of lines is asked for.
    Its argument errors show a well-formed call, as the computer tools' do;
  - the Codex Operate brief gives `submit_report`'s call shape beside the computer tools', and says the shapes are
    complete, so it never needs to list `ALL_TOOLS`;
  - the computer tools accept the remaining argument spellings Codex used.
  Then one Codex run of the suite, to check E1 against the gate.
- **The six dev trials**: not run, because the person closed the dev build's window and it was still closed at the
  end; reopening it ourselves would take the front.
- **The full bench on the final engine**, on a quiet Mac.
- **The comparisons**, once tools A–C are turned on and granted.
- **The helper bundle's own grants.** Every live run used the terminal's inherited grants.
- **A window on another Space that no accessibility client has reached yet** can't be found by remote token. It is
  found once its app is touched, or when it is the main or focused window.
- **WebKit's first contact** can give a partial tree until `AXEnhancedUserInterface` or `AXManualAccessibility`
  takes effect; a second read is complete.
- **Contention between terminal-granted processes:** the helper lives as long as its daemon, by design, so a second
  terminal-granted process's capture times out while it lives (Phase 3, above). It bites only in development.
- **`launch` puts the new window on top.** An app a worker launches opens over the user's windows, though without
  keeping the front: it opens in the background, and gives the front back if the app takes it. Only the suite's
  fixtures open at the back.
- **A background app's window still flashes once per batch.** Synthetic activation makes the window redraw in its
  active look and back. It now happens once per batch, not once per action (above), so a two-click batch flashes once
  where it flashed twice. Only a window that never needs activation (an element action, or a view that takes a first
  click) never flashes.
- **Quota steers Operate to Codex.** With Claude's 5-hour window projected high, the router picks Codex for Operate.
  That is by design, but Codex took about twice as many calls and 2.8× the wall time per trial.

### Phase 5: Browsers and hard surfaces

**Scope**
- Pages in a Chrome the session launched: CDP refs (page snapshot, ref click and fill), so pixels are the last rung.
  Pages in the user's own browsers: their accessibility web area.
- App quirks: Electron, Catalyst, SwiftUI, canvases (vision plus zoom), sheets and dialogs, several displays and
  Spaces, hidden and minimised windows.

**Done when**
- The suite gains 10 browser and quirk tasks, and they meet P3 and F1.
- Synthetic activation (§2.1), if the spike left it open, is measured with its own gate: ordinary inactive
  controls clicked in the background with F1 at 0.

#### Stream B — app quirks (results, 2026-10-09)

Built on branch `cu-quirks` (from `computer-use` d6931c02). macOS only, as the user ruled. All evidence comes from
the cmux terminal, so the helper ran with the terminal's inherited Accessibility and Screen Recording grants, not
grants of its own. No TCC setting was touched.

**What the engine does now**
- **First contact** (`macos/quirks.rs`, once per process instance):
  - Electron apps get `AXManualAccessibility`. An observe waits for the page's web area, up to 3.5 s.
  - Each Electron window is made key once by synthetic activation, held until the app reports it focused (up
    to 300 ms). Until then Electron serves no page tree, or ignores presses.
  - The first observe of any window walks it until its element count holds for 250 ms. AppKit, SwiftUI and
    Catalyst windows add elements just after the first read. An app younger than 2 s is settled no sooner than
    2 s after its launch: a Catalyst app built a stepper about 1.2 s after launch, after a quiet second, in 5 of
    8 launches. An app that is already running pays only the 250 ms once.
- **Off-Space windows nobody has touched:** made key inside their app, read as its focused window, then
  defocused (never the user's front app). Elements are framed relative to the window's position as
  accessibility reports it, because accessibility places an off-Space window whole display widths away.
- **Text fields:** a value is set, then replaced as an edit (focus, select all, insert). A SwiftUI binding hears
  only the edit; some AppKit controls hear only the set.
- **Minimised windows and hidden apps:** a window's observation now says `· minimised` or `· app hidden`.
  Element actions work on a minimised window; pointer actions refuse it with `background_unavailable`.
- **Several displays:** window-to-global points, the scale of the display under a point and Cocoa screen frames
  are small functions in `geom.rs`. They are unit-tested for a 1× display left of and above a 2× main display.
  **This Mac has one display**, so there is no live second-display check.

**Suite tasks** (`suite_quirks.rs`; fixtures `quirk-pad`, `electron-pad`, `catalyst-pad`, built by the suite):
`electron-signup`, `swiftui-item`, `catalyst-order`, `save-panel`, `minimised-code`. Each has a checker on the
fixture's own log and state, and a scripted reference. All 5 scripted references pass, and so do `name` and `form`.

**Model runs** (Opus medium, gpt-6.1-sol medium; 2 runs per provider). F1 suite-caused focus changes: **0** in
every run.

| run | as run | after the checker fix | failures |
|---|---|---|---|
| claude-1 | 4/5 | 4/5 | `minimised-code`: the worker used `osascript` to confirm the window stayed minimised (shortcut audit). Fixed in the engine: the observation now says `minimised` |
| codex-1 | 4/5 | 5/5 | `catalyst-order`: the order was placed, but the worker's own expectation on Place Order went unmet |
| claude-2 | 3/5 | 4/5 | `catalyst-order` as above; `save-panel`: the worker searched the disk with `find /`, then `pkill -f` (shortcut audit) |
| codex-2 | 4/5 | 5/5 | `catalyst-order` as above |

Runs 1 used an earlier engine; runs 2 used the final one. The checker fix aligns the quirk checker with the main
suite's (`suite.rs`): an action that was sent, where only the worker's expectation went unmet, counts as the tool's.
An action refused before it was sent still doesn't count.
- **P3:** not exercised. No worker made a pixel action in any of the 20 trials; every action went through
  structure (element and background-activated rungs).
- **E1** (calls ÷ (scripted tool calls + 2), median ≤ 1.3): **MISS**. Claude was 1.5 in both runs; Codex was
  1.67 in run 1 and 1.5 in run 2. Pooled, both providers are at 1.5. The workers observe once more than the
  reference, to verify. Phase 5's done-when asks for P3 and F1, not E1; E1 is reported for completeness.
- **Off-Space save panel:** while its window is on another Space, a save panel keeps Save disabled. This held for
  15 s whether the name was set, inserted, or both, and after a synthetic make-key. Pressing the disabled button
  does nothing. `save-panel` passes when the window is on the user's Space (both providers, and the scripted
  reference). It can't finish while the user is in another Space, for example a full-screen app. Open.

**SA gate** (bench): a background pixel click, with no ref, on quirk-pad's inactive NSButton, NSTextView and
SwiftUI button, 200 each. **600/600 landed, 0 focus changes: pass.** p50 dispatch is 3.3–3.4 ms; p50 effect is
9.9–15.4 ms. The quick bench gave 60/60, 0 focus changes.

**Full bench** (200 reps, `--no-foreground`, 2026-10-09 17:03–17:23, under a load average of 12–20 from other
workers' builds): every gate passes but F1. The 4 focus changes are all a browser fixture, "Google Chrome for
Testing — Web Range", coming to the front three times. The parallel browser stream was launching it at the same
time; the front-app log has it at 17:04:27, 17:14:17 and 17:20:06, and the bench's own target-range never took
the front. The bench can't tell another worker's launches from its own, so F1 counts as a MISS for this run.
Rerun it when nothing else is driving the Mac.

**Also open:**
- A menu-bar pick brought the target app to the front once. The quick bench's `S3 pick menu-bar` did it while a
  system alert ("… quit unexpectedly", from UserNotificationCenter) was the front app. With an ordinary front app,
  200/200 menu picks changed nothing. This is the menu path, not stream B's; the repro is that alert in front,
  then a menu pick.
- A fixture that crashes raises the system's "quit unexpectedly" alert, and that alert takes the front. It
  happened once, when catalyst-pad's plist still carried `NSPrincipalClass` (fixed in e426472b).

**Private dependencies:** no new calls. The reveal and the wake reuse the make-key and focus records (§12).
`AXManualAccessibility` is an undocumented attribute (§12).

#### Stream A: browsers (2026-10-09, branch `cu-browser`)

**Built**
- **A browser the session launched, through its debugging protocol** (`crates/computer/src/cdp/`, `engine/web.rs`).
  - `launch` of a Chromium browser starts it in the background with these flags:
    - a scratch `--user-data-dir` under `$TMPDIR/brigadier-browser/`;
    - `--remote-debugging-port=0` on 127.0.0.1;
    - `--no-startup-window`.
  - Pages open with `Target.createTarget {background: true}`, so the browser never comes to the front.
  - Another helper process takes on such a browser by its scratch profile, so the suite's setup and the worker's
    helper can be different processes.
  - `observe` gives a page tree with refs, built from the accessibility tree of every frame and stitched under its
    iframe owner. Out-of-process frames come from their own sessions; boxes are mapped from the document, through
    the viewport (scroll and zoom), to the window.
  - Refs use the same action schema as native ones. `click` and `drag` are trusted input after a frame-by-frame
    hit test, so a covered element is `occluded`. The other actions:
    - `set_value` selects all, then inserts;
    - `type` inserts at the selection;
    - `<select>` is picked with keys;
    - `scroll` goes through the page's root ref;
    - `navigate` is the one new action.
  - Screenshots and `zoom` come from the page itself, so they never take focus. Settling waits for network idle
    and a quiet DOM.
  - JS `alert`, `confirm` and `prompt` stop in the debugger (`Debugger.paused`). They show as virtual refs (accept,
    dismiss, the prompt's text) and are answered without page script and never auto-accepted. The browser's own
    dialog window activated Chrome, which this avoids.
  - Actions on this path are recorded with the rung `page`. The UI words it as "Inside the page, through the
    browser".
- **Browsers without a debugging port: their accessibility web area** (`macos/web.rs`).
  - The first look at a Chromium browser sets `AXManualAccessibility` and `AXEnhancedUserInterface` on the app.
    Chromium builds the page tree about 2.6 s later.
  - The first look at any web area waits until its node count is stable and its holder lies inside the window.
    WebKit reports its scroll area at a stale place on first contact.
  - A WKWebView builds its tree only when asked. Until then it is an empty group covering much of the window, and
    the first look waits up to 1 s for the page to appear.
  - **A page's field gets focus within its page before `set_value`.** WebKit gives a value to whichever field is
    focused, not the one it was sent to: unfocused, "Email" overwrote "Name". This is focus inside the app, not
    the user's.
  - A web view in a background app names no focused element. `type` therefore trusts the field's own `AXFocused`
    there, and waits up to 1 s for focus.
- **Fixture:** a std-only local HTTP server (`web_fixture.rs`, started and stopped by the suite) serving one page.
  - The page has a form (an 8 px checkbox, 9 px star buttons), JS dialogs, a `<dialog>`, a canvas, a same-origin
    iframe and a cross-origin one in a scrolled box.
  - Every event is logged with `isTrusted`, and a request that no browser made goes in a `.foreign` file.
  - `web-view` is a WKWebView window: the stand-in for Safari, which tests never drive.
- **Suite:** `web-form`, `web-iframe`, `web-dialog` and `web-canvas` run through the protocol; `web-ax-form` runs on
  Chrome without a port; `web-ax-form-webkit` runs on the WKWebView. `Web grounding 8 px` and `16 px` are P3 boards
  on the page canvas.
  - The checkers fail any of these:
    - untrusted input;
    - a wrong control;
    - a missing page rung on the protocol path, or a page rung where the protocol isn't offered;
    - a request no browser made;
    - no done record.
  - The audit counts the debugging port and the scratch profile as shortcuts.

**Results** (Chrome for Testing 155.0.8059.12; the helper ran from the cmux terminal with its inherited grants;
the protocol path needs none)

| Gate | Result | Evidence |
|---|---|---|
| Scripted references | **8/8 web pass**; the whole scripted suite 28/28 in one run before the two grounding tasks were added | `brigadier-computer suite scripted <out>`: web-form 1 batch / 2 calls, web-iframe 1/2, web-dialog 4/7, web-canvas 1/3, web-ax-form 1/2, web-ax-form-webkit 1/2, each grounding board set 10/30 |
| Model runs, 1 per provider | **Claude 6/6, Codex 6/6** | `tools/computer-suite/run.py <root> claude 1 …` / `codex 1 …`; no shortcuts, no peeks, only computer tools |
| E1 (indicative, one run) | **Claude 1.25 (PASS); Codex 1.75 (MISS)** | Claude: 1.25, 1.5, 0.89, 0.6, 1.25, 1.75; Codex: 2.5, 1.75, 1.22, 1.4, 2.25, 1.75. Codex's miss repeats Phase 4's (pooled 2.25 there) |
| P3 on the page canvas (Claude, Opus medium) | **PASS: 50/50 at 8 px, 50/50 at 16 px** | 0 wrong, 0 misses; mean error 0.85 px, largest 1.41 px; Wilson lower bound 0.929 per size. An empirical pass, as in Phase 4. Every click was a pixel read off the page's screenshot and went through the page |
| F1 during the runs | **0 changes caused by actions** | The focus monitor ran through every run. Its one change per run (≈60–230 ms, given straight back) came 4.5 s before `web-ax-form` started: the suite's own plain launch of Chrome, which a launch without a port can't avoid |

**Open**
- Chrome with no debugging port: background key events (`type`) don't land in its page, though `set_value` does.
- A plain launch of Chrome (not through `launch`) flicks the front for about 0.1–0.2 s. The suite's setup gives it
  straight back; workers launch through `launch`, which never does this.
- `AXEnhancedUserInterface` stays set on a Chromium process once it has been looked at, which costs it some speed.
- **Regressions:** the scripted suite passed 28/28, and `bench --quick --no-foreground` passes every gate. The
  combined verifier runs the full bench once, on the merged tree.
  - The first quick bench caught S3p at 390.3 / 402.4 ms. The browser pop-up handling had reached native pop-ups:
    a second press while AppKit blinks the item, and a look through the window's menus on every poll.
  - Limited to pop-ups inside a page, S3p is back to 368.0 / 373.9 ms; native and page pop-up tasks still pass.

### Phases 4 and 5: combined verification (2026-10-09)

The two Phase 5 streams were merged into `computer-use` and the merged tree was verified once, as a whole. All live
evidence ran from the cmux terminal, so the release helper used **the terminal's inherited grants** (the
helper-bundle gate below is the exception, and its answer is that the grants aren't given yet). No privacy setting
was touched.

**The merge.** Stream B (`cu-quirks`) was rebased onto `computer-use` and fast-forwarded, then stream A
(`cu-browser`) on top: linear history, both branches and worktrees removed. Conflicts were in `engine.rs` (A moved
an observation's rendering into `render_observation`; B's "structure may be incomplete" line now goes through it),
`macos/mod.rs`, the suite's registry (one `suite::all_tasks()` lists native, grounding, quirk, web and web-grounding
tasks), its teardown (B's executable check, A's web server and scratch profile: the teardown no longer returns
before ending them), and this plan.

**One first-contact path** (`macos/quirks.rs`). The two streams each had one, and a browser window waited twice:
once in the observe's settle, then again, blocking and uncancellable, inside every first tree read. Now:
- **once per process instance, on the application element**: Electron gets `AXManualAccessibility`; Chromium
  browsers get it and `AXEnhancedUserInterface`. Setting a flag again would restart the app's build, so it never is;
- **one settle per window**, polled by the engine's own cancellable wait: where the window holds a page (or will:
  Electron and Chromium windows always do), its page elements until the count holds for 250 ms and the page has
  content inside the window, **up to 4.5 s**, past the ≈2 s these apps take to build it; otherwise every element,
  up to 2.5 s; an app launched under 2 s ago, no sooner than 2 s after its launch; an empty view that may be a web
  view waits up to 1 s for its page. A page that never comes is reported as incomplete;
- `web.rs` keeps only the page probes. All six first-contact tasks pass on it (Electron, SwiftUI, Catalyst,
  Chrome's accessibility page, a WKWebView, the native form).

**Two regressions the merged suite caught.** Neither stream had run the whole native suite after its last change.
- *Background scrolls missed their view* (from stream B's display mapping): the scroll's window-local point had
  been shadowed by its global point, so a click on a row scrolled out of view failed (`row-173`, `last-row`).
- *The find bar's search ran twice* (from stream B's set-then-edit for fields): the extra edit on a search field
  searched again, and the found-text highlight windows ended the batch as if it had navigated (`find-replace`).
  Search fields now get the set alone; text fields and combo boxes keep the edit a SwiftUI binding needs.

**Codex's review** (base `5c16a3ad`, six findings, all valid, all fixed):
- page actions now wait for the user's pause, as native ones do;
- the page's password check follows shadow roots and asks every out-of-process frame when the focus rests on one;
  a frame that can't answer counts as a password field;
- every page key is checked, so paste, Delete and Backspace no longer bypass it;
- a launch stopped while the browser starts makes no page (a browser it started is still reported, owned and quit);
- the scroll point (also found by the suite, above);
- a cleared page field equals `""` in a value expectation instead of reading as its label.
Live: in the web fixture, Backspace in the page's password field and paste, type and Delete in a PIN field inside
the cross-origin frame were refused (the PIN logged no input), while typing into that frame's Code field worked;
clearing "Name" with `value_equals ""` was confirmed.

**The safety rule from the live finding.** A trial had run `find /` and `pkill`. The Operate brief now says it in
plain words: use only the computer tools and the files and apps the task names; never kill or signal a process; never
search the whole disk. `observe`'s description, which every worker sees, says the last two. Tests check both.

**Results on the merged tree**

| Gate or check | Result | Evidence |
|---|---|---|
| Full bench, 200 reps, `--no-foreground` | **every gate passes** | 1360 s, load average 1.7–2.9 (one-minute, sampled each minute), nothing else of ours running; the bench ran under `nice -n 10`. `bench-1791560210.json` |
| S1 / S2 | pass | observe 6.5 / 7.0 ms; with a screenshot 46.6 / 50.0 ms (p50 / p95) |
| S3 / S3p | pass | set_value 3.6 / 4.4 ms effect; menu-bar pick 6.4 / 8.3 ms; press 2.8 / 3.7 ms; pop-up item 370.2 / 377.4 ms, 200/200 |
| S4 / S5 / SEL | pass | background pixel click 14.6 / 36.7 ms effect; 100 characters 8.4 ms set, 16.8 ms as keys; select and type over 20/20 |
| P1 / P2 / P2r / P4 | pass | 1600/1600; 1600/1600 inside, worst error 0.00 pt; 200/200 refusals; 0 wrong-target effects |
| SA | pass | 600/600 inactive NSButton, NSTextView and SwiftUI button clicks landed, 0 focus changes |
| F1 | **pass: 0** | the clean rerun stream B asked for, with no other worker driving the Mac |
| Scripted suite, every task | **PASS: 35/35** | `brigadier-computer suite scripted <out>`: 18 native, 4 grounding sets, 5 quirk, 6 web, 2 web-grounding sets. The focus monitor saw one front change, 200 ms during `web-ax-form`'s setup (Chrome launched without a port, as stream A reported), before the task began |
| Model sanity run, Claude (Opus, medium) | **4/4** | `row-173`, `find-replace`, `catalyst-order`, `web-form`; 0 shortcuts; F1 0; E1 median 1.5 |
| Model sanity run, Codex (gpt-6.1-sol, medium) | **4/4** | the same tasks; 0 shortcuts; F1 0; E1 median 2.25 |
| P3 | **PASS** (measured before) | Phase 4: 50/50 at 8, 12, 16 and 24 pt; stream A: 50/50 at 8 and 16 px on a page |
| E1 (≤ 1.3) | **MISS for both** | one small run each: Claude 1.5 (1.0–1.8), Codex 2.25 (1.8–2.5). Phase 4's pooled medians were Claude 1.0 and Codex 2.25 |
| Signed build | **PASS** | `APPLE_SIGNING_IDENTITY=… pnpm tauri:debug-app`; `codesign -dvv`: `Brigadier Dev.app` (`ai.brigadier.dev`), `Brigadier Computer Use.app` (`ai.brigadier.dev.computer-use`) and `brigadierd` all "Developer ID Application: SYNCRA, SRL (7JQSPMWT79)", TeamIdentifier 7JQSPMWT79, hardened runtime; `codesign --verify --deep --strict` passes |
| Helper-bundle gate | **Not verified: no grant yet** | A scratch daemon from the signed bundle (no helper override) launched the bundled helper through LaunchServices (parent pid 1, its own responsible process); `getComputerAccess` answered accessibility **false**, screen recording **false**. The helper exited when the daemon did |

**Not done or open** (Phases 4 and 5 together)
- **The helper-bundle gate.** Every live result used the terminal's grants. Once the user grants Brigadier Computer
  Use: check that System Settings lists it (not Brigadier or a terminal), that the dev daemon's helper reports both
  grants and passes the scripted suite, that the grants survive a helper restart and a rebuild signed by the same
  team, and, as a user check, that revoking one gives `permission_missing` with the fix.
- **E1 misses for both providers** in the small runs. Workers observe once more than the reference, to verify;
  Codex adds its own tool discovery and one-action batches (Phase 4's list of fixes still stands: accept Codex's
  `submit_report` shapes, give their call shape in its brief, accept the argument spellings it uses).
- **The six dev trials** of Phase 4 never ran.
- **The comparisons** with other computer-use tools, once they are turned on and granted (a future build).
- **A save panel on another Space** keeps Save disabled; `save-panel` can't finish while the user is on another
  Space or in a full-screen app. Suggested: report `background_unavailable` with that reason.
- **Several displays**: unit-tested only; this Mac has one display.
- **Chrome without a debugging port**: background key events don't reach its page; `set_value` and presses do.
  Only Chrome for Testing was driven; Arc and Dia never were.
- **A plain launch of Chrome flicks the front** for 0.1–0.2 s (only the suite's setup does this; workers launch
  through `launch`).
- **A menu pick took the front once** while a system "quit unexpectedly" alert was the front app (stream B's quick
  bench). 200/200 menu picks changed nothing in a full bench.
- **`AXEnhancedUserInterface` stays set** on a Chromium process once looked at, which costs it some speed.
- **A search field's value is set without an edit**, so a SwiftUI `.searchable` binding might not hear it.
- **A launch stopped mid-start** was reviewed, not reproduced live.
- **`launch` puts the new window on top** (without keeping the front), and **a background app's window flashes
  once per batch** under synthetic activation (Phase 4).
- **Contention between terminal-granted processes** (Phase 3) bites only in development.
- **Quota steers Operate to Codex** when Claude's window is projected high; Codex takes about twice the calls.

### Phase 7, item 2: efficiency, gate E1 (2026-10-09/10, branch `cu-e1`)

**The gate**, as the user amended it: the median of model calls ÷ (the scripted solver's tool calls + 2), per
provider, at most 1.3, with no drop in task success. Measured on the suite's 29 model tasks (the grounding boards
and the dev-build tasks have no reference or need the dev app), Operate at the router's medium effort, against the
combined verification's scripted run.

**Result: E1 passes for both providers.**

| run | build | passed | E1 median |
|---|---|---|---|
| Claude 1 | `920ebeb4` | 29/29 | 1.0 |
| Claude 2 | `65fbcf2c` | 27/29 | 1.0 |
| Claude 3 | `484b24e3` | 29/29 | 1.0 |
| Codex 2 | `21b896e2` | 29/29 | 0.89 |
| Codex 3 | `d4930498` | 28/29 | 1.0 |
| **Claude, pooled** | | **85/87** | **1.0** (median 4 calls) |
| **Codex, pooled** | | **57/58** | **1.0** (median 4 calls) |

Before this work: Claude ≈1.5 and Codex 2.25 (1.75 on browser tasks). Codex run 1 (`185c42eb`, 21/29, E1 0.8)
doesn't count: all eight failures were one bug, an `appears` expect without `find`, fixed in `21b896e2`.

**Where the extra calls went, and the fixes** (each with a unit test):
- *Re-observing after an act.* The Operate brief now says observe once, do the whole job in one act with expects,
  and the act's reply is the check; a reply opens with a verdict line ("All 4 actions done; 4 expects held: no need
  to observe again"). `act`'s description says the same.
- *Argument errors and retries.* The computer tools take the spellings models use; an argument error names the
  failing action, what its expect needs, shows a well-formed call and says "Nothing ran: send the call again".
  `submit_report` takes its lists as objects or one text. An unknown image id names the ids there are.
- *Every error names the next call*, with an example.
- *Codex's own discovery and one-action batches.* Its brief gives every call shape, `submit_report` included, says
  not to list the tools, to act and report in one exec, that an error is never the end, and to report only when the
  act's text starts with "All ".
- *Looks that found nothing yet.* A full look names the window's file (a worker had run `lsof` for it); a window
  whose page is still being built is waited for a whole bound again on the next look; an incomplete look now offers
  one `wait` with `appears` rather than another look.
- *Page lists.* A click on a closed `<select>`'s option failed ("Node does not have a layout object"); the worker
  then picked by keys and its Return submitted the form twice (Codex 3's one failure). The click now picks the
  option through the page's own keys, as `set_value` does, and the scripted `web-form` solution clicks the option.
  Through accessibility, `type` sent a choice's whole name as one key event, which Chrome ignores in a closed list;
  it now sends one key per character (live: "Team" picked in 55 ms), and `set_value` on a list whose menu won't
  open falls back to that type-ahead, typing the old choice back if it lands elsewhere.
- *A WebKit view stopped answering* ("app_not_responding: AXValue") under a `wait` for `appears`, which walked the
  whole window every few milliseconds; it now walks every 75 ms.

The safety rule (only the computer tools and the files the brief names, never kill or signal a process, never
search the whole disk) and its tests are unchanged.

**Per task**: model calls (E1) per run.

| task | ref batches | claude-1 | claude-2 | claude-3 | codex-2 | codex-3 |
|---|---|---|---|---|---|---|
| append-line | 1 | 6 (1.5) | 6 (1.5) | 6 (1.5) | 4 (1) | 5 (1.25) |
| catalyst-order | 1 | 5 (1.25) | 4 (1) | 5 (1.25) | 3 (0.75) | 3 (0.75) |
| check-8 | 1 | 4 (1) | 4 (1) | 4 (1) | 3 (0.75) | 3 (0.75) |
| electron-signup | 1 | 4 (1) | 4 (1) | 4 (1) | 3 (0.75) | 3 (0.75) |
| find-replace | 2 | 8 (1.6) | 6 (1.2) | 5 (1) | 3 (0.6) | 5 (1) |
| form | 1 | 4 (1) | 4 (1) | 4 (1) | 3 (0.75) | 3 (0.75) |
| last-row | 1 | 5 (1.25) | 6 (1.5) | 6 (1.5) | 5 (1.25) | 5 (1.25) |
| menu | 1 | 3 (1) | 3 (1) | 3 (1) | 4 (1.33) | 4 (1.33) |
| minimised-code | 1 | 4 (1) | 4 (1) | 4 (1) | 5 (1.25) | 5 (1.25) |
| name | 1 | 4 (1) | 4 (1) | 4 (1) | 3 (0.75) | 3 (0.75) |
| password | 2 | 4 (0.8) | 4 (0.8) | 4 (0.8) | 4 (0.8) | 4 (0.8) |
| popup | 1 | 5 (1.25) | 6 (1.5) | 7 (1.75) | 4 (1) | 4 (1) |
| press-3 | 1 | 4 (1) | 4 (1) | 4 (1) | 4 (1) | 4 (1) |
| red-dot | 1 | 5 (1.25) | 5 (1.25) | 5 (1.25) | 8 (2) | 7 (1.75) |
| replace-text | 1 | 7 (1.75) | 5 (1.25) | 5 (1.25) | 3 (0.75) | 4 (1) |
| row-173 | 1 | 4 (1) | 4 (1) | 7 (1.75) | 4 (1) | 5 (1.25) |
| save-panel | 2 | 16 (2.67) | 9 (1.5) | 5 (0.83) | 8 (1.33) | 4 (0.67) |
| sheet | 2 | 5 (0.83) | 5 (0.83) | 5 (0.83) | 7 (1.17) | 7 (1.17) |
| slider | 1 | 4 (1) | 4 (1) | 4 (1) | 3 (0.75) | 3 (0.75) |
| stepper | 1 | 4 (1) | 4 (1) | 4 (1) | 3 (0.75) | 3 (0.75) |
| swiftui-item | 1 | 4 (1) | 4 (1) | 4 (1) | 5 (1.25) | 5 (1.25) |
| tab | 1 | 5 (1.25) | 4 (1) | 4 (1) | 3 (0.75) | 3 (0.75) |
| two-dots | 1 | 4 (1) | 4 (1) | 4 (1) | 8 (2) | 7 (1.75) |
| web-ax-form | 1 | 15 (3.75) | 18 (4.5) **fail** | 4 (1) | 4 (1) | 4 (1) |
| web-ax-form-webkit | 1 | 4 (1) | 12 (3) **fail** | 4 (1) | 3 (0.75) | 4 (1) |
| web-canvas | 1 | 4 (0.8) | 4 (0.8) | 4 (0.8) | 5 (1) | 6 (1.2) |
| web-dialog | 4 | 7 (0.78) | 7 (0.78) | 7 (0.78) | 8 (0.89) | 6 (0.67) |
| web-form | 1 | 6 (1.5) | 4 (1) | 5 (1.25) | 3 (0.75) | 4 (1) **fail** |
| web-iframe | 1 | 5 (1.25) | 5 (1.25) | 5 (1.25) | 3 (0.75) | 3 (0.75) |

**Open**
- **A Chrome window on another Space.** Claude 2 failed `web-ax-form` and `web-ax-form-webkit`, and two repeats on
  that build failed `web-ax-form` again; each time the user was on a full-screen Space, so the window was off screen.
  Chrome built no page tree for 20 s and more, its screenshots were stale, and synthetic activation didn't bring the
  tree (the first repeat); its list opened no menu. With the user on the desktop's Space, both tasks passed in every run.
  The engine now sends list keys one at a time and falls back to type-ahead; the incomplete note tells a worker of
  a window off screen to wait in one call, then ask for the window to be brought into view, and never to click or
  press keys into a page it can't read. Neither path has been exercised live off screen; that needs the user on
  another Space during a run. Making Chromium's windows key once, as Electron's are, was considered and not done:
  activation didn't bring the tree.
- **`save-panel` (Codex 2): a pixel click into a save panel's field under synthetic activation brought the app to
  the front 74 ms later**, with no cursor activity. An old delivery path, unchanged here. Suggested: refuse pointer
  events into a save panel with `background_unavailable`.
- **F1 in these runs**: the suite's own front changes were Chrome's plain launch in `web-ax-form`'s setup (known)
  and the save panel above; the rest overlapped the person's own clicks or window switches. The user was at the Mac throughout.
- `password` passes with the note that the report repeats the password the request itself gave.

### Phase 6: Windows and Linux backends (a future build)

**Not in this build.** The user ruled on 2026-10-09 that computer use ships macOS-only; Windows and Linux are a
future build. The scope below stays as the plan for it.

**Scope**
- §6 backends behind the same trait and tools.
- The suite on each OS's own fixture.

**Done when**
- Windows meets P1 and F1 on its fixture.
- Linux on X11 meets P1.
- Wayland's limits are reported.

## 9. Rulings

From the grill (2026-10-09 02:10, binding):
1. **Purpose**: real, human-like computer use on the user's real desktop, for any task. macOS first, then Windows and
   Linux. No VMs. Background only, no cursor or focus theft, with a per-session agent cursor.
2. **Orchestration**: every worker (Claude and Codex) gets the tool for quick checks. The thread routes long
   GUI-heavy jobs to a GUI-specialist worker on a fast, precise model, fitted into THREAD-PLAN's router and roles.
3. **Engine**: our own native helper, a signed helper app inside Brigadier. Structure first, exact screenshots and
   zoom as fallback, background input, private window-server calls allowed (Developer ID, not the App Store). A CLI
   and an MCP server. Fully ours.
4. **Tonight**: plan, then build as far as possible, unmerged.

Decided by the Delegator for the user (2026-10-09):
- Safety without per-action prompts: block list, visible cursor, action log, global stop. No quality settings.
- Browsers: structure where it applies, pixels last.
- Computer use is a tool the user's own CLIs call. Brigadier runs no model loop.

Decided in this plan:
- **Codex workers use Brigadier's tool; Codex's built-in `computer_use` stays off.** Evidence: in Codex CLI 0.161
  the feature is stable and on by default (`codex features list`). It runs through a plugin bundled with a desktop
  app (`computer-use@…-bundled`, executed through its `cua_repl`/`node_repl` tools, with its own app-access config,
  `default_app_access` and `bundle_ids`), so it needs the plugins feature, which Brigadier turns off for every
  session to keep the user's own setup out (`P/codex/mod.rs:114`). Running it beside ours would put two drivers on
  one desktop, with no shared lease, block list, cursor or log. Phase 4 measures it side by side on macOS, as a
  comparison target only; it is never a production path ("fully ours").
- **The fixture app is Swift**, because it stands in for real AppKit apps; it is test-only and built by the bench.
- **The helper is Rust, in the workspace**, not a separate Swift project: one toolchain, the same fmt, clippy and
  tests, and the Windows and Linux backends share the code.
- **Permission levels limit `act`** (§5), as Codex asked: Full access is unchanged, and the lower levels don't become
  Full access through the GUI.
- **Later** (user, when awake): one-time grants for the helper app (Accessibility, Screen Recording); whether other
  computer-use tools may be granted for the Phase 6 comparison.
- **E1 amended (user, 2026-10-09):** the reference is the scripted solver's tool calls + 2, for one look before
  acting and one report. The gate stays a median ≤ 1.3. The first gate, 1.3× the fewest `act` batches, counted those
  two calls against every worker.

## 10. Codex's reviews

### 10.1 The critique of the research note, point by point

1. **The model loop belongs to the worker's CLI.** Agree. §3 is that applicability section.
2. **Authorization boundary.** Agree, with one difference: no confirmations in Full access (the user's rule). The
   boundary comes from:
   - the authenticated socket;
   - per-call grants that are revoked on stop;
   - per-window leases;
   - permission-level limits on `act`;
   - the block list;
   - secure-field redaction;
   - treating screen content as untrusted (§5).
3. **Codex's native computer use.** Partly agree. It was checked, and it stays off for now with the evidence in §9.
   The side-by-side measurement is Phase 6.
4. **Background input must be the main path.** Agree. §4.4's ladder puts it first, and results distinguish
   confirmed, unverified, no change and needs foreground. On reusing an outside driver: disagree. The user ruled
   "fully ours", and an outside driver brings its own telemetry and its own release cycle. It is a Phase 6 comparison
   target only.
5. **One `observe` contract.** Agree. §4.3 covers target identity, observation id, coordinate metadata, a compact tree
   with refs, an optional sized image, paging, a subtree, a structure-only refresh and zoom. Brigadier's MCP server
   becomes able to send image blocks (today it sends text only).
6. **No fixed Retina rule.** Agree. Every image has its own transform, and coordinates are named against an image
   id (§4.3).
7. **Batches need leases and partial results.** Agree. §4.4 and §5 cover window leases, a desktop lease for
   foreground, the block list re-checked per action, per-action status, no replay of uncertain submissions, and a
   closing diff. The batch starts from a complete tool call, so nothing streams half-formed.
8. **Settle is not readiness.** Agree. Delivered, settled and effect are separate (§4.4). `expect` predicates come
   first, notifications next, and a masked hash last, with a bound.
9. **Caching and pruning are outside our control.** Agree. They're dropped (§3). Costs are stated per image: 1,196
   tokens at 1280×720, 2,691 at 1920×1080, 4,784 at the cap.
10. **VMs and best-of-N are not product defaults.** Agree. The user ruled no VMs. Wayland's limits are stated, not
    worked around (§6).
11. **Evidence before choosing.** Agree on measuring. §1's targets and §7's benchmark do that. Disagree on ranking
    outside drivers first, for the same reason as point 4. The macOS 14 floor matches ScreenCaptureKit's screenshot
    API. Intel Macs build from the same code and are tested when hardware is available.

### 10.2 The review of the outline (2026-10-09), as ruled by the Delegator

All 18 points are accepted and folded in, with one change to point 1:
- 1. Foreground is kept as a strict last resort: only after 60 s of user idle, re-checked before raising, with focus,
  window order and cursor restored, logged as `foreground_used`, and aborted to `background_unavailable` if the user
  becomes active (§4.4).
- 2. Feasibility spike first (§2.1, Phase 1 step 1).
- 3. Private dependencies are listed with their ABI (§12), isolated in one module, and report
  `unsupported_capability` when missing. Validated on macOS 27, untested below 27.
- 4. `launch` and `act` are escalations at the lower levels, bound to instances (§5).
- 5. Per-app serialization of keyboard, menu and focus work, with the recipient verified first (§4.4).
- 6. Generations and pre-action revalidation; navigation invalidates the rest of the batch (§4.3, §4.4).
- 7. Responder checks before every key, text and paste; redaction before delivery and persistence (§4.3, §4.4).
- 8. Cancellation generations, deadlines, in-sequence checks and the input-release guard (§4.7).
- 9. Terminal-launched development evidence vs. the helper-bundle gate (§2, Phase 2).
- 10. Catalog selection, typed replies, provider injection, cancellation and identity (§4.6).
- 11. The `Operate` kind, delegation, routing and access (Phase 4).
- 12. Per-worker diff bases, an explicit base, and paged values (§4.3).
- 13. Per-provider image estimates, bounded zoom, and both CLIs tested (§4.3, Phase 2).
- 14. The zoom contract is our own (§4.3, §11).
- 15. Separate benchmark reporting and timings (§1, §7).
- 16. Mandatory gates with sample counts; macOS comparisons moved to Phase 4 (§1, Phase 4).
- 17. Records inside session events, artifacts, the blob store and the cleanup ledger (§5).
- 18. Other drivers are comparison targets only; the crate joins the cross checks; tests only where they guard
  behaviour (§8).

The Delegator's own correction: Brigadier's windows are not blocked in general. Only the hosting instance and the
installed app are blocked, so workers can drive the dev builds they launch (§5).

### 10.3 The review of Phase 2's code (2026-10-09)

All 7 findings were accepted and fixed, each with a test:
- A batch's leases stayed `running` when its call was dropped mid-await; a guard now releases them however the call
  ends.
- Any worker's end denied every pending computer card; only the user's Stop does now, and a worker's end ends its
  own (its grant is gone).
- A launch cancelled after `open` lost the new process's ownership; the broker runs the launch to its end apart
  from its caller, and the helper reports an app it started even when stopped while waiting for its window.
- The foreground rung restored focus by process only; it now remembers the focused window, refocuses the user's
  window of the same app, and keeps a window the user picked meanwhile.
- Computer cards outlived their call (and a restart) as plain action cards; they are marked live, a dropped call
  expires its card, a restart expires one nothing waits for, and the answer goes to the call instead of a message.
- `select` passed character offsets to accessibility, which counts UTF-16 units; they are converted both ways.
- `launch` checked the block list against the request's words only; it now checks the app LaunchServices would run.

## 11. Checked third-party contracts (2026-10-09)

- **Claude images** (platform.claude.com/docs/en/build-with-claude/vision):
  - cost is `⌈w/28⌉ × ⌈h/28⌉` visual tokens;
  - Claude 4.7 and later (Opus 5.5, Sonnet 5.5, Haiku 5.5) allow 2,576 px on the long edge and 4,784 visual tokens;
  - more than 20 images in one request (tool results included) means each image must be ≤ 2,000 px per side;
  - JPEG, PNG, GIF and WebP are accepted, at ≤ 10 MB each;
  - images placed before text work best.
- **Claude computer toolset** (…/agents-and-tools/tool-use/computer-use-tool):
  - zoom takes `region [x0,y0,x1,y1]`, and coordinates stay in full-screenshot space;
  - batches run in order and stop at the first failure;
  - keyboard is preferred for dropdowns and scrollbars;
  - clients shouldn't prune screenshots on Opus 5.5 and Sonnet 5.5;
  - it lists no accessibility-tree actions.
  We don't use the toolset itself (no API loop). Our batch semantics follow it; our zoom coordinate contract is our
  own (§4.3).
- **Claude Code MCP** (code.claude.com/docs/en/mcp):
  - tool output warns at 10,000 tokens and is capped at 25,000 by default (`MAX_MCP_OUTPUT_TOKENS`), images included;
  - an MCP image "may be scaled down or compressed to fit the model's image size limits", and the original is saved
    under the session's `tool-results` (v2.1.283+);
  - each tool description is cut at 2,048 characters;
  - servers not marked `alwaysLoad` stay behind tool search;
  - a server's `timeout` field (ms) overrides `MCP_TOOL_TIMEOUT`; Brigadier already sets it per server.
  Installed: 2.1.295.
- **Codex CLI** 0.161.0 (`codex features list`, `codex exec --help`):
  - `computer_use` and `browser_use` are stable and on;
  - `--disable <FEATURE>` is the same as `-c features.<name>=false`;
  - the binary's tool-output content items include `input_image` with `detail`, so MCP images reach the model (to be
    shown live in Phase 2).
- **macOS 27 SDK** (Command Line Tools, Swift 6.4, no Xcode, no signing identity on this Mac):
  - `SCScreenshotManager` (macOS 14+) captures one window;
  - `AXUIElementCopyMultipleAttributeValues`, `AXObserver` and `AXUIElementSetMessagingTimeout` are available;
  - `CGEvent.postToPid` exists;
  - `AXIsProcessTrusted` and `CGPreflightScreenCaptureAccess` check the grants;
  - the System Settings deep links are `x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility`
    and `…?Privacy_ScreenCapture`.
- **Launch and window calls used in Phase 2** (macOS 27 SDK headers, checked 2026-10-09):
  - `NSWorkspace` `URLForApplicationWithBundleIdentifier:` and `URLForApplicationToOpenURL:` (macOS 10.6+) give the
    app for a bundle id and for a file or URL; `fullPathForApplication:` (deprecated since macOS 11, still answers)
    gives it for a name, the lookup `open -a` makes; `NSBundle bundleWithURL:` then `bundleIdentifier`;
  - `kAXDocumentAttribute` (`AXDocument`) is the window's document as a URL string, and `kAXCloseButtonAttribute`
    (`AXCloseButton`) its close button (`AXAttributeConstants.h`);
  - `AXSelectedTextRange` is a `CFRange` over the element's `CFString` value, whose indices are UTF-16 units.

## 12. Private dependencies (macOS)

All of these are bound in `CU/macos/private.rs` and nowhere else, resolved with `dlsym` at start-up. A missing symbol
disables the capabilities that need it (`unsupported_capability`) without affecting the rest. Validated on macOS 27
(build 26A428) on Apple Silicon; **untested below 27**.

| Dependency | Kind | ABI / value | Used for |
|---|---|---|---|
| `_AXUIElementGetWindow` | function (HIServices) | `AXError (AXUIElementRef, CGWindowID *)` | pair an accessibility window with its window-server id |
| `_AXUIElementCreateWithRemoteToken` | function (HIServices) | `AXUIElementRef (CFDataRef token)`, +1 retained; token = pid (i32), 0 (i32), `0x636f636f` (i32), element id (u64), 20 bytes | reach a window on another Space that is neither the app's main nor its focused window (Phase 4, pulled forward from Phase 5); only elements an accessibility client already reached have ids |
| `SLSMainConnectionID` | function (SkyLight) | `int32 (void)` | this process's window-server connection, for the capture below |
| `SLSHWCaptureWindowList` | function (SkyLight) | `CFArrayRef (int32 connection, const CGWindowID *ids, int32 count, uint32 options)`, +1 retained array of `CGImage`; options `1<<11 \| 1<<9 \| 1<<8` (ignore the global clip shape, nominal and best resolution) | pixels of a window on another Space, from the window server's backing store (~100 ms); ScreenCaptureKit times out on such a window (measured, Phase 4) |
| mouse event field 51 | `CGEventField` raw value | `int64` window id | AppKit's window for a pid-posted mouse down/up (measured, §2) |
| mouse event field 103 | `CGEventField` raw value | `int64` window id | the same for mouse-moved events (measured, §2) |
| `CGEventSetWindowLocation` | function (SkyLight, exported) | `void (CGEventRef, CGPoint)`, window-local, top-left | the event's location inside the target window (measured, §2) |
| `SLEventPostToPid` | function (SkyLight) | `void (pid_t, CGEventRef)` | posting through the window server's own path where the public post isn't enough (spike decides) |
| `SLEventSetAuthenticationMessage` | function (SkyLight) | `void (CGEventRef, SLSEventAuthenticationMessage *)` | the authentication envelope; bound, not attached by default (§2.1) |
| `SLSEventAuthenticationMessage` | Objective-C class (SkyLight) | `+messageWithEventRecord:(SLSEventRecord *)pid:(int32)version:(uint32)` | builds the envelope for one event |
| `SLEventRecordPointer` | function (SkyLight) | `SLSEventRecord * (CGEventRef)` | the event record the envelope signs |
| `SLPSPostEventRecordTo` | function (SkyLight) | `int32 (const ProcessSerialNumber *, const uint8_t record[0xf8])` | synthetic activation (§2.1) |
| focus record | 0xf8-byte record | `[0x04]=0xf8`, `[0x08]=0x0d`, `[0x3c..0x40]` = window id, `[0x8a]` = 1 focus / 2 defocus | the target app believes it is active, or stops believing it |
| make-key records | 0xf8-byte records | `[0x04]=0xf8`, `[0x08]` = 1 then 2, `[0x20..0x30]=0xff`, `[0x3a]=0x10`, `[0x3c..0x40]` = window id | the target window becomes key inside its app |
| `_SLPSGetFrontProcess` | function (SkyLight) | `OSStatus (ProcessSerialNumber *)` | the window server's front process, for the focus checks (F1) |
| `GetProcessForPID` | function (deprecated, public) | `OSStatus (pid_t, ProcessSerialNumber *)` | the PSN for the record call |
| `_SLPSSetFrontProcessWithOptions` | function (SkyLight) | `OSStatus (const ProcessSerialNumber *, CGWindowID, uint32 mode)`, mode `0x200` | the foreground rung's raise and give-back; `AXFrontmost` reports success but moves nothing (measured). Missing: the rung is `unsupported_capability` |
| `GetProcessPID` | function (deprecated, public) | `OSStatus (const ProcessSerialNumber *, pid_t *)` | the pid of `_SLPSGetFrontProcess`'s answer; `NSWorkspace.frontmostApplication` is stale off a running main run loop |

| `AXManualAccessibility` | accessibility attribute (undocumented), set to `true` on the application element | `CFBoolean` | an Electron app builds its accessibility tree (Phase 5, stream B); set once per process instance |

The spike adds a row for anything else it needs, with the ABI it verified.

