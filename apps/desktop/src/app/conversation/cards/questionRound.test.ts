import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import {
  askedRow,
  codeParts,
  questionAnswers,
  questionRound,
  questionRowWords,
  shortQuestion,
} from "@/app/conversation/cards/questionRound";
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

const watch: Question = {
  id: "ci-watch", conversationId: "c", taskId: null, requestId: "r", position: 1,
  kind: { type: "orchestrator" },
  text: "Watch CI for `d177d69`? It will probably fail because CI runs Node 24 and the project now requires Node 26.",
  options: ["Watch", "No"], recommended: 0, items: [],
  answer: null, answers: [], createdAtMs: 1, answeredAtMs: null,
};

test("an answered question reads as what it asked, in one line, and the answer", () => {
  assert.equal(shortQuestion(watch.text), "Watch CI for `d177d69`?");
  assert.equal(shortQuestion("Which format\nJSON or YAML?"), "Which format");
  assert.equal(shortQuestion("Is `a?b` right? Or not"), "Is `a?b` right?");
  // Pending: the wording stays.
  assert.equal(askedRow(watch), null);
  assert.equal(questionRowWords(watch), "Asking questions");
  const answered = { ...watch, answer: "Watch", answeredAtMs: 2 };
  assert.deepEqual(askedRow(answered), { asked: "Watch CI for `d177d69`?", more: 0, answer: "Watch" });
  assert.equal(questionRowWords(answered), "Asked: Watch CI for `d177d69`? → Watch");
  // A round: its first question, and how many more.
  const round: Question = {
    ...watch, text: "First?\nSecond?\nThird?", options: [], recommended: null,
    items: ["First?", "Second?", "Third?"].map((text) => ({ text, options: [], recommended: null })),
    answer: "Yes\nNo\nYes", answers: ["Yes", "No", "Yes"], answeredAtMs: 2,
  };
  assert.equal(questionRowWords(round), "Asked: First? +2 more → Yes");
  // Withdrawn, and the merge card, keep their words.
  assert.equal(questionRowWords({ ...watch, answeredAtMs: 2 }), "Asked a question · withdrawn");
  assert.equal(askedRow({ ...answered, kind: { type: "uncommittedChanges" } as Question["kind"] }), null);
});

test("the answered question's row shows the question, its code, and the answer as a chip", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { QuestionCardView } = await server.ssrLoadModule("/src/app/conversation/cards/QuestionCardView.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const render = (question: Question) => {
      const store = createStore(() => ({ board: { ...emptyBoard("c"), questions: { [question.id]: question } }, orchestrator: null }));
      return ReactDOMServer.renderToStaticMarkup(
        React.createElement(BoardStoreContext.Provider, { value: store },
          React.createElement(QuestionCardView, { cardId: question.id })),
      );
    };
    const html = render({ ...watch, answer: "Watch", answeredAtMs: 2 });
    assert.match(html.replace(/<[^>]+>|<!-- -->/g, ""), /Asked: Watch CI for d177d69\?→Watch/);
    assert.match(html, /<code[^>]*>d177d69<\/code>/);
    assert.match(html, /data-slot="question-answer"[^>]*><span class="truncate">Watch<\/span><\/span>/);
    // The whole question waits in the row's tooltip; the rest opens under it.
    assert.match(html, /title="Watch CI for `d177d69`\? It will probably fail/);
    assert.doesNotMatch(html, />Asked a question</);
    // Pending, it still says it asks.
    const pending = render(watch);
    assert.match(pending, />Asking questions</);
    assert.doesNotMatch(pending, /question-answer/);
  } finally {
    await server.close();
  }
});
