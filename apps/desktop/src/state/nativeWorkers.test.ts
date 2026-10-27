import assert from "node:assert/strict";
import { test } from "node:test";

import type { FileEntry, ThreadTurn, ThreadView, WorkerRow } from "@/ipc/generated";
import { workersOf } from "@/state/nativeWorkers";
import { cliBlocks } from "@/app/conversation/cliBlocks";
import { subagentsStrip } from "@/app/conversation/activity/stripView";

const agentCall = (itemId: string, atMs: number): FileEntry => ({
  event: { type: "toolCall", itemId, name: "Agent", input: JSON.stringify({ description: itemId, prompt: "Look" }), status: "inProgress", output: null },
  atMs,
});
const turn = (userMessage: string, startedAtMs: number, end: ThreadTurn["end"]): ThreadTurn => ({ userMessage, startedAtMs, endedAtMs: end ? startedAtMs + 50 : null, end, error: null, requests: [] });
const user = (itemId: string, atMs: number): FileEntry => ({ event: { type: "message", itemId, role: "user", text: itemId }, atMs });

const read = (itemId: string, status: "inProgress" | "completed"): FileEntry => ({
  event: { type: "toolCall", itemId, name: "Read", input: JSON.stringify({ file_path: `/w/${itemId}.ts` }), status, output: null },
  atMs: 2020, parent: "live",
});

test("subagent lifecycle calls leave the main thread while their source, strip and child work remain", () => {
  for (const name of ["Agent", "Task", "agents/spawnAgent", "spawn_agent"]) {
    const call = agentCall("live", 2010);
    assert.ok(call.event.type === "toolCall");
    call.event.name = name;
    const child = read("child", "completed");
    const view: ThreadView = { entries: [user("u2", 2000), call, child], turns: [turn("u2", 2000, null)] };
    const rows = workersOf(`strip-only-${name}`, view);
    assert.equal(rows.length, 1, name);
    assert.equal(rows[0]?.title, "live", name);
    assert.deepEqual(subagentsStrip(rows, 2030)?.rows, [rows[0]?.id], name);
    assert.deepEqual(cliBlocks(view, "c").flatMap((block) => block.orchestratorSteps), [], name);
    assert.equal(view.entries[1], call);
    assert.equal(view.entries[2], child);
    assert.deepEqual(rows[0]?.doing, { type: "call", step: { name: "Read", detail: "/w/child.ts" } });
  }
});

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
