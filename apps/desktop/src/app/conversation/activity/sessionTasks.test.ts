import assert from "node:assert/strict";
import { test } from "node:test";

import { checkLines, checkWords, STRIP_TAIL_MS } from "@/app/conversation/activity/sessionTasks";
import type { BackgroundTask, FileEntry } from "@/ipc/generated";

const NOW = 10_000_000;
const task = (id: string, label: string, patch: Partial<BackgroundTask> = {}): BackgroundTask => ({
  id, itemId: `call-${id}`, kind: "shell", label, status: "running", startedAtMs: NOW - 60_000, endedAtMs: null,
  outputFile: `/tmp/cli/tasks/${id}.output`, summary: null, lastLine: null, ...patch,
});
const bash = (id: string, command: string): FileEntry => ({
  event: { type: "toolCall", itemId: `call-${id}`, name: "Bash", input: JSON.stringify({ command, run_in_background: true }), status: "inProgress", output: null },
});

test("a wait that watches one command's output is folded into it: the session waits on that command", () => {
  const tests = task("t", "Run full core tests", { lastLine: "48 passed" });
  const wait = task("w", "Wait for the full suite to finish", { startedAtMs: NOW - 30_000 });
  const view = checkLines([tests, wait], [bash("t", "pnpm test; echo \"test exit $?\""),
    bash("w", "until grep -q \"^test exit\" /tmp/cli/tasks/t.output 2>/dev/null; do sleep 10; done; cat /tmp/cli/tasks/t.output")], NOW);
  assert.deepEqual(view.lines.map((line) => line.id), ["t"]);
  assert.equal(view.lines[0]!.waitedOn, true);
  assert.equal(view.lines[0]!.detail, "48 passed");
  // Its Stop stops the wait too.
  assert.deepEqual(view.lines[0]!.stops, ["t", "w"]);
  assert.equal(checkWords(view), "Waiting on Run full core tests · 1 check running");
});

test("a wait on a file only one command writes folds into it; a command after the wait is queued behind it", () => {
  const build = task("b", "Build the app");
  const smoke = task("s", "Smoke test the built app");
  const view = checkLines([build, smoke], [bash("b", "pnpm build > /tmp/r/build.log 2>&1"),
    bash("s", "until grep -q 'built in' /tmp/r/build.log; do sleep 5; done && pnpm smoke")], NOW);
  assert.deepEqual(view.lines.map((line) => [line.name, line.state, line.word]), [["Build the app", "running", null], ["Smoke test the built app", "queued", "starts after Build the app"]]);
  assert.equal(checkWords(view), "1 check running · 1 queued");
  // Once the build ended it runs.
  const after = checkLines([{ ...build, status: "completed", endedAtMs: NOW - 1_000, summary: "completed (exit code 0)" }, smoke], [bash("b", "pnpm build > /tmp/r/build.log 2>&1"),
    bash("s", "until grep -q 'built in' /tmp/r/build.log; do sleep 5; done && pnpm smoke")], NOW);
  assert.deepEqual(after.lines.map((line) => [line.state, line.word]), [["done", "passed"], ["running", null]]);
});

test("a wait that names nothing for sure is a row of its own, by its description; /dev/null links nothing", () => {
  const wait = task("w", "Wait for the benchmark process to exit");
  const other = task("o", "Quiet run");
  const view = checkLines([wait, other], [bash("w", "while kill -0 25996 2>/dev/null; do sleep 20; done; tail -30 /tmp/bench.log"), bash("o", "pnpm quiet 2>/dev/null")], NOW);
  assert.deepEqual(view.lines.map((line) => [line.name, line.state, line.waitedOn]), [["Wait for the benchmark process to exit", "running", false], ["Quiet run", "running", false]]);
  assert.equal(view.waitingOn, null);
});

test("a wait on two commands' output is no fold: never a guess", () => {
  const a = task("a", "First"), b = task("b", "Second"), w = task("w", "Wait for both");
  const view = checkLines([a, b, w], [bash("w", "until test -s /tmp/cli/tasks/a.output -a -s /tmp/cli/tasks/b.output; do sleep 5; done")], NOW);
  assert.deepEqual(view.lines.map((line) => line.id), ["a", "b", "w"]);
});

test("ended commands say how in a word; the strip hides a moment after the last one ends; subagents and brigadier wait are not checks", () => {
  const lint = task("l", "Lint", { status: "completed", endedAtMs: NOW - 2_000, summary: "Background command completed (exit code 1)" });
  const stopped = task("x", "Dev server", { status: "stopped", endedAtMs: NOW - 2_000 });
  const view = checkLines([lint, stopped, task("a", "Explore", { kind: "agent" }), task("bw", "Wait for workers")], [bash("bw", "brigadier wait --any")], NOW);
  assert.deepEqual(view.lines.map((line) => [line.name, line.state, line.word]), [["Lint", "failed", "failed · exit 1"], ["Dev server", "stopped", "stopped"]]);
  assert.equal(checkWords(view), "1 check failed · 1 stopped");
  assert.equal(checkLines([{ ...lint, endedAtMs: NOW - STRIP_TAIL_MS - 1 }], [], NOW).lines.length, 0);
  assert.equal(checkLines([], [], NOW).lines.length, 0);
});
