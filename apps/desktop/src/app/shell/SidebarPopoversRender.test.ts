import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The sidebar's foot in Chromium (a window wide enough for the expanded sidebar) over a faked daemon: Help is the row's last icon, flush right,
// and feedback has left the row; the usage list puts the most used first, each with its account in
// use's tightest limit (amber past 75%, a reached limit said); a provider's row opens its page;
// Providers › and Sign in go to Settings › Providers; Help opens Send feedback… and Keyboard shortcuts.
test("sidebar: usage list, a provider's page, and the help menu", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "sidebar-popovers.html?drive=1", "popovers-result")) as Record<string, unknown>;
  assert.deepEqual(seen.foot, { last: "Help", flushRight: true, feedbackIcon: false });
  assert.equal(seen.header, "UsageProviders");
  // The icon's words name the agent and say used, as its list does (#547).
  assert.equal(seen.tip, "Usage · Codex 82% used");
  assert.deepEqual(seen.rows, [
    { words: ["Codex", "5-hour · resets in 3h", "82% used"], amber: true },
    { words: ["Claude Code", "Weekly · resets in 1d 8h", "61% used"], amber: false },
  ]);
  // Several accounts: the tip names the one in use (new work starts on it), masked; never the
  // one only picked with Use.
  assert.deepEqual(seen.tips, ["", "In use: esc•••••@•••••.com"]);
  // A row no longer opens under itself: it opens its provider's page.
  assert.equal(seen.expandable, 0);
  assert.deepEqual(seen.claudePage, {
    ask: null,
    selection: { type: "settings", page: "providers", detail: { provider: "claude" } },
    closed: true,
  });
  assert.deepEqual(seen.codexPage, { ask: null, closed: true });
  // The account in use at its limit: said in its row, red, and on the icon.
  assert.deepEqual(seen.limitRows, [
    ["Claude Code", "At limit · resets in 3h 38m", "100% used"],
    ["Codex", "5-hour · resets in 3h", "82% used"],
  ]);
  assert.deepEqual(seen.limitDot, { tip: "Usage · limit reached", red: true });
  assert.deepEqual(seen.providers, { selection: { type: "settings", page: "providers" }, closed: true });
  // A provider that needs signing in comes first, as the Providers page says it.
  assert.deepEqual(seen.signInRows, [
    ["Codex", "Not signed in", "Sign in"],
    ["Claude Code", "Weekly · resets in 1d 8h", "61% used"],
  ]);
  assert.deepEqual(seen.signIn, { ask: { provider: "codex", under: { type: "settings", page: "providers", detail: { provider: "codex" } } }, closed: true });
  assert.deepEqual(seen.help, ["Send feedback…", "Keyboard shortcuts"]);
  assert.deepEqual(seen.shortcuts, { type: "settings", page: "shortcuts" });
  assert.equal(seen.feedback, true);
  // A click with no pointer before it (an accessibility press, a driven click) opens Help as the
  // foot's other icons open (#474).
  assert.deepEqual(seen.helpByClick, ["Send feedback…", "Keyboard shortcuts"]);

  // Drawn with the app's shared parts (#385): the usage list is a shared popover with menu rows
  // and a menu label, its amber the shared warning; Help is a shared menu with its rows.
  const look = seen.look as Record<string, unknown> & { canon: Record<string, unknown> };
  assert.ok(look.canon.popover && look.canon.menu && look.canon.item, "the shared popover and menu opened");
  assert.deepEqual(look.usage, look.canon.popover);
  assert.deepEqual(look.usageRow, look.canon.item);
  assert.deepEqual(look.usageLabel, look.canon.label);
  assert.equal(look.usageWash, true);
  assert.equal(look.amber, look.warning);
  assert.deepEqual(look.help, look.canon.menu);
  assert.deepEqual(look.helpItem, look.canon.item);
  // Help is as wide as the design draws it, never narrower than the app's menus (#386): sized to
  // its two rows it read as an oval.
  const helpWidth = look.helpWidth as number;
  assert.ok(helpWidth >= (look.canon.menuMinWidth as number), `help menu ${helpWidth}px under the shared min-width`);
  assert.equal(helpWidth, 240);
});
