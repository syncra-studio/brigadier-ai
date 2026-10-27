import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("the conversation moves a follow-up through its queue and admission without a duplicate, and its rail shows Deciding, held, offered and sent to a worker; a message while only its workers work waits there, Deciding, a Chat's too", async (t) => {
  const result = JSON.parse(await renderFixturePage(t, "follow-ups.html", "follow-ups-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(result, {
    idle: { rail: 0, chat: 1 },
    queued: { rail: 1, chat: 0 },
    lateResponse: { rail: 1, chat: 0 },
    joined: { rail: 0, chat: 1 },
    removed: { rail: 0, chat: 0 },
    deciding: { deciding: true, note: "Deciding", buttons: [], notice: null },
    held: { deciding: true, note: "Read when the current command ends", buttons: [], notice: null },
    offered: { deciding: false, note: null, buttons: ["Open as a new session", "Run here"], notice: null },
    sentToWorker: { deciding: false, note: null, buttons: [], notice: `Sent to Docs: ${"Follow-up awaiting admission"}`, rail: 0, chat: 0 },
    whileWorkersWork: { rail: 1, chat: 0, deciding: true },
    chatWhileWorkersWork: { rail: 1, chat: 0, deciding: true },
  });
});
