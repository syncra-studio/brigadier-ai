import assert from "node:assert/strict";
import { test } from "node:test";

import { codeParts, questionAnswers, questionRound, questionRowWords } from "@/app/conversation/cards/questionRound";
import type { Question } from "@/ipc/generated";

test("a card line shows its backticked names as code, without the backticks", () => {
  assert.deepEqual(codeParts("Merge `brigadier/c/session` into `main`?"), [
    { text: "Merge ", code: false },
    { text: "brigadier/c/session", code: true },
    { text: " into ", code: false },
    { text: "main", code: true },
    { text: "?", code: false },
  ]);
  assert.deepEqual(codeParts("Which format?"), [{ text: "Which format?", code: false }]);
  assert.deepEqual(codeParts("An unclosed ` stays"), [{ text: "An unclosed ` stays", code: false }]);
});

test("a conflicted merge card preserves its warning and resolution choice", () => {
  const item = {
    text: "Merge `session` into `main`? 1 file conflicts with `main`: `README.md`.",
    options: [
      { label: "Merge & resolve conflicts", description: null },
      { label: "Not yet", description: null },
    ],
    recommended: null,
  };
  const question: Question = {
    id: "merge-card", conversationId: "session", taskId: null, requestId: null,
    position: 1,
    kind: { type: "merge", branch: "session", base: "main", conflicted: true, conflicts: ["README.md"] },
    text: item.text, options: [], recommended: null, items: [item],
    answer: null, answers: [], createdAtMs: 1, answeredAtMs: null,
  };
  assert.deepEqual(questionRound(question), [item]);
  assert.equal(questionRowWords(question), "Asking whether to merge into main");
  assert.deepEqual(questionAnswers({
    ...question, answer: "Merge & resolve conflicts", answers: ["Merge & resolve conflicts"], answeredAtMs: 2,
  }), ["Merge & resolve conflicts"]);
  assert.deepEqual(questionAnswers({ ...question, answer: "Not yet", answers: ["Not yet"], answeredAtMs: 2 }), ["Not yet"]);
});
