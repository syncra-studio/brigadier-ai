import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Landing = { landed: boolean; offset: number | null };

/** Within a pixel: rows lay out on fractions, the view scrolls by whole pixels. */
function near(actual: number | null, expected: number, label: string): void {
  assert.ok(actual !== null && Math.abs(actual - expected) <= 1, `${label}: ${actual}, not ${expected}`);
}

/** Lands 24px under the view's top. */
function landsAtTop(step: Landing, label: string): void {
  assert.equal(step.landed, true, `${label} landed`);
  near(step.offset, 24, `${label} under the view's top`);
}

test("the rail beside the thread marks the user's messages, jumps to them, and pages older history in", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "message-rail.html?drive=1", "message-rail-result", 150000)) as {
    error?: string;
    opened: {
      rail: boolean; gutter: number; left: number | null; centreOff: number | null; listMax: string | null; windowHeight: number; listScrolls: boolean | null; faded: boolean | null;
      marker: number[] | null; line: number[] | null; markers: string[]; folded: string | null; current: string[]; inView: string[];
      lines: (string | null)[];
    };
    hover: { lines: number[]; previewText: string[]; previewWidth: number | null; previewRightOfRail: number | null; afterLeave: { preview: boolean; line: number } };
    keysFromBottom: Landing & { expected: string; moved: number };
    clickAndKeys: { clicked: Landing; down: Landing; up: Landing; composerMoved: number; current: string[]; inView: string[] };
    scrub: { offset: number | null; flashed: boolean };
    narrow: { narrow: { rail: boolean; gutter: number }; wide: boolean };
    intoFolded: Landing & { first: string; at: number; target: string; folded: boolean; markers: number };
    intoUnloaded: Landing & {
      oldest: string; target: string; autoFailed: boolean; spinner: boolean; spinnerSeen: { role: string | null; text: string | null; classes: string; svg: boolean } | null;
      pages: number; markers: number;
    };
    atTop: {
      spinner: boolean; failed: boolean; failure: { text: string | null; pages: number }; anchor: string; before: number | null; after: number | null;
      loaded: boolean; pages: number; first: string; more: boolean; markers: number; listScrolls: boolean | null; faded: boolean | null;
    };
    longAnswer: { messagesShown: number; current: string[]; inView: string[] };
    short: { rail: boolean; rows: number };
  };
  assert.equal(seen.error, undefined);

  // 40 turns loaded (10 folded behind "10 earlier messages"): a marker for each user message,
  // oldest first, folded ones too, on a rail 16px from the view's start, centred on it.
  const { opened } = seen;
  assert.equal(opened.rail, true);
  assert.ok(opened.gutter >= 48, `room beside the column: ${opened.gutter}`);
  near(opened.left, 16, "rail from the view's start");
  near(opened.centreOff, 0, "rail off the view's centre");
  assert.deepEqual(opened.markers, Array.from({ length: 40 }, (_, index) => `long-u${40 + index}`));
  assert.equal(opened.folded, "10 earlier messages");
  assert.deepEqual(opened.marker, [36, 10]);
  assert.deepEqual(opened.line, [6, 2]);
  // The list stops at 70% of the window or 40rem, whichever is less (40rem in a 976px window, 70%
  // in a shorter one, as a Linux runner's), then scrolls.
  near(Number.parseFloat(opened.listMax ?? ""), Math.min(40 * 16, opened.windowHeight * 0.7), "the list's height cap");
  assert.equal(opened.listScrolls, false);
  assert.equal(opened.faded, false);
  // The turns in view are the current ones.
  assert.ok(opened.current.length > 0);
  assert.deepEqual(opened.current, opened.inView);
  // A current marker is brighter than one at rest.
  assert.notEqual(opened.lines[0], opened.lines[1]);

  // Hovered, the line grows to 26px; its neighbours by 0.7, 0.4 and 0.2 of the growth.
  const { hover } = seen;
  // Markers 17 to 24, the 21st (index 20) hovered: 6px at rest.
  assert.deepEqual(hover.lines, [10, 14, 20, 26, 20, 14, 10, 6]);
  assert.deepEqual(hover.previewText, [
    "Question 61: where does this turn sit in the thread?",
    "Answer 61, paragraph 1. The rail marks each message the user sent, and a jump lands its turn at the top.",
  ]);
  assert.equal(hover.previewWidth, 320);
  assert.ok((hover.previewRightOfRail ?? -1) >= 0, "the preview sits right of the rail");
  assert.deepEqual(hover.afterLeave, { preview: false, line: 6 });

  // ⌥↑ at the bottom: the message above the view's top lands; it no longer follows new content.
  landsAtTop(seen.keysFromBottom, "⌥↑ from the bottom");
  assert.equal(seen.keysFromBottom.moved, 0);

  // A click lands its turn at the top; ⌥↓ the next, ⌥↑ back; in the composer ⌥↑ is the field's.
  const { clickAndKeys } = seen;
  landsAtTop(clickAndKeys.clicked, "click");
  landsAtTop(clickAndKeys.down, "⌥↓");
  landsAtTop(clickAndKeys.up, "⌥↑");
  assert.equal(clickAndKeys.composerMoved, 0);
  assert.deepEqual(clickAndKeys.current, clickAndKeys.inView);

  // A press-and-drag scrubs: the turn under the pointer lands at once, without a flash.
  near(seen.scrub.offset, 24, "scrubbed turn under the view's top");
  assert.equal(seen.scrub.flashed, false);

  // Under 48px beside the column the rail goes, and comes back with the room.
  assert.equal(seen.narrow.narrow.rail, false);
  assert.ok(seen.narrow.narrow.gutter < 48);
  assert.equal(seen.narrow.wide, true);

  // ⌥↑ from the oldest turn shown opens the folded ones and lands on the one before it.
  const { intoFolded } = seen;
  assert.deepEqual([intoFolded.first, intoFolded.at, intoFolded.target], ["long-u50", 10, "long-u49"]);
  landsAtTop(intoFolded, "⌥↑ into the folded turns");
  assert.equal(intoFolded.folded, false);
  assert.equal(intoFolded.markers, 40);

  // ⌥↑ from the oldest message loaded: a page loads, a spinner meanwhile, and its newest lands.
  const { intoUnloaded } = seen;
  assert.deepEqual([intoUnloaded.oldest, intoUnloaded.target], ["long-u40", "long-u39"]);
  assert.equal(intoUnloaded.autoFailed, true, "the page loading by itself at the top failed, as asked");
  assert.equal(intoUnloaded.spinner, true);
  assert.equal(intoUnloaded.spinnerSeen?.role, "status");
  assert.equal(intoUnloaded.spinnerSeen?.text, "Loading older messages…");
  assert.equal(intoUnloaded.spinnerSeen?.svg, true);
  assert.match(intoUnloaded.spinnerSeen?.classes ?? "", /\bflex\b.*\bjustify-center\b.*\bpy-4\b/);
  landsAtTop(intoUnloaded, "⌥↑ into an older page");
  assert.equal(intoUnloaded.pages, 1);
  assert.equal(intoUnloaded.markers, 60);

  // Scrolled to the top the next page loads by itself; it fails, Try again loads it, and the
  // view stays put.
  const { atTop } = seen;
  assert.equal(atTop.spinner, true);
  assert.equal(atTop.failed, true);
  assert.deepEqual(atTop.failure, { text: "Couldn't load older messagesTry again", pages: 0 });
  assert.equal(atTop.loaded, true);
  assert.equal(atTop.anchor, "long-u20");
  near(atTop.after, atTop.before ?? Number.NaN, "the anchor row after the page loads");
  assert.equal(atTop.pages, 1);
  assert.equal(atTop.first, "long-u0");
  assert.equal(atTop.more, false);
  assert.equal(atTop.markers, 80);
  // 80 markers reach past the list's 640px: it scrolls, its ends fading.
  assert.equal(atTop.listScrolls, true);
  assert.equal(atTop.faded, true);

  // Only a long answer in view, its message scrolled away above: that message is the current one.
  assert.deepEqual(seen.longAnswer, { messagesShown: 0, current: ["long-tall-u"], inView: ["long-tall-u"] });

  // Three user messages: no rail.
  assert.deepEqual(seen.short, { rail: false, rows: 3 });
});
