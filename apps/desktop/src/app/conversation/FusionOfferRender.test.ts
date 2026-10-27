import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("Fusion offers wait for Ask, stay in view, and have no session card", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "fusion-offer.html?drive=1", "fusion-offer-result", 15000)) as Record<string, unknown>;
  assert.deepEqual(seen.cards, ["Fix the flaky upload test", "Rename the settings keys"]);
  assert.deepEqual(seen.places, ["kept", "work"]);
  assert.equal(seen.heading, "Ask every installed CLI?");
  assert.deepEqual(seen.buttons, ["Ask"]);
  assert.equal(seen.ignoredCalls, 0);
  assert.deepEqual(seen.calls, [{ method: "startOfferedFusion", offerId: "first" }]);
  assert.equal(seen.started, "Asked");
  assert.deepEqual(seen.startedButtons, []);
  assert.equal(seen.stayed, true);
  assert.equal(seen.disabled, true);
  assert.equal(seen.hint, true);
  assert.equal(seen.sessionCards, 0);
});
