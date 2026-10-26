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

const observed = (id: string, command: string): FileEntry => ({ event: { type: "command", itemId: `call-${id}`, command, cwd: "/repo", status: "inProgress", exitCode: null, output: null, durationMs: null } });

test("a wait that watches one command's output is folded into it: the session waits on that command", () => {
  const tests = task("t", "Run full core tests", { lastLine: "48 passed" });
  const wait = task("w", "Wait for the full suite to finish", { startedAtMs: NOW - 30_000 });
  const view = checkLines([tests, wait], [bash("t", "pnpm test; echo \"test exit $?\""),
    bash("w", "until grep -q \"^test exit\" /tmp/cli/tasks/t.output 2>/dev/null; do sleep 10; done; cat /tmp/cli/tasks/t.output")], NOW);
  assert.deepEqual(view.lines.map((line) => line.id), ["t"]);
  assert.equal(view.lines[0]!.waitedOn, true);
  assert.equal(view.lines[0]!.detail, "48 passed");
  // Row Stop owns only its producer; explicit Stop all also includes the watcher.
  assert.deepEqual(view.lines[0]!.stops, ["t"]);
  assert.deepEqual(view.stops, ["t", "w"]);
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
  assert.deepEqual(after.lines.map((line) => [line.state, line.word]), [["running", null]]);
});

test("a wait that names nothing for sure is a row of its own, by its description; /dev/null links nothing", () => {
  const wait = task("w", "Wait for the benchmark process to exit");
  const other = task("o", "Quiet run");
  const view = checkLines([wait, other], [bash("w", "while kill -0 25996 2>/dev/null; do sleep 20; done; tail -30 /tmp/bench.log"), bash("o", "pnpm quiet 2>/dev/null")], NOW);
  assert.deepEqual(view.lines.map((line) => [line.name, line.state, line.waitedOn]), [["Wait for the benchmark process to exit", "running", false], ["Quiet run", "running", false]]);
  assert.equal(view.waitingOn, "the benchmark process to exit");
});

test("a wait on two commands' output is no fold: never a guess", () => {
  const a = task("a", "First"), b = task("b", "Second"), w = task("w", "Wait for both");
  const view = checkLines([a, b, w], [bash("w", "until test -s /tmp/cli/tasks/a.output -a -s /tmp/cli/tasks/b.output; do sleep 5; done")], NOW);
  assert.deepEqual(view.lines.map((line) => line.id), ["a", "b", "w"]);
});

test("finished rows leave immediately while worker waits remain live", () => {
  const ended = task("t", "Tests", { status: "completed", endedAtMs: NOW, summary: "exit code 0" });
  const view = checkLines([ended, task("bw", "Workers")], [bash("bw", "brigadier wait --any"), bash("t", "pnpm test; echo done")], NOW);
  assert.deepEqual(view.lines.map((line) => line.id), ["bw"]);
  assert.equal(checkWords(view), "Waiting on Workers");
  assert.deepEqual(checkLines([ended], [], NOW).lines, []);
  assert.equal(STRIP_TAIL_MS, 0);
});

test("suffix paths, mentions, writing continuations and substitutions never fold or couple Stop", () => {
  const build = task("b", "Build"), wait = task("w", "Wait");
  for (const command of [
    "until grep -q ready /tmp/build.log.backup; do sleep 1; done",
    "until grep -q ready /tmp/build.log; do sleep 1; done; cat /tmp/build.log > /tmp/copied",
    "until grep -q ready /tmp/build.log; do sleep 1; done; echo $(touch /tmp/file)",
    "until echo /tmp/build.log; do sleep 1; done",
  ]) {
    const view = checkLines([build, wait], [bash("b", "cargo build > /tmp/build.log"), bash("w", command)], NOW);
    assert.deepEqual(view.lines.map((line) => line.id), ["b", "w"], command);
    assert.deepEqual(view.lines[0]?.stops, ["b"], command);
    assert.equal(view.lines[0]?.waitedOn, false, command);
  }
  assert.equal(checkLines([build, wait], [bash("b", "echo '> /tmp/build.log'"), bash("w", "until grep -q ready /tmp/build.log; do sleep 1; done")], NOW).lines.length, 2);
});

test("relative paths need an observed cwd, and ambiguous producers never link", () => {
  const build = task("b", "Build"), wait = task("w", "Wait");
  const command = "until grep -q ready ./build.log; do sleep 1; done";
  assert.equal(checkLines([build, wait], [bash("b", "cargo build > build.log"), bash("w", command)], NOW).lines.length, 2);

  assert.deepEqual(checkLines([build, wait], [observed("b", "cargo build > build.log"), observed("w", command)], NOW).lines.map((line) => line.id), ["b"]);
  assert.equal(checkLines([build, task("c", "Other writer"), wait], [observed("b", "cargo build > build.log"), observed("c", "echo other > build.log"), observed("w", command)], NOW).lines.length, 3);
});

// A word that looks like a writer is not proof that a shell invoked one.
test("literal tee arguments, comments and tee input redirects do not prove an output writer", () => {
  const build = task("b", "Build"), wait = task("w", "Wait");
  for (const producer of ["echo tee /tmp/build.log", "cargo build # > /tmp/build.log", "cat < /tmp/build.log", "echo x | tee < /tmp/build.log"]) {
    const view = checkLines([build, wait], [bash("b", producer), bash("w", "until grep -q ready /tmp/build.log; do sleep 1; done")], NOW);
    assert.deepEqual(view.lines.map((line) => line.id), ["b", "w"], producer);
    assert.equal(view.lines[0]?.waitedOn, false, producer);
  }
});
