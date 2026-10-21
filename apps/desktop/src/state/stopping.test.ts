import assert from "node:assert/strict";
import { beforeEach, test } from "node:test";
import { mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";
import type { Conversation, Task, TaskState } from "@/ipc/generated";

const data = new Map<string, string>();
Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
  getItem: (key: string) => data.get(key) ?? null,
  setItem: (key: string, value: string) => data.set(key, value),
  removeItem: (key: string) => data.delete(key),
} });
Object.defineProperty(globalThis, "window", { configurable: true, value: { localStorage } });
const { interrupt, restore, stopWorkers } = await import("./actions");
const { fromSnapshot, rowActivity, useActivity } = await import("./activity");
const { emptyBoard, stoppedBoard, useBoard } = await import("./board");
const { useStopping } = await import("./stopping");
const { useApp } = await import("./store");

const id = "session-1";

const task = (taskId: string, state: TaskState): Task =>
  ({ id: taskId, conversationId: id, number: 1, title: taskId, state, requestId: "r1", createdAtMs: 1, updatedAtMs: 1 }) as unknown as Task;

/** The daemon's answers, held until the test lets them go; a rejection is its error. */
let pending: { method: string; settle: (fail: boolean) => void }[];
let sent: string[];

/** The open board as its view shows it, the user's stops under way included. */
const shown = () => stoppedBoard(useBoard.getState().board!, useStopping.getState().stops);
const sidebar = () => rowActivity(useActivity.getState().byConversation[id]);

/** Lets the oldest held request of `method` go, as answered or failed. */
async function answer(method: string, fail = false): Promise<void> {
  const index = pending.findIndex((entry) => entry.method === method);
  assert.ok(index >= 0, `no ${method} request waits`);
  pending.splice(index, 1)[0]!.settle(fail);
  await new Promise((resolve) => setTimeout(resolve, 0));
}

/** Waits until the action's first request is out. */
const flushed = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  pending = [];
  sent = [];
  useStopping.setState({ stops: [] });
  useBoard.setState({
    board: {
      ...emptyBoard(id),
      run: "running",
      tasks: { w1: task("w1", "running"), w2: task("w2", "queued") },
      requests: { r1: { id: "r1", conversationId: id, state: { type: "working" }, startedAtMs: 1 } as never },
    },
  });
  useActivity.setState({
    byConversation: {
      [id]: fromSnapshot({ conversationId: id, run: "running", tasks: [["w1", "running"], ["w2", "queued"]], approvals: [], questions: [], phases: [] }),
    },
  });
  mockIPC((_command, payload) => {
    const req = (payload as { request: RequestOf<"interrupt"> | RequestOf<"stopTask"> | RequestOf<"stopWorkers"> | RequestOf<"restore"> }).request;
    sent.push(req.method);
    return new Promise((resolve, reject) => {
      pending.push({
        method: req.method,
        settle: (fail) => {
          if (fail) reject(new Error(`${req.method} failed`));
          else if (req.method === "stopWorkers") resolve({ method: req.method, taskIds: ["w1"] });
          else if (req.method === "restore") resolve({ method: req.method, outcomes: [{ id, conversation: { ...archived, lifecycle: "active" }, error: null }] });
          else resolve({ method: req.method });
        },
      });
    });
  });
});

test("the composer's Stop shows the session and its workers stopped before the daemon answers (issue 498)", async () => {
  const done = interrupt(id);
  await flushed();
  assert.deepEqual(sent, ["interrupt"]);
  assert.equal(shown().run, "idle");
  assert.equal(shown().requests.r1?.state.type, "stopped");
  assert.equal(shown().tasks.w1?.state, "stopped");
  assert.equal(shown().tasks.w2?.state, "stopped");
  assert.equal(sidebar().running, false);
  // The daemon's own state is kept as it is meanwhile.
  assert.equal(useBoard.getState().board?.run, "running");
  await answer("interrupt");
  await answer("stopTask");
  await answer("stopTask");
  await done;
  // Answered before its events came: it still shows stopped until they do.
  assert.equal(shown().run, "idle");
  useBoard.setState(({ board }) => ({
    board: { ...board!, run: "idle", tasks: { w1: task("w1", "stopped"), w2: task("w2", "stopped") }, requests: {} },
  }));
  assert.deepEqual(useStopping.getState().stops, []);
});

test("a Stop that fails shows the real state again and says why", async () => {
  const failed = interrupt(id).catch((error: unknown) => error);
  await flushed();
  assert.equal(sidebar().running, false);
  await answer("interrupt", true);
  await answer("stopTask");
  await answer("stopTask");
  assert.match(String(await failed), /interrupt failed/);
  assert.deepEqual(useStopping.getState().stops, []);
  assert.equal(shown().run, "running");
  assert.equal(shown().tasks.w1?.state, "running");
  assert.equal(sidebar().running, true);
});

test("Stop all shows the workers stopped at once; one the daemon didn't stop shows as it is", async () => {
  const done = stopWorkers(id);
  await flushed();
  assert.equal(shown().tasks.w1?.state, "stopped");
  assert.equal(shown().tasks.w2?.state, "stopped");
  assert.equal(shown().run, "running", "Stop all leaves the turn be");
  await answer("stopWorkers");
  assert.deepEqual(await done, ["w1"]);
  assert.equal(shown().tasks.w1?.state, "stopped");
  assert.equal(shown().tasks.w2?.state, "queued");
});

test("a Stop all that fails shows the workers running again", async () => {
  const failed = stopWorkers(id).catch((error: unknown) => error);
  await flushed();
  assert.equal(sidebar().workers, 0);
  await answer("stopWorkers", true);
  assert.match(String(await failed), /stopWorkers failed/);
  assert.equal(shown().tasks.w1?.state, "running");
  assert.equal(sidebar().workers, 2);
});

const archived = { id, kind: "session", title: "Fix it", lifecycle: "archived" } as unknown as Conversation;

test("Restore brings an archived conversation back before the daemon answers (issue 499)", async () => {
  useApp.setState({ conversations: { [id]: archived } });
  const done = restore(id);
  await flushed();
  assert.equal(useApp.getState().conversations[id]?.lifecycle, "active");
  await answer("restore");
  await done;
  assert.equal(useApp.getState().conversations[id]?.lifecycle, "active");
});

test("a Restore that fails archives it again and says why", async () => {
  useApp.setState({ conversations: { [id]: archived } });
  const failed = restore(id).catch((error: unknown) => error);
  await flushed();
  assert.equal(useApp.getState().conversations[id]?.lifecycle, "active");
  await answer("restore", true);
  assert.match(String(await failed), /restore failed/);
  assert.equal(useApp.getState().conversations[id]?.lifecycle, "archived");
});
