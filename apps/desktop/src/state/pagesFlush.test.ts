import assert from "node:assert/strict";
import { test } from "node:test";

import { answerFlush, flushPages, onFlushPages } from "@/state/pagesFlush";
import { emitPageChanged, onPageChanged, type PageChanged } from "@/state/pageChanges";

test("a quit's flush waits for every editor's save, then answers with its token", async () => {
  const order: string[] = [];
  const gate: { open?: () => void } = {};
  const slow = new Promise<void>((resolve) => {
    gate.open = resolve;
  });
  const offSlow = onFlushPages(async () => {
    await slow;
    order.push("slow saved");
  });
  const offQuick = onFlushPages(async () => {
    order.push("quick saved");
  });
  const answered = answerFlush(7, async (token) => {
    order.push(`answered ${token}`);
  });
  await new Promise((resolve) => setTimeout(resolve, 0));
  assert.deepEqual(order, ["quick saved"], "no answer while a save is still going");
  gate.open?.();
  await answered;
  assert.deepEqual(order, ["quick saved", "slow saved", "answered 7"]);
  offSlow();
  offQuick();
});

test("a failing save doesn't stop the others or the answer", async () => {
  const saved: string[] = [];
  const quiet = console.error;
  console.error = () => {};
  const offs = [
    onFlushPages(() => Promise.reject(new Error("disk full"))),
    onFlushPages(() => {
      throw new Error("thrown before any promise");
    }),
    onFlushPages(async () => {
      saved.push("other page");
    }),
  ];
  let answer: number | null = null;
  try {
    await answerFlush(3, async (token) => {
      answer = token;
    });
  } finally {
    console.error = quiet;
    for (const off of offs) off();
  }
  assert.deepEqual(saved, ["other page"]);
  assert.equal(answer, 3);
});

test("with no editor registered the flush answers at once, and an unregistered one isn't run", async () => {
  let ran = false;
  const off = onFlushPages(async () => {
    ran = true;
  });
  off();
  await flushPages();
  assert.equal(ran, false);
  let answer: number | null = null;
  await answerFlush(1, async (token) => {
    answer = token;
  });
  assert.equal(answer, 1);
});

test("page changes reach every listener until it unsubscribes", () => {
  const heard: PageChanged[] = [];
  const off = onPageChanged((change) => heard.push(change));
  emitPageChanged({ id: "a", revision: "r1" });
  emitPageChanged({ id: "a", revision: null });
  off();
  emitPageChanged({ id: "b", revision: "r2" });
  assert.deepEqual(heard, [
    { id: "a", revision: "r1" },
    { id: "a", revision: null },
  ]);
});
