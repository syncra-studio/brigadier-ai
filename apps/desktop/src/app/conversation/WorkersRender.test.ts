import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage, renderFixtureLive } from "@/fixtures/headless";

type Bounds = { width: number; overflow: number; content: { slot: string; left: number; right: number }[] };
test("workers fit narrow panels, open details from their pill, keep close reachable and offer no Plan", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "workers-ux.html?panel-test=1&approvals=0&sidebar=0", "workers-result", 15000));
  assert.equal(seen.error, undefined);
  for (const name of ["wide", "narrow", "expanded", "minimum"]) {
    const bounds = seen[name] as Bounds;
    assert.ok(bounds.overflow <= 1, `${name}: horizontal overflow ${bounds.overflow}`);
    assert.ok(bounds.content.length >= 2, `${name}: brief and code are rendered`);
    for (const box of bounds.content) assert.ok(box.left >= -1 && box.right >= -1, `${name}: ${JSON.stringify(box)}`);
  }
  assert.ok(seen.narrow.width <= 320);
  for (const name of ["details", "toggled", "noMenu", "noTabs", "closeInHeader", "listClose", "closed", "workerClosed", "headerFits", "noPlan"]) assert.equal(seen[name], true, `${name}: ${JSON.stringify(seen)}`);

  assert.match(seen.tooltip, /Worker details.*GPT-6.1 Sol.*High/);
});

// ResizeObserver needs real browser frames; virtual-time DOM dumping does not deliver them.
test("a brief gains Show brief when its panel narrows", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "workers-ux.html?panel-test=1&live-resize=1&approvals=0&sidebar=0", "workers-result"));
  assert.equal(seen.error, undefined);
  assert.equal(seen.shortWide, false, JSON.stringify(seen.shortWideBounds));
  assert.equal(seen.shortNarrow, true, JSON.stringify(seen.shortMetrics));
});

test("the worker view is a region named for its worker that nothing hides from the accessibility tree", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "workers-ux.html?panel-test=1&approvals=0&sidebar=0", "workers-result", 15000));
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.region, { role: "region", label: "Worker: File summary" });
  assert.deepEqual(seen.hiddenBy, []);
});

test("the worker view's header gives the title its room first in a 1280-wide window: the model pill shrinks instead", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "workers-ux.html?header-test=1&approvals=0", "workers-header-result"));
  assert.equal(seen.error, undefined);
  assert.equal(seen.window, 1280);
  assert.ok(seen.title.shown >= seen.title.full, `the title is cut short: ${JSON.stringify(seen)}`);
  assert.ok(seen.pill.shown > 0, `the model pill is gone: ${JSON.stringify(seen)}`);
});

test("a worker's task-1.diff chip opens its diff in colour, titled for the worker, within the window and scrolling", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "workers-ux.html?diff-test=1&approvals=0", "workers-diff-result"));
  assert.equal(seen.error, undefined);
  const { colouredLines, ...rest } = seen;
  assert.deepEqual(rest, { title: "Diff of “File summary”", fits: true, scrolls: true });
  // Every added line tinted, as the diff engine draws them.
  assert.ok(colouredLines >= 240, `coloured lines: ${colouredLines}`);
});
