import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixturePage } from "@/fixtures/headless";

test("a session's context card opens from its toolbar where it floats, also while another tab is in front", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "context-card.html", "context-card-result", 3000)) as Record<string, unknown>;
  assert.equal(seen.layout, "float", "a narrow window floats the card");
  assert.equal(seen.coveredBefore, true);
  assert.equal(seen.chatInFront, true, "the button brings the chat back");
  assert.equal(seen.openedOverTab, true, "and the card opens over it");
  assert.equal(seen.closed, true);
  assert.equal(seen.openedOverChat, true);
});
