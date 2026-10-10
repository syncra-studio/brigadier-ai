import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Button = { shown: boolean; tabbable: boolean; size: number[]; aboveStack: number; offCentre: number; dots: boolean };
type Toggle = { before: number; after: number; distance: number };

/** Within half a pixel: rows lay out on fractions, the view scrolls by whole pixels. */
function near(actual: number | null, expected: number, label: string): void {
  assert.ok(actual !== null && Math.abs(actual - expected) <= 0.5, `${label}: ${actual}, not ${expected}`);
}

test("the thread scrolls like ChatGPT's in the conversation view", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-scroll.html?drive=1", "thread-scroll-result", 90000)) as {
    error?: string;
    firstOpen: { distance: number; lastGap: number; button: Button; topFade: number; bottomFade: number };
    button: { at8: Button; at9: Button };
    switchBack: { top: number; restored: number; offset: number; restoredOffset: number | null; early: number[] };
    revealedBack: { offset: number; restoredOffset: number | null };
    fullscreen: {
      hiddenHeight: number;
      middle: { top: number; restored: number; offset: number; restoredOffset: number; button: boolean };
      bottom: number;
    };
    expand: { midOpen: Toggle; midClose: Toggle; userOpen: Toggle; bottomOpen: Toggle; buttonAfter: Button; bottomClose: Toggle };
    composerGrows: {
      before: { top: number; stackTop: number };
      grown: { top: number; stackTop: number; distance: number; button: Button };
      atBottom: { lastGap: number; button: Button };
      shrunk: { distance: number; lastGap: number };
    };
    send: {
      topPadding: number;
      placed: { userTop: number; distance: number; room: number; belowUser: number };
      unfollowed: { top: number; tops: number[]; distance: number; button: Button };
      followed: { distances: number[]; grew: number };
      shrunkWindow: number;
      grownWindow: number;
    };
    resizeAtBottom: { placed: number; shrunk: number; grown: number };
    newChat: { userTop: number | null; top: number };
    capsule: { atBottom: { capsule: boolean; capsuleInStack: number | null; lastGap: number; distance: number }; button: Button };
    working: { distance: number; rows: number };
  };
  assert.equal(seen.error, undefined);

  // A first open lands at the bottom: the last content 84px above the composer, no button.
  const { firstOpen } = seen;
  assert.equal(firstOpen.distance, 0);
  near(firstOpen.lastGap, 84, "last content to the composer");
  assert.equal(firstOpen.button.shown, false);
  assert.equal(firstOpen.button.tabbable, false);
  near(firstOpen.topFade, 20, "fade under the top bar");
  near(firstOpen.bottomFade, 30, "fade above the composer");

  // The button: hidden within 8px of the bottom, shown past it; a 32px circle 24px above the
  // composer, centred on the column.
  assert.equal(seen.button.at8.shown, false);
  assert.equal(seen.button.at9.shown, true);
  assert.equal(seen.button.at9.tabbable, true);
  assert.deepEqual(seen.button.at9.size, [32, 32]);
  near(seen.button.at9.aboveStack, 24, "button above the composer");
  near(seen.button.at9.offCentre, 0, "button off the column's centre");

  // A switch away and back restores the exact place, while the history and long replies load late.
  const { switchBack } = seen;
  assert.equal(switchBack.restored, switchBack.top);
  assert.equal(switchBack.restoredOffset, switchBack.offset);
  assert.ok(switchBack.early.every((top) => top <= switchBack.top), `no overshoot: ${switchBack.early.join(", ")}`);
  assert.equal(switchBack.early.at(-1), switchBack.top);
  // Earlier turns, once shown, stay shown: their row is found again.
  assert.equal(seen.revealedBack.restoredOffset, seen.revealedBack.offset);

  // A fullscreen side panel hiding the thread and back: the same place, mid-scroll and at the bottom.
  const { fullscreen } = seen;
  assert.equal(fullscreen.hiddenHeight, 0);
  assert.equal(fullscreen.middle.restored, fullscreen.middle.top);
  assert.equal(fullscreen.middle.restoredOffset, fullscreen.middle.offset);
  assert.equal(fullscreen.middle.button, true);
  assert.equal(fullscreen.bottom, 0);

  // A toggle stays where it was, opening or closing, mid-scroll and at the bottom.
  const { expand } = seen;
  for (const [name, toggle] of Object.entries(expand)) {
    if ("before" in toggle) near(toggle.after, toggle.before, `${name} toggle`);
  }
  assert.ok(expand.bottomOpen.distance > 8, "opening at the bottom leaves the view above it");
  assert.equal(expand.buttonAfter.shown, true);
  assert.equal(expand.bottomClose.distance, 0);

  // The composer growing doesn't scroll (it covers content, the button shows); the gaps are to
  // the top of what floats over the thread.
  const { composerGrows } = seen;
  assert.equal(composerGrows.grown.top, composerGrows.before.top);
  assert.ok(composerGrows.grown.stackTop < composerGrows.before.stackTop);
  assert.equal(composerGrows.grown.distance, Math.round(composerGrows.before.stackTop - composerGrows.grown.stackTop));
  assert.equal(composerGrows.grown.button.shown, true);
  near(composerGrows.atBottom.lastGap, 84, "last content to the notice");
  near(composerGrows.atBottom.button.aboveStack, 24, "button above the notice");
  assert.equal(composerGrows.shrunk.distance, 0);
  near(seen.capsule.atBottom.capsuleInStack, 0, "capsule at the top of the footer");
  assert.equal(seen.capsule.atBottom.capsule, true);
  near(seen.capsule.atBottom.lastGap, 84, "last content to the capsule");
  near(seen.capsule.button.aboveStack, 24, "button above the capsule");

  // A send: the turn 171px under the view's top, room down to the composer; the answer streams
  // without moving the view, and is followed once the user goes to the bottom.
  const { send } = seen;
  near(send.topPadding, 32, "the list's top padding");
  near(send.placed.userTop, 171, "sent message under the view's top");
  assert.equal(send.placed.distance, 0);
  assert.ok(send.placed.room > 0, "room under the sent turn");
  assert.deepEqual(send.unfollowed.tops, [send.unfollowed.top]);
  assert.ok(send.unfollowed.distance > 8, "the answer went on under the composer");
  assert.equal(send.unfollowed.button.shown, true);
  assert.equal(send.unfollowed.button.dots, true);
  assert.deepEqual(send.followed.distances, [0]);
  assert.ok(send.followed.grew > 0);
  assert.equal(send.shrunkWindow, 0);
  assert.equal(send.grownWindow, 0);
  assert.deepEqual(seen.resizeAtBottom, { placed: 0, shrunk: 0, grown: 0 });

  // The first message of a new chat sits at the top padding; an unseen working chat opens at
  // its bottom.
  near(seen.newChat.userTop, 32, "first message of a new chat");
  assert.equal(seen.newChat.top, 0);
  assert.equal(seen.working.distance, 0);
  assert.ok(seen.working.rows >= 2);
});
