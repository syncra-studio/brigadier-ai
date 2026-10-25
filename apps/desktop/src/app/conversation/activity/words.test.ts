import assert from "node:assert/strict";
import { test } from "node:test";

import {
  type ActionItem,
  agentDetail,
  allowedAt,
  deniedCall,
  editHunks,
  foldRepeats,
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
  for (const action of ["Grep", "Read", "Edit", "exec_command", "apply_patch", "WebSearch", "imagegen"]) {
    for (const name of [action, `functions.${action}`, `custom_server/${action}`]) {
      assert.equal(toolName(name), action);
      for (const status of ["completed", "inProgress", "failed", "declined"] as const) {
        assert.equal(lead(name, status), lead(action, status));
        assert.doesNotMatch(lead(name, status), /functions\.|custom_server\//);
      }
    }
  }
  assert.equal(lead("custom_server/export_document"), "Used export document");
  assert.equal(lead("imagegen", "failed"), "Created an image — failed");
});

test("the CLIs' plumbing is never a row", () => {
  for (const name of ["ToolSearch", "tool_search"]) assert.ok(isPlumbing(name), name);
  // Codex's wait for its own agents: their rows say when each finished (D44); a tool of another's named wait is a row.
  assert.ok(isPlumbing("agents/wait"));
  for (const name of ["mcp__timer__wait", "spawn_agent", "agents/spawnAgent", "Agent"]) assert.ok(!isPlumbing(name), name);
  // The CLI's own question tool and plan: their cards say them (harness D7).
  for (const name of ["AskUserQuestion", "request_user_input", "ExitPlanMode"]) assert.ok(isPlumbing(name), name);
  for (const name of ["Read", "Bash", "WebSearch", "imagegen"]) assert.ok(!isPlumbing(name), name);
});

test("a step names what it acts on, by the file's name", () => {
  assert.equal(lead("Bash", "inProgress", "cargo test -p core"), "Running cargo test -p core");
  assert.equal(lead("shell", "failed", "cargo build"), "Ran cargo build");
  // Moving into the worktree first is no part of what it runs.
  assert.equal(lead("shell", "completed", "cd /tmp/w/session-1 && python3 -m pytest -q"), "Ran python3 -m pytest -q");
  assert.equal(lead("Bash", "completed", 'python3 -c "\nprint(1)\n"'), 'Ran python3 -c " …');
  assert.equal(lead("Bash", "completed", "cd '/tmp/a b' && cat notes.py"), "Read notes.py");
  assert.equal(lead("Read", "inProgress", "/repo/src/main.rs"), "Reading main.rs");
  assert.equal(lead("apply_patch", "completed", "src/a.rs and 2 more"), "Edited a.rs and 2 more");
  assert.equal(lead("Grep", "completed", "sidebar"), "Searched for “sidebar”");
  assert.equal(lead("WebFetch", "completed", "https://tauri.app/start/"), "Read tauri.app");
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
    [call("WebSearch", "completed", "tauri window decorations"),
      { kind: "tool", key: "f", name: "WebSearch", input: JSON.stringify({ query: "tauri window decorations" }), status: "completed", output: null }],
  ];
  for (const [main, other] of pairs) assert.equal(lead(main.name, main.status, main.detail), worker(other));
  // And a run of them sums up the same way in both.
  const mainRun = pairs.map(([main]) => toolStepWords(main));
  const workerRun = pairs.map(([, other]) => stepWords(itemCall(other)));
  assert.equal(summarize(mainRun), summarize(workerRun));
  assert.equal(summarize(mainRun), "Edited a file, read files, ran a command, searched the web");
});

const words = (name: string, detail: string | null = null) => toolStepWords(call(name, "completed", detail));

