import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

/** The master's shapes: fifteen strokes, the core and its eleven bars. */
const MASTER = { path: 26, circle: 1, rect: 0 };
/** The cut for 20 px: nine stripes and the core. */
const CUT_20 = { path: 0, circle: 1, rect: 9 };

// Drawn small, Windows shows the mark cut for its pixel size and macOS the master as before;
// drawn large, both show the master. Either way it is one image, hidden from screen readers.
for (const [platform, small] of [["macos", MASTER], ["windows", CUT_20]] as const) {
  test(`the Brigadier mark drawn small on ${platform}`, { timeout: 60000 }, async (t) => {
    const seen = JSON.parse(
      await renderFixturePage(t, `brand-glyph.html?platform=${platform}`, "brand-glyph-result"),
    ) as Record<string, unknown>;
    assert.deepEqual(seen, { small, large: MASTER, images: 1, hidden: "true" });
  });
}
