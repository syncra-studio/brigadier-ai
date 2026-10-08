import assert from "node:assert/strict";
import { test } from "node:test";

import { type StatusInput, threadStatus } from "@/app/conversation/liveStatus";
import { taskActivityLines } from "@/app/conversation/taskActivity";
import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { OrchestratorStep, ProviderEvent, Question, Task, WaitingItem } from "@/ipc/generated";
import { formatTime } from "@/lib/format";
import { applyToBoard, type Board, doingOf, emptyBoard, type ShownApproval } from "@/state/board";

const base = Object.values(night.tasks)[0] as unknown as Task;
let next = 0;
const task = (patch: Partial<Task> = {}): Task => {
  next += 1;
  return { ...base, id: `t${next}`, requestId: "r1", state: "running", createdAtMs: next, updatedAtMs: next,
    attempts: [], quotaWait: null, blockedReason: null, candidate: null, run: null, ...patch };
};
const byId = <T extends { id: string }>(items: T[]) => Object.fromEntries(items.map((item) => [item.id, item]));

/** A board for request `r1`; the lead is idle unless `run` says otherwise. */
function input(patch: Partial<Board> = {}, rest: Partial<StatusInput> = {}): StatusInput {
  return {
    board: { ...emptyBoard("c"), ...patch },
    requestIds: ["r1"],
    state: "working",
    thinkingLive: false,
    compacting: false,
    quotaWait: null,
    ...rest,
  };
}
const leading = { run: "running", runRequest: "r1" } as const;
const head = (status: StatusInput) => threadStatus(status).head;
const line = (worker: Task) => taskActivityLines({ task: worker, activity: "Editing 3 files" }, 60_000);
const call = (name: string) => ({ requestId: "r1", atMs: 0, position: 1,
  kind: { type: "tool", name, status: "inProgress" } }) as unknown as OrchestratorStep;
const error = (kind: string, willRetry = true) =>
  ({ type: "error", error: { kind, message: "", willRetry, limit: null, code: null } }) as unknown as ProviderEvent;

test("a plain chat: Thinking until its own rows show, then nothing extra", () => {
  assert.deepEqual(head(input(leading)), { text: "Thinking", tone: "busy" });
  assert.deepEqual(head(input({ run: "starting", runRequest: "r1" })), { text: "Starting", tone: "busy" });
  assert.deepEqual(head(input({ ...leading, doing: "Searching the web" })), { text: "Searching the web", tone: "busy" });
  // Streaming text, a live thinking snippet, a running tool and a compaction show themselves.
  assert.equal(head(input({ ...leading, streaming: { messageId: "m", text: "Hi", requestId: "r1" } })), null);
  assert.equal(head(input(leading, { thinkingLive: true })), null);
  assert.equal(head(input(leading, { compacting: true })), null);
  assert.equal(head(input({ ...leading, orchestratorSteps: [call("Read")] })), null);
  // A call to a worker has no row of its own: the line says it.
  assert.deepEqual(head(input({ ...leading, doing: "Delegating to a worker", orchestratorSteps: [call("mcp__brigadier__delegate_task")] })),
    { text: "Delegating to a worker", tone: "busy" });
  // A message just sent, before the lead's turn starts.
  assert.deepEqual(head(input()), { text: "Thinking", tone: "busy" });
});

test("an over turn shows nothing", () => {
  for (const state of ["done", "stopped", "failed"] as const) {
    assert.deepEqual(threadStatus(input({ ...leading, tasks: byId([task()]) }, { state })), { head: null, workers: [], more: 0 });
  }
});

test("lead idle while workers run: waits for them by name, shimmering only while one works", () => {
  const one = task();
  assert.deepEqual(threadStatus(input({ tasks: byId([one]) })),
    { head: { text: "Waiting for a worker", tone: "busy" }, workers: [one.id], more: 0 });
  const two = [task(), task({ state: "queued" })];
  assert.deepEqual(head(input({ tasks: byId(two) })), { text: "Waiting for 2 workers", tone: "busy" });
  // Nobody actually works: the line stays still.
  const waiting = [task({ state: "queued" }), task({ state: "paused", blockedReason: "Waiting for a free slot" })];
  assert.deepEqual(head(input({ tasks: byId(waiting) })), { text: "Waiting for 2 workers", tone: "still" });
  // Finished workers and other requests' workers are not waited for.
  const others = [task({ state: "done" }), task({ requestId: "r2" })];
  assert.deepEqual(head(input({ tasks: byId(others) })), { text: "Thinking", tone: "busy" });
});

