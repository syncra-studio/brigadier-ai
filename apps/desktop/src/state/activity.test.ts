import assert from "node:assert/strict";
import { test } from "node:test";

import type { EventEnvelope, WorkerRow } from "@/ipc/generated";
import { applyActivityEvents, applyActivityNews, fromSnapshot, rowActivity, stoppedOf, useActivity, workHolds } from "@/state/activity";
import { applyToBoard, emptyBoard } from "@/state/board";
import { rowMark, tabState } from "@/state/tabStates";
import { turnState } from "@/app/conversation/cliBlocks";

const on = (conversationId: string, event: unknown): EventEnvelope =>
  ({ stream: `conversation:${conversationId}`, event }) as EventEnvelope;

const row = (conversationId: string) => rowActivity(useActivity.getState().byConversation[conversationId]);

test("the CLI's turn opening and ending decides Working; Brigadier handing it a message works too", () => {
  applyActivityEvents([on("c2", { type: "runStateChanged", conversationId: "c2", state: "starting" })]);
  assert.equal(row("c2").running, true);
  applyActivityEvents([on("c2", { type: "runStateChanged", conversationId: "c2", state: "running" })]);
  assert.equal(row("c2").running, true, "a run going on between turns (a handover) works, as in its thread (D4)");
  applyActivityNews({ conversationId: "c2", entries: [], opened: true });
  assert.equal(row("c2").running, true);
  applyActivityNews({ conversationId: "c2", entries: [], ended: "done" });
  applyActivityEvents([on("c2", { type: "runStateChanged", conversationId: "c2", state: "idle" })]);
  assert.deepEqual(row("c2"), { running: false, awaiting: null, workers: 0 });
});

const worker = (state: string) =>
  on("c3", { type: "taskUpdated", task: { id: "t1", conversationId: "c3", kind: "write", state, title: "Add a test" } });

test("a task state alone does not prove a worker CLI works; its count still shows", () => {
  applyActivityEvents([worker("running")]);
  assert.deepEqual(row("c3"), { running: false, awaiting: null, workers: 0 }, "a quiet row");
  applyActivityNews({ conversationId: "c3", entries: [], opened: true });
  assert.deepEqual(row("c3"), { running: true, awaiting: null, workers: 1 });
  applyActivityEvents([worker("done")]);
  assert.equal(row("c3").workers, 0);
});

const servers = (taskId: string | null, list: { name: string; status: string; managed?: boolean }[]) =>
  on("c4", { type: "mcpServersUpdated", conversationId: "c4", taskId, provider: "claude", atStart: false, boot: "b",
    servers: list.map((server) => ({ ...server, tools: [], transport: null })) });

test("an MCP server of the session's own CLI that wants a sign-in needs the user; one managed elsewhere or a worker's doesn't", () => {
  applyActivityEvents([servers(null, [{ name: "Gmail", status: "needs-auth", managed: true }, { name: "github", status: "connected" }])]);
  assert.equal(row("c4").awaiting, null);
  applyActivityEvents([servers("t1", [{ name: "linear", status: "needs-auth" }])]);
  assert.equal(row("c4").awaiting, null, "a worker's server");
  applyActivityEvents([servers(null, [{ name: "linear", status: "needs-auth" }])]);
  assert.equal(row("c4").awaiting, "signIn");
  assert.deepEqual(rowMark(row("c4")), { look: "dot", amber: true, hover: "Needs you · sign-in" });
  applyActivityEvents([servers(null, [{ name: "linear", status: "connected" }])]);
  assert.equal(row("c4").awaiting, null);
});

const card = (id: string, taskId: string | null, agent?: string, conversationId = "c5") =>
  on(conversationId, { type: "approvalUpdated", approval: { id, conversationId, taskId, requestId: null, position: 0, state: { type: "pending" },
    createdAtMs: 0, resolvedAtMs: null, subject: { type: "cli", request: { id, kind: "command", tool: "Bash", command: "touch /x", cwd: null, paths: [], reason: null, escalation: false, input: null, ...(agent && { agent }) } } } });

