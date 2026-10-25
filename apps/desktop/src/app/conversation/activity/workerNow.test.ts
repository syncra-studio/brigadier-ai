import assert from "node:assert/strict";
import { test } from "node:test";

import { nowInLine, stepAtWork, workerNow } from "@/app/conversation/activity/workerNow";
import type { WorkerRow } from "@/ipc/generated";

const NOW = 1_000_000;
const row = (patch: Partial<WorkerRow> = {}): WorkerRow => ({
  id: "t1", number: 1, title: "Worker", agent: "claude", model: null, startedAtMs: 10,
  working: true, lastEnd: null, gone: false, lastEventAtMs: NOW, limit: null, freshContext: false, waiting: [], mcp: [],
  branch: null, base: null, diff: null, ahead: 0, merged: false, ...patch,
});

test("a worker at work says its newest call at work in the thread's words", () => {
  const steps = [{ name: "shell", detail: "cd /w && pnpm build" }, { name: "Edit", detail: "/w/src/foo.ts" }];
  assert.deepEqual(workerNow(row({ steps }), NOW), { kind: "working", word: "Editing foo.ts", step: true });
  assert.equal(workerNow(row({ steps: steps.slice(0, 1) }), NOW).word, "Running pnpm build");
  // Plumbing is no step: the call before it still says what the worker does.
  assert.equal(workerNow(row({ steps: [...steps, { name: "ToolSearch", detail: "select:Read" }] }), NOW).word, "Editing foo.ts");
});

test("between its calls, and whenever it isn't working, a worker says its state", () => {
  assert.deepEqual(workerNow(row(), NOW), { kind: "working", word: "Working" });
  assert.deepEqual(workerNow(row({ steps: [] }), NOW), { kind: "working", word: "Working" });
  const steps = [{ name: "shell", detail: "pnpm test" }];
  assert.equal(workerNow(row({ steps, waiting: [{ kind: "question", what: "Which?", card: null }] }), NOW).word, "Needs you");
  assert.equal(workerNow(row({ steps, lastEventAtMs: NOW - 21 * 60_000 }), NOW).word, "Quiet 21m");
  assert.equal(workerNow(row({ steps, working: false, lastEnd: "done" }), NOW).word, "Done");
});

test("after a worker's name the line reads lowercase, but for a name it starts with", () => {
  const now = (detail: string, name = "shell") => nowInLine(workerNow(row({ steps: [{ name, detail }] }), NOW));
  assert.equal(now("pnpm build"), "running pnpm build");
  assert.equal(now("", "mcp__pencil__batch_design"), "Pencil: batch design");
  assert.equal(nowInLine(workerNow(row({ waiting: [{ kind: "permission", what: "rm", card: null }] }), NOW)), "needs you");
});

test("a thread's item is a call at work while it runs, named as the engine names a worker's", () => {
  assert.deepEqual(stepAtWork({ type: "command", itemId: "c", command: "pnpm build", cwd: null, status: "inProgress", exitCode: null, output: null, durationMs: null }), { name: "shell", detail: "pnpm build" });
  assert.deepEqual(stepAtWork({ type: "toolCall", itemId: "t", name: "Read", input: '{"file_path":"/w/a.ts"}', status: "inProgress", output: null }), { name: "Read", detail: "/w/a.ts" });
  assert.equal(stepAtWork({ type: "toolCall", itemId: "t", name: "Read", input: null, status: "completed", output: "…" }), null);
  assert.equal(stepAtWork({ type: "message", itemId: "m", role: "assistant", text: "hi" }), null);
});
