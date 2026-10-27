import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("plugin sign-in runs only on details, with Cancel, Sign in again and live callbacks", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "../src/app/settings/plugins/SignIn.fixture.html", "result", 20000)) as {
    error?: string; warning: boolean; gridWaiting: string; startsBeforeSignIn: number; initialWaiting: boolean; canceled: string;
    detailWaiting: boolean; beforeCallbackStarts: number; callbackMs: number; signedInButtons: string[];
    againWaiting: boolean; canceledAgainButtons: string[]; grid: string; gridLine: boolean; gridButtons: { label: string; role: string }[][]; fallback: string; fallbackWaiting: string; setup: string; setupButtons: string[];
    dialogs: number; banners: number;
  };
  assert.equal(seen.error, undefined);
  assert.equal(seen.warning, true, "Must sign in uses the existing warning token");
  assert.equal(seen.startsBeforeSignIn, 0, "opening details does not start sign-in");
  assert.equal(seen.initialWaiting, true, "detail shows a spinner and Waiting for browser");
  assert.equal(seen.gridWaiting, "Waiting for your browser…", "card keeps its sign-in status line");
  assert.equal(seen.canceled, "Must sign in");
  assert.equal(seen.detailWaiting, true);
  assert.equal(seen.beforeCallbackStarts, 2, "two explicit detail-page sign-ins start two flows");
  assert.ok(seen.callbackMs <= 1000, `callback took ${seen.callbackMs} ms to display`);
  t.diagnostic(`callback to Signed in: ${seen.callbackMs} ms`);
  assert.deepEqual(seen.signedInButtons, ["Sign in again"]);
  assert.equal(seen.againWaiting, true, "re-sign-in uses the same spinner and Cancel");
  assert.deepEqual(seen.canceledAgainButtons, ["Sign in again"], "Cancel keeps the saved sign-in");
  assert.equal(seen.grid, "");
  assert.equal(seen.gridLine, false, "a signed-in card has no status line");
  assert.equal(seen.gridButtons.length, 6);
  for (const buttons of seen.gridButtons) {
    assert.deepEqual(buttons, [
      { label: "Open Test server", role: "button" },
      { label: "Test server", role: "switch" },
    ], "cards only have open and switch buttons in every sign-in state");
  }
  assert.match(seen.fallback, /Uses each CLI's sign-in: Codex still need it/);
  assert.match(seen.fallbackWaiting, /Codex: waiting for browser/);
  assert.match(seen.setup, /add it in that CLI/);
  assert.deepEqual(seen.setupButtons, [], "setup-only providers have no dead browser action");
  assert.equal(seen.dialogs, 0);
  assert.equal(seen.banners, 0);
});
