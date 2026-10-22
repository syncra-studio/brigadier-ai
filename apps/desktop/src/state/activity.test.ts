import assert from "node:assert/strict";
import { test } from "node:test";

import type { EventEnvelope } from "@/ipc/generated";
import { applyActivityEvents, applyActivityNews, fromSnapshot, rowActivity, stoppedOf, useActivity } from "@/state/activity";
import { sessionStatus } from "@/state/tabStates";

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

test("a worker alone is not work; its count still shows", () => {
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
  assert.deepEqual(sessionStatus(row("c4"), null, undefined), { state: "needsYou", reason: "sign-in" });
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
  assert.deepEqual(sessionStatus(row("c5"), null, undefined), { state: "failed", reason: null });
  applyActivityNews({ conversationId: "c5", entries: [], opened: true });
  assert.equal(row("c5").awaiting, null);
  applyActivityNews({ conversationId: "c5", entries: [], ended: "done" });
  applyActivityEvents([on("c5", { type: "runStateChanged", conversationId: "c5", state: "failed" })]);
  assert.equal(row("c5").awaiting, "failed");
  const reloaded = fromSnapshot({ conversationId: "c6", run: "idle", tasks: [], approvals: [], questions: [], failed: true });
  assert.equal(rowActivity(reloaded).awaiting, "failed", "a failed last turn through a reconnect");
});
