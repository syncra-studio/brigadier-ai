import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Measured = {
  header: number[];
  body: number[];
  gap: number;
  insets: number[];
  footer: number[];
  footerFits: boolean;
  bodyHeight: number;
};

const KINDS = ["command", "outward", "landing", "action", "outline", "peer", "browser", "bare"] as const;

// The thread's decided approval cards against the design (app.pen #352 ApprovalCard, #381
// ApprovalCardView): every kind's body 16 in from the card's edges, 8 between its parts; the
// header 14 / 16 / 10; the footer 10 / 16 / 14 and as tall as its words.
test("a decided approval card pads its header, body and footer as the design does, whatever its kind", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "approval-thread.html?drive=1", "approval-thread-result")) as Record<(typeof KINDS)[number], Measured | null> & { error?: string };
  assert.equal(seen.error, undefined);
  for (const kind of KINDS) {
    const card = seen[kind];
    assert.ok(card, `${kind} renders`);
    // Padding: top, right, bottom, left.
    assert.deepEqual(card.header, [14, 16, 10, 16], `${kind}: header`);
    assert.deepEqual([card.body, card.gap], [[0, 16, 0, 16], 8], `${kind}: body`);
    assert.deepEqual(card.footer, [10, 16, 14, 16], `${kind}: footer`);
    assert.ok(card.footerFits, `${kind}: the footer has no height of its own`);
    // Everything the body holds (badge, code blocks, "in <cwd>", diff stat, prose) starts 16 in.
    if (kind !== "bare") assert.deepEqual(card.insets, [16], `${kind}: body parts' inset`);
  }
  // A card with no body: nothing between its header and its footer.
  assert.deepEqual([seen.bare!.insets, seen.bare!.bodyHeight], [[], 0]);
});
