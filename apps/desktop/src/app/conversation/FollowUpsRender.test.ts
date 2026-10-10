import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("the conversation moves a follow-up through deciding, waiting and admission without a duplicate", async (t) => {
  const result = JSON.parse(await renderFixturePage(t, "follow-ups.html", "follow-ups-result", 10000)) as Record<string, { rail: number; chat: number }>;
  assert.deepEqual(result, {
    idle: { rail: 0, chat: 1 },
    deciding: { rail: 1, chat: 0 },
    lateResponse: { rail: 1, chat: 0 },
    waiting: { rail: 1, chat: 0 },
    joined: { rail: 0, chat: 1 },
    removed: { rail: 0, chat: 0 },
  });
});
