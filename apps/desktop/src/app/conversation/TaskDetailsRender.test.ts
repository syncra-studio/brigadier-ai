import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import type { Task } from "@/ipc/generated";

test("a worker's diff button names its worker, never the raw task id", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ArtifactButtons } = await server.ssrLoadModule("/src/app/conversation/TaskDetails.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const task = { id: "t1", number: 1, title: "File summary", kind: "implement", state: "done", createdAtMs: 1000, updatedAtMs: 9000, attempts: [] } as unknown as Task;
    const store = createStore(() => ({ board: { ...emptyBoard("c"), tasks: { t1: task } }, orchestrator: null }));
    const diff = { id: "d1", title: "Diff of task-1", kind: "diff", mime: "text/x-diff", bytes: 10, fileName: "task-1.diff" };
    const html = ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: store }, React.createElement(ArtifactButtons, { artifacts: [diff], onOpen: () => {} })));
    const text = html.replace(/<[^>]+>/g, "");
    // As the dialog it opens says it.
    assert.equal(text, "Diff of “File summary”diff");
    assert.doesNotMatch(text, /task-1/);
  } finally {
    await server.close();
  }
});
