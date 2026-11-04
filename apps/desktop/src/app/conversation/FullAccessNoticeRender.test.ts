import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("the Full access card is the full warning at nine tenths size, rounded like the composer", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixturePage(t, "full-access-notice.html?drive=1", "full-access-notice-result", 10000),
  ) as {
    title: string | null;
    paragraphs: number;
    warning: string | null;
    fontSize: number;
    padding: [number, number];
    icon: number;
    radius: number;
    composerRadius: number;
    height: number;
    buttons: (string | null)[];
    lastBottom: number;
    noticeTop: number;
  };
  // The card's title over its warning.
  assert.equal(seen.title, "Full access is on");
  assert.equal(seen.paragraphs, 2);
  // It ends on the risks: no "Learn more about elevated risks." (D92).
  assert.match(seen.warning ?? "", /unexpected changes\.$/);
  // Nine tenths of a card's text-sm, px-4 py-3 and 16px icon; heights on whole pixels.
  assert.equal(seen.fontSize, 12.6);
  assert.deepEqual(seen.padding, [11, 14.4]);
  assert.equal(seen.height % 1, 0, `a whole-pixel height, was ${seen.height}px`);
  assert.equal(seen.icon, 14.4);
  // Rounded like the composer under it.
  assert.equal(seen.radius, seen.composerRadius);
  assert.ok(seen.radius >= 18, `rounder than a card, was ${seen.radius}px`);
  assert.deepEqual(seen.buttons, ["Don’t show again"]);
  // At the bottom, the thread's last row ends above the card: the footer's height is room.
  assert.ok(seen.lastBottom <= seen.noticeTop, `last row ${seen.lastBottom} under the card at ${seen.noticeTop}`);
});

test("the Full access card steps aside while approvals wait above the composer", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixturePage(t, "full-access-notice.html?drive=1&waiting=1", "full-access-notice-result", 10000),
  ) as { approvals: boolean; notice: boolean };
  assert.equal(seen.approvals, true);
  assert.equal(seen.notice, false);
});

test("the Full access card steps aside while messages wait in the queue, and stays gone once it empties (issue 448, demo12)", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixturePage(t, "full-access-notice.html?drive=1&queued=1", "full-access-notice-result", 10000),
  ) as { before: boolean; rail: boolean; withQueue: boolean; after: boolean };
  assert.deepEqual(seen, { before: true, rail: true, withQueue: false, after: false });
});

test("the Full access card counts as seen only in a session where it was on screen while the window could be seen (issue 722)", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "full-access-seen.html", "full-access-seen-result", 10000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen, {
    // Drawn in a hidden window and stepped aside: still there once the window shows.
    hiddenDrawn: true,
    hiddenAfter: true,
    // Seen in A, aside in B where it never showed: A still shows it.
    aShown: true,
    bAside: false,
    closedByB: false,
    aAgain: true,
    // Seen in A and stepped aside there: gone in every session (D191, D216).
    closed: true,
    aAfter: false,
    cAfter: false,
  });
});
