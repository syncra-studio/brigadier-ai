import assert from "node:assert/strict";
import { test } from "node:test";

import { commandsWaitWords, headerLabel } from "@/app/conversation/requestTime";
import { turnCommands } from "@/app/conversation/turnCommands";
import type { BackgroundTask, FileEntry, ThreadView } from "@/ipc/generated";

const task = (id: string, itemId: string | null, startedAtMs: number, more: Partial<BackgroundTask> = {}): BackgroundTask => ({
  id, itemId, kind: "shell", label: id, status: "running", startedAtMs, endedAtMs: null, outputFile: null, summary: null, lastLine: null, ...more,
});
const bash = (itemId: string, command: string, atMs: number): FileEntry => ({
  event: { type: "toolCall", itemId, name: "Bash", input: JSON.stringify({ command, run_in_background: true }), status: "completed", output: null },
  atMs,
});
const codex = (itemId: string, command: string, atMs: number): FileEntry => ({
  event: { type: "command", itemId, command, cwd: null, status: "completed", exitCode: null, output: null, durationMs: null },
  atMs,
});

const turns = [{ startedAtMs: 1000 }, { startedAtMs: 5000 }];

test("a background command holds the block whose turn started it, by its call, not by when it was seen (D21)", () => {
  const view: Pick<ThreadView, "background" | "entries" | "jobs"> = {
    entries: [bash("toolu_watch", "gh run watch 12 --exit-status", 1200), codex("item_ci", "gh pr checks 7 --watch", 1300)],
    // Codex lists its terminal only when it polls: seen after the next turn began.
    background: [task("bwatch", "toolu_watch", 1250), task("proc-9", "item_ci", 5200, { kind: "terminal", label: "gh pr checks 7 --watch" })],
    jobs: { holding: ["bwatch", "proc-9"] },
  };
  assert.equal(turnCommands(view, turns, [1000]), 2, "both started in the first turn");
  assert.equal(turnCommands(view, turns, [5000]), 0, "the later turn started neither");
  // A task with no call in the thread falls back to when it started.
  assert.equal(turnCommands({ ...view, entries: [] }, turns, [5000]), 1);
  // Only what the engine says holds: a wait, a preview call, a subagent never do.
  assert.equal(turnCommands({ ...view, jobs: { holding: ["bwatch"] } }, turns, [1000]), 1);
  assert.equal(turnCommands({ ...view, jobs: {} }, turns, [1000]), 0);
  assert.equal(turnCommands(view, turns, []), 0, "a live turn isn't held");
});

test("a command that ended holds the newest block that started it until the turn its end starts begins (D21)", () => {
  const ended = task("bwatch", "toolu_watch", 5250, { status: "completed", endedAtMs: 9000 });
  const waiting = task("bwait", "toolu_wait", 5260, { status: "completed", endedAtMs: 9000 });
  const entries = [bash("toolu_watch", "gh run watch 12", 5200), bash("toolu_wait", "brigadier start fix && brigadier wait", 5210)];
  assert.equal(turnCommands({ entries, background: [ended], jobs: { waking: true } }, turns, [5000]), 1);
  assert.equal(turnCommands({ entries, background: [ended], jobs: { waking: true } }, turns, [1000]), 0, "an older block isn't opened again");
  assert.equal(turnCommands({ entries, background: [ended], jobs: {} }, turns, [5000]), 0, "its turn began, or the grace passed");
  // A wait's end is the workers' hold, not a command's.
  assert.equal(turnCommands({ entries, background: [waiting], jobs: { waking: true } }, turns, [5000]), 0);
});

test("a held turn's header says it waits for its commands, its workers first", () => {
  assert.equal(commandsWaitWords(1), "Waiting for a command");
  assert.equal(commandsWaitWords(2), "Waiting for 2 commands");
  assert.equal(headerLabel("done", 240_000, false, false, 0, 1), "Waiting for a command · 4m 0s");
  assert.equal(headerLabel("done", 120_000, false, false, 1, 2), "Waiting for a worker · 2m 0s");
});
