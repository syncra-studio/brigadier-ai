import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

const card = async (t: test.TestContext, query: string) =>
  JSON.parse(await renderFixturePage(t, `pr-card.html?${query}`, "pr-card-result", 15000)) as { row: boolean; shown: string[]; sent?: string[]; asked?: boolean };

// The pull request row offers what applies to it, one button each; none for a draft with nothing to fix.
test("the pull request row shows the buttons its state calls for", { timeout: 120000 }, async (t) => {
  assert.deepEqual((await card(t, "state=failing")).shown, ["Fix CI", "Merge now"]);
  assert.deepEqual((await card(t, "state=conflicts")).shown, ["Address comments", "Resolve conflicts"]);
  assert.deepEqual((await card(t, "state=merged")).shown, ["Revert"]);
  const draft = await card(t, "state=draft");
  assert.equal(draft.row, true);
  assert.deepEqual(draft.shown, []);
});

// Approved, with a review thread still unresolved: Address comments shows, and sends its request
// once as the user's.
test("an approved pull request with an unresolved thread asks to address its comments", { timeout: 60000 }, async (t) => {
  const seen = await card(t, "state=approved&click=Address%20comments");
  assert.deepEqual(seen.shown, ["Address comments", "Merge now"]);
  assert.deepEqual(seen.sent, ["Address the review comments on PR #7."]);
  assert.equal(seen.asked, true);
});

// Revert on a merged pull request sends the same request as the merge message's Revert.
test("Revert on a merged pull request sends the merge message's request", { timeout: 60000 }, async (t) => {
  const seen = await card(t, "state=merged&click=Revert");
  assert.deepEqual(seen.sent, ["Revert merge f00dfee (PR #7) on main and push it the way it went in"]);
});
