import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import type { Finding, FindingStatus, OrchestratorStep, ReviewRun, ReviewState, Task } from "@/ipc/generated";

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
  endedAtMs: state.type === "running" ? null : startedAtMs + 1,
  findings: null,
  number: 0,
  items,
});

const merged = (atMs: number): OrchestratorStep => ({
  requestId: null,
  kind: {
    type: "merged", branch: "brigadier/flow", base: "main", commits: 1, askedIn: "m1", mergeCommit: "1234567abc",
    record: { head: "1234567abc", checks: [{ command: "pnpm test", status: "exit 0", onHead: true } as never], reviews: [], ci: null, proofs: 0, verifier: null },
  },
  atMs,
  position: 0,
});

const task = (id: string, createdAtMs: number): Task =>
  ({ id, number: 2, title: "Fix the CLI", kind: "implement", state: "running", requestId: "r2", subject: null, createdAtMs, updatedAtMs: createdAtMs }) as unknown as Task;

const item = (id: string, status: FindingStatus, title = `Finding ${id}`): Finding =>
  ({ id, severity: "P3", title, location: "cli.ts:6", status, high: false });

/** What a reader sees: the markup's text, without its tags. */
const words = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
const count = (html: string, slot: string) => html.split(`data-slot="${slot}"`).length - 1;

test("the job tab shows the job at work, then each merge's, with every finding at full width and what was checked of the merge", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { SessionJobTab } = await server.ssrLoadModule("/src/app/conversation/JobTab.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const reviews = [
      review("1", 10, { type: "findings", count: 3 }, [
        item("R1.1", { type: "dismissed", why: "the CLI ignores extra arguments everywhere" }, "`--version` ignores extra arguments"),
        item("R1.2", { type: "open", why: null }),
        item("R1.3", { type: "fixed", task: "task-7", why: null, checked: "read at the tip" }),
      ]),
      review("2", 40, { type: "running" }),
    ];
    const board = {
      ...emptyBoard("c"),
      orchestratorSteps: [merged(20)],
      reviews: Object.fromEntries(reviews.map((r) => [r.id, r])),
      tasks: { t2: task("t2", 30) },
    };
    const store = createStore(() => ({ board, orchestrator: null }));
    const html = ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: store }, React.createElement(SessionJobTab, { conversationId: "c" })),
    );
    const text = words(html);
    // The job at work first, then the merge's, each a timeline of its own.
    assert.equal(count(html, "job-part"), 2);
    assert.ok(text.indexOf("Now") < text.indexOf("Merged 1234567 into main"), text);
    assert.match(text, /Building/);
    assert.match(text, /Review found 3 issues/);
    assert.match(text, /1 fixed, 1 dismissed, 1 left open/);
    // Every finding, its chip coloured as the app's badges are.
    assert.equal(count(html, "reviews-finding"), 3);
    for (const [label, variant] of [["Open", "warning"], ["Fixed", "success"], ["Dismissed", "outline"]]) {
      assert.match(html, new RegExp(`data-slot="finding-status" data-variant="${variant}"[^>]*>${label}</span>`));
    }
    // The reviewer's backticks are code; the thread's words on a finding are there in full.
    assert.match(html, /<code[^>]*>--version<\/code> ignores extra arguments/);
    assert.match(text, /the CLI ignores extra arguments everywhere/);
    assert.match(text, /cli\.ts:6/);
    // What was checked of the merged commit.
    assert.equal(count(html, "job-merge-checked"), 1);
    assert.match(text, /pnpm test: passed on 1234567/);

    const empty = createStore(() => ({ board: emptyBoard("c"), orchestrator: null }));
    const none = ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: empty }, React.createElement(SessionJobTab, { conversationId: "c" })),
    );
    assert.match(words(none), /Nothing has run yet/);
  } finally {
    await server.close();
  }
});
