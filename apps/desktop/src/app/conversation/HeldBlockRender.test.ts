import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = { blocks: number; headers: (string | null)[]; waits: string[][]; steers: string[][] };

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
  assert.deepEqual(result.done.waits, [[]]);
  // Over: on the clock from its first message to its last turn, the waits for its workers counted.
  assert.equal(result.done.headers[0], "Worked for 5m 2s");
});
