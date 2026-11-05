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

type SeenRefresh = Seen & { refresh: { gap: number | null; lineUnder: boolean | null; wokenUnder: boolean | null } };

test("a context refresh while a block is held leaves its live line under the divider, and the divider where it is when the wait ends", async (t) => {
  const woke = JSON.parse(await renderFixturePage(t, "held-block.html?refresh=wake", "held-block-result", 10000)) as Record<"held" | "woken" | "over" | "asked" | "closed", SeenRefresh>;
  assert.deepEqual(woke.held.waits, [["Waiting for 2 workers"]]);
  assert.equal(woke.held.refresh.lineUnder, true, JSON.stringify(woke.held));
  assert.ok(woke.held.refresh.gap !== null && woke.held.refresh.gap >= 0, JSON.stringify(woke.held));
  // The wait over, the thread goes on under the divider, and the divider stays under its reply.
  assert.equal(woke.woken.blocks, 1, JSON.stringify(woke.woken));
  assert.equal(woke.woken.refresh.gap, woke.held.refresh.gap, JSON.stringify(woke.woken));
  assert.equal(woke.woken.refresh.wokenUnder, true, JSON.stringify(woke.woken));
  // Its wake-up's turn over with a worker still working: its rows stay, and the block still works on the clock.
  assert.equal(woke.over.blocks, 1, JSON.stringify(woke.over));
  assert.equal(woke.over.refresh.wokenUnder, true, JSON.stringify(woke.over));
  assert.match(woke.over.headers[0] ?? "", /^Working for /);
  assert.deepEqual(woke.over.waits, [["Waiting for Second bug"]]);
  // What the user sends then shows in the block, and what answers it comes under it.
  assert.equal(woke.asked.blocks, 1, JSON.stringify(woke.asked));
  assert.deepEqual(woke.asked.steers, [["How is it going?"]], JSON.stringify(woke.asked));
  assert.equal(woke.asked.refresh.wokenUnder, true, JSON.stringify(woke.asked));
  assert.equal(woke.closed.blocks, 1, JSON.stringify(woke.closed));
  assert.deepEqual(woke.closed.steers, [["How is it going?"]], JSON.stringify(woke.closed));
  assert.equal(woke.closed.refresh.wokenUnder, true, JSON.stringify(woke.closed));
  // Over: on the clock from its first message to its last turn's end.
  assert.equal(woke.closed.headers[0], "Worked for 4m 12s");
  // Nothing follows the worker's end: the block closes with the divider under its answer, as after any finished turn.
  const quiet = JSON.parse(await renderFixturePage(t, "held-block.html?refresh=quiet", "held-block-result", 10000)) as Record<"held" | "closed", SeenRefresh>;
  assert.equal(quiet.held.refresh.lineUnder, true, JSON.stringify(quiet.held));
  assert.deepEqual(quiet.closed.waits, [[]]);
  assert.ok(quiet.closed.refresh.gap !== null && quiet.closed.refresh.gap >= 0, JSON.stringify(quiet.closed));
});

type SeenSeam = Seen & { liveLines: number[]; composer: string | null; order: string[][] };

test("a hand-off while a block is held leaves one block with one live line, its divider and rows in order, and the composer's clock the same as its header's", async (t) => {
  const seam = JSON.parse(await renderFixturePage(t, "held-block.html?seam=held", "held-block-result", 20000)) as Record<"running" | "over" | "recordedClosed" | "joinedRunning" | "joinedClosed" | "nothingHeld", SeenSeam>;
  const order = ["A worker is on the first.", "divider", "Picking up where the last session stoppe", "Messaged First bug", "The worker has what it needs."];
  for (const step of [seam.running, seam.over]) {
    assert.equal(step.blocks, 1, JSON.stringify(step));
    assert.deepEqual(step.liveLines, [1], JSON.stringify(step));
    assert.deepEqual(step.order, [order], JSON.stringify(step));
    assert.match(step.headers[0] ?? "", /^Working for /);
    assert.equal(step.composer, step.headers[0], JSON.stringify(step));
  }
  assert.deepEqual(seam.over.waits, [["Waiting for 2 workers"]]);
  // Recorded without the join, it splits once its block closes; with it, it stays one.
  assert.equal(seam.recordedClosed.blocks, 2, JSON.stringify(seam.recordedClosed));
  assert.equal(seam.joinedRunning.blocks, 1, JSON.stringify(seam.joinedRunning));
  assert.equal(seam.joinedClosed.blocks, 1, JSON.stringify(seam.joinedClosed));
  assert.match(seam.joinedClosed.headers[0] ?? "", /^Worked for /);
  // Nothing held, the block open only through the hand-off's own turns: the later block alone is live.
  assert.equal(seam.nothingHeld.blocks, 2, JSON.stringify(seam.nothingHeld));
  assert.deepEqual(seam.nothingHeld.liveLines, [0, 1], JSON.stringify(seam.nothingHeld));
  assert.match(seam.nothingHeld.headers[0] ?? "", /^Worked for /);
  assert.match(seam.nothingHeld.headers[1] ?? "", /^Working for /);
});
