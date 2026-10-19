import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// An agent's Browser tab the user took over, in Chromium: a strip says the agent waits, in
// plain words, with Continue; Continue hands the tab back, and the toolbar's buttons pause it
// too. The user's own tabs show nothing of it, and an agent's page behind its tab pauses nothing
// (#416).
test("browser tool: the user's takeover shows who waits, and Continue hands the tab back; a page behind its tab pauses nothing", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "agent-paused.html?drive=1", "agent-paused-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.before, false, "no strip while the agent works");
  assert.deepEqual(seen.paused, {
    text: "You're using this page, so Check the shop is waiting.Continue",
    role: "status",
    height: (seen.paused as { height: number }).height,
    continue: true,
  });
  assert.ok((seen.paused as { height: number }).height >= 32, "a strip, not a line");
  assert.equal(seen.continued, true);
  assert.equal(seen.reloaded, true, "Reload in the toolbar is the user's input");
  assert.equal(seen.mine, true);
  assert.equal(seen.behind, false, "a page behind its tab isn't the user's");
  const tab = "browser:agent-1";
  assert.deepEqual(seen.told, [
    { tab, paused: true }, { tab, paused: false }, { tab, paused: true }, { tab, paused: false },
  ]);
});
