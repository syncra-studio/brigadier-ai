import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("a message sent while the session works shows in the chat at once, never as Deciding, and the thread's sent-on notice has Undo", async (t) => {
  const { noticeText, settled, ...result } = JSON.parse(await renderFixturePage(t, "follow-ups.html", "follow-ups-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(result, {
    pending: { rail: 0, chat: 1 },
    sent: { rail: 0, chat: 1, deciding: false },
    legacy: { rail: 1, deciding: false, offers: [] },
    removed: { rail: 0, chat: 1 },
  });
  assert.ok(String(noticeText).startsWith("Sent to “Login page”: Rename the login file.Undo"), String(noticeText));
  assert.deepEqual(settled, [{ method: "settleMoved", conversationId: "flow-session", itemId: "moved", undo: true }]);
});