test("a run sums up its kinds comma-joined, edits, reading, commands and the web first, one plural each", () => {
  assert.equal(summarize([words("Read", "a.ts"), words("Read", "b.ts"), words("Bash", "pnpm test")]), "Read files, ran a command");
  assert.equal(summarize([words("Bash", "git status"), words("Read", "a.ts"), words("Bash", "pnpm test")]), "Read files, ran commands");
  // Searching and listing are reading too.
  assert.equal(summarize([words("Read", "a.ts"), words("Bash", "ls src"), words("Grep", "add"), words("Bash", "sleep 4")]), "Read files, ran a command");
  assert.equal(summarize([words("Bash", "node math.js"), words("apply_patch", "math.js")]), "Edited a file, ran a command");
  assert.equal(summarize([words("imagegen"), words("Read", "a.ts")]), "Read files, created an image");
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
  assert.equal(stepLabel(toolStepWords(call("imagegen", "failed")), "failed"), "Created an image — failed");
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
  const steps = [words("imagegen", "x"), words("Bash", "pnpm test"), words("Read", "a.ts")];
  assert.equal(summarize(steps), "Read files, ran a command, created an image");
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

test("an edit's lines read back as diff text", () => {
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

test("a command the user had to allow runs from their answer, not from when it asked (issue 554)", () => {
  // Asked at 1s, allowed at 4s, done at 4.1s: it ran 0.1s, not 3.1s.
  const allowed = approval({ type: "allowed", by: "user", similar: false });
  const ran: ToolStep = { type: "tool", itemId: "c1", name: "shell", detail: "git push", status: "completed", throughPosition: 0, endedAtMs: 4_100, item: "command" };
  assert.equal(allowedAt(ran, 1_000, [allowed]), 4_000);
  // Still running once allowed: its run time ticks from the answer.
  const { endedAtMs: _ended, ...running } = ran;
  assert.equal(allowedAt({ ...running, status: "inProgress" }, 1_000, [allowed]), 4_000);
  // No approval held it up, another command's, a worker's, a denied one, or one answered
  // before it asked or after it ended: its own times.
  assert.equal(allowedAt(ran, 1_000, []), undefined);
  assert.equal(allowedAt({ ...ran, detail: "ls" }, 1_000, [allowed]), undefined);
  assert.equal(allowedAt(ran, 1_000, [approval(allowed.state, { taskId: "t1" })]), undefined);
  assert.equal(allowedAt(ran, 1_000, [approval({ type: "denied", by: "user", message: null })]), undefined);
  assert.equal(allowedAt(ran, 9_000, [allowed]), undefined);
  assert.equal(allowedAt(ran, 1_000, [approval(allowed.state, { resolvedAtMs: 60_000 })]), undefined);
  // Another tool's call, or a command its CLI timed itself, is timed as it was.
  assert.equal(allowedAt({ ...ran, name: "Edit", item: "files" }, 1_000, [allowed]), undefined);
  const timed = approval(allowed.state, { subject: { type: "cli", request: { id: "q", kind: "command", tool: "commandExecution", command: "git push", cwd: null, paths: [], reason: null, escalation: false, input: null, grant: null } } });
  assert.equal(allowedAt(ran, 1_000, [timed]), undefined);
});

test("a web search's site operator reads after a bar", () => {
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

const said = (name: string, detail = "x") => stepWords({ name, detail, status: "completed" });

test("three or more back-to-back calls of a tool with no words of its own fold; two, or tools with words, never do (issue 101)", () => {
  const search = said("mcp__linear__search_issues");
  const get = said("mcp__linear__get_issue");
  const read = said("Read", "/a.ts");
  const deploy = said("deploy_preview");
  assert.deepEqual(foldRepeats([read, deploy, deploy, deploy, read, read, read]), [[0], [1, 2, 3], [4], [5], [6]]);
  assert.equal(summarize([deploy, deploy, deploy, read]), "Read files, used deploy preview 3 times");
  // An integration's tools say the integration, never a raw tool name; what each did and was
  // given is in its fold.
  assert.deepEqual(foldRepeats([read, search, search, search, get, get, read, read, read]), [[0], [1, 2, 3, 4, 5], [6], [7], [8]]);
  assert.equal(summarize([search, search, search, get, get, read]), "Read files, used Linear 5 times");
  assert.equal(search.given, "search issues · x");
  assert.equal(said("mcp__claude_ai_Gmail__search_threads").done, "Used Gmail");
  // A tool Brigadier has words for keeps them: no fold, no count.
  const created = said("imagegen");
  assert.deepEqual(foldRepeats([created, created, created]), [[0], [1], [2]]);
  assert.equal(summarize([created, created]), "Created an image");
});

/** A finished call's words. */
const done = (name: string, detail: string) => toolStepWords(call(name, "completed", detail));

test("a subagent's row names its task, a skill's its name, and several sum up as agents (B1)", () => {
  // Claude's Agent call by its description, Codex's by its prompt's first line.
  const claude = agentDetail("Agent", JSON.stringify({ description: "Find the parser", prompt: "Look in src for…", subagent_type: "Explore" }));
  assert.equal(claude, "Find the parser");
  assert.equal(agentDetail("agents/spawnAgent", "Review the diff\nFocus on the reader."), "Review the diff");
  assert.equal(agentDetail("Skill", JSON.stringify({ skill: "brigade", args: "" })), "brigade");
  assert.equal(agentDetail("Bash", JSON.stringify({ command: "ls", description: "List" })), undefined);
  assert.equal(lead("Agent", "inProgress", claude), "Running agent: Find the parser");
  assert.equal(lead("Agent", "completed", claude), "Ran agent: Find the parser");
  assert.equal(lead("agents/spawnAgent", "completed", "Review the diff"), "Ran agent: Review the diff");
  assert.equal(lead("Task", "completed"), "Ran an agent");
  assert.equal(lead("Skill", "completed", "brigade"), "Used skill brigade");
  assert.equal(summarize([done("Agent", "Find the parser")]), "Ran an agent");
  assert.equal(summarize([done("Agent", "Find the parser"), done("agents/spawnAgent", "Review the diff"), done("Read", "a.ts")]), "Read files, ran agents");
  assert.equal(summarize([done("Skill", "brigade"), done("Skill", "grilling")]), "Used skills");
});
