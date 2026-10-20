import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The real dialog in Chromium over a seeded store: it asks only about the project the user is
// in, once, and nothing about the other unanswered projects meanwhile.
test("the trust question shows when an unanswered project is entered, once", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "trust.html?drive=1", "trust-result", 12000)) as Record<
    string,
    unknown
  >;
  // A plain chat and an answered project ask nothing, though two projects have no answer.
  assert.equal(seen.chat, null);
  assert.equal(seen.answered, null);
  // A chat in an unanswered project asks: the design's 420-wide card, Trust on the right; the
  // card has the focus, so Enter answers nothing.
  assert.deepEqual(seen.session, {
    title: "Trust this folder?",
    path: "/Users/you/Development/invoice-service",
    buttons: ["Don't trust", "Trust folder"],
    focused: "trust-dialog",
    width: 420,
  });
  assert.equal(seen.afterTrust, null);
  // The other unanswered project asks when its new session is open; once answered, never again.
  assert.equal((seen.newSession as { path: string }).path, "/Users/you/Development/ops-scripts");
  assert.equal(seen.backAgain, null);
  assert.deepEqual(seen.answers, ["p2 true", "p3 false"]);
});

// Drawn with the app's shared parts (#385): its card, title and line are the app's dialog's, its
// answers the app's buttons, and the path's tooltip the app's tooltip.
test("the trust question is drawn with the app's dialog, buttons and tooltip", { timeout: 60000 }, async (t) => {
  const pairs = JSON.parse(await renderFixturePage(t, "trust.html?look=1", "trust-look", 12000)) as Record<
    string,
    [unknown, unknown]
  >;
  for (const [part, [mine, shared]] of Object.entries(pairs)) {
    assert.notEqual(shared, null, `${part}: the shared part is drawn`);
    assert.deepEqual(mine, shared, `${part} looks like the app's own`);
  }
});
