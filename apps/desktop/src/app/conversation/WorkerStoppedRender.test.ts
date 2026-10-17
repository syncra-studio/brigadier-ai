import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import type { Task } from "@/ipc/generated";

test("a stopped worker's own view says it stopped, and after how long", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  Object.assign(globalThis, { document: { documentElement: {} }, getComputedStyle: () => ({ getPropertyValue: () => "" }) });
  try {
    const { WorkerThread } = await server.ssrLoadModule("/src/app/conversation/WorkerThread.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const header = (state: string) => {
      const task = { id: "t1", number: 1, title: "Rows", conversationId: "c", state, spec: "Build the rows.", landed: null,
        createdAtMs: 1000, updatedAtMs: 191_000, attempts: [{ startedAtMs: 1000, endedAtMs: 191_000 }], outputs: [], messages: [], report: null, error: null } as unknown as Task;
      const store = createStore(() => ({
        board: { ...emptyBoard("c"), loaded: true, tasks: { t1: task }, transcripts: { t1: { entries: [], loading: false, hasMore: false } } },
        orchestrator: null,
      }));
      const html = ReactDOMServer.renderToStaticMarkup(React.createElement(BoardStoreContext.Provider, { value: store }, React.createElement(WorkerThread, { task })));
      return /data-slot="worker-work-header"[^>]*>([^<]*)</.exec(html)?.[1];
    };
    assert.equal(header("stopped"), "Stopped after 3m 10s");
    assert.equal(header("landed"), "Worked for 3m 10s");
  } finally {
    Reflect.deleteProperty(globalThis, "document");
    Reflect.deleteProperty(globalThis, "getComputedStyle");
    await server.close();
  }
});
