import assert from "node:assert/strict";
import { test } from "node:test";

import type { EventEnvelope, WorkerRow } from "@/ipc/generated";
import { applyActivityEvents, applyActivityNews, fromSnapshot, rowActivity, stoppedOf, useActivity, workHolds } from "@/state/activity";
import { sessionStatus, sidebarIndicators, tabState } from "@/state/tabStates";

const on = (conversationId: string, event: unknown): EventEnvelope =>
  ({ stream: `conversation:${conversationId}`, event }) as EventEnvelope;

const row = (conversationId: string) => rowActivity(useActivity.getState().byConversation[conversationId]);

test("the CLI's turn opening and ending decides Working; Brigadier handing it a message works too", () => {
  applyActivityEvents([on("c2", { type: "runStateChanged", conversationId: "c2", state: "starting" })]);
  assert.equal(row("c2").running, true);
  applyActivityEvents([on("c2", { type: "runStateChanged", conversationId: "c2", state: "running" })]);
  assert.equal(row("c2").running, false, "a run with no turn open is not work");
  applyActivityNews({ conversationId: "c2", entries: [], opened: true });
  assert.equal(row("c2").running, true);
  applyActivityNews({ conversationId: "c2", entries: [], ended: "done" });
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

const servers = (taskId: string | null, list: { name: string; status: string }[]) =>
  on("c4", { type: "mcpServersUpdated", conversationId: "c4", taskId, provider: "claude", atStart: false, boot: "b",
    servers: list.map((server) => ({ ...server, tools: [], transport: null })) });

test("an MCP server of the session's own CLI that wants a sign-in needs the user; a claude.ai connector or a worker's doesn't", () => {
  applyActivityEvents([servers(null, [{ name: "claude.ai Gmail", status: "needs-auth" }, { name: "github", status: "connected" }])]);
  assert.equal(row("c4").awaiting, null);
  applyActivityEvents([servers("t1", [{ name: "linear", status: "needs-auth" }])]);
  assert.equal(row("c4").awaiting, null, "a worker's server");
  applyActivityEvents([servers(null, [{ name: "linear", status: "needs-auth" }])]);
  assert.equal(row("c4").awaiting, "signIn");
  assert.deepEqual(sessionStatus(row("c4"), null), { state: "needsYou", reason: "sign-in" });
  applyActivityEvents([servers(null, [{ name: "linear", status: "connected" }])]);
  assert.equal(row("c4").awaiting, null);
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
  assert.deepEqual(sessionStatus(row("c5"), null), { state: "failed", reason: null });
  applyActivityNews({ conversationId: "c5", entries: [], opened: true });
  assert.equal(row("c5").awaiting, null);
  applyActivityNews({ conversationId: "c5", entries: [], ended: "done" });
  applyActivityEvents([on("c5", { type: "runStateChanged", conversationId: "c5", state: "failed" })]);
  assert.equal(row("c5").awaiting, "failed");
  const reloaded = fromSnapshot({ conversationId: "c6", run: "idle", tasks: [], approvals: [], questions: [], failed: true });
  assert.equal(rowActivity(reloaded).awaiting, "failed", "a failed last turn through a reconnect");
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
  assert.deepEqual(sessionStatus(row(id), null, false, true), { state: "working", reason: null });
  assert.deepEqual(sidebarIndicators(row(id), false, true), { working: true, error: false, unread: false });
  applyActivityNews({ conversationId: id, entries: [], opened: true });
  applyActivityNews({ conversationId: id, entries: [], ended: "done" });
  assert.equal(row(id).running, true, "the session ending its turn does not end its workers");
  applyActivityNews({ conversationId: id, entries: [], workers: [workerRow(false)] });
  assert.equal(tabState(row(id), false, false, true), "done");
  applyActivityNews({ conversationId: id, entries: [], workers: [workerRow(true)] });
  applyActivityNews({ conversationId: id, entries: [], workers: [] });
  assert.deepEqual(sidebarIndicators(row(id), false, false), { working: false, error: false, unread: false });
});

test("the reconnect snapshot restores live worker activity without a thread view, with waiting cards taking precedence", () => {
  const snapshot = fromSnapshot({ conversationId: "snapshot", run: "idle", tasks: [], approvals: [], questions: [], workersWorking: true });
  useActivity.setState({ byConversation: { snapshot } });
  assert.equal(row("snapshot").running, true);
  assert.equal(tabState(row("snapshot"), false, false, true), "working");
  assert.deepEqual(sessionStatus(rowActivity({ ...snapshot, questions: new Set(["q"]) }), null, false, true), { state: "needsYou", reason: "question" });
  assert.deepEqual(sessionStatus(rowActivity({ ...snapshot, approvals: new Set(["a"]) }), null, false, true), { state: "needsYou", reason: "approval" });
  // A sign-in doesn't hold the turn: while it works, it reads Working.
  assert.deepEqual(sessionStatus(rowActivity({ ...snapshot, signIns: new Set(["mcp"]) }), null, false, true), { state: "working", reason: null });
  assert.equal(rowActivity({ ...snapshot, workersWorking: false }).running, false);
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

test("a session's work holds with no turn running while one of its workers or its CLI's background commands still work", () => {
  const idle = fromSnapshot({ conversationId: "h1", run: "idle", tasks: [], approvals: [], questions: [], signIns: [] } as never);
  assert.equal(workHolds(idle), false);
  assert.equal(workHolds(undefined), false);
  assert.equal(workHolds({ ...idle, tasks: { t1: "running" } }), true);
  assert.equal(workHolds({ ...idle, tasks: { t1: "reported" } }), false, "a worker that reported holds nothing");
  assert.equal(workHolds({ ...idle, workersWorking: true }), true);
  assert.equal(workHolds({ ...idle, commandsWorking: true }), true);
});