test("a worker CLI's permission prompt never needs the user: its orchestrator answers it (D48); the session's own does", () => {
  applyActivityEvents([card("w", "t1")]);
  assert.equal(row("c5").awaiting, null);
  const board = applyToBoard(emptyBoard("c5"), card("w", "t1"));
  assert.deepEqual(Object.keys(board.approvals), [], "no card for the user");
  applyActivityEvents([card("own", null)]);
  assert.equal(row("c5").awaiting, "approval");
  assert.deepEqual(Object.keys(applyToBoard(board, card("own", null)).approvals), ["own"]);
});

test("the session CLI's own subagent's permission request is the user's card, as its own is (D166)", () => {
  applyActivityEvents([card("sub", null, "toolu_agent", "c6")]);
  assert.equal(row("c6").awaiting, "approval");
  assert.deepEqual(Object.keys(applyToBoard(emptyBoard("c6"), card("sub", null, "toolu_agent", "c6")).approvals), ["sub"]);
});

const stopped = () => stoppedOf(useActivity.getState().byConversation.stopped);

test("a stopped turn is named through reconnect and clears when another turn opens", () => {
  const snapshot = fromSnapshot({ conversationId: "stopped", run: "idle", tasks: [], approvals: [], questions: [], stopped: true });
  useActivity.setState({ byConversation: { stopped: snapshot } });
  assert.equal(stopped(), true);
  applyActivityEvents([on("stopped", { type: "runStateChanged", conversationId: "stopped", state: "starting" })]);
  assert.equal(stopped(), false, "Brigadier hands the CLI a message");
  applyActivityNews({ conversationId: "stopped", entries: [], opened: true });
  applyActivityNews({ conversationId: "stopped", entries: [], ended: "stopped" });
  applyActivityEvents([on("stopped", { type: "runStateChanged", conversationId: "stopped", state: "idle" })]);
  assert.equal(stopped(), true);
  applyActivityNews({ conversationId: "stopped", entries: [], opened: true });
  assert.equal(stopped(), false);
});

test("a failed or cut-off last turn reads Failed until the next turn opens; a failed run too", () => {
  applyActivityNews({ conversationId: "c5", entries: [], opened: true });
  applyActivityNews({ conversationId: "c5", entries: [], ended: "interrupted" });
  assert.deepEqual(rowMark(row("c5")), { look: "error", amber: true, hover: "Failed" });
  applyActivityNews({ conversationId: "c5", entries: [], opened: true });
  assert.equal(row("c5").awaiting, null);
  applyActivityNews({ conversationId: "c5", entries: [], ended: "done" });
  applyActivityEvents([on("c5", { type: "runStateChanged", conversationId: "c5", state: "failed" })]);
  assert.equal(row("c5").awaiting, "failed");
  const reloaded = fromSnapshot({ conversationId: "c6", run: "idle", tasks: [], approvals: [], questions: [], failed: true });
  assert.equal(rowActivity(reloaded).awaiting, "failed", "a failed last turn through a reconnect");
});


test("a turn a limit cut while Brigadier switches it to another account or CLI never reads Failed", () => {
  applyActivityEvents([on("c7", { type: "runStateChanged", conversationId: "c7", state: "running" })]);
  applyActivityNews({ conversationId: "c7", entries: [], opened: true });
  applyActivityNews({ conversationId: "c7", entries: [], ended: "failed" });
  assert.equal(tabState(row("c7"), false), "working", "the switch holds the run: it spins, as in its thread (D4)");
  applyActivityEvents([on("c7", { type: "runStateChanged", conversationId: "c7", state: "starting" })]);
  assert.equal(tabState(row("c7"), false), "working");
  applyActivityEvents([on("c7", { type: "runStateChanged", conversationId: "c7", state: "idle" })]);
  assert.equal(tabState(row("c7"), false), "failed", "nowhere to go: the run lets go, failed");
});


test("D180: a turn a limit cut whose work goes on elsewhere stays working in the thread, the row and the tab", () => {
  applyActivityNews({ conversationId: "c8", entries: [], opened: true });
  applyActivityNews({ conversationId: "c8", entries: [], ended: "handedOver" });
  applyActivityEvents([on("c8", { type: "runStateChanged", conversationId: "c8", state: "idle" })]);
  assert.equal(tabState(row("c8"), false), "working", "even through a run that reads idle mid-switch");
  assert.deepEqual(turnState({ end: "handedOver", error: "Claude's usage limit was reached." }), { state: "working", error: null }, "no Stopped header, no Try again line");
  applyActivityNews({ conversationId: "c8", entries: [], ended: "failed" });
  assert.equal(tabState(row("c8"), false), "failed", "nothing to switch to: it failed");
});


