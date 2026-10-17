import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";

test("a session's context card opens from its toolbar where it floats, also while another tab is in front", { timeout: 90000 }, async (t) => {
  // In real time: the pane's layout comes from a resize observer, which only runs as frames are
  // drawn, and virtual time can run past those on a busy machine.
  const seen = JSON.parse(await renderFixtureLive(t, "context-card.html", "context-card-result", 60000)) as Record<string, unknown>;
  assert.equal(seen.layout, "float", "a narrow window floats the card");
  assert.equal(seen.coveredBefore, true);
  assert.equal(seen.chatInFront, true, "the button brings the chat back");
  assert.equal(seen.openedOverTab, true, "and the card opens over it");
  assert.equal(seen.closed, true);
  assert.equal(seen.openedOverChat, true);
  assert.equal(seen.cardText, "Nothing here yet", "a session with no setup yet still shows its card");
  assert.equal(seen.keptAfterClick, true, "a click on the thread leaves the card shown");
  assert.equal(seen.keptAfterWiden, true, "widening the window keeps it shown, beside the thread");
  assert.equal(seen.keptAfterNarrow, true, "narrowing it again keeps it shown, floating");
});
