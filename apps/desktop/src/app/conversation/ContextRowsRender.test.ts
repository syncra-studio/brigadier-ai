import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the context card reads in the reference order, one row-height line per row, a line per worker while a few work, one Changes and one Reviewed row, and its clicks open things instead of growing it", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "context-rows.html?drive=1", "context-rows-result", 60000)) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["todo-app", "Plan", "Workers", "Activities", "Sources"]);
  assert.equal(seen.projectMenu, true, "the project has its ⋯");
  assert.deepEqual(seen.rowHeights, [28], JSON.stringify(seen.odd));
  assert.deepEqual(seen.workers, ["API routes working+48 −0", "Web page working+120 −4", "1 done"]);
  assert.equal(seen.stepOpens, "task-3", "a plan step opens its worker");
  assert.deepEqual(seen.project, ["Changes+380 −2", "Reviewed · 5 fixed"]);
  assert.equal(seen.changesTip, "brigadier/web-ui", "the branch is in the Changes row's tooltip");
  assert.equal(seen.findingsInCard, 0, "no findings list in the card");
  assert.equal(seen.reviewedOpens, "panel:reviews");
  assert.equal(seen.changesOpens, "branch", "Changes opens the branch's diff");
  assert.equal(seen.grew, false);
});
