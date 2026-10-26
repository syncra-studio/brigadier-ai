# Computer use, phase 1: cheap and lean

Scope is the user's pick (L6): phase 1 only. Phases 2 (fixed waits), 3 (failure modes
F1-F13) and 4 (benchmark harness) are out unless a phase 1 change cannot work without them.
Goal (L1): cheapest and "not really context filling", without getting slower or less
reliable. Baseline numbers: `OURS-AUDIT.md` (L5). Techniques: `COMPETITORS.md` (L4).

Lines marked "proposed" are mine, not the user's.

## Targets (proposed)

| Measure (audit method, bytes/4) | Now | Target |
| --- | ---: | ---: |
| Tool list a GUI worker loads up front | 24,011 | ≤ 6,000 |
| Skill body | 3,354 | ≤ 1,200 |
| Task A (TextEdit type + save), model tokens | 2,482 | ≤ 900 |
| Task B (find a file in Finder), model tokens | 3,827 | ≤ 600 |
| Task C (open URL, read a value), model tokens | 3,556 | ≤ 800 |
| Model turns per task A/B/C | 3 / 2 / 2 | 2 / 1 / 1 |
| Tool wall time per task | 0.9-1.9 s | no worse than +10% |

## Changes (proposed)

Where: Brigadier's layer first (`helpers/computer-use/crates/cu-layer`, `crates/brigadier-cu`),
then a small new patch in `helpers/computer-use/patches/` only where the driver must change
(the vendored files stay byte-identical apart from patches; README "Origin"). Nothing is
sent upstream without asking the user.

1. **Agent tool set (W-A).** Brigadier's agents (sessions, chats, workers) get only the
   tools they use; the daemon keeps all 40, and a helper started by hand still lists all.
   Drop for agents: cursor (6), recording (4: start/stop/get_recording_state,
   replay_trajectory), sessions (5), config and diagnostics (get/set_config, health_report,
   check_permissions, get_screen_size), and duplicates the worker judges redundant
   (e.g. get_accessibility_tree, get_desktop_state, move_cursor). Remove the `session`
   parameter from every schema agents see (the host passes it), dedupe the repeated
   `target`/`scope`/`element_token` blocks, and tighten descriptions. Served from the proxy
   that already rewrites `tools/list` (`proxy.rs:49-60`), keyed on `BRIGADIER_CU_AUDIENCE`.
2. **Text-first reads (W-B).** For agents, `get_window_state` returns no screenshot unless
   asked. A requested screenshot is 1x, JPEG, long edge ≤ 1280 px; `zoom` stays the way to
   see detail. A `since` read with no change returns no image and one line. The tree goes
   out once (no second copy in `structuredContent`; check what Claude and Codex each show
   the model, and keep the copy each one shows). Prune noise: the user's desktop icons and
   icon descriptions in Finder, empty 0-element windows, `list_apps` structured payload
   (16.8 KB for a 396 B answer).
3. **Fewer turns (W-B, after 2).** `launch_app` returns the opened window's first compact
   read. Actions (click, type_text, set_value, press_key, hotkey, scroll, invoke_menu,
   page actions) end with a compact change report: what rows appeared, changed or went
   away, with their element ids, or "no visible change". Element ids of elements that
   survive a read stay valid (today any read kills all older ids, `snapshot_store.rs:384-430`,
   F2); without this, change reports would break the agent's next action. If a slow app
   needs a settle, the report says "still changing" rather than waiting long.
4. **Short skill (W-A, after 2-3 land).** Rewrite `crates/core/src/manager/computer_use_skill.md`
   to ≤ 1.2k tokens for the new loop: open (gets the read), act (gets the change), read
   only when the report isn't enough, screenshot or zoom only when text can't answer.
   Keep the safety rules (never call it done without evidence; the change report counts).
5. **Re-measure (W-B's author, last).** Re-run the audit's tool-list, read and task A/B/C
   measurements on the integrated tree and add an "After phase 1" section to `OURS-AUDIT.md`.

## Workers and files

- W-A (new, "Bounded implementation"): `crates/brigadier-cu/src/proxy.rs`, `mcp.rs`,
  `crates/cu-layer/src/catalog.rs`, `crates/core/src/manager/computer_use.rs` (if the
  audience must be passed), then `computer_use_skill.md`.
- W-B (worker 2, wrote the audit): everything else under `crates/cu-layer/src` that reads
  and reports, new `patches/00NN-*.diff` for the driver, and their tests.
- Order: W-A (1) and W-B (2) in parallel; W-B (3); W-A (4); W-B (5); end verification.

## Done when

- Targets above met, or the gap explained with numbers.
- `cd helpers/computer-use && cargo fmt --all --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace` passes; root workspace checks for any `crates/core` change (`cargo test -p brigadier-core computer`).
- Tasks A/B/C still succeed in the live re-measure, with the user's front app never changed.
