# Computer use, phase 1: cheap and lean

Scope is the user's pick (L6): phase 1 only. Phases 2 (fixed waits), 3 (failure modes
F1-F13) and 4 (benchmark harness) are out. Goal (L1): cheapest and "not really context
filling", without getting slower or less reliable. Baseline: `OURS-AUDIT.md` (L5).
Techniques: `COMPETITORS.md` (L4). Revised after the plan review (L7).

Lines marked "proposed" are mine, not the user's; my calls after the review are L8.

## The design in one paragraph (proposed)

The daemon, the driver and the guarded catalog stay as they are for every caller. New
behaviour comes in two layers. (a) **Driver options** (W-B): new optional request
parameters that any caller may pass, all off by default. (b) **The lean view** (W-A): the
`brigadier-cu mcp` shim that Brigadier's agents talk to (`proxy.rs`) shows agents a smaller
tool list, fills in the lean options by default on their calls, and compacts results on
the way out. Internal consumers (`run_steps`' final observation, `cu-layer/src/tokens.rs`
ownership) keep reading full structured results inside the daemon; only the outward MCP
reply changes. A helper started by hand, direct MCP use without the Brigadier audience, and
in-process callers see today's behaviour.

## Contract between W-B and W-A (proposed; names may change if both agree and tell me)

W-B adds, all optional and off by default:
- `get_window_state`: `screenshot_scale: "1x" | "2x"` and `screenshot_format: "jpeg" | "png"`
  (with a long-edge cap of 1280 px for 1x). The stored frame and its coordinate transform
  match the image actually returned, so screenshot → zoom → pixel click lands right.
- `launch_app`: `observe: true` returns a compact read (no screenshot) of the window it
  opened or reused, with `query` passed through. Several windows: the one the call names
  as its target; otherwise the newest. A thin first read (loading) says so in one line.
- Actions (click, double_click, right_click, type_text, set_value, press_key, hotkey,
  scroll, invoke_menu): `observe: true` appends the existing `since` diff of the target
  window, with its fresh element ids and reindex mapping (`window_state_view.rs:1055-1083`).
  Inside `run_steps`, inner steps never observe; `run_steps` keeps its one final
  observation and its own baseline, and `observe:false` stays honoured.
- The report on an action is in the result where the registry's `ActionResult` projection
  can't drop it (`tool.rs:2408`, `:3524`): W-B extends the typed contract or attaches it
  after projection, and says which.

W-A, in the shim, for the Brigadier audience only:
- Sets `screenshot` off on `get_window_state` unless the agent asked for one; when asked,
  sets 1x JPEG. An explicit request for an image is always honoured, also on a `since`
  read with no AX change; "no change" means the accessibility view only.
- Sets `observe: true` on `launch_app` and on direct action calls unless the agent set it.
- Sends the tree once: keeps whichever copy each client shows the model (check live:
  Claude Code and Codex, for trees, images, token metadata and errors) and drops the other.
- Compacts `list_apps` without losing installed-but-not-running apps (one short line each).

## Agent tool roster (proposed: 20 of 40)

Kept: get_window_state, list_windows, list_apps, page, zoom, click, double_click,
right_click, type_text, set_value, press_key, hotkey, scroll, invoke_menu, run_steps,
launch_app, kill_app, verify_state, start_recording, stop_recording.
Hidden from agents (still in the daemon, still allowed for other callers):
6 cursor tools, get_recording_state, replay_trajectory, 5 session tools, get/set_config,
health_report, check_permissions, get_screen_size, get_accessibility_tree, get_desktop_state.
The user's pick said "about 14"; 20 keeps recordings, double/right-click and verify_state,
which the skill and real users rely on. The size target holds (below).
The `session` parameter is removed from every schema agents see (the shim fills it);
repeated `target`/`scope`/`element_token` blocks are shortened; descriptions tightened.

## Targets (proposed)

Measured on two builds of this checkout: before = `2c131c69b`, after = the integrated tree.

| Measure | Before | Target |
| --- | ---: | ---: |
| Tool list a GUI worker loads (bytes/4) | 24,011 | ≤ 7,000 |
| Skill body (bytes/4) | 3,354 | ≤ 1,200 |
| Tasks A/B/C, tool-result size (bytes/4, text + image tokens) | 2,482 / 3,827 / 3,556 | each ≥ 50% lower |
| Tasks A/B/C, tool calls a fresh agent actually makes (Claude worker, same prompt, both builds) | measured | fewer or equal on each |
| Tool wall time per task (median of 5) | measured | no worse than +10% |

Response sizes are estimates (bytes/4, image tokens by the provider's formula); turn
counts are measured from a real agent's transcript. Three tasks validate this phase; they
don't prove a market lead (that's phase 4). Anything missed is reported as missed.

## Short skill (W-A, after W-B lands)

Rewrite `crates/core/src/manager/computer_use_skill.md` to ≤ 1.2k tokens for the new loop:
open (the reply has the read), act (the reply has the change), read again only when the
report doesn't show the outcome, screenshot only when text can't answer, and take a
screenshot before `zoom` or any pixel click. A change report proves the action was
delivered and what the accessibility view did; it proves the task only when it shows the
requested outcome. Saved files, sent messages and other effects outside the window still
need their own check (F5).

## Workers and files

- W-A (new, "Bounded implementation"): `crates/brigadier-cu/src/proxy.rs` and a new
  module beside it for the lean view, `mcp.rs`, `crates/cu-layer/src/text.rs` (schema
  rewriting), `crates/core/src/manager/computer_use.rs` and `workers.rs:734` only if the
  audience or always-load must change, then `computer_use_skill.md`.
- W-B (worker 2, wrote the audit): new `patches/00NN-*.diff` for the driver, cu-layer
  launch/action code other than `text.rs`, `serve.rs` if needed, and their tests.
- Both change only their own files; the contract above is the interface. Order: W-A and W-B
  in parallel; W-A's skill after W-B merges; W-B re-measures last; then end verification.

## Done when

- Targets above met, or each miss stated with its number.
- Helper: `cd helpers/computer-use && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- Driver: the affected upstream suites run explicitly (`upstream/` is excluded from the
  helper workspace), and the patches reapply cleanly on the pristine release
  (`PRISTINE.sha256`, `scripts/sync-upstream.sh`).
- New regression tests: tool list per audience; lean defaults; explicit screenshot honoured;
  run_steps inner steps don't observe and its final diff is intact; action reports survive
  the `ActionResult` projection; screenshot → zoom → click scaling at 1x.
- Root workspace for any `crates/core` change: `cargo test -p brigadier-core computer`.
- Tasks A/B/C still succeed live, the user's front app never changed.
