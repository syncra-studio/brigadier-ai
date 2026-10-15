import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Button = { shown: boolean; tabbable: boolean; size: number[]; aboveStack: number; offCentre: number; dots: boolean };
type Toggle = { before: number; after: number; distance: number };
/** The share of the way still to go at the last frame before a time, and at the first frame from it on. */
type Around = { before: number; after: number };

/** Within half a pixel: rows lay out on fractions, the view scrolls by whole pixels. */
function near(actual: number | null, expected: number, label: string): void {
  assert.ok(actual !== null && Math.abs(actual - expected) <= 0.5, `${label}: ${actual}, not ${expected}`);
}

test("the thread keeps its place, follows from the bottom and places a sent turn in the conversation view", { timeout: 180000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-scroll.html?drive=1", "thread-scroll-result", 150000)) as {
    error?: string;
    firstOpen: { distance: number; lastGap: number; button: Button; topFade: number; bottomFade: number; buttonOnSolid: number };
    button: { at8: Button; at9: Button };
    switchBack: { top: number; restored: number; offset: number; restoredOffset: number | null; early: number[]; loading: boolean; placeholders: number };
    revealedBack: { offset: number; restoredOffset: number | null };
    fullscreen: {
      hiddenHeight: number;
      middle: { top: number; restored: number; offset: number; restoredOffset: number; button: boolean };
      bottom: number;
    };
    expand: { midOpen: Toggle; midClose: Toggle; userOpen: Toggle; bottomOpen: Toggle; gapOpen: number; buttonAfter: Button; bottomClose: Toggle };
    composerGrows: {
      before: { top: number; stackTop: number };
      grown: { top: number; stackTop: number; distance: number; lastGap: number; button: Button };
      scrolled: { before: number; after: number };
      atBottom: { lastGap: number; button: Button };
      shrunk: { distance: number; lastGap: number };
    };
    send: {
      topPadding: number;
      spring: { moved: number; at100: Around; at200: Around; settledMs: number; overshoot: boolean };
      placed: { turnEnd: number; line: number; distance: number; room: number; userTop: number; viewHeight: number };
      overflowed: { top: number; grew: number; distances: number[]; lastGap: number; button: Button; room: number };
      unfollowed: { top: number; tops: number[]; distance: number; button: Button };
      followed: { distances: number[]; grew: number };
      shrunkWindow: number;
      grownWindow: number;
    };
    resizeAtBottom: { placed: number; shrunk: number; grown: number };
    returnAfterSend: { room: number; distance: number; roomBack: number; lastGap: number; button: boolean };
    sendScrolledUp: { top: number; after: number; button: boolean };
    newChat: { userTop: number | null; top: number };
    capsule: { atBottom: { capsule: boolean; capsuleInStack: number | null; lastGap: number; distance: number }; button: Button };
    peerExchange: { stages: { stage: string; userTop: number; status: string | null; rows: string[] }[]; tops: number[] };
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
  // Solid over the composer, then a 20px fade; the scroll button floats over the text above it,
  // with no strip of its own, and the last row, 84px above the composer, stays clear of the fade.
  near(firstOpen.bottomFade, 20, "fade above the composer");
  assert.ok(firstOpen.buttonOnSolid < 0, `the scroll button floats above the solid background (${firstOpen.buttonOnSolid})`);

  // The button: hidden while the last row reaches at most 8px under the composer's top (the
  // 84px gap above the composer is in view), shown past it; a 32px circle 24px above the
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
  // While its history loads the column stays empty, saying so only to a screen reader.
  assert.deepEqual([switchBack.loading, switchBack.placeholders], [true, 0]);
  // Earlier turns, once shown, stay shown: their row is found again.
  assert.equal(seen.revealedBack.restoredOffset, seen.revealedBack.offset);

  // A fullscreen side panel hiding the thread and back: the same place, mid-scroll and at the bottom.
  const { fullscreen } = seen;
  assert.equal(fullscreen.hiddenHeight, 0);
  assert.equal(fullscreen.middle.restored, fullscreen.middle.top);
  assert.equal(fullscreen.middle.restoredOffset, fullscreen.middle.offset);
  assert.equal(fullscreen.middle.button, true);
  assert.equal(fullscreen.bottom, 0);

  // Mid-scroll a toggle stays where it was, opening or closing. At the bottom, following, the
  // bottom stays: the opened fold shows above the composer, the toggle moving up, then down.
  const { expand } = seen;
  for (const name of ["midOpen", "midClose", "userOpen"] as const) {
    near(expand[name].after, expand[name].before, `${name} toggle`);
  }
  assert.equal(expand.bottomOpen.distance, 0);
  assert.ok(expand.bottomOpen.after < expand.bottomOpen.before - 20, "the opened fold pushed its toggle up");
  near(expand.gapOpen, 84, "opened fold to the composer");
  assert.equal(expand.buttonAfter.shown, false);
  assert.equal(expand.bottomClose.distance, 0);
  near(expand.bottomClose.after, expand.bottomOpen.before, "closed toggle back where it was");

  // The composer growing at the bottom keeps the bottom: the content moves up with it, its gap to
  // the notice kept. Scrolled up, the view stays still. The gaps are to the top of what floats
  // over the thread.
  const { composerGrows } = seen;
  assert.ok(composerGrows.grown.stackTop < composerGrows.before.stackTop);
  assert.equal(composerGrows.grown.top - composerGrows.before.top, Math.round(composerGrows.before.stackTop - composerGrows.grown.stackTop));
  assert.equal(composerGrows.grown.distance, 0);
  near(composerGrows.grown.lastGap, 84, "last content to the grown footer");
  assert.equal(composerGrows.grown.button.shown, false);
  assert.equal(composerGrows.scrolled.after, composerGrows.scrolled.before);
  near(composerGrows.atBottom.lastGap, 84, "last content to the notice");
  near(composerGrows.atBottom.button.aboveStack, 24, "button above the notice");
  assert.equal(composerGrows.shrunk.distance, 0);
  near(seen.capsule.atBottom.capsuleInStack, 0, "capsule at the top of the footer");
  assert.equal(seen.capsule.atBottom.capsule, true);
  near(seen.capsule.atBottom.lastGap, 84, "last content to the capsule");
  near(seen.capsule.button.aboveStack, 24, "button above the capsule");

  // A send: a critically damped spring of about half a second puts the turn's end a third of the
  // way down the view (at least 240px), room below it down to the composer. The answer fills the
  // room without moving the view and is followed past it. Scrolled away, it goes on under the
  // composer; back at the bottom, it is followed again.
  const { send } = seen;
  near(send.topPadding, 32, "the list's top padding");
  assert.ok(send.spring.moved > 100, `the spring moved ${send.spring.moved}px`);
  assert.equal(send.spring.overshoot, false);
  // Between 0.35 and 0.75 of the way left at 100ms, 0.04 and 0.25 at 200ms: no frame before the
  // time is already past the lower share, none from it on still short of the higher one (a slow
  // machine's frames come far apart, so the time itself can fall between two).
  const { at100, at200 } = send.spring;
  assert.ok(at100.before > 0.35 && at100.after < 0.75, `left around 100ms: ${at100.before} before, ${at100.after} from it on`);
  assert.ok(at200.before > 0.04 && at200.after < 0.25, `left around 200ms: ${at200.before} before, ${at200.after} from it on`);
  assert.ok(send.spring.settledMs >= 300 && send.spring.settledMs <= 700, `settled in ${send.spring.settledMs}ms`);
  near(send.placed.turnEnd, send.placed.line, "sent turn's end");
  assert.ok(send.placed.line >= Math.max(240, send.placed.viewHeight / 4), `line at ${send.placed.line}`);
  assert.equal(send.placed.distance, 0);
  assert.ok(send.placed.room > 0, "room under the sent turn");
  assert.ok(send.overflowed.grew > 0, "followed past the room");
  assert.deepEqual(send.overflowed.distances, [0]);
  near(send.overflowed.lastGap, 84, "followed answer to the composer");
  assert.equal(send.overflowed.button.shown, false);
  assert.equal(send.overflowed.room, 0);
  assert.deepEqual(send.unfollowed.tops, [send.unfollowed.top]);
  assert.ok(send.unfollowed.distance > 200, "the answer went on under the composer");
  assert.equal(send.unfollowed.button.shown, true);
  assert.equal(send.unfollowed.button.dots, true);
  assert.deepEqual(send.followed.distances, [0]);
  assert.ok(send.followed.grew > 0);
  assert.equal(send.shrunkWindow, 0);
  assert.equal(send.grownWindow, 0);
  assert.deepEqual(seen.resizeAtBottom, { placed: 0, shrunk: 0, grown: 0 });

  // Left right after a send and back once it answered: the bottom, without the room.
  const { returnAfterSend } = seen;
  assert.ok(returnAfterSend.room > 0);
  assert.equal(returnAfterSend.distance, 0);
  assert.equal(returnAfterSend.roomBack, 0);
  near(returnAfterSend.lastGap, 84, "returned to the bottom");
  assert.equal(returnAfterSend.button, false);
  // Scrolled up, a send doesn't move the view: the turn is added below, the scroll button shown.
  assert.deepEqual(seen.sendScrolledUp, { top: seen.sendScrolledUp.top, after: seen.sendScrolledUp.top, button: true });

  // The first message of a new chat sits at the top padding; an unseen working chat opens at
  // its bottom.
  near(seen.newChat.userTop, 32, "first message of a new chat");
  assert.equal(seen.newChat.top, 0);
  // A sent turn that messages another session and waits for its reply: its row goes from sending
  // straight to waiting, with no line in between, and the user's message never moves.
  assert.deepEqual(seen.peerExchange.stages.map(({ stage, status, rows }) => [stage, status, rows.at(-1) ?? null]), [
    ["starting", "Starting", null],
    ["running", "Thinking", null],
    ["call", "Sending a message", null],
    ["recorded", null, "Sending message to A long session"],
    ["delivered", null, "Sending message to A long session"],
    ["sent", null, "Sending message to A long session"],
    ["waiting", null, "Waiting for A long session to reply"],
    ["waited", "Thinking", "Sent message to A long session, waited; no reply yet"],
    ["again", null, "Sending message to A long session"],
    // A retry stalling the turn still says so.
    ["retrying", "Reconnecting", "Sending message to A long session"],
  ]);
  assert.equal(seen.peerExchange.tops.length, 1, `the user's message moved: ${seen.peerExchange.tops.join(", ")}`);
  assert.equal(seen.working.distance, 0);
  assert.ok(seen.working.rows >= 2);
});
