import assert from "node:assert/strict";
import { test } from "node:test";

import {
  type ActionItem,
  deniedCall,
  editHunks,
  fileDiffs,
  hunkDiff,
  isPlumbing,
  itemCall,
  labelParts,
  stepLabel,
  stepWords,
  summarize,
  summaryLead,
  toolHasOwnResult,
  toolName,
  type ToolStep,
  toolStepWords,
} from "@/app/conversation/activity/words";
import type { Approval, CardState } from "@/ipc/generated";

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
  assert.equal(lead("shell", "failed", "cargo build"), "Ran cargo build");
  // Moving into the worktree first is no part of what it runs.
  assert.equal(lead("shell", "completed", "cd /tmp/w/session-1 && python3 -m pytest -q"), "Ran python3 -m pytest -q");
  assert.equal(lead("Bash", "completed", 'python3 -c "\nprint(1)\n"'), 'Ran python3 -c " …');
  assert.equal(lead("Bash", "completed", "cd '/tmp/a b' && cat notes.py"), "Read notes.py");
  assert.equal(lead("Read", "inProgress", "/repo/src/main.rs"), "Reading main.rs");
  assert.equal(lead("apply_patch", "completed", "src/a.rs and 2 more"), "Edited a.rs and 2 more");
  assert.equal(lead("Grep", "completed", "sidebar"), "Searched for “sidebar”");
  assert.equal(lead("query_brain", "completed", "sidebar modes"), "Checked project memory for “sidebar modes”");
  assert.equal(lead("WebFetch", "completed", "https://tauri.app/start/"), "Read tauri.app");
  assert.equal(lead("mcp__brigadier__preview_log"), "Read the preview’s output");
  // A command that reads, lists or searches is told as that.
  assert.equal(lead("Bash", "completed", "sed -n 1,80p src/app/Sidebar.tsx"), "Read Sidebar.tsx");
  assert.equal(lead("Bash", "completed", "rg -n sidebar src"), "Searched for “sidebar” in src");
  assert.equal(lead("Bash", "inProgress", "grep -rn add ."), "Searching for “add” in .");
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
  assert.equal(summarize(mainRun), "Edited a file, read files, ran a command, searched the web, checked project memory");
});

const words = (name: string, detail: string | null = null) => toolStepWords(call(name, "completed", detail));

