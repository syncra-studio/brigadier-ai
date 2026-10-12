import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("the Full access line is one calm row above the composer that never covers the thread", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixturePage(t, "full-access-notice.html?drive=1", "full-access-notice-result", 10000),
  ) as {
    text: string | null;
    paragraphs: number;
    height: number;
    lineHeight: number;
    buttons: (string | null)[];
    lastBottom: number;
    noticeTop: number;
    closed: boolean;
  };
  // One short line with its link, no paragraph of warnings.
  assert.equal(seen.text, "Full access: workers run without asking. Learn more");
  assert.equal(seen.paragraphs, 1);
  // A notice row's height: one line of text and its padding.
  assert.ok(seen.height <= seen.lineHeight + 14, `one row tall, was ${seen.height}px`);
  assert.deepEqual(seen.buttons, ["Learn more", "Don’t show again", "Close"]);
  // At the bottom, the thread's last row ends above the line: the footer's height is room.
  assert.ok(seen.lastBottom <= seen.noticeTop, `last row ${seen.lastBottom} under the line at ${seen.noticeTop}`);
  assert.equal(seen.closed, true);
});
