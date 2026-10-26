# Lean MCP evidence (2026-10-26)

Scope: outward Brigadier MCP view (session ledger L8, revised L13/L15;
worker ledger L4, L6-L8). No daemon, catalog, driver or skill changes.

## Live client rendering

A local stdio MCP fixture returned distinct text/structured tree markers,
`SNAPSHOT_METADATA_37`, element token `37:1`, and a 256×256 PNG split red on the
left, blue on the right. Another tool returned isError:true with distinct
text/structured error markers. No desktop access was needed.

- **Claude Code 2.1.310**: normal configured `claude -p --tools ''
  --strict-mcp-config --mcp-config <fixture> --permission-mode bypassPermissions
  --output-format stream-json --verbose --no-session-persistence`.
  The model reported `TREE_STRUCT_MARKER`, `OK_CODE_MARKER`, the snapshot and
  element token, and correctly described the image. It did not see
  `TREE_TEXT_MARKER`. On error it saw only `ERROR_TEXT_MARKER`; it explicitly
  reported no snapshot/token metadata or other fields.
- **Codex CLI 0.164.0**: `codex exec --ignore-user-config --ignore-rules
  --ephemeral --skip-git-repo-check --dangerously-bypass-approvals-and-sandbox
  -c mcp_servers.lean_probe.command=python3 -c mcp_servers.lean_probe.args=[<fixture>]
  --json`. The model reported both `TREE_TEXT_MARKER` and `TREE_STRUCT_MARKER`,
  `OK_CODE_MARKER`, the snapshot/token metadata, and the same image arrangement.
  On error it saw `ERROR_TEXT_MARKER`, `ERROR_STRUCT_MARKER`, `ERROR_CODE_MARKER`
  and both metadata fields. Its JSONL tool-call event also retained both copies.

Therefore the shim keeps structured trees, images, metadata and nonduplicated
text, removing only exact repeated tree bodies from text on success. On errors
the text copy survives, with any exact duplicate structured tree removed; error
text and unique metadata stay intact. Regression tests cover these transformations, including run_steps
diffs. This establishes rendering behavior, not an end-to-end task benchmark.

Raw scratch evidence under this worker's TMPDIR, `brigadier-lean-probe/`:
`probe.py`, `claude-live.jsonl`, `codex-live.jsonl`, `codex-live-sol.jsonl`.
The initial Claude --bare attempt failed authentication; normal launch worked.
The initial Codex approval-never attempt did not invoke the fixture; the
isolated fixture-only launch above did. Failed attempts are not counted as
rendering evidence.

## Thin-tree fixture

`lean-empty-window.json` normalizes two actual reads of our scratch TextEdit
document: zero elements, no tree_markdown field, degraded:ax_window_unresolved.
Pids/window ids are replaced, diagnostics omitted, and the text block summarized.
The read used the installed signed helper. `/tmp/brigadier-focus.lock` was held
through launch, reads and cleanup; `lsappinfo front` stayed identical. Only our
exactly named scratch documents were closed; the scratch files were deleted.

The audit's normal reads were TextEdit 14, Safari 32, Finder 97 and Settings 99
elements (OURS-AUDIT.md section 2). The conservative heuristic is fewer than
four labelled rows, or a canvas/image/web area without readable content.
Label-rich 14-row, sparse, empty and opaque-media fixtures test that boundary.
Since diffs inherit the previous full tree's thin status per window, so an
unchanged canvas still receives fresh pixels. A readable window's small diff
does not itself trigger a screenshot. Explicit include_screenshot:false opts
out; true, screenshot_out_file and screenshot-only reads remain honoured.

Fallback is one screenshot-only read, retaining the original snapshot/tree and
adding fresh capture/frame metadata plus a short screenshot_note. Failed image
capture retains the text read and reports screenshot_failure and an unavailable
note. Pixels do not by themselves prove an action or its external effects.

## Tool-list measurements

Audit method: sum per-tool `len(json.dumps(tool, separators=(',', ':')).encode())`,
ASCII escaping enabled, divided by four. Before is the unchanged 40-tool proxy
view (outputSchema removed); after is the Brigadier-only 20-tool view. Input
constraints and MCP safety annotations remain; driver inventory extensions
and redundant prose are omitted only from the lean advertisement.

Before: 96,046 bytes (~24,011.5 tokens). After: 27,454 bytes (~6,863.5 tokens), 71.4% smaller.

The full after tools/list envelope is 27,531 bytes (~6,882.8 tokens);
its actual UTF-8 wire form is 27,528 bytes. Both stay below 28,000 bytes.

| Tool | Before bytes | After bytes | After ~tokens |
| --- | ---: | ---: | ---: |
| list_apps | 2094 | 319 | 79.8 |
| list_windows | 1599 | 732 | 183.0 |
| get_window_state | 7850 | 3273 | 818.2 |
| verify_state | 3026 | 2206 | 551.5 |
| launch_app | 3663 | 1425 | 356.2 |
| kill_app | 943 | 379 | 94.8 |
| invoke_menu | 1685 | 1173 | 293.2 |
| click | 4058 | 2272 | 568.0 |
| double_click | 1795 | 842 | 210.5 |
| right_click | 2312 | 998 | 249.5 |
| type_text | 2908 | 1455 | 363.8 |
| press_key | 2414 | 1368 | 342.0 |
| hotkey | 3596 | 1369 | 342.2 |
| set_value | 2084 | 824 | 206.0 |
| scroll | 2971 | 1584 | 396.0 |
| zoom | 1901 | 1030 | 257.5 |
| page | 4156 | 2104 | 526.0 |
| start_recording | 5260 | 1236 | 309.0 |
| stop_recording | 1214 | 301 | 75.2 |
| run_steps | 7171 | 2564 | 641.0 |
