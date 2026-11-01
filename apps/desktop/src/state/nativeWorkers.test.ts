import assert from "node:assert/strict";
import { test } from "node:test";

import type { FileEntry, ThreadTurn, ThreadView } from "@/ipc/generated";
import { backgroundOf, runningSubagents, subagentPanel, subagentPanelOf, subagentRow } from "@/state/nativeWorkers";
import { cliBlocks } from "@/app/conversation/cliBlocks";

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

test("a subagent is one row of its parent's work, on every CLI, its own items not drawn; it is no worker (D96)", () => {
  for (const name of ["Agent", "Task", "agents/spawnAgent", "spawn_agent"]) {
    const call = agentCall("live", 2010);
    assert.ok(call.event.type === "toolCall");
    call.event.name = name;
    const view: ThreadView = { entries: [user("u2", 2000), call, read("child", "completed")], turns: [turn("u2", 2000, null)] };
    const steps = cliBlocks(view, "c").flatMap((block) => block.orchestratorSteps);
    assert.deepEqual(steps.map((step) => step.kind.type === "tool" && [step.kind.itemId, step.kind.status]), [["live", "inProgress"]], name);
    assert.equal(subagentRow("c", "c", view, "live")?.title, "live", name);
  }
});

test("a subagent's panel is named by whose thread holds its call", () => {
  assert.deepEqual(subagentPanelOf(subagentPanel("task-3", "call:1")), { owner: "task-3", callId: "call:1" });
  assert.equal(subagentPanelOf("task-3"), null);
});

test("an agent call its turn left unfinished is over; one in the running turn works and holds its block", () => {
  const view: ThreadView = {
    entries: [user("u1", 1000), agentCall("old", 1010), user("u2", 2000), agentCall("live", 2010)],
    turns: [turn("u1", 1000, "interrupted"), turn("u2", 2000, null)],
  };
  assert.deepEqual(["live", "old"].map((id) => subagentRow("c-ended", "c-ended", view, id)?.working), [true, false]);
  assert.equal(runningSubagents(view, 2000, null), 1);
  assert.equal(runningSubagents(view, 1000, 1050), 0);
  const over = { ...view, turns: [turn("u1", 1000, "interrupted"), turn("u2", 2000, "stopped")] };
  assert.equal(runningSubagents(over, 0, null), 0);
});

test("a background agent that ended after its lead's turn holds its block until the turn its end owes starts (D24)", () => {
  const task = { id: "a1", itemId: "bg", kind: "agent", label: "bg", status: "completed", startedAtMs: 1020, endedAtMs: 3000, provider: "claude" } as never;
  const ended = turn("u1", 1000, "done");
  const view = { entries: [user("u1", 1000), agentCall("bg", 1010)], turns: [ended], background: [task] };
  assert.equal(runningSubagents({ ...view, jobs: { holding: [], waking: true } } as never, 1000, 1050), 1);
  assert.equal(runningSubagents(view as never, 1000, 1050), 0, "no turn owed: done");
  const starting = { ...view, turns: [ended, turn("", 3100, null)], jobs: { holding: [], waking: true } };
  assert.equal(runningSubagents(starting as never, 1000, 1050), 1, "its turn listed, not yet its own: still owed");
  const woken = { ...starting, jobs: { holding: [], waking: false } };
  assert.equal(runningSubagents(woken as never, 1000, 1050), 0, "its turn started: done");
  const running = { ...view, background: [{ ...(task as object), status: "running", endedAtMs: null }] };
  assert.equal(runningSubagents(running as never, 1000, 1050), 1, "running in the background: holds");
});

test("a subagent call its CLI lists no task for that outlives its turn's own end ran in the background, on every CLI (D97)", () => {
  // Turn 1000–1050 ended done; its spawn, made at 1010, still runs.
  const spawn = agentCall("bg", 1010);
  assert.ok(spawn.event.type === "toolCall");
  spawn.event.name = "agents/spawnAgent";
  const view: ThreadView = { entries: [user("u1", 1000), spawn], turns: [turn("u1", 1000, "done")] };
  assert.deepEqual(backgroundOf(view).map((task) => [task.itemId, task.kind, task.status, task.endedAtMs]), [["bg", "agent", "running", null]]);
  assert.equal(runningSubagents(view, 1000, 1050), 1, "it holds its block (D24)");
  // It ends after its turn: still listed, as over (D98).
  const ended = { ...view, entries: [user("u1", 1000), { ...spawn, event: { ...spawn.event, status: "completed" as const }, updatedAtMs: 1900 }] };
  assert.deepEqual(backgroundOf(ended).map((task) => [task.status, task.endedAtMs]), [["completed", 1900]]);
  assert.equal(runningSubagents(ended, 1000, 1050), 0);
  // One its turn waited for, or one a stopped turn left unfinished, never ran in the background.
  assert.deepEqual(backgroundOf({ ...ended, entries: [user("u1", 1000), { ...ended.entries[1]!, updatedAtMs: 1040 }] }), []);
  assert.deepEqual(backgroundOf({ ...view, turns: [turn("u1", 1000, "stopped")] }), []);
  // One its CLI lists a task for is that task.
  const listed = { id: "t1", itemId: "bg", kind: "agent", label: "bg", status: "running", startedAtMs: 1020, endedAtMs: null, outputFile: null, summary: null, lastLine: null } as const;
  assert.deepEqual(backgroundOf({ ...view, background: [listed] }), [listed]);
});
