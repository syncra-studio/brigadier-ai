import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Look = { cursor: string; fill: string; name: string; words: string };

// #735: the strip's only worker has no hover fill (a07bda08f), but under the pointer it shows it
// opens: the pointer cursor, and its words turn white, as a thread row's do.
test("a lone worker's strip row answers the pointer quietly: a pointer cursor and brighter words, no fill", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "workers-strip-hover.html", "workers-strip-hover-result", 60000)) as {
    error?: string;
    rows: number;
    before: Look;
    hovered: Look;
  };
  assert.equal(seen.error, undefined, seen.error ?? "");
  assert.equal(seen.rows, 1);
  assert.equal(seen.hovered.cursor, "pointer");
  assert.notEqual(seen.hovered.name, seen.before.name, "its name brightens under the pointer");
  assert.notEqual(seen.hovered.words, seen.before.words, "what it does brightens under the pointer");
  assert.equal(seen.hovered.fill, seen.before.fill, "still no hover fill");
});
