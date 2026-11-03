import assert from "node:assert/strict";
import { test } from "node:test";
import { renderFixtureLive } from "@/fixtures/headless";
import type { Request } from "@/ipc/generated";

test("question fields paste image and long-text chips, grow with Shift+Enter, and send a plain answer with the saved image path and pasted text", { timeout: 90000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "question-card-paste.html", "question-card-paste-result", 60000)) as {
    error?: string; chips: { image: boolean; paste: boolean }; labels: string; mainDraft: string; multiline: boolean; beforeEnter: number;
    answer: Extract<Request, { method: "answerQuestion" }>;
    prepared: Extract<Request, { method: "prepareCardAnswer" }>;
    emptyChip: boolean; escapeFocus: boolean; mentionAnswered: boolean; restored: { image: boolean; paste: boolean }; dropIgnored: boolean; failedKept: { image: boolean; paste: boolean }; answersAfterFailure: number;
  };
  assert.equal(seen.error, undefined);
  assert.deepEqual(seen.chips, { image: true, paste: true });
  assert.match(seen.labels, /Image #1/);
  assert.match(seen.labels, /Pasted text #1/);
  assert.equal(seen.mainDraft, "Main composer draft");
  assert.equal(seen.multiline, true);
  assert.equal(seen.beforeEnter, 0);
  assert.match(seen.prepared.text, /^\[Image #1\] +and \[Pasted text #1\] +in @src\/main.ts *\nUse these details$/);
  assert.deepEqual(seen.prepared.attachments.map(({ pasted, inline }) => ({ pasted, inline })), [{ pasted: false, inline: 1 }, { pasted: true, inline: 1 }]);
  const answer = seen.answer.answers[0]!;
  assert.match(answer, /^\/tmp\/brigadier-card-images\/[a-f0-9]+-image\.png +and /);
  assert.ok(answer.includes("Keep the screenshot and these review notes together.\n".repeat(55)));
  assert.ok(answer.endsWith("\nUse these details"));
  assert.equal(seen.mentionAnswered, false);
  assert.deepEqual(seen.prepared.mentions, [{ type: "file", path: "src/main.ts" }]);
  assert.deepEqual(seen.restored, { image: true, paste: true });
  assert.equal(seen.dropIgnored, true);
  assert.deepEqual(seen.failedKept, { image: true, paste: true });
  assert.equal(seen.answersAfterFailure, 1);
  assert.equal(seen.emptyChip, true);
  assert.equal(seen.escapeFocus, true);
});