test("worker lines shimmer only while that worker works", () => {
  assert.equal(line(task()).firstWorking, true);
  for (const worker of [task({ state: "queued" }), task({ state: "paused" }),
    task({ state: "paused", quotaWait: { reason: "Claude resets at 21:10", resetsAtMs: 1, rule: null, ranking: null, sinceMs: 0 } })]) {
    assert.equal(line(worker).firstWorking, false);
  }
});

test("a worker that just finished reads as handing back, not as one being waited for", () => {
  const finished = task({ state: "reported" });
  assert.deepEqual(threadStatus(input({ tasks: byId([finished]) })),
    { head: { text: "Worker finished, handing back", tone: "busy" }, workers: [finished.id], more: 0 });
  assert.deepEqual(head(input({ tasks: byId([task({ state: "reported" }), task({ state: "reported" })]) })),
    { text: "2 workers finished, handing back", tone: "busy" });
  assert.equal(line(finished).first.split(" · ")[0], "Finished, handing back");
  assert.equal(line(finished).firstWorking, false);
  // Others still at it: the line waits only for them, and still lists the finished one.
  const view = threadStatus(input({ tasks: byId([finished, task()]) }));
  assert.deepEqual(view.head, { text: "Waiting for a worker", tone: "busy" });
  assert.equal(view.workers.length, 2);
  const queued = [task({ state: "reported" }), task({ state: "queued" }), task({ state: "queued" })];
  assert.deepEqual(head(input({ tasks: byId(queued) })), { text: "Waiting for 2 workers", tone: "still" });
});

test("more than three workers: three by name, the rest counted", () => {
  const five = Array.from({ length: 5 }, () => task());
  const view = threadStatus(input({ tasks: byId(five) }));
  assert.deepEqual(view.workers, five.slice(0, 3).map((worker) => worker.id));
  assert.equal(view.more, 2);
  assert.equal(view.head?.text, "Waiting for 5 workers");
});

test("the lead at work keeps its own line, with its workers under it", () => {
  const worker = task();
  const view = threadStatus(input({ ...leading, doing: "Messaging a worker", tasks: byId([worker]) }));
  assert.deepEqual(view, { head: { text: "Messaging a worker", tone: "busy" }, workers: [worker.id], more: 0 });
});

test("only a card or a question for the user reads as needing them, and workers stay listed", () => {
  const approval = { id: "a", requestId: "r1", state: { type: "pending" } } as unknown as ShownApproval;
  const worker = task();
  const view = threadStatus(input({ ...leading, approvals: byId([approval]), tasks: byId([worker]) }, { state: "waiting" }));
  assert.deepEqual(view, { head: { text: "Waiting for your approval", tone: "needsYou" }, workers: [worker.id], more: 0 });
  const allowed = { ...approval, state: { type: "allowed" } } as unknown as ShownApproval;
  assert.equal(head(input({ approvals: byId([allowed]) }))?.tone, "busy");
  const question = { id: "q", requestId: "r1", answer: null, answeredAtMs: null } as unknown as Question;
  assert.deepEqual(head(input({ questions: byId([question]) }, { state: "waiting" })), { text: "Waiting for your answer", tone: "needsYou" });
  const item = { id: "w", requestId: "r1", what: "Sign in to GitHub" } as unknown as WaitingItem;
  assert.deepEqual(head(input({ waiting: { w: item } }, { state: "waiting" })), { text: "Waiting for you · Sign in to GitHub", tone: "needsYou" });
  // The lead asked in its reply: no card, nothing paused.
  assert.deepEqual(head(input({}, { state: "waiting" })), { text: "Waiting for your answer", tone: "needsYou" });
  // A change ready to land waits for the user once the lead's turn is over.
  assert.equal(head(input({ tasks: byId([task({ state: "readyToLand" })]) }, { state: "waiting" }))?.text, "Waiting for your approval");
});

test("quota waits are not the user's: they say when quota comes back, still", () => {
  const resetsAtMs = Date.UTC(2026, 9, 7, 21, 10);
  const quotaWait = { reason: "Claude's 5-hour window resets", resetsAtMs, rule: null, ranking: null, sinceMs: 0 };
  // The lead's own messages wait for quota.
  assert.deepEqual(head(input({}, { quotaWait })), { text: `Waiting for quota · resets ${formatTime(resetsAtMs)}`, tone: "still" });
  assert.deepEqual(head(input({}, { quotaWait: { ...quotaWait, resetsAtMs: null } })), { text: "Waiting for quota", tone: "still" });
  // Quota fallback paused the workers and made the request `waiting`: not the user's to do.
  const paused = [task({ state: "paused", quotaWait }), task({ state: "paused", quotaWait: { ...quotaWait, resetsAtMs: resetsAtMs + 60_000 } })];
  const view = threadStatus(input({ tasks: byId(paused) }, { state: "waiting" }));
  assert.deepEqual(view.head, { text: `Waiting for quota · resets ${formatTime(resetsAtMs)}`, tone: "still" });
  assert.equal(view.workers.length, 2);
});

