import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = { blocks: number; headers: (string | null)[]; waits: string[][]; steers: string[][]; gaps: number[] };

test("messages sent while a block's workers work stay in that one block, which waits for the workers its every turn started until the last finishes", async (t) => {
  const result = JSON.parse(await renderFixturePage(t, "held-block.html", "held-block-result", 10000)) as Record<"bothWork" | "oneWorks" | "done", Seen>;
  for (const step of [result.bothWork, result.oneWorks, result.done]) {
    assert.equal(step.blocks, 1, JSON.stringify(step));
    assert.deepEqual(step.steers, [["Also keep it fast", "How is it going?"]], JSON.stringify(step));
  }
  // The workers its first turn and its joined turn started both hold it: its header keeps working
  // on the clock, and one line, the live one, says what it waits for.
  assert.match(result.bothWork.headers[0] ?? "", /^Working for /);
  assert.deepEqual(result.bothWork.waits, [["Waiting for 2 workers"]]);
  assert.match(result.oneWorks.headers[0] ?? "", /^Working for /);
  // One left: named as the Workers card names it (D106).
  assert.deepEqual(result.oneWorks.waits, [["Waiting for Second bug"]]);
  // The live line sits the rows' 16px gap under what is above it, never tight under it (demo12).
  assert.deepEqual([result.bothWork.gaps, result.oneWorks.gaps], [[16], [16]]);
  assert.deepEqual(result.done.waits, [[]]);
  // Over: on the clock from its first message to its last turn, the waits for its workers counted.
  assert.equal(result.done.headers[0], "Worked for 5m 2s");
});

type SeenFailed = Seen & { errors: boolean[]; afterError: (number | null)[]; buttons: string[][] };

test("a turn that fails while its worker works stays open and working with its error shown, and closes as it really ended once the worker's end wakes the thread: one block throughout, the error kept (D24, D155)", async (t) => {
  const result = JSON.parse(await renderFixturePage(t, "held-block.html?failed=wake", "held-block-result", 10000)) as Record<"works" | "woken" | "closed", SeenFailed>;
  for (const step of [result.works, result.woken, result.closed]) {
    assert.equal(step.blocks, 1, JSON.stringify(step));
    assert.deepEqual(step.errors, [true], JSON.stringify(step));
  }
  assert.match(result.works.headers[0] ?? "", /^Working for /);
  assert.deepEqual(result.works.waits, [["Waiting for First bug"]]);
  assert.deepEqual(result.works.buttons, [[]]);
  assert.match(result.woken.headers[0] ?? "", /^Working for /);
  assert.match(result.closed.headers[0] ?? "", /^Worked for /);
  // The thread went on after it: nothing is left to try again.
  assert.deepEqual(result.closed.buttons, [[]]);
  // The error stays where that turn failed, above what the thread said once it went on (D16).
  assert.deepEqual(result.closed.afterError, [1], JSON.stringify(result.closed));
});

test("a turn that fails while its worker works, nothing following the worker's end, closes as failed with Try again only then", async (t) => {
  const result = JSON.parse(await renderFixturePage(t, "held-block.html?failed=nothing", "held-block-result", 10000)) as Record<"works" | "closed", SeenFailed>;
  assert.match(result.works.headers[0] ?? "", /^Working for /);
  assert.deepEqual(result.works.errors, [true]);
  assert.deepEqual(result.works.buttons, [[]]);
  assert.match(result.closed.headers[0] ?? "", /^Stopped after /);
  assert.deepEqual(result.closed.errors, [true]);
  assert.deepEqual(result.closed.buttons, [["Try again"]]);
});
