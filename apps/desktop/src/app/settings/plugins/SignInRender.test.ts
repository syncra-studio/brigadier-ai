import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("plugin sign-in: pending clicks, callback, dialog, detail action and grid follow real state", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "../src/app/settings/plugins/SignIn.fixture.html", "result", 15000)) as {
    error?: string; waitingDisabled: boolean; midFlowStarts: number; callbackMs: number;
    signedInButtons: string[]; detailButtons: string[]; grid: string; staleReply: string;
  };
  assert.equal(seen.error, undefined);
  assert.equal(seen.waitingDisabled, true);
  assert.equal(seen.midFlowStarts, 1, "reopening a pending dialog does not start another flow");
  assert.ok(seen.callbackMs <= 1000, `callback took ${seen.callbackMs} ms to display`);
  t.diagnostic(`callback to Signed in: ${seen.callbackMs} ms`);
  assert.deepEqual(seen.signedInButtons.filter(Boolean), ["Done"]);
  assert.deepEqual(seen.detailButtons, [], "signed-in details have no dead Sign in action");
  assert.equal(seen.grid, "Signed in");
  assert.equal(seen.staleReply, "Signed in");
});
