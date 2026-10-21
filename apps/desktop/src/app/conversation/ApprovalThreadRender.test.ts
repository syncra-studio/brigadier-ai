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
  codeBoxes: number;
  textInset: number | null;
  textSize: number;
  textWeight: string;
  foreground: boolean;
  whiteSpace: string;
  text: string;
  textHeight: number;
  contentFits: boolean;
};

const KINDS = ["command", "outward", "landing", "action", "outline", "peer", "browser", "bare"] as const;
const PROSE = ["action", "outline", "peer", "browser"] as const;

// The thread's decided approval cards against the design (app.pen #352 ApprovalCard, #381
// ApprovalCardView): every kind's body 16 in from the card's edges, 8 between its parts; the
// header 14 / 16 / 10; the footer 10 / 16 / 14 and as tall as its words.
test("a decided approval card pads its header, body and footer as the design does, whatever its kind", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "approval-thread.html?drive=1", "approval-thread-result")) as Record<(typeof KINDS)[number], Measured | null> & { error?: string; longProse: Record<(typeof PROSE)[number], Measured> };
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
  for (const kind of PROSE) {
    const card = seen[kind]!;
    assert.equal(card.codeBoxes, 0, `${kind}: prose has no code-surface box`);
    assert.equal(card.textInset, 16, `${kind}: the text itself starts at the body's inset`);
    assert.deepEqual([card.textSize, card.textWeight, card.foreground], [14, "400", true], `${kind}: normal foreground body text`);
    assert.equal(card.whiteSpace, "pre-wrap", `${kind}: preserve plain text lines`);
    const long = seen.longProse[kind];
    assert.ok(long.textHeight > 240, `${kind}: long prose grows beyond the old scroll limit`);
    assert.ok(long.contentFits, `${kind}: long prose and unbroken paths wrap without scrolling`);
  }
  assert.equal(seen.outline!.text, "1. Split the reader and the writer.\n2. Move the retry loop into the writer.\n3. Test a dropped connection.");
  assert.equal(seen.command!.codeBoxes, 2, "command and paths keep their code boxes");
  assert.equal(seen.outward!.codeBoxes, 1, "outward command keeps its code box");
  assert.equal(seen.command!.text, "pnpm --filter importer run sync --target staging");
  assert.equal(seen.outward!.text, "git push origin fix/importer-retries");
});
