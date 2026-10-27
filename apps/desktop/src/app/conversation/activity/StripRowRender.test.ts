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
    // Between its calls, what it last did (D39).
    assert.deepEqual(line({ doing: { type: "call", step: { name: "shell", detail: "pnpm build" } } }), ["shimmer", "Running pnpm build"]);
    assert.deepEqual(line({ doing: { type: "thinking" } }), ["shimmer", "Thinking…"]);
    assert.deepEqual(line({ steps: [{ name: "shell", detail: "pnpm build" }], waiting: [{ kind: "question", what: "Which?", card: null }] }), ["text-foreground/60", "Needs you"]);
  } finally {
    await server.close();
  }
});

test("a subagent's strip row is plain: a neutral mark, its task and time, no worker glyph or name", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { StripRow, SubagentRow } = await server.ssrLoadModule("/src/app/conversation/activity/StripRow.tsx");
    const { useCliThread } = await server.ssrLoadModule("/src/state/cliThread.ts");
    const subagent = { ...row({ id: "native:c:call1", title: "Explore the parser" }), native: { conversationId: "c", callId: "call1", backgroundId: null, prompt: "" } };
    Object.assign(useCliThread.getInitialState().views, { c: { entries: [], turns: [], workers: [subagent] } });
    const render = (component: unknown) => ReactDOMServer.renderToStaticMarkup(React.createElement(component as React.FC<{ conversationId: string; taskId: string }>, { conversationId: "c", taskId: "native:c:call1" }));
    // A worker's row carries its glyph (the two-tone avatar in its own colour); a subagent's never does.
    assert.match(render(StripRow), /text-glyph-/);
    const html = render(SubagentRow);
    assert.doesNotMatch(html, /text-glyph-/);
    assert.match(html, /data-slot="subagents-strip-row"/);
    assert.equal(html.match(/<svg/g)?.length, 1);
    assert.match(html, /data-slot="subagent-mark"[^>]*><svg/);
    const text = html.replace(/<[^>]+>/g, " ").replace(/\s+/g, " ").trim();
    assert.match(text, /^Explore the parser \d+s$/);
  } finally {
    await server.close();
  }
});
