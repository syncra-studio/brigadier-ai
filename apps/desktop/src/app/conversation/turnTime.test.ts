import assert from "node:assert/strict";
import { test } from "node:test";

import { type BoardDigest, buildBlocks, requestSpans, requestWaitsOnCard, turnTime, waitsOnCard, workedMs } from "@/app/conversation/blocks";
import type { Message, RequestState, UserRequest, WorkSpan } from "@/ipc/generated";

const span = (fromMs: number, toMs: number | null): WorkSpan => ({ fromMs, toMs });

function request(id: string, state: RequestState, fields: Partial<UserRequest> = {}): UserRequest {
  return {
    id,
    conversationId: "c",
    preview: id,
    state,
    startedAtMs: 0,
    endedAtMs: null,
    steeredInto: null,
    steeredAfter: null,
    undo: null,
    worked: [],
    quotaWait: false,
    ...fields,
  };
}

function userMessage(id: string, seq: number, createdAtMs: number): Message {
  return {
    id,
    conversationId: "c",
    seq,
    role: "user",
    text: id,
    blob: null,
    createdAtMs,
    attachments: [],
    mentions: [],
    model: null,
    requestId: id,
    parentId: null,
  };
}

const empty: BoardDigest = {
  tasks: {},
  approvals: {},
  questions: {},
  plans: {},
  requests: {},
  orchestratorSteps: [],
  machineSteps: [],
  decisions: [],
  compactions: {},
  runRequest: null,
  streaming: null,
};

test("worked time is the union of the spans, an open one running until now", () => {
  assert.equal(workedMs([span(0, 10), span(30, 40)], 100), 20);
  // Overlapping spans (a follow-up working alongside) count once.
  assert.equal(workedMs([span(0, 50), span(20, 30), span(40, 70)], 100), 70);
  assert.equal(workedMs([span(30, 40), span(0, 10)], 100), 20);
  assert.equal(workedMs([span(0, 10), span(60, null)], 100), 50);
  // A span that ends before it starts, or opens after now, never takes time away.
  assert.equal(workedMs([span(50, 20), span(200, null)], 100), 0);
  assert.equal(workedMs([], 100), 0);
});

test("a request stored before spans were kept worked from its start to its end", () => {
  const old = request("r", { type: "done" }, { startedAtMs: 100, endedAtMs: 700, worked: undefined as unknown as WorkSpan[] });
  assert.deepEqual(requestSpans(old), [span(100, 700)]);
  assert.deepEqual(requestSpans({ ...old, worked: [] }), [span(100, 700)]);
  const live = request("r", { type: "working" }, { startedAtMs: 100 });
  assert.deepEqual(requestSpans(live), [span(100, null)]);
});

test("a span left open on a request that is over ends with the request", () => {
  const over = request("r", { type: "done" }, { endedAtMs: 90, worked: [span(0, 10), span(50, null)] });
  assert.deepEqual(requestSpans(over), [span(0, 10), span(50, 90)]);
  const working = request("r", { type: "working" }, { worked: [span(0, 10), span(50, null)] });
  assert.deepEqual(requestSpans(working), [span(0, 10), span(50, null)]);
});

test("a turn shows its worked time, or how long it has waited for the user", () => {
  const base = { worked: [span(0, 10), span(30, 40)], quotaWait: false, cardWait: false, endedAtMs: 40 };
  assert.equal(turnTime({ ...base, state: "done" }, 1000), 20);
  assert.equal(turnTime({ ...base, state: "stopped" }, 1000), 20);
  assert.equal(turnTime({ ...base, state: "working", worked: [span(0, 10), span(30, null)] }, 100), 80);
  // Waiting for the user: the time since work stopped, not the total.
  assert.equal(turnTime({ ...base, state: "waiting" }, 100), 60);
  // Waiting only for quota still counts as work.
  assert.equal(turnTime({ ...base, state: "waiting", quotaWait: true, worked: [span(0, null)] }, 100), 100);
  // So does waiting on its own question card: its span stays open, and it reads as working.
  const card = { ...base, state: "waiting" as const, cardWait: true, worked: [span(0, null)] };
  assert.ok(waitsOnCard(card));
  assert.equal(turnTime(card, 40_000), 40_000);
  assert.ok(!waitsOnCard({ ...base, state: "waiting" }));
  assert.ok(requestWaitsOnCard(request("r", { type: "waiting" }, { worked: [span(0, null)] })));
  assert.ok(!requestWaitsOnCard(request("r", { type: "waiting" }, { worked: [span(0, null)], quotaWait: true })));
  assert.ok(!requestWaitsOnCard(request("r", { type: "waiting" }, { worked: [span(0, 10)] })));
});

test("a follow-up steered into a turn keeps the turn's start and adds its own work", () => {
  const first = request("first", { type: "done" }, { startedAtMs: 0, endedAtMs: 600, worked: [span(0, 100), span(500, 600)] });
  // Sent at 300 while the first request waited for the user (100 to 500).
  const steer = request("steer", { type: "done" }, {
    startedAtMs: 300,
    endedAtMs: 550,
    steeredInto: "first",
    worked: [span(300, 550)],
  });
  const board = { ...empty, requests: { first, steer } };
  const blocks = buildBlocks([userMessage("first", 1, 0), userMessage("steer", 2, 300)], {}, false, board, []);
  assert.equal(blocks.length, 1);
  const [block] = blocks;
  assert.ok(block);
  assert.deepEqual(block.requestIds, ["first", "steer"]);
  assert.equal(block.startedAtMs, 0);
  // 0–100, then 300–600 (the steer's 300–550 and the first's 500–600 overlap).
  assert.equal(turnTime(block, 10_000), 400);
});

test("a joined turn waits for quota only while every waiting part does", () => {
  const first = request("first", { type: "waiting" }, { endedAtMs: 100, worked: [span(0, null)], quotaWait: true });
  const steer = request("steer", { type: "waiting" }, {
    startedAtMs: 50,
    endedAtMs: 80,
    steeredInto: "first",
    worked: [span(50, 80)],
  });
  const messages = [userMessage("first", 1, 0), userMessage("steer", 2, 50)];
  const [userWait] = buildBlocks(messages, {}, false, { ...empty, requests: { first, steer } }, []);
  assert.equal(userWait?.state, "waiting");
  assert.equal(userWait?.quotaWait, false);
  // It has waited for the user since the follow-up stopped, the quota wait's open span aside.
  assert.ok(userWait);
  assert.equal(turnTime(userWait, 1000), 920);
  const [quota] = buildBlocks(messages, {}, false, { ...empty, requests: { first, steer: { ...steer, quotaWait: true, worked: [span(50, null)] } } }, []);
  assert.equal(quota?.quotaWait, true);
});
