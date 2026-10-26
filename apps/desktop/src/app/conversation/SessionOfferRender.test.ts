import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// Out-of-scope work the thread found: its card shows the moment the thread offers it, in place
// in the running work (#439); one click starts it as its own session, the user stays where
// they are, and the card then opens it.
test("a session offer starts on Start as session and then opens the session", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "session-offer.html?drive=1", "session-offer-result", 15000)) as Record<string, unknown>;
  // The running request's offer shows at once, in its live work, not after it.
  assert.deepEqual(seen.cards, ["Fix the flaky upload test", "Rename the settings keys"]);
  assert.deepEqual(seen.places, ["kept", "work"]);
  assert.equal(seen.heading, "Start this as its own session?");
  assert.deepEqual(seen.buttons, ["Start as session"]);
  assert.deepEqual(seen.calls, [{ method: "startOfferedSession", offerId: "first" }]);
  assert.equal(seen.started, "Started as a session");
  assert.deepEqual(seen.startedButtons, ["Open"]);
  assert.equal(seen.stayed, true);
  assert.equal(seen.stored, true);
  assert.deepEqual(seen.opened, { type: "conversation", id: "offered-session" });
});
