import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("the conversation moves a follow-up through its queue and admission without a duplicate, and its rail shows a judged row as any other, offered, and gone with no notice once sent to a worker; a message while only its workers work waits there, a Chat's too; another session's shows who sent it (goal 4)", async (t) => {
  const result = JSON.parse(await renderFixturePage(t, "follow-ups.html", "follow-ups-result", 10000)) as Record<string, unknown>;
  assert.deepEqual(result, {
    idle: { rail: 0, chat: 1 },
    queued: { rail: 1, chat: 0 },
    lateResponse: { rail: 1, chat: 0 },
    joined: { rail: 0, chat: 1 },
    removed: { rail: 0, chat: 0 },
    judged: { label: false, steer: true, buttons: [], notice: null },
    offered: { label: false, steer: true, buttons: ["Open as a new session", "Run here"], notice: null },
    sentToWorker: { label: false, steer: false, buttons: [], notice: null, rail: 0, chat: 0 },
    whileWorkersWork: { rail: 1, chat: 0 },
    chatWhileWorkersWork: { rail: 1, chat: 0 },
    fromPeer: { from: true, words: true, header: false },
  });
});
