import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("queue drag follows the pointer, slides neighbors, preserves each identity across daemon updates, and deletes exactly one row", { timeout: 120_000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "queue-drag.html?drive=1", "queue-drag-result", 90_000));
  assert.equal(seen.error, undefined, JSON.stringify(seen));
  assert.equal(seen.followsPointer, true);
  assert.equal(seen.neighborSlides, true);
  assert.deepEqual(seen.reordered, ["q2", "q3", "q4", "q1", "q5"]);
  assert.deepEqual(seen.moves.map(({ itemId, index }: { itemId: string; index: number }) => ({ itemId, index })), [{ itemId: "q1", index: 3 }]);
  for (const snapshot of [...seen.snapshots, seen.duringUpdate, seen.afterUpdate]) {
    assert.deepEqual([...snapshot].sort(), ["q1", "q2", "q3", "q4", "q5"], "each queued identity renders exactly once throughout a drag and external reorder");
  }
  assert.deepEqual(seen.deletes, ["q3"]);
  assert.equal(seen.beforeDelete.length, 5);
  assert.deepEqual(seen.afterDelete, seen.beforeDelete.filter((id: string) => id !== "q3"));
  assert.equal(seen.afterDelete.length, 4);
});
