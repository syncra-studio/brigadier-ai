import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { WorkerRow } from "@/ipc/generated";

test("a stopped worker's own view says it stopped, and after how long, from its row", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  Object.assign(globalThis, { document: { documentElement: {} }, getComputedStyle: () => ({ getPropertyValue: () => "" }) });
  try {
    const { WorkerThread } = await server.ssrLoadModule("/src/app/side-panel/WorkerThread.tsx");
    const header = (patch: Partial<WorkerRow>) => {
      const row = { id: "t1", number: 1, title: "Rows", agent: "claude", model: null, startedAtMs: 1000, working: false, lastEnd: "done",
        gone: true, lastEventAtMs: 191_000, limit: null, freshContext: false, waiting: [], mcp: [], branch: null, base: null, diff: null,
        ahead: 0, merged: false, ...patch } satisfies WorkerRow;
      const html = ReactDOMServer.renderToStaticMarkup(React.createElement(WorkerThread, { conversationId: "c", row }));
      return /data-slot="worker-work-header"[^>]*>([^<]*)</.exec(html)?.[1];
    };
    assert.equal(header({ lastEnd: "stopped" }), "Stopped after 3m 10s");
    assert.equal(header({ merged: true }), "Worked for 3m 10s");
  } finally {
    Reflect.deleteProperty(globalThis, "document");
    Reflect.deleteProperty(globalThis, "getComputedStyle");
    await server.close();
  }
});
