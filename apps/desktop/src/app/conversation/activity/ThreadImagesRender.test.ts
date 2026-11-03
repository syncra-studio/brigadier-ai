import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Case = { drawn: number; broken: number; text: string };

// The real components in Chromium, under the app's image policy, over a faked daemon: local paths
// go through the daemon and show as data URLs, and what can't be shown is a quiet tile.
test("an answer's local image and a kept tile show, and what can't be shown says Not available", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "thread-images.html", "thread-images-result", 6000)) as Record<string, Case> & { asked: string[] };
  const drawn = { drawn: 1, broken: 0 };
  const unavailable = { drawn: 0, broken: 0, text: "Not available" };
  const { text: _answer, ...answer } = seen.answer!;
  assert.deepEqual(answer, drawn);
  const { text: _fileUrl, ...fileUrl } = seen["file-url"]!;
  assert.deepEqual(fileUrl, drawn);
  // A tile whose source was deleted reads the session's copy, as a tile and as a proof.
  const { text: _tile, ...tile } = seen.tile!;
  assert.deepEqual(tile, drawn);
  assert.equal(seen.proof!.drawn, 1);
  assert.equal(seen.proof!.broken, 0);
  // Refused, undrawable, not absolute, and a tile with no copy: no broken-image icon.
  assert.deepEqual(seen.refused, unavailable);
  assert.deepEqual(seen.undrawable, unavailable);
  assert.deepEqual(seen.relative, unavailable);
  assert.deepEqual(seen["tile-gone"], unavailable);
  // Only absolute local paths reach the daemon, and a file: URL's path is decoded first.
  assert.deepEqual([...new Set(seen.asked)].sort(), ["/tmp/answer copy.png", "/tmp/answer.png", "/tmp/notes.png", "/tmp/undrawable.png"]);
});
