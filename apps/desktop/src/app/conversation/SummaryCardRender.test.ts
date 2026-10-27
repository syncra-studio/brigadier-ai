import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("the summary card starts closed, opens by itself once on a new session's first send (floating where narrow) and for a session's first project and branch, stays closed once closed, and never covers the thread", { timeout: 90000 }, async (t) => {
  // In real time: the pane's layout comes from a resize observer, which only runs as frames are drawn.
  const seen = JSON.parse(await renderFixtureLive(t, "summary-card.html", "summary-card-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined);
  assert.equal(seen.freshClosed, true, "a fresh install's new chat has its card closed");
  assert.equal(seen.draftClosed, true, "the new session's screen has its card closed");
  assert.equal(seen.firstSendOpened, true, "its first send opens the card by itself, though closed on the draft");
  assert.equal(seen.firstSendOnce, true, "only once: closed by the user, it stays closed");
  assert.equal(seen.layout, "shift", "the default window keeps the thread beside the card");
  assert.equal(seen.noSetupClosed, true, "a session with nothing to show yet keeps it closed");
  assert.equal(seen.firstContentOpened, true, "its project and branch open the card by itself");
  assert.match(String(seen.cardText), /release-notes/);
  // The project's branch line (Q91: no Changes row).
  assert.match(String(seen.cardText), /main/);
  assert.doesNotMatch(String(seen.cardText), /Changes/);
  assert.equal(typeof seen.messageRight, "number");
  assert.equal(typeof seen.cardLeft, "number");
  assert.ok((seen.messageRight as number) <= (seen.cardLeft as number),
    `the user's message ends before the card: message right ${String(seen.messageRight)}, card left ${String(seen.cardLeft)}`);
  assert.equal(seen.closedAfterPreview, true, "closed by the user, a new preview leaves it closed");
  assert.equal(seen.closedOnReturn, true, "and coming back to the session too");
  assert.equal(seen.narrowClosed, true, "where it would cover the thread, it doesn't open by itself");
  assert.equal(seen.narrowDraftClosed, true, "a narrow new session's screen has its card closed");
  assert.equal(seen.narrowFirstSendFloats, true, "its first send opens the card once, floating over the thread");
  assert.equal(seen.narrowClosedStays, true, "closed by the user, it stays closed");
});
