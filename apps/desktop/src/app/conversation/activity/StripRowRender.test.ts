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

const renderSubagent = (component: unknown) => ReactDOMServer.renderToStaticMarkup(React.createElement(component as React.FC<{ conversationId: string; taskId: string }>, { conversationId: "c", taskId: "native:c:call1" }));

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
    // Between its calls, what it last did (D39).
    assert.deepEqual(line({ doing: { type: "call", step: { name: "shell", detail: "pnpm build" } } }), ["shimmer", "Running pnpm build"]);
    assert.deepEqual(line({ doing: { type: "thinking" } }), ["shimmer", "Thinking…"]);
    assert.deepEqual(line({ steps: [{ name: "shell", detail: "pnpm build" }], waiting: [{ kind: "question", what: "Which?", card: null }] }), ["text-foreground/60", "Needs you"]);
  } finally {
    await server.close();
  }
});

test("a subagent's strip row is a worker's: its glyph, its name, what it does and its time (D50)", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { StripRow } = await server.ssrLoadModule("/src/app/conversation/activity/StripRow.tsx");
    const { useCliThread } = await server.ssrLoadModule("/src/state/cliThread.ts");
    const subagent = { ...row({ id: "native:c:call1", title: "Explore the parser" }), native: { conversationId: "c", callId: "call1", backgroundId: null, prompt: "" } };
    Object.assign(useCliThread.getInitialState().views, { c: { entries: [], turns: [], workers: [subagent] } });
    const html = renderSubagent(StripRow);
    assert.match(html, /text-glyph-/);
    assert.match(html, /data-slot="workers-strip-row"/);
  } finally {
    await server.close();
  }
});

test("a subagent's panel is headed by its name as its rows say it, as a worker's is (D50), and shows its last words from its session's thread", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { WorkersTab } = await server.ssrLoadModule("/src/app/side-panel/Agents.tsx");
    const { AgentsPanelContext } = await server.ssrLoadModule("/src/app/conversation/WorkerChip.tsx");
    const { TooltipProvider } = await server.ssrLoadModule("/src/components/overlays/tooltip.tsx");
    const { useCliThread } = await server.ssrLoadModule("/src/state/cliThread.ts");
    const subagent = (call: string, number: number) => ({
      ...row({ id: `native:c:${call}`, number, title: "Run sleep then ls src" }),
      native: { conversationId: "c", callId: call, backgroundId: null, prompt: "" },
    });
    // Its last words came after its call ended: the session's thread holds them once read again.
    const said = { event: { type: "message", itemId: "s1", role: "assistant", text: "Ran it: greet.js, index.js" }, atMs: NOW, parent: "call2" };
    Object.assign(useCliThread.getInitialState().views, { c: { entries: [said], turns: [], workers: [subagent("call1", 1), { ...subagent("call2", 2), working: false, lastEnd: "done", gone: true }] } });
    const html = ReactDOMServer.renderToStaticMarkup(
      React.createElement(TooltipProvider, null, React.createElement(AgentsPanelContext.Provider, { value: { panel: "native:c:call2", setPanel: () => {} } },
        React.createElement(WorkersTab as React.FC<{ conversationId: string }>, { conversationId: "c" }))),
    );
    assert.match(html, /<h2[^>]*>Run sleep then 2<\/h2>/);
    assert.match(html, /Ran it: greet\.js, index\.js/);
  } finally {
    await server.close();
  }
});
