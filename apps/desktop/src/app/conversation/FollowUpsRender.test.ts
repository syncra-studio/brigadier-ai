import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("the conversation moves a follow-up through deciding, waiting and admission without a duplicate, and shows the thread's new-tab offer and sent-on notice", async (t) => {
  const { offered, noticeText, settled, ...result } = JSON.parse(await renderFixturePage(t, "follow-ups.html", "follow-ups-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(result, {
    idle: { rail: 0, chat: 1 },
    deciding: { rail: 1, chat: 0 },
    lateResponse: { rail: 1, chat: 0 },
    waiting: { rail: 1, chat: 0 },
    joined: { rail: 0, chat: 1 },
    removed: { rail: 0, chat: 0 },
  });
  // A follow-up the thread offered as a new tab, and one it sent on to another tab.
  assert.equal(offered, 1);
  assert.ok(String(noticeText).startsWith("Sent to “Login page”: Rename the login file.Undo"), String(noticeText));
  assert.deepEqual(settled, [{ method: "settleMoved", conversationId: "flow-session", itemId: "moved", undo: true }]);
});