test("a landing change reads as landing", () => {
  assert.deepEqual(head(input({ tasks: byId([task({ state: "landing" })]) })), { text: "Landing the changes", tone: "busy" });
});

test("the lead's retries show while its CLI retries, and clear when it goes on", () => {
  assert.equal(doingOf(error("network"), "Thinking"), "Reconnecting");
  assert.equal(doingOf(error("overloaded"), null), "The model is busy, retrying");
  assert.equal(doingOf(error("network", false), "Searching the web"), "Searching the web");
  assert.equal(doingOf({ type: "reasoningDelta" } as unknown as ProviderEvent, "Reconnecting"), null);
  assert.equal(doingOf({ type: "reasoningDelta" } as unknown as ProviderEvent, "Searching the web"), "Searching the web");
  // A retry after some text or thinking streamed still shows over the stalled rows.
  const stalled = { ...leading, doing: "Reconnecting", streaming: { messageId: "m", text: "Hi", requestId: "r1" } };
  assert.deepEqual(head(input(stalled, { thinkingLive: true })), { text: "Reconnecting", tone: "busy" });
});

test("a worker's live line says what it does in plain words, never a raw tool name or shell wrapper", () => {
  const worker = task();
  const board = { ...emptyBoard("c"), tasks: byId([worker]) };
  const live = (event: unknown) => applyToBoard(board, { seq: 1, stream: `task:${worker.id}`, streamSeq: 1, atMs: 1,
    event: { type: "workerEvent", conversationId: "c", taskId: worker.id, event } as never }).activity[worker.id];
  assert.equal(live({ type: "command", itemId: "x", command: "/bin/zsh -lc 'git status'", cwd: null, status: "inProgress", exitCode: null }),
    "Running git status");
  assert.equal(live({ type: "toolCall", itemId: "y", name: "mcp__brigadier__project_map", input: null, status: "inProgress", output: null }),
    "Mapping the project");
});

test("the thread's own tool steps show as their rows, never as Thinking or a worker", () => {
  for (const name of ["Bash", "Read", "Edit", "mcp__brigadier__run", "mcp__brigadier__run_check", "mcp__brigadier__start_preview",
    "mcp__brigadier__preview_log", "mcp__brigadier__review_code", "shell", "apply_patch"]) {
    assert.deepEqual(threadStatus(input({ ...leading, orchestratorSteps: [call(name)] })), { head: null, workers: [], more: 0 }, name);
  }
  // Once the step is done the thread thinks again.
  const done = { ...call("Bash"), kind: { type: "tool", name: "Bash", status: "completed" } } as unknown as OrchestratorStep;
  assert.deepEqual(head(input({ ...leading, orchestratorSteps: [done] })), { text: "Thinking", tone: "busy" });
  // Another request's running step doesn't hide this one's line.
  assert.deepEqual(head(input({ ...leading, orchestratorSteps: [{ ...call("Bash"), requestId: "r0" }] })), { text: "Thinking", tone: "busy" });
});

test("a running preview or review is no worker: over requests show nothing, the findings turn shows its line", () => {
  // The answer is out (the daemon keeps the request done while a preview or review runs).
  assert.deepEqual(threadStatus(input({}, { state: "done" })), { head: null, workers: [], more: 0 });
  // The review's findings start a turn for the same request: it works and says so.
  assert.deepEqual(threadStatus(input(leading)), { head: { text: "Thinking", tone: "busy" }, workers: [], more: 0 });
  assert.deepEqual(head(input({ run: "starting", runRequest: "r1" })), { text: "Starting", tone: "busy" });
});

test("a worker open in the user's terminal waits on the user", () => {
  const workers = [task({ state: "takenOver" }), task()];
  const view = threadStatus(input({ tasks: byId(workers) }, { state: "waiting" }));
  assert.deepEqual(view.head, { text: "Working in your terminal", tone: "needsYou" });
  assert.deepEqual(view.workers, workers.map((worker) => worker.id));
  // Even while the lead's own turn runs.
  assert.deepEqual(head(input({ ...leading, tasks: byId([task({ state: "takenOver" })]) })),
    { text: "Working in your terminal", tone: "needsYou" });
});
