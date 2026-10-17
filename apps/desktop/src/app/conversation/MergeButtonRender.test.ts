import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("Merge into main shows only for a workspace with commits to merge, even ones whose sum changes nothing, sends mergeSession on a click, leaves the branch its whole name, and Close workspace is listed only with a workspace", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "merge-button.html", "merge-button-result", 60000)) as {
    error?: string;
    buttons: Record<string, string[]>;
    menus: Record<string, string[]>;
    calls: unknown[];
    nameCut: boolean;
  };
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.buttons, { "with-work": ["Merge into main"], "net-empty": ["Merge into main"], "no-work": [], "no-workspace": [] });
  assert.deepEqual(seen.menus["with-work"], ["Commit", "Push", "Create branch", "Switch branch", "Close workspace…"]);
  assert.deepEqual(seen.menus["no-workspace"], ["Commit", "Push", "Create branch", "Switch branch"]);
  assert.equal(seen.nameCut, false, "the branch name is cut short beside the merge button");
  assert.deepEqual(seen.calls, [{ method: "mergeSession", conversationId: "with-work" }]);
});
