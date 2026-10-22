import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import type { OrchestratorStep, Task } from "@/ipc/generated";

const merged = (atMs: number): OrchestratorStep => ({
  requestId: null,
  kind: {
    type: "merged", branch: "brigadier/flow", base: "main", commits: 1, askedIn: "m1", mergeCommit: "1234567abc",
  },
  atMs,
  position: 0,
});

const task = (id: string, createdAtMs: number): Task =>
  ({ id, number: 2, title: "Fix the CLI", kind: "implement", state: "running", requestId: "r2", subject: null, createdAtMs, updatedAtMs: createdAtMs }) as unknown as Task;

/** What a reader sees: the markup's text, without its tags. */
const words = (html: string) => html.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
const count = (html: string, slot: string) => html.split(`data-slot="${slot}"`).length - 1;

test("the job tab shows the job at work, then each merge's", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { SessionJobTab } = await server.ssrLoadModule("/src/app/side-panel/JobTab.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const board = {
      ...emptyBoard("c"),
      orchestratorSteps: [merged(20)],
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
    const empty = createStore(() => ({ board: emptyBoard("c"), orchestrator: null }));
    const none = ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: empty }, React.createElement(SessionJobTab, { conversationId: "c" })),
    );
    assert.match(words(none), /Nothing has run yet/);
  } finally {
    await server.close();
  }
});
