import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// The search palette in Chromium: projects (with their sessions' count), sessions (with their
// project) and chats, grouped; the selected row says Open ↵ and ↓ moves it; ↵ opens it; typing
// part of "Send feedback" offers it, and ↵ opens Send feedback with its note taking the keyboard.
test("search palette: grouped results, keys, and Send feedback", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "search-palette.html?drive=1", "palette-result", 10000)) as Record<
    string,
    unknown
  >;
  assert.equal(seen.placeholder, "Search projects, sessions and chats");
  assert.equal(seen.focused, true);
  assert.deepEqual(seen.keys, ["esc", "↵", "↑", "↓", "↵"]);
  assert.deepEqual(seen.groups, [
    ["Projects", ["brigadier-demo", "Open↵", "(selected)"]],
    [
      "Sessions",
      ["am nevoie sa ma ghidezi in domeniul 3d printing, care sunt pasii", "brigadier-demo"],
      ["use computer use and go to makerworld website for 3d models", "brigadier-demo"],
      ["hi", "brigadier-demo"],
    ],
    ["Chats", ["hi"]],
  ]);
  assert.deepEqual(seen.moved, [
    "Sessions",
    ["am nevoie sa ma ghidezi in domeniul 3d printing, care sunt pasii", "Open↵", "(selected)"],
    ["use computer use and go to makerworld website for 3d models", "brigadier-demo"],
    ["hi", "brigadier-demo"],
  ]);
  assert.deepEqual(seen.opened, { selection: { type: "conversation", id: "s1" }, closed: true });
  assert.deepEqual(seen.feedbackGroups, [["Help", ["Send feedback…", "Open↵", "(selected)"]]]);
  assert.deepEqual(seen.feedback, { open: true, focused: true });
});
