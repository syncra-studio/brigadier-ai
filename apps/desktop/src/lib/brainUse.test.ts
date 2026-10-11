import assert from "node:assert/strict";
import { test } from "node:test";

import { brainUseLabel, brainUseText, lowBrainUse } from "@/lib/brainUse";

const use = (queries: number, factsIn: number, factsOut: number, calls: number) => ({ queries, factsIn, factsOut, calls });

test("a worker's row shows its Brain queries and write-backs once it used the Brain", () => {
  assert.equal(brainUseLabel(use(3, 6, 2, 40)), "Brain 3↓ 2↑");
  assert.equal(brainUseLabel(use(0, 4, 0, 12)), "Brain 0↓ 0↑");
  assert.equal(brainUseLabel(use(0, 0, 0, 80)), null);
  assert.equal(brainUseLabel(undefined), null);
  assert.equal(
    brainUseText(use(1, 6, 2, 51)),
    "asked the Brain 1 time · its brief carried 6 facts · taught it 2 facts · 51 calls",
  );
});

test("only a long task that never asked nor taught the Brain is low Brain use", () => {
  assert.equal(lowBrainUse(use(0, 5, 0, 51)), true);
  assert.equal(lowBrainUse(use(0, 5, 0, 50)), false);
  assert.equal(lowBrainUse(use(1, 0, 0, 200)), false);
  assert.equal(lowBrainUse(use(0, 0, 1, 200)), false);
  assert.equal(lowBrainUse(undefined), false);
});
