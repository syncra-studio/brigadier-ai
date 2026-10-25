import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import {
  answerWords,
  codeParts,
  multiPicks,
  questionAnswers,
  questionRound,
  questionRowWords,
  uncommittedCard,
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

test("the uncommitted-changes card lists the user's files as theirs and the thread's as the thread's", () => {
  const question: Question = {
    id: "uncommitted", conversationId: "c", taskId: null, requestId: null, position: 1,
    kind: { type: "uncommittedChanges", files: ["a.txt"], threadFiles: ["b.txt"] },
    text: "Your checkout has 2 uncommitted changes: 1 of yours and 1 made by the thread.",
    options: ["Yes, show them to workers", "No, use the last commit"], recommended: null, items: [],
    answer: null, answers: [], createdAtMs: 1, answeredAtMs: null,
  };
  assert.deepEqual(uncommittedCard(question), {
    title: "Should workers see your uncommitted changes?",
    files: [{ path: "a.txt", threads: false }, { path: "b.txt", threads: true }],
  });
  // Only the thread's: the title doesn't call them the user's.
  assert.deepEqual(uncommittedCard({ ...question, kind: { type: "uncommittedChanges", files: [], threadFiles: ["b.txt"] } }), {
    title: "Should workers see the thread's uncommitted changes?",
    files: [{ path: "b.txt", threads: true }],
  });
  // A card stored before the thread's were told apart.
  assert.deepEqual(uncommittedCard({ ...question, kind: { type: "uncommittedChanges", files: ["a.txt"] } })?.files, [
    { path: "a.txt", threads: false },
  ]);
  assert.equal(uncommittedCard(watch), null);
});

const watch: Question = {
  id: "ci-watch", conversationId: "c", taskId: null, requestId: "r", position: 1,
  kind: { type: "orchestrator" },
  text: "Watch CI for `d177d69`? It will probably fail because CI runs Node 24 and the project now requires Node 26.",
  options: ["Watch", "No"], recommended: 0, items: [],
  answer: null, answers: [], createdAtMs: 1, answeredAtMs: null,
};

test("a multi-select answer is its marked options, each worded as the CLI offered it (harness D7)", () => {
  const item = {
    text: "Which colours?",
    options: [{ label: "red", description: null }, { label: "blue", description: null }, { label: "red, or blue", description: null }],
    recommended: 0,
    multiSelect: true,
  };
  assert.deepEqual(multiPicks(item, "red, blue"), [0, 1]);
  assert.deepEqual(multiPicks(item, "blue"), [1]);
  // An option whose label holds ", " is one pick; the user's own words are no picks.
  assert.deepEqual(multiPicks(item, "red, or blue"), [2]);
  assert.equal(multiPicks(item, "green, blue"), null);
  assert.equal(answerWords(item, "red, blue"), "red (Recommended), blue");
  assert.equal(answerWords(item, "green please"), "green please");
  // A one-pick question with no option marked recommended marks nothing.
  const single = { ...item, recommended: null, multiSelect: false };
  assert.equal(answerWords(single, "red, blue"), "red, blue");
  assert.equal(answerWords(single, "red"), "red");
});

test("a question's row says it asks while open, and how many it asked once answered", () => {
  assert.equal(questionRowWords(watch), "Asking a question");
  const answered = { ...watch, answer: "Watch", answeredAtMs: 2 };
  assert.equal(questionRowWords(answered), "Asked 1 question");
  const round: Question = {
    ...watch, text: "First?\nSecond?\nThird?", options: [], recommended: null,
    items: ["First?", "Second?", "Third?"].map((text) => ({ text, options: [], recommended: null })),
    answer: "Yes\nNo\nYes", answers: ["Yes", "No", "Yes"], answeredAtMs: 2,
  };
  assert.equal(questionRowWords(round), "Asked 3 questions");
  assert.equal(questionRowWords({ ...round, answer: null, answers: [], answeredAtMs: null }), "Asking questions");
  // An interview's round names itself once answered (Q78).
  assert.equal(questionRowWords({ ...round, title: "Round 2 · Budget" }), "Asked 3 questions · Round 2 · Budget");
  // Withdrawn, and the merge card, keep their words.
  assert.equal(questionRowWords({ ...watch, answeredAtMs: 2 }), "Asked 1 question · withdrawn");
});

test("the answered question's row says how many it asked and opens to each with its answer; asking, it shimmers", async () => {
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
    assert.match(html, /<span class="truncate">Asked 1 question<\/span>/);
    assert.doesNotMatch(html, /question-answer|→/);
    // The whole question waits in the row's tooltip; the rest opens under it.
    assert.match(html, /title="Watch CI for d177d69\? It will probably fail/);
    // Pending, it says it asks, its words sweeping.
    const pending = render(watch);
    assert.match(pending, /<span class="truncate shimmer">Asking a question<\/span>/);
  } finally {
    await server.close();
  }
});
