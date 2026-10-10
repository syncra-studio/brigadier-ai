import assert from "node:assert/strict";
import { test } from "node:test";

import {
  type ActionItem,
  editHunks,
  fileDiffs,
  hunkDiff,
  isPlumbing,
  itemCall,
  stepLabel,
  stepWords,
  summarize,
  toolHasOwnResult,
  toolName,
  type ToolStep,
  toolStepWords,
} from "@/app/conversation/activity/words";

const call = (name: string, status: ToolStep["status"] = "completed", detail: string | null = null): ToolStep => ({
  type: "tool",
  name,
  status,
  detail,
  itemId: "call",
  throughPosition: 12,
});

/** A lead's step's row. */
const lead = (name: string, status: ToolStep["status"] = "completed", detail: string | null = null) =>
  stepLabel(toolStepWords(call(name, status, detail)), status);

/** A worker's action's row. */
const worker = (item: ActionItem) => {
  const step = itemCall(item);
  return stepLabel(stepWords(step), step.status);
};

test("a tool keeps its words across provider namespaces and never shows them", () => {
  for (const action of ["query_brain", "code_search", "Read", "Edit", "exec_command", "apply_patch", "run", "run_check", "WebSearch", "start_preview", "review_code"]) {
    for (const name of [action, `mcp__brigadier__${action}`, `functions.${action}`, `brigadier/${action}`]) {
      assert.equal(toolName(name), action);
      for (const status of ["completed", "inProgress", "failed", "declined"] as const) {
        assert.equal(lead(name, status), lead(action, status));
        assert.doesNotMatch(lead(name, status), /mcp__|functions\.|brigadier\//);
      }
    }
  }
  assert.equal(lead("custom_server/export_document"), "Used export document");
  assert.equal(lead("query_brain", "failed"), "Checked project memory — failed");
});

test("Brigadier's plumbing is never a row", () => {
  for (const name of ["read_report", "read_artifact", "finish_session", "note_for_user", "remember", "delegate_task", "message_worker", "answer_worker", "stop_worker", "land_phase", "ask_user", "mcp__brigadier__propose_merge", "mcp__brigadier__submit_report", "ToolSearch"]) {
    assert.ok(isPlumbing(name), name);
  }
  for (const name of ["Read", "run", "query_brain", "review_code"]) assert.ok(!isPlumbing(name), name);
});

test("a step names what it acts on, by the file's name", () => {
  assert.equal(lead("Bash", "inProgress", "cargo test -p core"), "Running cargo test -p core");
  assert.equal(lead("mcp__brigadier__run", "completed", "pnpm test"), "Ran pnpm test");
  assert.equal(lead("mcp__brigadier__run_check", "inProgress", "cargo test"), "Running cargo test");
  assert.equal(lead("mcp__brigadier__run_check"), "Ran checks");
  assert.equal(lead("shell", "failed", "cargo build"), "Ran cargo build — failed");
  // Moving into the worktree first is no part of what it runs.
  assert.equal(lead("shell", "completed", "cd /tmp/w/session-1 && python3 -m pytest -q"), "Ran python3 -m pytest -q");
  assert.equal(lead("Bash", "completed", 'python3 -c "\nprint(1)\n"'), 'Ran python3 -c " …');
  assert.equal(lead("Bash", "completed", "cd '/tmp/a b' && cat notes.py"), "Read notes.py");
  assert.equal(lead("Read", "inProgress", "/repo/src/main.rs"), "Reading main.rs");
  assert.equal(lead("apply_patch", "completed", "src/a.rs and 2 more"), "Edited a.rs and 2 more");
  assert.equal(lead("Grep", "completed", "sidebar"), "Searched code for “sidebar”");
  assert.equal(lead("query_brain", "completed", "sidebar modes"), "Checked project memory for “sidebar modes”");
  assert.equal(lead("WebFetch", "completed", "https://tauri.app/start/"), "Read tauri.app");
  assert.equal(lead("mcp__brigadier__preview_log"), "Read the preview’s output");
  // A command that reads, lists or searches is told as that.
  assert.equal(lead("Bash", "completed", "sed -n 1,80p src/app/Sidebar.tsx"), "Read Sidebar.tsx");
  assert.equal(lead("Bash", "completed", "rg -n sidebar src"), "Searched code for “sidebar”");
  assert.equal(lead("Bash", "completed", "ls apps/desktop"), "Listed files in desktop");
});

test("the main thread and a worker's thread say the same step the same way", () => {
  const pairs: [ToolStep, ActionItem][] = [
    [call("Read", "completed", "/repo/apps/desktop/src/app/RequestBlock.tsx"),
      { kind: "tool", key: "a", name: "Read", input: JSON.stringify({ file_path: "/repo/apps/desktop/src/app/RequestBlock.tsx" }), status: "completed", output: "…" }],
    [call("Bash", "failed", "pnpm test"),
      { kind: "command", key: "b", command: "/bin/zsh -lc 'pnpm test'", cwd: null, status: "failed", exitCode: 1, output: "", durationMs: 41_000 }],
    [call("Grep", "completed", "Worked for"),
      { kind: "tool", key: "c", name: "Grep", input: JSON.stringify({ pattern: "Worked for", path: "src" }), status: "completed", output: null }],
    [call("apply_patch", "completed", "apps/desktop/src/app/Sidebar.tsx and 1 more"),
      { kind: "files", key: "d", changes: [{ path: "apps/desktop/src/app/Sidebar.tsx", kind: "update" }, { path: "apps/desktop/src/app/sidebar.css", kind: "update" }] as never, status: "completed" }],
    [call("mcp__brigadier__query_brain", "inProgress", "sidebar"),
      { kind: "tool", key: "e", name: "mcp__brigadier__query_brain", input: JSON.stringify({ query: "sidebar" }), status: "inProgress", output: null }],
    [call("WebSearch", "completed", "tauri window decorations"),
      { kind: "tool", key: "f", name: "WebSearch", input: JSON.stringify({ query: "tauri window decorations" }), status: "completed", output: null }],
  ];
  for (const [main, other] of pairs) assert.equal(lead(main.name, main.status, main.detail), worker(other));
  // And a run of them sums up the same way in both.
  const mainRun = pairs.map(([main]) => toolStepWords(main));
  const workerRun = pairs.map(([, other]) => stepWords(itemCall(other)));
  assert.equal(summarize(mainRun), summarize(workerRun));
  assert.equal(summarize(mainRun), "Read a file, ran a command, searched code, edited a file, checked project memory and searched the web");
});

const words = (name: string, detail: string | null = null) => toolStepWords(call(name, "completed", detail));

test("a run sums up its kinds in the order they came, one plural each", () => {
  assert.equal(summarize([words("Read", "a.ts"), words("Read", "b.ts"), words("Bash", "pnpm test")]), "Read files and ran a command");
  assert.equal(summarize([words("Bash", "git status"), words("Read", "a.ts"), words("Bash", "pnpm test")]), "Ran commands and read a file");
  assert.equal(summarize([words("review_code"), words("Read", "a.ts")]), "Asked for a review and read a file");
  assert.equal(summarize([words("run_check", "pnpm test"), words("start_preview", "web")]), "Ran checks and started a preview");
});

test("deduplication needs the matching authored result in this call's window", () => {
  const raw = call("mcp__brigadier__delegate_task");
  const result = { requestId: "request", position: 13, kind: { type: "created" } };
  assert.equal(toolHasOwnResult(raw, [], "request", 10), false);
  assert.equal(toolHasOwnResult(raw, [result], "request", 10), true);
  assert.equal(toolHasOwnResult(raw, [result], "other", 10), false);
  assert.equal(toolHasOwnResult({ ...raw, status: "failed" }, [result], "request", 10), false);
  assert.equal(toolHasOwnResult(raw, [{ ...result, position: 9 }], "request", 10), false);
  assert.equal(
    toolHasOwnResult(raw, [{ ...result, position: 11, kind: { type: "tool" } }, result], "request", 10),
    false,
    "an overlapping call's result cannot hide the earlier action",
  );
});

test("a command that exited badly says how, a stopped one says so", () => {
  const ran = toolStepWords(call("Bash", "completed", "pnpm test"));
  assert.equal(stepLabel(ran, "completed", 0), "Ran pnpm test");
  assert.equal(stepLabel(ran, "failed", 1), "Ran pnpm test — failed (exit 1)");
  // A command can end "completed" with a bad exit code (Codex): still a failure.
  assert.equal(stepLabel(ran, "completed", 2), "Ran pnpm test — failed (exit 2)");
  assert.equal(stepLabel(ran, "failed"), "Ran pnpm test — failed");
  assert.equal(stepLabel(ran, "declined", 130), "Ran pnpm test — stopped");
});

test("an edit's call reads as its lines out and in", () => {
  assert.deepEqual(editHunks("Edit", JSON.stringify({ file_path: "/r/a.ts", old_string: "a\nb", new_string: "a\nc\n" })), [
    { path: "/r/a.ts", removed: ["a", "b"], added: ["a", "c"] },
  ]);
  assert.deepEqual(
    editHunks("mcp__x__MultiEdit", JSON.stringify({ file_path: "f", edits: [{ old_string: "x", new_string: "y" }, { old_string: "", new_string: "z" }] })),
    [
      { path: "f", removed: ["x"], added: ["y"] },
      { path: "f", removed: [], added: ["z"] },
    ],
  );
  assert.deepEqual(editHunks("Write", JSON.stringify({ file_path: "n.md", content: "hi" })), [{ path: "n.md", removed: [], added: ["hi"] }]);
  assert.deepEqual(editHunks("apply_patch", "not json"), []);
  assert.deepEqual(editHunks("Read", JSON.stringify({ file_path: "f" })), []);
});

test("an edit's diff text splits back into its files", () => {
  assert.deepEqual(fileDiffs("/r/a.ts\n-a\n+b\n/r/b.md\n@@ -1 +1 @@\n context\n+new"), [
    { path: "/r/a.ts", diff: "-a\n+b" },
    { path: "/r/b.md", diff: "@@ -1 +1 @@\n context\n+new" },
  ]);
  assert.equal(hunkDiff({ path: "f", removed: ["x"], added: ["y", "z"] }), "-x\n+y\n+z");
});
