import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { WorkerRow } from "@/ipc/generated";

const NOW = Date.now();
const row = (patch: Partial<WorkerRow> = {}): WorkerRow => ({
  id: "t1", number: 1, title: "Demo of the fixes", agent: "claude", model: null, startedAtMs: NOW - 5000,
  working: true, lastEnd: null, gone: false, lastEventAtMs: NOW, limit: null, freshContext: false, waiting: [], mcp: [],
  branch: null, base: null, diff: null, ahead: 0, merged: false, ...patch,
});

test("a strip row says what its worker does while it works, shimmering, else its state", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { StripRow } = await server.ssrLoadModule("/src/app/conversation/activity/StripRow.tsx");
    const { useCliThread } = await server.ssrLoadModule("/src/state/cliThread.ts");
    const line = (patch: Partial<WorkerRow>) => {
      // A server render reads a store's first state.
      Object.assign(useCliThread.getInitialState().views, { c: { entries: [], turns: [], workers: [row(patch)] } });
      const html = ReactDOMServer.renderToStaticMarkup(React.createElement(StripRow, { conversationId: "c", taskId: "t1" }));
      return /<span class="min-w-0 flex-1 truncate ([^"]*)">([^<]*)</.exec(html)?.slice(1);
    };
    assert.deepEqual(line({ steps: [{ name: "shell", detail: "cd /w && pnpm build" }] }), ["shimmer", "Running pnpm build"]);
    assert.deepEqual(line({ steps: [{ name: "Edit", detail: "/w/src/foo.ts" }] }), ["shimmer", "Editing foo.ts"]);
    assert.deepEqual(line({}), ["shimmer", "Working"]);
    assert.deepEqual(line({ steps: [{ name: "shell", detail: "pnpm build" }], waiting: [{ kind: "question", what: "Which?", card: null }] }), ["text-foreground/60", "Needs you"]);
  } finally {
    await server.close();
  }
});
