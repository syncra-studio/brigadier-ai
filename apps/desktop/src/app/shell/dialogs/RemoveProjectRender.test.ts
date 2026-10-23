import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("Remove project confirms one fixed project before removing it, even with repeated clicks", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "project-dialogs.html?drive=1", "remove-project-result")) as Record<string, unknown>;
  assert.equal(seen.error, undefined);
  assert.equal(seen.title, "Remove “Project 1”?");
  assert.match(String(seen.choices), /merged branch/);
  assert.match(String(seen.choices), /Also move its Brain and code index to the Trash/);
  assert.deepEqual(seen.beforeConfirm, []);
  assert.deepEqual(seen.cancelled, { removals: [], remaining: 8 });
  assert.deepEqual(seen.removals, [{ method: "removeProject", id: "p1", deleteBranches: [{ repo: "/Users/you/code/website", name: "brigadier/task-1", tip: "abc123" }], keepBrain: true }]);
  assert.deepEqual(seen.remaining, ["p2", "p3", "p4", "p5", "p6", "p7", "p8"]);
});
