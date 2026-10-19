import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the context card reads in the reference order, one row-height line per row, a line per worker while a few work, and its clicks open things instead of growing it", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "context-rows.html?drive=1", "context-rows-result", 60000)) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["todo-app", "Plan", "Workers", "Activities", "Sources"]);
  assert.equal(seen.projectMenu, true, "the project has its ⋯");
  assert.deepEqual(seen.rowHeights, [28], JSON.stringify(seen.odd));
  assert.deepEqual(seen.workers, ["API routes working+48 −0", "Web page working+120 −4", "1 done"]);
  assert.equal(seen.stepOpens, "task-3", "a plan step opens its worker");
  assert.equal(seen.grew, false);
});
