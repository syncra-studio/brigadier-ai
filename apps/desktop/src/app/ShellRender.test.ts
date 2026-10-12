import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Check = { moved: string[]; still: boolean };

// The whole app in Chromium, the wheel turned and things brought into view as a user and the
// app do: only the panes inside scroll; the window, its titlebar and sidebars never move.
test("the window never scrolls: only the panes inside it do", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "shell.html?drive=scroll", "shell-result", 90000)) as Record<string, unknown>;
  if (process.env.SHELL_SEEN) console.log(JSON.stringify(seen, null, 2));
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.rootStyles, ["html clip/clip static", "body clip/clip static", "#root clip/clip relative"]);
  assert.equal(seen.documentFits, true, "nothing reaches past the window, a long thread's screen reader labels included");
  for (const name of ["threadTop", "threadBottom", "chrome", "threadIntoView", "tabs", "pageBottom", "pageTop", "pageIntoView", "pushed"]) {
    const check = seen[name] as Check;
    assert.deepEqual(check.moved, [], `${name}: nothing but a pane scrolled`);
    assert.equal(check.still, true, `${name}: the titlebar and sidebars stayed put`);
  }
  assert.equal(seen.threadScrolled, true, "the thread scrolls under the wheel");
  assert.equal(seen.stripScrolls, true, "the tab strip scrolls its tab in front into view");
  assert.equal(seen.pageScrolled, true, "a page scrolls under the wheel");
  assert.equal(seen.pageBelowTitlebar, true, "a page scrolls below the titlebar");
  const settings = seen.settingsTop as { scroller: number; titlebar: number };
  assert.equal(settings.scroller, settings.titlebar, "Settings scrolls below the titlebar, clipped at its bottom edge");
  assert.equal(seen.settingsScrolled, true);
  assert.equal(seen.settingsNavStill, true);
  assert.deepEqual(seen.settingsMoved, []);
  assert.deepEqual(seen.settingsPushed, []);
});
