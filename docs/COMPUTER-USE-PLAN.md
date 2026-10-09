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
| E1 | model calls per eval task | ≤ 1.3× the task's reference batch count (the fewest `act` batches a script needs, recorded per task); published agents take 1.4–2.7× |

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

- The helper draws one overlay per display: a borderless, transparent panel above all windows, ignoring mouse events,
  on every Space, and left out of every capture.
- Each session gets its own color (hashed from the session id) and a small label with the worker's name.
- The cursor glides to each target in 120–180 ms and pulses on a click. For an element action it outlines the
  element's frame. It never moves the real cursor.
- It fades out after 5 s without actions and disappears when the session ends.

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
  actions, model calls (E1), tokens per step, wall time, focus theft.

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
  card is pending ends the call.
- **Ownership.** `launch` tells a new process or window from a reused one (LaunchServices may hand back the user's
  running app). Only new instances become the worker's artifacts in the cleanup ledger, recorded before the launch
  returns, and only those are quit when the worker ends. A launch that takes the front gives it back (§2.1).
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
  - `apps` marks cmux "blocked (a terminal the session didn't launch)", and an `observe` of a cmux window is
    refused with `blocked` and no image.
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
- **Not done or open:**
  - The helper-bundle gate needs the user's one-time grants.
  - A new-process launch counts every window the app restores (TextEdit reopening earlier documents) among its new
    windows. `open -F` avoids that but erases the app's saved state, so it isn't used.
  - The ledger quits an owned app with its windows open, so the app may restore them at the user's next launch.
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

### Phase 6: Windows and Linux backends

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

## 12. Private dependencies (macOS)

All of these are bound in `CU/macos/private.rs` and nowhere else, resolved with `dlsym` at start-up. A missing symbol
disables the capabilities that need it (`unsupported_capability`) without affecting the rest. Validated on macOS 27
(build 26A428) on Apple Silicon; **untested below 27**.

| Dependency | Kind | ABI / value | Used for |
|---|---|---|---|
| `_AXUIElementGetWindow` | function (HIServices) | `AXError (AXUIElementRef, CGWindowID *)` | pair an accessibility window with its window-server id |
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

The spike adds a row for anything else it needs, with the ABI it verified.

