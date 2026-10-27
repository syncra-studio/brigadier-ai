import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("plugin sign-in stays inline on cards and details, with Cancel, Sign out and live callbacks", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "../src/app/settings/plugins/SignIn.fixture.html", "result", 20000)) as {
    error?: string; warning: boolean; gridWaiting: boolean; gridStayedOpen: boolean; canceled: string;
    detailWaiting: boolean; beforeCallbackStarts: number; callbackMs: number; signedInButtons: string[];
    signedOutButtons: string[]; grid: string; fallback: string; setup: string; setupButtons: string[];
    dialogs: number; banners: number;
  };
  assert.equal(seen.error, undefined);
  assert.equal(seen.warning, true, "Must sign in uses the existing warning token");
  assert.equal(seen.gridWaiting, true, "card shows a spinner and Waiting for browser");
  assert.equal(seen.gridStayedOpen, true, "card sign-in does not navigate");
  assert.equal(seen.canceled, "Must sign in");
  assert.equal(seen.detailWaiting, true);
  assert.equal(seen.beforeCallbackStarts, 2, "opening the details does not start another flow");
  assert.ok(seen.callbackMs <= 1000, `callback took ${seen.callbackMs} ms to display`);
  t.diagnostic(`callback to Signed in: ${seen.callbackMs} ms`);
  assert.deepEqual(seen.signedInButtons, ["Sign out"]);
  assert.deepEqual(seen.signedOutButtons, ["Sign in"]);
  assert.equal(seen.grid, "Signed in", "callback updates the grid after leaving details");
  assert.match(seen.fallback, /Uses each CLI's sign-in: Codex still need it/);
  assert.match(seen.setup, /add it in that CLI/);
  assert.deepEqual(seen.setupButtons, [], "setup-only providers have no dead browser action");
  assert.equal(seen.dialogs, 0);
  assert.equal(seen.banners, 0);
});
