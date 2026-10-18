import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

for (const reduced of [false, true]) {
  test(`new tab tools, history and favicon phases${reduced ? " with reduced motion" : ""}`, { timeout: 90000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, "new-tab.html?drive=1", "new-tab-result", 60000, reduced)) as Record<string, unknown>;
    if (process.env.NEW_TAB_SEEN) console.log(JSON.stringify(seen, null, 2));
    assert.deepEqual(seen.sections, ["Tools", "Suggested"]);
    assert.equal(seen.session, false, "a session's New tab has no New session");
    assert.deepEqual(seen.tools, ["Terminal⌃`", "New page", "Side chat⌥⌘S", "More tools…"]);
    assert.equal(seen.searchFields, 0, "search belongs to the browser toolbar");
    assert.deepEqual(seen.suggested, [{ title: "Latest site", url: "https://one.test/latest" }, { title: "Second site", url: "https://two.test/" }]);
    const requests = seen.historyCalls as { query: string; filter: string; limit: number }[];
    assert.equal(requests[0]?.query, ""); assert.equal(requests[0]?.filter, "all"); assert.equal(requests[0]?.limit, 200);
    const measure = seen.measure as Record<string, unknown>;
    assert.equal((measure.column as { width: number }).width, 768);
    assert.equal(measure.top, 32);
    assert.deepEqual(measure.card, { width: 376, height: 40 });
    assert.equal(measure.radius, 10); assert.equal(measure.rowGap, 4); assert.equal(measure.columnGap, 16);
    assert.equal(measure.font, 13); assert.equal(measure.sectionGap, 40); assert.equal(measure.headingGap, 16);
    assert.deepEqual(measure.tile, { width: 378, height: 104 }); assert.deepEqual(measure.icon, { width: 32, height: 32 }); assert.equal(measure.overflow, false);
    assert.deepEqual(seen.layouts, [
      { width: 320, column: 280, columns: 1, fits: true }, { width: 375, column: 335, columns: 1, fits: true },
      { width: 414, column: 374, columns: 1, fits: true }, { width: 768, column: 728, columns: 2, fits: true },
    ]);
    assert.deepEqual(seen.dismissed, ["Second site"]);
    assert.deepEqual(seen.terminalOptions, ["Open bottom terminal"]);
    assert.equal(seen.bottomTerminal, true); assert.equal(seen.bottomKeptNewTab, "newTab");
    assert.deepEqual(seen.more, ["Find file⌘P"]);
    assert.deepEqual(seen.files, { open: true, tab: "files", placeholder: "newTab" });
    assert.equal(seen.newPage, "page"); assert.equal(seen.terminal, "terminal"); assert.equal(seen.sideChat, "sideChat");
    assert.equal((seen.siteTab as { kind: string; url: string }).kind, "browser"); assert.equal((seen.siteTab as { url: string }).url, "https://two.test/");
    assert.equal(seen.clearedSuggested, true);
    const spinner = seen.spinner as Record<string, unknown>;
    assert.equal(spinner.phase, "spinner-only"); assert.equal(spinner.visible, "false"); assert.equal(spinner.spinner, true);
    const loading = seen.loadingFavicon as Record<string, unknown>;
    assert.equal(loading.phase, "loading-favicon"); assert.equal(loading.image, true); assert.equal(loading.spinner, true);
    assert.equal(loading.transform, "matrix(0.625, 0, 0, 0.625, 0, 0)");
    assert.equal(loading.animation, reduced ? "none" : "browser-tab-spin"); assert.equal(loading.transition, reduced ? "0s" : "0.25s");
    const settled = seen.settled as Record<string, unknown>;
    assert.equal(settled.phase, "settled"); assert.equal(settled.image, true); assert.equal(settled.spinner, false); assert.equal(settled.transform, "matrix(1, 0, 0, 1, 0, 0)");
    assert.equal((seen.failedLoading as Record<string, unknown>).phase, "spinner-only");
    assert.equal((seen.failedSettled as Record<string, unknown>).globe, true); assert.equal((seen.failedSettled as Record<string, unknown>).spinner, false);
    assert.equal(seen.reduced, reduced);
  });
}
test("new tab with no history keeps Tools and omits Suggested", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "new-tab.html?drive=1&empty=1", "new-tab-result")) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["Tools"]); assert.deepEqual(seen.suggested, []);
});
test("recent pages show on the New tab and reopen after every page tab was closed", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "new-tab.html?drive=1&empty=1&pages=1", "new-tab-result")) as Record<string, unknown>;
  assert.deepEqual(seen.sections, ["Tools", "Recents"]);
  assert.deepEqual(seen.rows, ["🚀Release notes5 min ago", "Untitled page3 h ago"]);
  const opened = seen.opened as { kind: string; pageId: string; title: string };
  assert.equal(opened.kind, "page"); assert.equal(opened.pageId, "b".repeat(32)); assert.equal(opened.title, "");
  const reopened = seen.reopened as { kind: string; pageId: string; title: string };
  assert.equal(reopened.kind, "page"); assert.equal(reopened.pageId, "a".repeat(32)); assert.equal(reopened.title, "Release notes");
  assert.equal(seen.tabCount, 1, "the page took the New tab's place");
});