test("a run sums up its kinds comma-joined, edits, reading, commands and the web first, one plural each", () => {
  assert.equal(summarize([words("Read", "a.ts"), words("Read", "b.ts"), words("Bash", "pnpm test")]), "Read files, ran a command");
  assert.equal(summarize([words("Bash", "git status"), words("Read", "a.ts"), words("Bash", "pnpm test")]), "Read files, ran commands");
  // Searching and listing are reading too.
  assert.equal(summarize([words("Read", "a.ts"), words("Bash", "ls src"), words("Grep", "add"), words("Bash", "sleep 4")]), "Read files, ran a command");
  assert.equal(summarize([words("Bash", "node math.js"), words("apply_patch", "math.js")]), "Edited a file, ran a command");
  assert.equal(summarize([words("review_code"), words("Read", "a.ts")]), "Read files, asked for a review");
  assert.equal(summarize([words("run_check", "pnpm test"), words("start_preview", "web")]), "Ran checks, started a preview");
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

/** A page read the board has: the result of the call that read `page`. */
const readPage = (at: number, page: string) => ({ requestId: "request", position: at, kind: { type: "readPage", url: page } });

test("a page read alongside other calls is one row: its result is the call's by its page, not by what came next", () => {
  const url = "https://blog.rust-lang.org/2026/10/01/Rust-1.95.0.html";
  const first = call("WebFetch", "completed", url);
  const second = { requestId: "request", position: 11, kind: { type: "tool" } };
  // Two pages read at once: the first ends after the second started, and its result still hides it.
  assert.equal(toolHasOwnResult(first, [second, readPage(12, url), readPage(13, "https://docs.rs")], "request", 10), true);
  // Another page's result is not its own.
  assert.equal(toolHasOwnResult(first, [second, readPage(13, "https://docs.rs")], "request", 10), false);
  assert.equal(toolHasOwnResult(first, [readPage(12, url)], "other", 10), false);
  assert.equal(toolHasOwnResult(first, [readPage(9, url)], "request", 10), false);
  // A search the same: by its query.
  const search = call("WebSearch", "completed", "rust 1.95");
  const searched = { requestId: "request", position: 12, kind: { type: "searchedWeb", query: "rust 1.95" } };
  assert.equal(toolHasOwnResult(search, [second, searched], "request", 10), true);
  // A Codex search call that opened a page: its result is that page's read.
  assert.equal(toolHasOwnResult(call("web_search", "completed", url), [second, readPage(12, url)], "request", 10), true);
});

test("a command reads as the call it was however it ended; another tool that failed says so", () => {
  const ran = toolStepWords(call("Bash", "completed", "pnpm test"));
  assert.equal(stepLabel(ran, "completed", 0), "Ran pnpm test");
  // The exit code is in the Shell box; the row is the call.
  assert.equal(stepLabel(ran, "failed", 1), "Ran pnpm test");
  assert.equal(stepLabel(ran, "completed", 2), "Ran pnpm test");
  // A declined call shows as the call it was: the header counts the declined ones.
  assert.equal(stepLabel(ran, "declined", 130), "Ran pnpm test");
  assert.equal(stepLabel(toolStepWords(call("review_code", "failed")), "failed"), "Asked for a review — failed");
  // A worker's question to the thread, by its name since #66 and its older one.
  for (const name of ["mcp__brigadier__ask_thread", "mcp__brigadier__ask_orchestrator"]) {
    assert.equal(stepLabel(toolStepWords(call(name, "completed")), "completed"), "Asked a question");
  }
});

test("a listing has the folder, `date` and a public page's GET read as what they are", () => {
  assert.equal(toolStepWords(call("Bash", "completed", "ls src")).list, true);
  assert.equal(toolStepWords(call("Grep", "completed", "add")).list, undefined);
  assert.equal(toolStepWords(call("Bash", "completed", "date")).done, "Checked the current date and time");
  const page = toolStepWords(call("Bash", "completed", "curl -s https://nodejs.org/dist/index.json"));
  assert.deepEqual([page.kind, page.web, page.done], ["web", true, "Ran curl -s https://nodejs.org/dist/index.json"]);
  for (const sends of ["curl -X POST https://api.example.com/x", "curl -d a=1 https://api.example.com/x", "curl http://localhost:3000"]) {
    assert.notEqual(toolStepWords(call("Bash", "completed", sends)).kind, "web", sends);
  }
  assert.equal(summarize([page, toolStepWords(call("Bash", "completed", "pnpm test"))]), "Ran a command, searched the web");
  // A command that only reaches the network, in a pipe or not, keeps its terminal.
  for (const command of ["curl -s https://nodejs.org/dist/index.json | python3 -c 'import json'", "git fetch origin", "ssh host uptime"]) {
    const ran = toolStepWords(call("Bash", "completed", command));
    assert.deepEqual([ran.kind, ran.web], ["run", undefined], command);
  }
});

test("a summary row's icon is its first segment's first step", () => {
  const steps = [words("query_brain", "x"), words("Bash", "pnpm test"), words("Read", "a.ts")];
  assert.equal(summarize(steps), "Read files, ran a command, checked project memory");
  assert.equal(summaryLead(steps)?.kind, "read");
  assert.equal(summaryLead([words("Bash", "a"), words("apply_patch", "b.ts")])?.kind, "edit");
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

test("a row's words split into its verb and, a step fainter, what it acts on", () => {
  const edited = stepWords({ name: "Edit", detail: "/r/src/math.ts", status: "completed" });
  assert.deepEqual(labelParts(edited, stepLabel(edited, "completed")), { verb: "Edited ", object: "math.ts", rest: "" });
  const ran = stepWords({ name: "Bash", detail: "pnpm test", status: "failed" });
  assert.deepEqual(labelParts(ran, stepLabel(ran, "completed", 1)), { verb: "Ran ", object: "pnpm test", rest: "" });
  const searched = stepWords({ name: "Grep", detail: "sidebar", status: "completed" });
  assert.deepEqual(labelParts(searched, stepLabel(searched, "completed")), { verb: "Searched for ", object: "“sidebar”", rest: "" });
  // Words with nothing to act on are all verb.
  const listed = stepWords({ name: "Glob", detail: null, status: "completed" });
  assert.deepEqual(labelParts(listed, stepLabel(listed, "completed")), { verb: "Listed files", object: "", rest: "" });
  const read = stepWords({ name: "Read", detail: null, status: "completed" });
  assert.deepEqual(labelParts(read, stepLabel(read, "completed")), { verb: "Read a file", object: "", rest: "" });
});

/** The thread's own approval, in `state`. */
const approval = (state: CardState, fields: Partial<Approval> = {}): Approval =>
  ({
    id: "a1",
    conversationId: "c",
    taskId: null,
    requestId: "r2",
    position: 5,
    subject: { type: "cli", request: { id: "q", kind: "command", tool: "Bash", command: "git push", cwd: null, paths: [], reason: null, escalation: false, input: null, grant: null } },
    state,
    createdAtMs: 1_500,
    resolvedAtMs: 4_000,
    ...fields,
  }) as Approval;

test("a lead's call its denied approval stopped reads as declined, other failures stay failures", () => {
  const denied = approval({ type: "denied", by: "user", message: null });
  const failed = { ...call("Bash", "failed", "git push"), endedAtMs: 4_100 };
  const denies = (approvals: Approval[], shown = failed, atMs: number | undefined = 1_000) => deniedCall(shown, atMs, ["r2"], approvals);
  assert.equal(denies([denied]), true);
  assert.equal(denies([denied], failed, undefined), true);
  assert.equal(denies([approval({ type: "allowed", by: "user", similar: false })]), false);
  // A worker's approval, another request's, another tool's, or one answered long after: not this call.
  assert.equal(denies([approval(denied.state, { taskId: "t1" })]), false);
  assert.equal(denies([approval(denied.state, { requestId: "r1" })]), false);
  assert.equal(denies([denied], { ...failed, name: "Edit" }), false);
  assert.equal(denies([approval(denied.state, { resolvedAtMs: 60_000 })]), false);
  // A call that completed, or failed with no approval, keeps its own status.
  assert.equal(denies([denied], { ...failed, status: "completed" }), false);
  assert.equal(denies([]), false);
});

const doneWords = (name: string) => stepWords({ name: `mcp__brigadier__${name}`, detail: null, status: "completed" }).done;

test("Brigadier's own tools read in plain words, never as their internal names", () => {
  const done = doneWords;
  assert.equal(done("record_findings"), "Wrote down its findings");
  assert.equal(done("approve_outline"), "Checked a worker’s outline");
  assert.equal(done("watch_ci"), "Watched CI");
  // A web search's site operator reads after a bar.
  assert.equal(stepWords({ name: "WebSearch", detail: "node lts site:nodejs.org", status: "completed" }).done, "Searched the web for node lts | nodejs.org");
});

const editOf = (paths: string[]) => ({ ...stepWords({ name: "apply_patch", detail: paths[0] ?? null, status: "completed" }), paths });

test("edits that name their files sum up by files, not by calls", () => {
  const edit = editOf;
  // Two edits of one file: a file.
  assert.equal(summarize([edit(["/r/a.ts"]), edit(["/r/a.ts"])]), "Edited a file");
  // One patch of two files: files.
  assert.equal(summarize([edit(["/r/a.ts", "/r/b.ts"])]), "Edited files");
  // Without their files, each call counts.
  assert.equal(summarize([stepWords({ name: "Edit", detail: "/r/a.ts", status: "completed" })]), "Edited a file");
});