const workerRow = (working: boolean): WorkerRow => ({
  id: "worker", number: 1, title: "Worker", agent: "codex", model: null, startedAtMs: 1,
  working, lastEnd: working ? null : "done", gone: false, lastEventAtMs: 1, limit: null,
  freshContext: false, waiting: [], mcp: [], branch: null, base: null, diff: null, ahead: 0, merged: false,
});

test("unopened sessions fold worker news directly into activity, keep Working until all workers stop, and clear it on an empty list", () => {
  useActivity.setState({ byConversation: {} });
  const id = "unopened";
  applyActivityNews({ conversationId: id, entries: [], workers: [workerRow(true), { ...workerRow(false), id: "other" }] });
  assert.equal(row(id).running, true);
  assert.equal(tabState(row(id), false, false, true), "working");
  assert.deepEqual(rowMark(row(id), { unread: true }), { look: "working", hover: "Working" });
  applyActivityNews({ conversationId: id, entries: [], opened: true });
  applyActivityNews({ conversationId: id, entries: [], ended: "done" });
  assert.equal(row(id).running, true, "the session ending its turn does not end its workers");
  applyActivityNews({ conversationId: id, entries: [], workers: [workerRow(false)] });
  assert.equal(tabState(row(id), false, false, true), "done");
  applyActivityNews({ conversationId: id, entries: [], workers: [workerRow(true)] });
  applyActivityNews({ conversationId: id, entries: [], workers: [] });
  assert.deepEqual(rowMark(row(id)), { look: null, hover: null });
});

test("the reconnect snapshot restores live worker activity without a thread view, with waiting cards taking precedence", () => {
  const snapshot = fromSnapshot({ conversationId: "snapshot", run: "idle", tasks: [], approvals: [], questions: [], workersWorking: true });
  useActivity.setState({ byConversation: { snapshot } });
  assert.equal(row("snapshot").running, true);
  assert.equal(tabState(row("snapshot"), false, false, true), "working");
  assert.deepEqual(rowMark(rowActivity({ ...snapshot, questions: new Set(["q"]) })), { look: "dot", amber: true, hover: "Needs you · question" });
  assert.deepEqual(rowMark(rowActivity({ ...snapshot, approvals: new Set(["a"]) })), { look: "dot", amber: true, hover: "Needs you · approval" });
  // A sign-in doesn't hold the turn: while it works, it spins.
  assert.deepEqual(rowMark(rowActivity({ ...snapshot, signIns: new Set(["mcp"]) })), { look: "working", hover: "Working" });
  assert.equal(rowActivity({ ...snapshot, workersWorking: false }).running, false);
});


test("a turn that failed while its work runs reads Working until the block closes, then Failed", () => {
  const working = { look: "working", hover: "Working" };
  const failed = { look: "error", hover: "Failed" };
  const id = "failed-held";
  applyActivityNews({ conversationId: id, entries: [], opened: true, workers: [workerRow(true)] });
  applyActivityNews({ conversationId: id, entries: [], ended: "failed" });
  assert.deepEqual(rowMark(row(id)), working, "a worker holds it");
  // Its word's wake turn comes and goes: no red between them.
  applyActivityNews({ conversationId: id, entries: [], workers: [workerRow(false)], jobs: { waking: true } });
  assert.deepEqual(rowMark(row(id)), working, "the turn the worker's end owes holds it");
  applyActivityNews({ conversationId: id, entries: [], jobs: { holding: ["ci"] } });
  assert.deepEqual(rowMark(row(id)), working, "a background job holds it");
  applyActivityNews({ conversationId: id, entries: [], jobs: {} });
  assert.deepEqual(rowMark(row(id)), failed, "closed: its mark as before");

  const held = (tasks: [string, "starting"][]) => fromSnapshot({ conversationId: id, run: "failed", tasks, approvals: [], questions: [] });
  assert.deepEqual(rowMark(rowActivity(held([["t", "starting"]]))), working, "a worker not yet at its CLI holds it too, and a failed run");
  assert.deepEqual(rowMark(rowActivity(held([]))), failed, "nothing runs: red at once");
  assert.deepEqual(rowMark(rowActivity({ ...held([["t", "starting"]]), approvals: new Set(["a"]) })), { look: "dot", hover: "Needs you · approval" });
});

