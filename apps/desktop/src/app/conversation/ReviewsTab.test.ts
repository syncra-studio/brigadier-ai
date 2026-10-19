import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

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

test("the Reviews tab lists every batch's findings at full width: status chips, whole titles with code spans, where and why", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ReviewsTab } = await server.ssrLoadModule("/src/app/conversation/ReviewsTab.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const reviews = [
      review("1", 10, { type: "findings", count: 3 }, [
        item("R1.1", { type: "dismissed", why: "the CLI ignores extra arguments everywhere" }, "`--version` ignores extra arguments"),
        item("R1.2", { type: "open", why: null }),
        item("R1.3", { type: "fixed", task: "task-7", why: null, checked: "read at the tip" }),
      ]),
      review("2", 30, { type: "running" }),
    ];
    const board = { ...emptyBoard("c"), orchestratorSteps: [merged(20)], reviews: Object.fromEntries(reviews.map((r) => [r.id, r])) };
    const store = createStore(() => ({ board, orchestrator: null }));
    const html = ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: store }, React.createElement(ReviewsTab, { conversationId: "c" })),
    );
    // One part per batch, each with its own outcome.
    assert.match(words(html), /Merged work · 1 open/);
    assert.match(words(html), /Since the merge · running…/);
    assert.equal(count(html, "reviews-finding"), 3);
    for (const [label, variant] of [["Open", "warning"], ["Fixed", "success"], ["Dismissed", "outline"]]) {
      assert.match(html, new RegExp(`data-slot="finding-status" data-variant="${variant}"[^>]*>${label}</span>`));
    }
    // The reviewer's backticks are code; the thread's words on a finding are there in full.
    assert.match(html, /<code[^>]*>--version<\/code> ignores extra arguments/);
    assert.match(words(html), /the CLI ignores extra arguments everywhere/);
    assert.match(words(html), /cli\.ts:6/);
  } finally {
    await server.close();
  }
});
