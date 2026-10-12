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

type Panels = Record<string, unknown> & { filesOpen: { top: number; row: number }; workersOpen: { top: number; row: number; state: string; width: number } };
type Start = { nav: string[]; divider: number; sidebarEnd: number; newChatEnd: number | null; firstTab: number; rule: { width: number; shown: boolean } };

for (const [name, page] of [["", "shell.html?drive=layout"], [", a widened left sidebar", "shell.html?drive=layout&wide=1"], [", compact", "shell.html?drive=layout&density=compact"]] as const) {
  test(`the titlebar spans the window, the right panels open below it and nothing in it moves${name}`, { timeout: 120000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, page, "shell-result", 90000)) as Record<string, unknown>;
    if (process.env.SHELL_SEEN) console.log(JSON.stringify(seen, null, 2));
    assert.equal(seen.error, undefined);
    for (const run of ["expanded", "folded", "narrow"]) {
      const panels = seen[run] as Panels;
      for (const step of ["filesOpening", "workersOpening", "filesClosing", "filesAgain", "workersClosing", "filesClosingLast"]) {
        assert.deepEqual(panels[step], [], `${run}, ${step}: no titlebar control, divider or the left sidebar moved, and the window didn't scroll`);
      }
      assert.equal(panels.toggleLast, "Right sidebar", `${run}: the right sidebar toggle is the titlebar's last control`);
      assert.ok(Number(panels.toggleToEdge) <= 12, `${run}: flush with the window's end edge (${panels.toggleToEdge}px)`);
      assert.equal(panels.filesOpen.top, 0, `${run}: Files opens right below the titlebar`);
      assert.equal(panels.filesOpen.row, 0, `${run}: Files, Search and Source in a row at the panel's top`);
      assert.equal(panels.workersOpen.state, "expanded", `${run}: Workers opens`);
      assert.ok(panels.workersOpen.width > 100, `${run}: at its width`);
      assert.equal(panels.workersOpen.top, 0, `${run}: Workers opens right below the titlebar`);
      assert.equal(panels.workersOpen.row, 0, `${run}: its tabs in a row at the panel's top`);
    }
    // Expanded: the controls over the sidebar, the divider at its end edge, the tabs after it.
    const expanded = seen.expandedStart as Start;
    assert.deepEqual(expanded.nav, ["Back", "Forward", "Hide sidebar"]);
    assert.equal(expanded.newChatEnd, null);
    assert.ok(Math.abs(expanded.divider - expanded.sidebarEnd) <= 1, `the divider at the sidebar's end edge: ${JSON.stringify(expanded)}`);
    assert.ok(expanded.firstTab > expanded.divider, "the tabs start after the divider");
    assert.equal(expanded.rule.shown, true);
    // Folded: New chat after the toggle, then the divider, then the tabs.
    const folded = seen.foldedStart as Start;
    assert.deepEqual(folded.nav, ["Back", "Forward", "Show sidebar", "New chat"]);
    assert.ok(folded.newChatEnd !== null && folded.divider > folded.newChatEnd && folded.divider - folded.newChatEnd <= 8, `the divider right after New chat: ${JSON.stringify(folded)}`);
    assert.ok(folded.firstTab > folded.divider && folded.firstTab - folded.divider <= 10, `the tabs right after the divider: ${JSON.stringify(folded)}`);
    assert.deepEqual(seen.folding, [], "folding the sidebar leaves the toolbar where it is");
  });
}