test("turn-end timestamps survive snapshots and worker-only news, independent of finished requests", () => {
  const snapshot = fromSnapshot({ conversationId: "done", run: "idle", tasks: [], approvals: [], questions: [], turnEndedAtMs: 123 });
  useActivity.setState({ byConversation: { done: snapshot } });
  assert.equal(snapshot.turnEndedAtMs, 123);
  assert.equal(tabState(row("done"), false, false, snapshot.turnEndedAtMs !== undefined), "done");
  applyActivityNews({ conversationId: "done", entries: [], workers: [workerRow(false)] });
  assert.equal(useActivity.getState().byConversation.done?.turnEndedAtMs, 123);
  applyActivityNews({ conversationId: "done", entries: [], opened: true });
  assert.equal(row("done").running, true);
  applyActivityNews({ conversationId: "done", entries: [], ended: "done", turnEndedAtMs: 456 });
  assert.equal(useActivity.getState().byConversation.done?.turnEndedAtMs, 456);
});

test("a session stays Working while its CLI's background commands hold its turn, and while one's end owes it a turn (D21)", () => {
  const id = "watching";
  applyActivityNews({ conversationId: id, entries: [], opened: true });
  applyActivityNews({ conversationId: id, entries: [], jobs: { holding: ["bwatch12"] } });
  applyActivityNews({ conversationId: id, entries: [], ended: "done" });
  assert.equal(row(id).running, true, "its turn ended; the watcher it started still runs");
  assert.equal(tabState(row(id), false, false, true), "working");
  // The watcher ended: its end owes the turn it starts.
  applyActivityNews({ conversationId: id, entries: [], jobs: { waking: true } });
  assert.equal(row(id).running, true);
  // That turn starts, then ends.
  applyActivityNews({ conversationId: id, entries: [], opened: true, jobs: {} });
  assert.equal(row(id).running, true);
  applyActivityNews({ conversationId: id, entries: [], ended: "done" });
  assert.equal(row(id).running, false);
  assert.equal(tabState(row(id), false, false, true), "done");
  // A reconnect's snapshot says it too.
  const snapshot = fromSnapshot({ conversationId: id, run: "idle", tasks: [], approvals: [], questions: [], commandsWorking: true });
  assert.equal(rowActivity(snapshot).running, true);
  assert.equal(rowActivity({ ...snapshot, commandsWorking: false }).running, false);
});

test("a background subagent of its CLI holds its work past its turn: what is sent meanwhile waits in its block (D113)", () => {
  const id = "subagent";
  applyActivityNews({ conversationId: id, entries: [], opened: true });
  applyActivityNews({ conversationId: id, entries: [], ended: "done", jobs: { subagents: true } });
  assert.equal(workHolds(useActivity.getState().byConversation[id]), true, "the send path queues it, as the daemon does");
  applyActivityNews({ conversationId: id, entries: [], jobs: {} });
  assert.equal(workHolds(useActivity.getState().byConversation[id]), false);
});

test("a session's work holds with no turn running while one of its workers or its CLI's background commands still work", () => {
  const idle = fromSnapshot({ conversationId: "h1", run: "idle", tasks: [], approvals: [], questions: [], signIns: [] } as never);
  assert.equal(workHolds(idle), false);
  assert.equal(workHolds(undefined), false);
  assert.equal(workHolds({ ...idle, tasks: { t1: "running" } }), true);
  assert.equal(workHolds({ ...idle, tasks: { t1: "reported" } }), false, "a worker that reported holds nothing");
  assert.equal(workHolds({ ...idle, workersWorking: true }), true);
  assert.equal(workHolds({ ...idle, commandsWorking: true }), true);
});

test("a session put to sleep closes its turn and shows no spinner (D10)", () => {
  applyActivityNews({ conversationId: "c9", entries: [], opened: true });
  applyActivityEvents([on("c9", { type: "runStateChanged", conversationId: "c9", state: "hibernated" })]);
  assert.equal(useActivity.getState().byConversation.c9?.turnOpen, false);
  assert.deepEqual(row("c9"), { running: false, awaiting: null, workers: 0 });
});
