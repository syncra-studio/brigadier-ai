import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Edges = { top: boolean; bottom: boolean };
type Look = { height: number; image: string; from: number; width: number; scrolls: boolean; position: string; pointerEvents: string };

// A Settings page's scroll view fades its edges as the conversation's does (ThreadScrollRender
// asserts the conversation's 20px under its top and 32px above its composer): an edge shows
// only while there is more to scroll to that way.
test("settings pages: the conversation's scroll fade at each edge with more to scroll to", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "settings-fade.html?drive=1", "settings-fade-result", 8000)) as {
    scrollable: number;
    background: string;
    clear: string;
    atStart: Edges;
    midway: Edges;
    atEnd: Edges;
    top: Look;
    bottom: Look;
    short: Edges & { scrollable: number };
  };
  assert.ok(seen.scrollable > 400, `the long page scrolls (${seen.scrollable}px)`);
  assert.deepEqual(seen.atStart, { top: false, bottom: true }, "at the top: only the bottom fade");
  assert.deepEqual(seen.midway, { top: true, bottom: true }, "mid-way: both");
  assert.deepEqual(seen.atEnd, { top: true, bottom: false }, "at the end: only the top fade");
  // The conversation's fades: the page's background into transparent, 20px down from the view's
  // top and 32px up from its bottom, over the view (never scrolling with it), across its width,
  // taking no clicks.
  const over = { width: 0, scrolls: false, position: "absolute", pointerEvents: "none", from: 0 };
  assert.deepEqual(seen.top, { ...over, height: 20, image: `linear-gradient(${seen.background}, ${seen.clear})` });
  assert.deepEqual(seen.bottom, {
    ...over,
    height: 32,
    image: `linear-gradient(to top, ${seen.background} 0%, ${seen.background} calc(100% - ${seen.bottom.height}px), ${seen.clear} 100%)`,
  });
  assert.deepEqual(seen.short, { scrollable: 0, top: false, bottom: false }, "a short page: none");
});
