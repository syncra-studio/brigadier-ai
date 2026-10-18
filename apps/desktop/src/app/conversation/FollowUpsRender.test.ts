import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("the conversation moves a follow-up through deciding, waiting and admission without a duplicate, and shows the thread's new-session offer until the user picks, and its sent-on notice", async (t) => {
  const { offered, ranHere, opened, noticeText, settled, ...result } = JSON.parse(await renderFixturePage(t, "follow-ups.html", "follow-ups-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(result, {
    idle: { rail: 0, chat: 1 },
    deciding: { rail: 1, chat: 0 },
    lateResponse: { rail: 1, chat: 0 },
    waiting: { rail: 1, chat: 0 },
    joined: { rail: 0, chat: 1 },
    removed: { rail: 0, chat: 0 },
  });
  // A follow-up the thread offered as a new session, and one it sent on to another session.
  assert.deepEqual(offered, ["Open as a new session", "Run here"]);
  assert.deepEqual(ranHere, { choices: [], rail: 1 }, "Run here: it stays queued, without the offer");
  assert.deepEqual(opened, { choices: [], rail: 0 }, "Open as a new session: it leaves the queue");
  assert.ok(String(noticeText).startsWith("Sent to “Login page”: Rename the login file.Undo"), String(noticeText));
  assert.deepEqual(settled, [
    { method: "runQueuedHere", conversationId: "flow-session", itemId: "follow-up" },
    { method: "deleteQueued", conversationId: "flow-session", itemId: "follow-up" },
    { method: "startSession", from: "flow-session", text: "Follow-up awaiting admission" },
    { method: "settleMoved", conversationId: "flow-session", itemId: "moved", undo: true },
  ]);
});
