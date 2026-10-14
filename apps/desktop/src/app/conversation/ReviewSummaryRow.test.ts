import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import { findingRows } from "@/app/conversation/reviewStatus";
import type { Finding, FindingStatus, OrchestratorStep, ReviewRun, ReviewState } from "@/ipc/generated";

const review = (id: string, startedAtMs: number, state: ReviewState, items: Finding[] = []): ReviewRun => ({
  id,
  conversationId: "c",
  requestId: null,
  taskId: null,
  kind: "code",
  base: "a1",
  tip: `t-${id}`,
  author: "claude",
  reviewer: "codex",
  reviewerModel: "gpt-6.1-sol",
  notify: { type: "orchestrator" },
  state,
  startedAtMs,
  endedAtMs: null,
  findings: null,
  number: 0,
  items,
});

const merged = (atMs: number): OrchestratorStep => ({
  requestId: null,
  kind: { type: "merged", branch: "brigadier/flow", base: "main", commits: 1, askedIn: "m1" },
  atMs,
  position: 0,
});

const item = (id: string, status: FindingStatus, title = `Finding ${id}`): Finding =>
  ({ id, severity: "P3", title, location: "cli.ts:6", status, high: false });

/** What a reader sees: the markup's text, without its tags. */
const words = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
const count = (html: string, slot: string) => html.split(`data-slot="${slot}"`).length - 1;

test("the review row lists findings as compact rows: status and severity chips, code spans, a capped list", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ReviewSummaryRow } = await server.ssrLoadModule("/src/app/conversation/ReviewSummaryRow.tsx");
    const findings = findingRows([review("1", 1, { type: "findings", count: 4 }, [
      item("R1.1", { type: "dismissed", why: "the CLI ignores extra arguments everywhere" }, "`--version` ignores extra arguments"),
      item("R1.2", { type: "open", why: null }),
      item("R1.3", { type: "fixed", task: "task-7", why: null, checked: "read at the tip" }),
      item("R1.4", { type: "new" }),
      ...Array.from({ length: 6 }, (_, n) => item(`R1.${n + 5}`, { type: "new" })),
    ])]);
    const html = ReactDOMServer.renderToStaticMarkup(React.createElement(ReviewSummaryRow, {
      row: { text: "Review: 1 open · 7 new", sections: [{ label: null, outcome: "1 open · 7 new", findings }] },
      defaultOpen: true,
    }));
    // Each status is a chip, coloured as badges are elsewhere; the severity is one too.
    for (const [label, variant] of [["Open", "warning"], ["New", "secondary"], ["Fixed", "success"], ["Dismissed", "outline"]]) {
      assert.match(html, new RegExp(`data-slot="finding-status" data-variant="${variant}"[^>]*>${label}</span>`));
    }
    assert.equal(count(html, "finding-severity"), 10);
    assert.match(html, />P3</);
    // The reviewer's backticks are code, not raw.
    assert.match(html, /<code[^>]*>--version<\/code> ignores extra arguments/);
    assert.doesNotMatch(words(html), /`/);
    // The dismissal's why and the id wait for hover; no id column.
    assert.doesNotMatch(words(html), /the CLI ignores|\bR1\.1\b/);
    assert.match(html, /title="R1\.1 · the CLI ignores extra arguments everywhere"/);
    assert.match(html, /title="R1\.3 · Fixed in task-7\. Checked: read at the tip"/);
    // Ten findings never grow the card: the list holds about six rows, then scrolls.
    assert.equal(count(html, "review-finding"), 10);
    assert.match(html, /data-slot="review-findings" class="[^"]*max-h-\[calc\(var\(--spacing-row\)\*6\)\][^"]*overflow-y-auto/);
  } finally {
    await server.close();
  }
});

test("the context card has one review row: clean for both batches, else each batch labelled with its own count", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ReviewRows, ReviewSummaryRow } = await server.ssrLoadModule("/src/app/conversation/ReviewSummaryRow.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const render = (reviews: ReviewRun[]) => {
      const board = { ...emptyBoard("c"), orchestratorSteps: [merged(20)], reviews: Object.fromEntries(reviews.map((r) => [r.id, r])) };
      const store = createStore(() => ({ board, orchestrator: null }));
      return ReactDOMServer.renderToStaticMarkup(
        React.createElement(BoardStoreContext.Provider, { value: store },
          React.createElement(ReviewRows, { conversationId: "c" })),
      );
    };
    const clean = render([review("1", 10, { type: "clean" }), review("2", 30, { type: "clean" })]);
    assert.equal(count(clean, "review-line"), 1);
    assert.equal(words(clean), "Review: clean");

    const reviews = [review("1", 10, { type: "findings", count: 1 }, [item("R1.1", { type: "open", why: null })]), review("2", 30, { type: "running" })];
    const split = render(reviews);
    assert.equal(count(split, "review-line"), 1);
    assert.equal(words(split), "Merged: 1 open · Since the merge: running…");
    // Opened, each batch is its own labelled section with its own outcome and findings.
    const { reviewLines, reviewRow } = await server.ssrLoadModule("/src/app/conversation/reviewStatus.ts");
    const open = ReactDOMServer.renderToStaticMarkup(React.createElement(ReviewSummaryRow, {
      row: reviewRow(reviewLines([merged(20)], reviews)),
      defaultOpen: true,
    }));
    assert.equal(count(open, "review-section"), 2);
    assert.match(words(open), /Merged work · 1 open Open P3 Finding R1\.1 cli\.ts:6 Since the merge · running…$/);
  } finally {
    await server.close();
  }
});
