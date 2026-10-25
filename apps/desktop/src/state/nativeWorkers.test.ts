import assert from "node:assert/strict";
import { test } from "node:test";

import type { FileEntry, ThreadTurn, ThreadView, WorkerRow } from "@/ipc/generated";
import { workersOf } from "@/state/nativeWorkers";

const agentCall = (itemId: string, atMs: number): FileEntry => ({
  event: { type: "toolCall", itemId, name: "Agent", input: JSON.stringify({ description: itemId, prompt: "Look" }), status: "inProgress", output: null },
  atMs,
});
const turn = (userMessage: string, startedAtMs: number, end: ThreadTurn["end"]): ThreadTurn => ({ userMessage, startedAtMs, endedAtMs: end ? startedAtMs + 50 : null, end, error: null, requests: [] });
const user = (itemId: string, atMs: number): FileEntry => ({ event: { type: "message", itemId, role: "user", text: itemId }, atMs });

test("an agent call its turn left unfinished is over; one in the running turn works", () => {
  const view: ThreadView = {
    entries: [user("u1", 1000), agentCall("old", 1010), user("u2", 2000), agentCall("live", 2010)],
    turns: [turn("u1", 1000, "interrupted"), turn("u2", 2000, null)],
  };
  const rows = workersOf("c-ended", view);
  assert.deepEqual(rows.map((row) => [row.title, row.working]), [["live", true], ["old", false]]);
  const over = workersOf("c-ended-2", { ...view, turns: [turn("u1", 1000, "interrupted"), turn("u2", 2000, "stopped")] });
  assert.deepEqual(over.map((row) => row.working), [false, false]);
});

test("native agents are numbered after the engine's workers, gaps and all", () => {
  const worker = { id: "w7", number: 7, startedAtMs: 1 } as unknown as WorkerRow;
  const rows = workersOf("c-numbers", { entries: [user("u1", 1000), agentCall("a", 1010)], turns: [turn("u1", 1000, null)], workers: [worker] });
  assert.deepEqual(rows.map((row) => row.number).toSorted(), [7, 8]);
});

const read = (itemId: string, status: "inProgress" | "completed"): FileEntry => ({
  event: { type: "toolCall", itemId, name: "Read", input: JSON.stringify({ file_path: `/w/${itemId}.ts` }), status, output: null },
  atMs: 2020, parent: "live",
});

test("a working agent's row lists its calls at work, from its own items", () => {
  const rows = workersOf("c-steps", { entries: [user("u2", 2000), agentCall("live", 2010), read("a", "completed"), read("b", "inProgress")], turns: [turn("u2", 2000, null)] });
  assert.deepEqual(rows[0]?.steps, [{ name: "Read", detail: "/w/b.ts" }]);
  const over = workersOf("c-steps-2", { entries: [user("u2", 2000), agentCall("live", 2010), read("b", "inProgress")], turns: [turn("u2", 2000, "stopped")] });
  assert.equal(over[0]?.steps, undefined);
});

test("between its calls a working agent says what it last did, from its own items (D39)", () => {
  const said: FileEntry = { event: { type: "message", itemId: "m", role: "assistant", text: "Found the config." }, atMs: 2030, parent: "live" };
  const rows = workersOf("c-doing", { entries: [user("u2", 2000), agentCall("live", 2010), read("a", "completed")], turns: [turn("u2", 2000, null)] });
  assert.deepEqual(rows[0]?.doing, { type: "call", step: { name: "Read", detail: "/w/a.ts" } });
  const after = workersOf("c-doing-2", { entries: [user("u2", 2000), agentCall("live", 2010), read("a", "completed"), said], turns: [turn("u2", 2000, null)] });
  assert.deepEqual(after[0]?.doing, { type: "said", line: "Found the config." });
  const over = workersOf("c-doing-3", { entries: [user("u2", 2000), agentCall("live", 2010), said], turns: [turn("u2", 2000, "stopped")] });
  assert.equal(over[0]?.doing, undefined);
});
