# Brigadier computer use: measured audit (2026-10-26)

What our computer use costs in context, per call, in time and in failures, measured
against the live helper, so the plan can start from numbers. No code was changed.

## Setup and method

- Machine: Apple M4 Pro, 24 GB, macOS 27.0.1, display 1728x1117 pt at 2x.
- Helper: the installed `Brigadier Computer Use.app` (`brigadier-cu`, driver 0.36.0, build
  `0.1.0-9f97aa35388fb70b`), its long-running daemon (up 4 h 22 m, 3 m 20 s CPU, 255 MB RSS).
- A small MCP stdio client (Python, kept outside the repo) started `brigadier-cu mcp` with
  the same environment Brigadier gives an agent, sent `initialize`, `tools/list` and
  `tools/call`, and logged for every response: wall-clock time, wire bytes, text-content
  bytes, `structuredContent` bytes, and each image's format, pixel size and raw bytes.
- **Tokens are estimated as bytes / 4** for text and JSON. Image tokens use Anthropic's
  `width x height / 750` rule; none of the screenshots here was over the 1568 px resize cap.
- What the model sees: in this worker's own Claude Code session, a `get_window_state` result
  showed the model the **image plus the `structuredContent` JSON**, not the text block (see
  the first table in section 2). So "model tokens" below count `structuredContent` when a tool
  returns one, otherwise the text.
