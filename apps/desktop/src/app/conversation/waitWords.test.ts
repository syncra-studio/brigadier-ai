import assert from "node:assert/strict";
import { test } from "node:test";

import { liveHead, type StatusInput, threadStatus } from "@/app/conversation/liveStatus";
import { heldWait, liveWait } from "@/app/conversation/waitWords";
import type { BackgroundTask, FileEntry, ThreadTurn, ThreadView, WorkerRow } from "@/ipc/generated";
import { emptyBoard } from "@/state/board";

const turn = (startedAtMs: number, end: ThreadTurn["end"] = null): ThreadTurn =>
  ({ userMessage: "u", startedAtMs, endedAtMs: end ? startedAtMs + 500 : null, end, error: null, requests: [] });
const call = (itemId: string, name: string, input: object, atMs: number, status: "inProgress" | "completed" = "inProgress"): FileEntry =>
  ({ event: { type: "toolCall", itemId, name, input: JSON.stringify(input), status, output: null }, atMs });
const command = (itemId: string, text: string, atMs: number, status: "inProgress" | "completed" = "inProgress"): FileEntry =>
  ({ event: { type: "command", itemId, command: text, cwd: null, status, exitCode: null, output: null, durationMs: null }, atMs });
const task = (id: string, itemId: string, kind: BackgroundTask["kind"], label: string, more: Partial<BackgroundTask> = {}): BackgroundTask =>
  ({ id, itemId, kind, label, status: "running", startedAtMs: 1010, endedAtMs: null, outputFile: null, summary: null, lastLine: null, ...more });
const worker = (id: string, title: string, more: Partial<WorkerRow> = {}): WorkerRow => ({
  id, number: 1, title, agent: "claude", model: null, startedAtMs: 1020, working: true, lastEnd: null, gone: false, lastEventAtMs: null,
  limit: null, freshContext: false, waiting: [], mcp: [], branch: null, base: null, diff: null, ahead: 0, merged: false, ...more,
});

test("Claude blocked on its own TaskOutput names the command or subagent it waits for, never Thinking (D106)", () => {
  const view: ThreadView = {
    entries: [command("toolu_bg", "sleep 40 && python3 -c 'print(6*7)'", 1010, "completed"), call("toolu_out", "TaskOutput", { task_id: "b1", block: true }, 1100)],
    turns: [turn(1000)],
    background: [task("b1", "toolu_bg", "shell", "Sleep then print")],
  };
  assert.deepEqual(liveWait(view), { text: "Waiting for sleep 40 && python3 -c 'print(6*7)'", tone: "still" });
  const agent = { ...view, background: [task("b1", "toolu_bg", "agent", "Run the sleep")] };
  assert.deepEqual(liveWait(agent), { text: "Waiting for subagent: Run the sleep", tone: "still" });
  // Its wait over, or the turn over, it waits on nothing.
  assert.equal(liveWait({ ...view, entries: [view.entries[0]!, call("toolu_out", "TaskOutput", { task_id: "b1" }, 1100, "completed")] }), null);
  assert.equal(liveWait({ ...view, turns: [turn(1000, "done")] }), null);
});

test("Codex's agent wait names its subagent; a brigadier wait names its worker, the same on every CLI (D106)", () => {
  const spawn = call("sp", "agents/spawnAgent", { message: "Sleep 40 then print" }, 1010, "completed");
  const view: ThreadView = {
    entries: [spawn, call("w", "agents/wait", { timeout_ms: 30000 }, 1100)],
    turns: [turn(1000)],
    background: [task("sp", "sp", "agent", "background_sleep")],
  };
  assert.deepEqual(liveWait(view), { text: "Waiting for subagent: background_sleep", tone: "still" });
  const workers = [worker("0000-aaaa-823718cc", "Fetch example.com"), worker("0000-bbbb-11112222", "Print the date", { number: 2 })];
  const waiting: ThreadView = { entries: [command("c", "brigadier wait 823718cc", 1100)], turns: [turn(1000)], workers };
  assert.deepEqual(liveWait(waiting), { text: "Waiting for Fetch example.com", tone: "busy" });
  assert.deepEqual(liveWait({ ...waiting, entries: [command("c", "brigadier wait", 1100)] }), { text: "Waiting for 2 workers", tone: "busy" });
  // In a shell wrapper with a timeout, after a start (seen live on Codex): the one working worker.
  const wrapped = command("c", `/bin/zsh -c "brigadier start 'Curl it' --title 'Curl' && brigadier wait --timeout 60"`, 1100);
  assert.deepEqual(liveWait({ ...waiting, entries: [wrapped], workers: [workers[0]!] }), { text: "Waiting for Fetch example.com", tone: "busy" });
});

test("the live line says the named wait where it said Thinking, and a held block names its one holder (D106)", () => {
  const wait = { text: "Waiting for subagent: background_sleep", tone: "still" } as const;
  const status: StatusInput = { board: { ...emptyBoard("c"), run: "running", runRequest: "r1" }, requestIds: ["r1"], state: "working", thinkingLive: false, compacting: false, quotaWait: null, wait };
  assert.deepEqual(threadStatus(status).head, wait);
  assert.deepEqual(threadStatus({ ...status, wait: null }).head, { text: "Thinking", tone: "busy" });
  // Held over: one holder by name, several by count.
  const ended: ThreadView = {
    entries: [command("toolu_bg", "sleep 40 && echo done", 1010, "completed")],
    turns: [turn(1000, "done")],
    background: [task("b1", "toolu_bg", "shell", "Sleep")],
    jobs: { holding: ["b1"] },
  };
  const named = heldWait(ended, [1000], 1000, 1500);
  assert.deepEqual(named, { text: "Waiting for sleep 40 && echo done", tone: "still" });
  assert.deepEqual(liveHead(null, 0, 1, 0, named), named);
  assert.equal(heldWait({ ...ended, background: [...ended.background!, task("b2", "x", "shell", "Other")], jobs: { holding: ["b1", "b2"] } }, [1000], 1000, 1500), null);
  assert.deepEqual(liveHead(null, 0, 2, 0, null), { text: "Waiting for 2 commands", tone: "still" });
});
