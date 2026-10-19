import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The sidebar's foot in Chromium (a window wide enough for the expanded sidebar) over a faked daemon: Help is the row's last icon, flush right,
// and feedback has left the row; the usage list puts the most used first, each with its tightest
// limit (amber past 75%); a provider opens its limits, one at a time; Providers › and Sign in go
// to Settings › Providers; Help opens Send feedback… and Keyboard shortcuts.
test("sidebar: usage list, a provider's limits, and the help menu", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "sidebar-popovers.html?drive=1", "popovers-result")) as Record<string, unknown>;
  assert.deepEqual(seen.foot, { last: "Help", flushRight: true, feedbackIcon: false });
  assert.equal(seen.header, "UsageProviders");
  assert.deepEqual(seen.rows, [
    { words: ["Codex", "5-hour · 3h", "82%"], amber: true },
    { words: ["Claude Code", "Weekly · 1d 8h", "61%"], amber: false },
  ]);
  assert.deepEqual(seen.claudeOpen, {
    rows: [
      ["Codex", "5-hour · 3h", "82%"],
      ["Claude Code", "3 limits"],
    ],
    limits: [
      ["5-hour", "2h 51m", "47%"],
      ["Weekly", "1d 8h", "61%"],
      ["Weekly · Fable", "1d 8h", "0%"],
    ],
  });
  // Opening another closes the first.
  assert.deepEqual(seen.codexOpen, {
    expanded: ["Codex"],
    limits: [
      ["5-hour", "3h", "82%"],
      ["Weekly", "6d 4h", "13%"],
    ],
  });
  assert.deepEqual(seen.providers, { selection: { type: "settings", page: "providers" }, closed: true });
  // A provider that needs signing in comes first, as the Providers page says it.
  assert.deepEqual(seen.signInRows, [
    ["Codex", "Not signed in", "Sign in"],
    ["Claude Code", "Weekly · 1d 8h", "61%"],
  ]);
  assert.deepEqual(seen.signIn, { ask: { provider: "codex", signIn: true }, closed: true });
  assert.deepEqual(seen.help, ["Send feedback…", "Keyboard shortcuts"]);
  assert.deepEqual(seen.shortcuts, { type: "settings", page: "shortcuts" });
  assert.equal(seen.feedback, true);

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
