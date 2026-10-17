import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive, renderFixturePage } from "@/fixtures/headless";

// The real sidebar in Chromium over a faked daemon: the Chats section's "+" next to Projects',
// a lone agent update run from the chip itself, two updates where the finished one leaves the
// open list by itself, and an update that can't start opening the list on why.
test("sidebar: Chats +, updates that run from the chip and leave by themselves", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "updates.html?drive=1", "updates-result", 20000)) as Record<string, unknown>;
  assert.deepEqual(seen.chatsPlus, { there: true, sameLook: true });
  assert.deepEqual(seen.selection, { type: "draft", kind: "chat" });

  assert.equal(seen.loneTip, "Update Codex to 1.1.0");
  assert.equal(seen.loneMenu, false, "a lone agent update runs without the list");
  assert.deepEqual(seen.loneCalls, [{ method: "runUpdate", target: "codex" }]);
  assert.equal(seen.loneUpdating, "Updating Codex");
  assert.equal(seen.loneUpdated, "Codex updated");
  assert.equal(seen.loneAfter, "gone");

  assert.deepEqual(seen.twoRows, ["Claude Code", "Codex"]);
  assert.equal(seen.twoCalls, 2);
  assert.deepEqual(seen.finishedShowing, ["Claude Code", "Codex"], "a finished row shows its check a moment");
  assert.deepEqual(seen.afterFirst, { rows: ["Claude Code"], open: true });
  assert.deepEqual(seen.afterLast, { open: false, pill: "gone" });

  assert.deepEqual(seen.failed, { open: true, alert: "Couldn't reach npm" });
});

// The narrowest sidebar with the updates chip and a nightly update waiting: "Restart to update"
// shortens its words and stays inside, rather than running past the sidebar's edge.
test("sidebar: Restart to update fits a narrow sidebar", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "updates.html?narrow=1", "updates-result")) as Record<string, unknown>;
  assert.deepEqual(seen, { open: "expanded", inside: true, shortened: true });
});

// The usage menu's link names the page it opens: Settings > Providers.
test("sidebar: the usage menu opens the Providers page", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "updates.html?usage=1", "updates-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(seen, { link: "Open the Providers page", selection: { type: "settings", page: "providers" } });
});
