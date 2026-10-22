import assert from "node:assert/strict";
import { test } from "node:test";

import { turnTime, waitsOnCard, workedMs } from "@/app/conversation/blocks";
import type { WorkSpan } from "@/ipc/generated";

const span = (fromMs: number, toMs: number | null): WorkSpan => ({ fromMs, toMs });

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

test("a turn shows its worked time, or how long it has waited for the user", () => {
  const base = { worked: [span(0, 10), span(30, 40)], cardWait: false, endedAtMs: 40 };
  assert.equal(turnTime({ ...base, state: "done" }, 1000), 20);
  assert.equal(turnTime({ ...base, state: "stopped" }, 1000), 20);
  assert.equal(turnTime({ ...base, state: "working", worked: [span(0, 10), span(30, null)] }, 100), 80);
  // Waiting for the user: the time since work stopped, not the total.
  assert.equal(turnTime({ ...base, state: "waiting" }, 100), 60);
  // So does waiting on its own question card: its span stays open, and it reads as working.
  const card = { ...base, state: "waiting" as const, cardWait: true, worked: [span(0, null)] };
  assert.ok(waitsOnCard(card));
  assert.equal(turnTime(card, 40_000), 40_000);
  assert.ok(!waitsOnCard({ ...base, state: "waiting" }));
});

test("a turn's time stops where its final answer started, whatever ran or waited after", () => {
  const meta = { state: "waiting" as const, cardWait: false, endedAtMs: null };
  assert.equal(turnTime({ ...meta, worked: [span(0, null)] }, 9_000, 4_000), 4_000);
  assert.equal(turnTime({ ...meta, worked: [span(0, 2_000), span(3_000, 8_000), span(5_000, null)] }, 9_000, 4_000), 3_000);
  // Work that began after the answer started doesn't count.
  assert.equal(turnTime({ ...meta, state: "done", worked: [span(0, 1_000), span(6_000, 7_000)] }, 9_000, 4_000), 1_000);
});