- Apps: TextEdit (my own scratch file), Finder (my own scratch folder of 13 files), Safari
  (`https://example.com`; Safari wasn't running and isn't the user's browser), System
  Settings › About. Everything ran in the background under `/tmp/brigadier-focus.lock`. The
  front app was checked before and after every launch: it never changed. All scratch windows,
  apps and files were closed or removed afterwards.
- Latency: warm daemon, median of 3 to 5 calls unless noted. Tool time only. Model think time
  comes from real transcripts (section 3).
- Real usage: the 3 Claude transcripts on this Mac that actually called `mcp__computer__*`
  tools (102 calls; this worker excluded). That's a small sample, so treat its percentages as
  hints, not rates.

## 1. Context cost

### What gets loaded

| Item | Bytes | ~Tokens | When it is in context |
| --- | ---: | ---: | --- |
| Full tool list (40 tools: names, descriptions, input schemas) | 96,046 | **24,011** | Up front on every turn for a worker started with `--with computer-use` (`always_load`, `crates/core/src/manager/workers.rs:734`, `crates/providers/src/claude/mod.rs:673`). Otherwise deferred. |
| Deferred form (names only, `mcp__computer__*`) | 1,157 | 289 | Every session, chat and worker while computer use is on (`crates/core/src/manager/conversation.rs:2782`, `workers.rs:734`) |
| Server `instructions` | 134 | 34 | Always (kept to one line on purpose, `computer_use.rs:118`) |
| Skill `computer-use` body | 13,415 | 3,354 | When the agent loads the skill (the server instructions ask it to before the first call) |
| Skill description (listing) | ~200 | ~50 | Always |

So a GUI worker pays **~27.4k tokens** (tools plus skill) before its first look. A deferred
agent pays ~0.3k up front, then each `ToolSearch` that loads a schema pays that tool's cost
(e.g. `get_window_state` alone is ~2k tokens).

### Per tool (compact JSON, bytes / 4)

| Tool | Bytes | ~Tok | Desc | Schema | Used by real agents (102 calls) |
| --- | ---: | ---: | ---: | ---: | --- |
| get_window_state | 7,850 | 1,962 | 2,529 | 4,843 | 46% |
| run_steps | 7,171 | 1,792 | 4,132 | 2,681 | 15% |
| start_session | 5,709 | 1,427 | 605 | 4,782 | 0 (skill says never call it) |
| start_recording | 5,260 | 1,315 | 3,409 | 1,524 | 0 |
| page | 4,156 | 1,039 | 1,596 | 2,272 | 5% |
| set_agent_cursor_motion | 4,149 | 1,037 | 89 | 3,752 | 0 (Brigadier's agents show no cursor) |
| click | 4,058 | 1,014 | 1,217 | 2,496 | 7% |
| set_config | 3,970 | 992 | 901 | 2,780 | 0 |
| launch_app | 3,663 | 915 | 1,856 | 1,496 | 5% |
| hotkey | 3,596 | 899 | 1,337 | 1,945 | 1% |
| health_report | 3,239 | 809 | 2,307 | 542 | 0 |
| verify_state | 3,026 | 756 | 435 | 2,236 | 0 |
| scroll | 2,971 | 742 | 642 | 1,985 | 0 |
| type_text | 2,908 | 727 | 782 | 1,773 | 2% |
| check_permissions | 2,421 | 605 | 1,312 | 728 | 0 (skill says don't) |
| press_key | 2,414 | 603 | 397 | 1,698 | 1% |
| right_click | 2,312 | 578 | 687 | 1,267 | 0 |
| list_apps | 2,094 | 523 | 1,509 | 280 | 0 |
| set_value | 2,084 | 521 | 892 | 878 | 3% |
| replay_trajectory | 2,053 | 513 | 1,018 | 743 | 0 |
| zoom | 1,901 | 475 | 696 | 881 | 0 |
| move_cursor | 1,841 | 460 | 305 | 1,230 | 0 |
| double_click | 1,795 | 448 | 379 | 1,040 | 0 |
| invoke_menu | 1,685 | 421 | 241 | 1,128 | 3% |
| list_windows | 1,599 | 399 | 641 | 680 | 13% |
| stop_recording | 1,214 | 303 | 647 | 280 | 0 |
| get_desktop_state | 1,140 | 285 | 271 | 564 | 0 |
| get_accessibility_tree | 975 | 243 | 365 | 280 | 0 |
| kill_app | 943 | 235 | 298 | 373 | 0 |
| set_agent_cursor_theme | 933 | 233 | 55 | 572 | 0 |
| get_recording_state | 884 | 221 | 307 | 280 | 0 |
| list_sessions | 788 | 197 | 132 | 360 | 0 |
| get_screen_size | 749 | 187 | 176 | 280 | 0 |
| end_session | 699 | 174 | 199 | 207 | 0 |
| get_session | 666 | 166 | 168 | 204 | 0 |
| set_agent_cursor_enabled | 659 | 164 | 49 | 300 | 0 |
| get_cursor_position | 658 | 164 | 76 | 280 | 0 |
| get_session_state | 626 | 156 | 118 | 204 | 0 |
| get_config | 618 | 154 | 49 | 280 | 0 |
| get_agent_cursor_state | 569 | 142 | 87 | 181 | 0 |

### Rarely or never needed by Brigadier's agents

| Group | Tools | Bytes | ~Tok |
| --- | --- | ---: | ---: |
| Cursor | set_agent_cursor_motion/theme/enabled, get_agent_cursor_state, move_cursor, get_cursor_position | 8,809 | 2,202 |
| Recording | start/stop_recording, get_recording_state, replay_trajectory | 9,411 | 2,352 |
| Sessions | start/end_session, get_session, get_session_state, list_sessions | 8,488 | 2,122 |
| Config and diagnostics | get/set_config, health_report, check_permissions, get_screen_size | 10,997 | 2,749 |
| **Total** | 20 of 40 tools | **37,705** | **9,426 (39%)** |

On top of that, the `session` parameter is described in 39 of the 40 schemas: 7,488 bytes
(~1.9k tokens), while the skill tells agents to leave it out (`computer_use_skill.md:12`).
`target`, `scope` and `element_token` blocks are repeated too (2.8k, 0.8k and 2.9k bytes).
Real agents used 11 of the 40 tools; the 6 most used (get_window_state, run_steps,
list_windows, click, launch_app, page) are 93% of calls and 28.5 KB (~7.1k tokens) of schema.

## 2. Per-call response cost

### Reads by app (one window, default `get_window_state` = tree + screenshot)

| App / window | Elements | Text B | Struct B (model sees) | Screenshot | Image tok | **~Model tok per look** | Wire B |
| --- | ---: | ---: | ---: | --- | ---: | ---: | ---: |
| TextEdit, scratch doc (656x422 pt) | 14 | 1,300 | 1,771 | PNG 1312x844, 47 KB | 1,476 | **1,919** | 66,456 |
| Finder, scratch folder | 97 | 8,884 | 9,671 | PNG 1365x647, 376 KB | 1,177 | **3,595** | 520,650 |
| Safari, example.com | 32 | 4,100 | 5,944* | PNG 1366x970, 284 KB | 1,766 | **3,252** | 388,115 |
| System Settings › About | 99 | 8,230 | 8,886 | PNG 1365x1109, 484 KB | 2,018 | **4,239** | 662,239 |

\* Safari's struct figure is inflated by my counter's `\uXXXX` escaping of the page's
Arabic, Chinese and Russian text; the text figure is the better one there.

- Screenshots are **PNG at the display's 2x scale**, with the long edge capped at about
  1366 px. The image is the larger part of every look except in big trees.
- AX tree size: 14 to 99 rows at the default 250-node walk; Finder's full walk is 235 nodes
  once menus are expanded. The deepest rendered nesting was 5 levels (System Settings, read
  with `max_elements: 2000`: 130 lines, nothing truncated).
- Finder's window tree includes the **user's desktop icons** (`AXImage "Screenshot 2026-10-26
  at 13.37.22.png"`) and icon descriptions (`AXImage (App Store letter A icon)`): tokens spent
  on nothing the agent asked for, and private file names in the answer.
- `structuredContent` repeats the whole tree as an escaped string plus metadata
  (`capture_id`, `timeout_ms`, `elements_complete`, `screenshot_frame_valid`, ...): +36% over
  the text block for TextEdit (1,771 vs 1,300 B), +9% for Finder.

### Read variants (TextEdit, same window)

| Variant | Median ms | Struct B | Image tok | ~Model tok |
| --- | ---: | ---: | ---: | ---: |
| default (tree + screenshot) | 66.9 | 1,771 | 1,476 | 1,919 |
| `include_screenshot: false` | 50.8 | 1,496 | 0 | 374 |
| `include_accessibility_tree: false` | 44.3 | 536 | 1,476 | 1,610 |
| `since: "latest"`, nothing changed | 62.8 | 957 | **1,476** | 1,715 |
| `since: "latest"` + no screenshot, nothing changed | 48.3 | 682 | 0 | 171 |
| `tree_format: "elements"`, no screenshot | 58.5 | 3,108 | 0 | 777 (2x markdown) |
| `query: "File"`, no screenshot | 274.6 | 1,370 | 0 | 343 (but walks 156 nodes, expands menus) |
| Finder with `max_image_dimension: 800` | 168 | 9,711 | **404** (vs 1,177) | 2,832 |
| Finder with `screenshot_out_file` | 150 | 9,818 | 0 | 2,455 |

A `since` diff that found **no change still sends the full screenshot** (1,476 image tokens
for a "no change" answer).

### Other calls

| Call | Median ms | Model B | ~Tok | Notes |
| --- | ---: | ---: | ---: | --- |
| list_windows (pid) | 5.4 | 287 | 72 | |
| get_accessibility_tree | 8.3 | 477 | 119 | apps and visible windows only, no tree |
| get_screen_size | 4.8 | 47 | 12 | |
| list_apps | 18.3 | **16,827** | **4,207** | text block is 396 B; the struct lists all 84 installed apps |
| health_report | 5.1 | 2,131 | 533 | always says "degraded" (see section 4) |
| zoom (400x200 region) | 40.8 | 67 + JPEG 480x240, 11 KB | 153 img | the only JPEG path |
| get_desktop_state | 198.5 | 372 + PNG 1366x883, 333 KB | 1,608 img | |
| launch_app | 108 to 1,653 | 395 to 1,434 | 100 to 360 | returns pid and windows, no tree |
| type_text (AX) | 135.3 | 203 | 51 | `effect: confirmed` by value read-back |
| press_key | 190.4 | 130 | 33 | unverifiable |
| click (AX press, button) | 190.6 | 142 | 36 | unverifiable |
| scroll | 315.4 | 217 to 241 | 57 | unverifiable ("confirm via screenshot") |
| hotkey | 46 to 974 | 146 to 166 | 40 | see latency |
| verify_state (`timeout_ms: 0`, 1 sample) | 283.7 | 463 | 116 | |
| run_steps, 1 step + `observe: {}` | 180 to 205 (token), 536 to 878 (by name) | 1,109 to 1,225 | 290 | includes the diff |
| page get_text (Safari, AX fallback) | 309.6 | 1,460 text | 365 | no structuredContent |
| page query_dom `a` + `attributes: [href]` | 329.5 | 27 text | 7 | AX fallback: returned `[227] AXLink "Learn more"`, no href |

**Actions return no state.** Single actions answer with a one-line summary plus delivery
metadata (130 to 240 B). Only `type_text` reads back and says `confirmed`; click, press_key,
hotkey and scroll say `unverifiable`, so the skill's loop needs another read to check. Only
`run_steps` with `observe` brings the state back in the same call.

## 3. Latency

### Per call (warm daemon)

- Pure transport: the shim to daemon round trip is ~5 ms (list_windows 5.4, get_screen_size
  4.8, health_report 4.5 to 5.4 ms). `tools/list` answers in under 1 ms.
- Reads: tree-only 40 to 70 ms on a small window; 150 to 290 ms with a screenshot on bigger
  windows (Finder 148 to 345, Safari 139 to 226, System Settings 229 to 440; the first read of
  a window is the slowest). `query` costs 270 to 640 ms because it expands the menus.
- Actions have a floor of roughly 120 to 200 ms each: type_text 114 to 160, press_key 178 to
  197, click 190 to 226, scroll 247 to 325, verify_state 263 to 313 even with `timeout_ms: 0`.
- `hotkey` has two speeds: 46 to 273 ms when it presses the matching menu item ("Pressed
  Close Window (cmd+w) in the menu bar"), **812 to 974 ms** when it falls back to posting the
  keystroke ("Pressed cmd+w on pid ..."; Finder and Safari ⌘W, TextEdit ⌘S once).
- `run_steps`: 180 to 205 ms for one token-targeted step plus observe; **536 to 878 ms** when
  the step names its target by role and name.
- `launch_app`: 108 to 142 ms for an app already running; TextEdit with a file 190 to 605 ms;
  Safari with a URL 574 to 970 ms; **Finder with a folder 1,640 to 1,653 ms**.
- `invoke_menu` took 683 to 1,578 ms to *refuse* Edit › Select All (disabled in background).

### Cold start

A daemon started from nothing in a scratch home (3 runs):

| Step | ms |
| --- | ---: |
| shim start + `initialize` | 42 to 71 |
| `tools/list` (served by the shim) | +1 |
| first call, including `open -n -g` of the helper app and the health wait | 181 to 199 |
| first `get_window_state` with screenshot | 133 to 168 |
| **total to first screenshot** | **375 to 434** |
| second `get_window_state` | 68 to 81 |
| `brigadier-cu stop` | 94 to 109 |

Cold start is not the problem. One trap: without `<home>/capture-confirmed`, calls in a
fresh home **wait for Brigadier's setup window**; my first run sat there for over 3 minutes
(section 4).

### Model round trips dominate

In the real transcripts the gap between a tool result and the agent's next tool call was
**median 2.8 s, p75 4.7 s** (n = 100). One extra round trip costs 10 to 500 times more wall
time than the call itself. The three common tasks, done the way the skill says:

| Task | Calls | Tool ms | Model tok (text + image) | Est. wall with model turns |
| --- | ---: | ---: | ---: | ---: |
| A. Type into a TextEdit doc and save (launch_app, get_window_state, run_steps[type_text, ⌘S] + observe) | 3 | 1,229 | 1,002 + 1,480 = **2,482** | ~9.6 s |
| A, lean (read without screenshot) | 3 | 639 | 933 + 0 = **933** | ~9.0 s |
| B. Find a file in a Finder folder (launch_app with folder, get_window_state) | 2 | 1,845 | 2,650 + 1,177 = **3,827** | ~7.4 s |
| B, lean (`query: "target-report"`, no screenshot) | 2 | 1,882 | **434** | ~7.5 s |
| C. Open a URL and read a value (launch_app with URL, get_window_state) | 2 | 913 | 1,790 + 1,766 = **3,556** | ~6.5 s |
| C, lean (`page get_text`) | 2 | 984 | **723** | ~6.6 s |

"Est. wall" = tool ms + (calls x 2.8 s). Closing what was opened adds one more call each
(⌘W: 46 to 974 ms; in Finder it did nothing, see section 4). All 6 runs finished the task
(the file saved, the file was found, the text was read). Lean variants cost **2.7x (A), 8.8x
(B) and 4.9x (C) fewer tokens** at the same speed, but they aren't what the skill tells agents
to do by default (`computer_use_skill.md:15`, step 2 reads with the screenshot).

## 4. Reliability

### Failure modes hit in this audit

| # | What happened | Count | Where |
| --- | --- | --- | --- |
| F1 | `click` on a text area by token failed: `AXUIElementPerformAction(AXPress) returned -25206` | 5/5 | TextEdit AXTextArea |
| F2 | `element_token is stale`: any new read of a window invalidates every older token, including reads made only to check | 1 (after my own interleaved read) | run_steps |
| F3 | `invoke_menu` Edit › Select All refused: "disabled while the app is in the background", after 0.7 to 1.6 s | 5/5 | TextEdit |
| F4 | Finder ⌘W answered "Pressed cmd+w on pid 671" but the window stayed open | 2/2 | Finder (keystroke route) |
| F5 | ⌘S answered before the file was written: the file was unchanged right after the call and saved within ~1 s | 1/1 | TextEdit (keystroke route) |
| F6 | `page execute_javascript` fails: Safari's "Allow JavaScript from Apple Events" is off by default; `get_text` and `query_dom` silently fall back to the AX tree, and `query_dom` drops the requested `attributes` (no `href`) | 3/3 | Safari |
| F7 | System Settings can be opened but not closed: ⌘Q is `target_not_allowed`, so an agent leaves the window it opened | 1/1 | System Settings |
| F8 | `type_text` with a token inserts at the caret, which sits at the start of a freshly opened document, so text lands before the existing text; a `press_key end` in between didn't move the caret | 14/14 | TextEdit |
| F9 | The first Safari read was thin (22 rows, no page text) while the page loaded; the next had 32 | 1/1 | Safari |
| F10 | `query` output nests rows under wrong ancestors (`AXMenuBarItem "Help"` as parent of the window's sidebar) and printed a stray `selector:(null)]]` line | 2 reads | Finder |
| F11 | `health_report` is always "degraded": `bundle_identity: Bundle is ai.brigadier.computer-use, not com.trycua.driver` | 4/4 | all |
| F12 | A fresh helper home without `capture-confirmed` makes calls wait for setup with no answer; my probe hung > 3 min (tool timeout is 120 s, `computer_use.rs:41`) | 1 | scratch home |
| F13 | Window lists carry noise: TextEdit's launch listed a 1000x1000 untitled window with 0 elements; Finder listed 5 untitled desktop windows | always | launch_app |

From the real transcripts (102 calls; run_steps failed 2 of 15, click 2 of 7, launch_app 1 of 5, page 1 of 5, type_text 2 of 2), the error messages were: an unknown bundle id in `launch_app`;
`page` without `window_id`; `click` with action `"raise"` (taken from the element's own
`actions=[raise]` hint, which `click` doesn't accept); `element_outside_target_window`;
`run_steps` observe needing `pid` and `window_id`; `type_text incomplete: delivered 0 of 245
character(s) via CGEvent`; `focus pixel-click at (830,754) failed`.

### GitHub issues

`gh issue list --search 'computer use' --state all` plus `computer-use`, `brigadier-cu`,
`helper screenshot`, `get_window_state`, `Accessibility permission`: 27 issues, 19 about
computer use, **2 open**.

| Cause | Issues |
| --- | --- |
| Setup and permission status wrong or stuck | #154, #568, #317 (closed; F11 shows the bundle check still fails), #529 (the kit reached no session) |
| Staying out of the user's way (focus, cursor, menu bar, windows) | #5, #156, #501, #228 (a new browser window per page) |
| Build, sandbox, signing and CI | #8, #339, #348, #510, #22, **#594 (open: load-sensitive test deadline)** |
| What agents are told (instructions, skill) | #23 (server instructions pulled agents into doing work themselves), **#611 (open: hand-off notes copy whole skill files)** |
| Capability gaps | #6 (pixel double-click in Finder), #334 (browser tools on Windows) |
| Upstream syncs | #7, #270 |

The issues are mostly about setup, packaging and staying in the background. None is about
speed or token cost, so those have never been measured or tracked until now.

## 5. Architecture: where the time and tokens go

Paths: `hc/` = `helpers/computer-use/`, `pm/` = `hc/upstream/crates/platform-macos/src`,
`core/` = `hc/upstream/crates/cua-driver-core/src`, `cl/` = `hc/crates/cu-layer/src`.
A sub-agent traced the code paths. I re-read the citations marked ✓; the rest are its reading
and haven't been checked line by line.

### The path of one call

agent → `brigadier-cu mcp` shim (stdio) → Unix socket → daemon → cu-layer guard → driver
tool → cu-layer text rewrite → back. Transport and the guard together cost about 5 ms
(measured: list_windows 5.4 ms). Every other millisecond goes to the tool's own work and the
fixed waits inside it.

### Where action time goes (fixed waits, per call)

- Every AX action shares a path. It reads the window list several times (background gate,
  change-detector snapshot, key-focus guard), then waits a fixed **50 ms post-action settle**
  (`pm/focus_guard.rs:124-129` ✓) and a 25 ms detect step (`pm/window_change_detector.rs:195-200`
  ✓, report window set to 0 by `cl/lib.rs:60-64` ✓).
- press_key adds 30 ms after pre-focus and 60 ms before the read-back (`pm/tools/press_key.rs:413-420,
  111-115` ✓), plus 8 ms between key events. That is roughly 175 ms of sleeps in a 190 ms call.
- click: native background windows get 80 ms to make the window key (`pm/tools/click.rs:1486-1494`).
  Collection items get a 200 ms settle plus a 250 ms stability check, and text fields wait
  800 ms for WebKit (`click.rs:130-133, 892-896`).
- The slow hotkey path (812 to 974 ms) only runs when cu-layer finds no enabled menu item:
  - The driver walks the whole menu bar again.
  - It makes the window key with 120 ms settles (`pm/input/skylight.rs:240` ✓).
  - It waits up to 600 ms for the item to enable (`pm/tools/invoke_menu.rs:582` ✓).
  - Only then does it post key events.
  This also explains `invoke_menu`'s 0.7 to 1.6 s refusals.
- verify_state with `timeout_ms: 0` still costs 280 ms. Its observation read walks up to 2000
  nodes with every menu expanded (`core/window_state_view.rs:219-235`,
  `pm/tools/get_window_state.rs:274-276`).
- run_steps by name: each lookup runs list_windows, a baseline read, and a
  `full_output: true` read of up to 2000 nodes (`core/batch_tools/locate.rs:712-722` ✓). It
  searches up to 4 windows and polls every 250 ms when nothing matches (`locate.rs:53-56` ✓).
  That is the 350 to 700 ms over a token step.
- launch_app with URLs into an app that is already running waits up to **1.5 s for a new
  window** (`cl/launch/mod.rs:350-358` ✓, `cl/browser/open_link.rs:19, 144-154` ✓). Finder
  reused my already-open `finder-dir` window, no new window appeared, and each call paid the
  whole 1.5 s (measured 1,640 to 1,653 ms; 400 ms the first time, when Finder made a new
  window). Safari adds an `lsof` subprocess and AppleScript (`open_link.rs:254, 281`).
- After the answer, the daemon writes an activity entry with an after-frame capture for each
  action (`hc/crates/brigadier-cu/src/serve.rs:2340-2351` ✓). It runs off the reply path, but two
  window-list pollers stay active afterwards: every 20 ms for 2.5 s
  (`pm/key_focus_guard.rs:253-330`) and every 25 ms for 1 s (`pm/window_change_detector.rs:197-200`
  ✓). Back-to-back calls compete with them.
- cu-layer's own checks on every call are cheap. never_front, catalog and token lookups are
  in-memory; target_policy does one app lookup and, when a `window_id` is given, one full
  window list of about 1 to 5 ms (`cl/guard.rs:43-66`, `cl/lookup.rs:24-63`). `keep_drawn`
  adds an occlusion sample to each get_window_state (`cl/keep_drawn.rs:207-223`). cu-layer is
  not where the time goes.

### Where tokens go

- **Screenshot by default, PNG, at 2x up to 1366 px.** `DEFAULT_MAX_IMAGE_DIMENSION = 1366`
  (`pm/tools/mod.rs:699` ✓). The image is only resized when it is above that cap, PNG is
  hard-coded (`pm/tools/get_window_state.rs:760`), and Brigadier sets no cap of its own
  (`cl/lib.rs:53-64`). JPEG exists only in `zoom`; `png_bytes_to_jpeg` is in
  `pm/capture.rs:1069` but get_window_state doesn't use it. A 1x capture of the same TextEdit
  window would be 656x422, about 369 image tokens instead of 1,476.
- **The tree twice.** In markdown mode the text block and structuredContent both carry
  `tree_markdown` (`core/window_state_view.rs:378-384, 553-557` ✓). Claude shows the model the
  structured copy, with escaped newlines and quotes and about 10 metadata fields.
  `background_input` is computed and then usually dropped (`window_state_view.rs:539-541`).
- **No audience filter on tools.** `cl/catalog.rs:7-48` ✓ offers all 40 tools, including 6
  cursor, 4 recording, 5 session and 4 config/diagnostic ones. `BRIGADIER_CU_AUDIENCE` only
  selects the one-line instructions (`hc/crates/brigadier-cu/src/mcp.rs:15-40`). The proxy
  already rewrites `tools/list` to drop `outputSchema` (`hc/crates/brigadier-cu/src/proxy.rs:49-60`
  ✓), so a smaller list could be served from the same place.
- **Tokens die on every read.** A new snapshot of a window removes the old one
  (`core/snapshot_store.rs:384-430`). Every check-read therefore forces later actions to use
  the new ids, and the skill has to spend lines warning about it (`computer_use_skill.md:23, 51`).

### What the skill makes agents do that costs round trips or tokens

- `computer_use_skill.md:14-19`: the loop is launch_app, then get_window_state, then act, then
  read again. That is at least 3 model turns for any task, even though launch_app could return
  the first read and every action could return its diff.
- `:15` and `:37`: the default read includes the screenshot, and leaving it out is an opt-out
  tip further down. That costs 1.2k to 2.0k image tokens per read (section 2).
- `:17` and `:105`: "Never call an action done unless the follow-up read shows it." That is
  right for safety, but click, press_key, hotkey and scroll return `unverifiable`, so every one
  needs a read.
- `:80`: "The first read of a Chromium or Electron app can be thin… Read once more." That is a
  built-in extra round trip. Measured on Safari: 22 rows, then 32.
- `:103-104`: on failure, do a full read (screenshot and full tree), then retry. On Finder or
  System Settings that is about 3.6k to 4.2k tokens.
- `:12`: "leave out `session`", which is in 39 schemas (~1.9k tokens).
- `:36`: "The `actions` listed on an element are only hints." A real agent sent
  `click action "raise"`, taken from a window row's `actions=[raise]`.
- The skill itself is 13.4 KB (~3.4k tokens), loaded before the first call.

### Host glue (`crates/core/src/manager/`)

- `computer_use.rs:78-138` builds the server entry. Workers started with `--with computer-use`
  get `always_load` (`workers.rs:734`), which makes the 24k-token list part of every turn. The
  tool timeout is 120 s (`computer_use.rs:41`).
- `computer_host.rs:29, 547-590`: a watchdog runs `brigadier-cu host status` every 2 s (measured
  20 to 40 ms per run). It is cheap and not on the call path.

## 6. Top 10 hotspots, ranked by expected win

Ranked by expected win for the user's goal (L1: cheapest, fastest, most reliable, "not really
context filling"). Each item names the measurement behind it.

1. **Screenshots by default, as 2x PNG.** Every default read carries 1,177 to 2,018 image
   tokens: 77% of a TextEdit look, 33 to 54% of the others. A "no change" `since` read still
   carries the image (1,476 tokens for nothing). The lean variants of the three tasks cost
   2.7x, 8.8x and 4.9x fewer tokens at the same speed. A 800 px cap alone cut Finder's image
   from 1,177 to 404 tokens, and a 1x capture would be about 4x smaller.
   *Win: about 50 to 85% of per-task tokens.* (sections 2 and 3; `pm/tools/mod.rs:699`,
   `computer_use_skill.md:15`)
2. **The always-loaded tool list is 24k tokens.** 39% of it (9.4k) is 20 tools that Brigadier's
   agents never call (cursor, recording, session, config). Another 1.9k is the `session`
   parameter the skill says to omit. Real agents used 11 tools, and the top 6 are 7.1k tokens
   of schema. Filter by audience in the proxy (`proxy.rs:49-60`) and tighten the descriptions.
   *Win: about 24k down to under 8k up front per GUI worker, plus a smaller ToolSearch cost
   for everyone else.* (section 1)
3. **Model round trips dominate wall time.** Median 2.8 s per turn against 5 to 300 ms per
   call, and the skill's loop is at least 3 turns. If launch_app returned the first tree read,
   and each action returned its diff (what `run_steps`+`observe` already does), each task would
   lose a turn. *Win: about 2.8 s per task, roughly 30 to 40% of task wall time.* (section 3,
   transcripts)
4. **Fixed waits in actions and launches.** press_key spends about 175 ms of its 190 ms in
   sleeps. hotkey's fallback path takes 812 to 974 ms. verify_state takes 280 ms with
   timeout 0. A by-name run_steps adds 350 to 700 ms. launch_app into a running app waits a
   full 1.5 s when the app reuses a window (Finder: 1,650 ms). *Win: about 0.2 to 1.5 s per
   affected call; matters most in long batches.* (section 3; `focus_guard.rs:126`,
   `skylight.rs:240`, `invoke_menu.rs:582`, `locate.rs:712`, `open_link.rs:19`)
5. **The tree is sent twice and padded.** structuredContent, which is what Claude shows, repeats
   `tree_markdown` escaped and adds metadata: +36% on TextEdit. The `elements` format is 2x
   markdown. *Win: 10 to 35% of every read's text tokens.* (`window_state_view.rs:378-384`)
6. **Noisy trees.** Finder's window read is 8.9 KB (~2.2k tokens) for 97 rows, including the
   user's desktop icons and icon descriptions. System Settings' About pane is 8.2 KB.
   `list_apps` returns 16.8 KB (~4.2k tokens) of structured data for an answer whose text is
   396 B. *Win: about 30 to 50% on big trees, 4k tokens per list_apps.* (section 2)
7. **Tokens go stale on every read.** Any read of a window kills older tokens. Real agents'
   run_steps and click failed 2/15 and 2/7, and each failure costs a re-read (3.6k to 4.2k
   tokens with the skill's full read) plus a turn. *Win: fewer failed steps and recovery
   reads.* (F2, `snapshot_store.rs:384-430`)
8. **Background keys and menus fail without saying so.** Finder ⌘W reported "Pressed" and did
   nothing (2/2). ⌘S returned before the file was on disk. invoke_menu takes 0.7 to 1.6 s to
   refuse a background-disabled item. A click on a text area fails with -25206 (5/5). *Win:
   reliability. Each silent failure costs a check read and a retry turn, or a wrong "done".*
   (F1, F3, F4, F5)
9. **The browser path depends on a setting that is off by default.** Safari refuses
   `execute_javascript` until "Allow JavaScript from Apple Events" is on, and turning it on
   restarts the browser after asking the user. `query_dom` falls back to the AX tree and
   silently drops `attributes`. The first read of a loading page is thin. *Win: reliability on
   web tasks, which are among the most common.* (F6, F9)
10. **Diagnostics and setup traps.** health_report always says "degraded" (bundle id check,
    F11). A home without `capture-confirmed` makes calls hang until the 120 s tool timeout (F12;
    over 3 min in my probe). `query` reads show wrong ancestors (F10). System Settings can't be
    closed by the agent that opened it (F7). *Win: fewer stuck or misled runs; small token
    effect.*

Not hotspots: cold start (375 to 434 ms to the first screenshot), the shim/socket hop (~5 ms),
cu-layer's per-call guard (in-memory checks plus about one window list), and the watchdog
(20 to 40 ms every 2 s).

## After phase 1 (re-measured 2026-10-26, branch at `4caf2350b`)

The user's check (L10): phase 1 must not lose anything on reliability or quality. Same apps,
windows and tasks as above, measured before and after in the same sitting.

**Method.** The helper was built from `4caf2350b`. A freshly built binary has no Accessibility
or Screen Recording grant (its `check_permissions` said so). I didn't raise a grant prompt on
the user's screen. The installed, granted helper only talks to peers signed like it, so the new
shim couldn't reach it directly.

Phase 1 changes only the MCP shim on macOS: `lean.rs` and its calls in `proxy.rs`. The daemon
and driver are unchanged. So the setup was:

- **After:** the newly built `brigadier-cu mcp`, with `BRIGADIER_CU_AUDIENCE=brigadier` as
  Brigadier sets it. It talked to a small scratch relay on its home's socket, which answered
  the daemon protocol and forwarded each tool call to the installed helper.
- **Before:** the installed shim, talking to that same installed helper.
- **Homes:** both ran in scratch homes, as in the cold-start test above.

The relay costs nothing measurable (warm `list_windows`: 10–11 ms on both paths). Tokens are
estimated as before: bytes / 4 of what Claude Code shows the model (`structuredContent`, or the
text on errors), plus `w × h / 750` per image. Everything ran in the background under the focus
lock; the front app never changed. All scratch files, windows, apps and the scratch helper were
closed afterwards.

### Context

| Item | Before | After | Target |
| --- | ---: | ---: | --- |
| Tool list (bytes / 4) | 40 tools, 95,899 B, **~23,974** | 20 tools, 27,451 B, **~6,862** | ≤ 7,000: met |
| Skill body (bytes / 4) | 13,415 B, ~3,354 | 4,798 B, **~1,200** | ≤ 1,200: met (just) |
| Server instructions | 134 B | 134 B | – |

A GUI worker now pays ~8.1k tokens up front instead of ~27.4k. All 11 tools real agents used
are kept; the largest schemas are now get_window_state at 3,273 B and run_steps at 2,564 B.

### One read, by app (default `get_window_state`, median of 3)

| App | Before: text + image = ~tok | After: text + image = ~tok | Change | ms before → after |
| --- | --- | --- | ---: | --- |
| TextEdit, scratch doc | 439 + 1,476 = 1,915 | 459 + 1,476 = 1,935 | **+1%** | 64 → 118 |
| Finder, scratch folder | 2,407 + 1,177 = 3,584 | 2,427 + 1,177 = 3,604 | **+1%** | 167 → 220 |
| Safari, example.com | 1,156 + 1,766 = 2,922 | 1,087 + 0 = 1,087 | **−63%** | 171 → 60 |
| System Settings › About | 2,213 + 2,018 = 4,231 | 2,143 + 0 = 2,143 | **−49%** | 281 → 178 |

The `since: "latest"` check read with no change goes from 1,975 to 140 tokens on Safari and from
2,251 to 163 on Settings. On TextEdit (1,714 → 1,734) and Finder (1,409 → 1,429) it is unchanged.

**Why TextEdit and Finder save nothing: the thin-tree check misfires on native macOS
windows** (`lean.rs` `thin_tree`):

- It counts a row as readable only if its role starts with `statictext`, `textarea` and so
  on. macOS rows are `AXStaticText` and `AXTextArea`, so the `ax` prefix means no native text
  row ever counts (Safari escapes only through `axlink`).
- Any `AXImage` (the title bar's document icon, Finder's file icons), or the word "document"
  anywhere in a row (it matched the scratch file's own text), then makes the whole window
  "media without readable text".

Both windows are judged thin, and every read, `since` diffs included, gets the automatic
screenshot: a second capture call, +~55 ms, and the full image cost. The text the model sees is
otherwise the same as before plus a ~20-token `screenshot_note`. This is a cost bug, not a
quality loss: the agent still gets everything it got before.

### Tasks A/B/C, replaying each skill's loop (5 runs each, medians)

| Task | Calls before → after | Tool ms before → after | ~Tok before → after | Success before / after |
| --- | --- | --- | --- | --- |
| A. Type into a TextEdit doc and save | 3 → 3 | 641 → 642 | 2,530 → **981 (−61%)** | 5/5 / 5/5 * |
| B. Find a file in a Finder folder | 2 → 2 | 1,877 → 1,898 (+1%) | 3,817 → **3,837 (+1%)** | 5/5 / 5/5 |
| C. Open a URL and read a value | 2 → 2 | 584 → 453 (−22%) | 3,231 → **1,397 (−57%)** | 5/5 / 5/5 |

\* One "before" run of A saved its file after the 1.5 s check (the audit's F5: ⌘S answers
before the write). All ten files held the right text afterwards.

**Against the plan's targets:**
- Tokens: A and C are ≥ 50% lower; B is not.
- Calls: the same on each task. Only the default read changed; launch and action replies didn't.
- Tool time: within +10% everywhere.

In task A the new document's tree wasn't judged thin, so its read came without an image. B
lands on Finder, where the misfire puts the image back. With the thin check fixed, B would be
~2,640 tokens (−31%). Phase 1 can't take B further: the rest of B is Finder's 2.4k-token tree,
noise pruning that phase 3 owns.

### Visual-only cases: does the agent still get the picture when text can't answer?

| Case | What the tree says | After: image? |
| --- | --- | --- |
| V1. Bar chart (PNG, no text) in Preview | Toolbar buttons only, nothing about the chart | **Yes**, automatically ("Thin accessibility tree; screenshot included automatically"): 629 + 1,489 tokens, the same image as before |
| V2. Canvas-drawn chart page in Safari | A web area with no text | **Yes**, automatically: 353 + 1,264 tokens |
| V3. Finder's icon-only view switcher ("which view is on?") | Four `AXRadioButton (icon view)` … `(gallery view)` rows, with no selected state | Today yes, but only because of the misfire above. With it fixed, the text can't answer, and the agent must ask for `include_screenshot: true`, which the skill tells it to do when text can't answer: one more call (~1.2k image tokens) |

So nothing visual is lost today. Fixing the misfire must keep V1 and V2 (canvas, image-only
document) thin and leave V3 to the agent's explicit request.

### Verdict per task

- **A (type and save):** same success (5/5), same 3 calls, same speed, **61% fewer tokens**. No
  quality loss found.
- **B (find a file in Finder):** same success, calls and speed. **No token saving (+1%)**,
  because of the thin-check misfire. No quality loss; a cost bug to fix (about −31% once fixed).
- **C (read a web page):** same success, same calls, 22% faster, **57% fewer tokens**. No
  quality loss: the page text answers, and the screenshot was never needed.
- **Context:** the GUI worker's up-front load goes from ~27.4k to ~8.1k tokens, with every tool
  real agents used still there.
- **Reliability:** no tool errors on either path in 30 task runs, 54 read-test calls or 6 visual reads.
  Error results keep their text, which is what Claude Code shows for errors.

**One fix to make, in `lean.rs` `thin_tree`:** match roles with or without the `AX` prefix, and
don't let the word "document" in row text, or a lone `AXImage` icon, mark a window that has
readable text rows as media. Then rerun B, V1, V2 and V3.
