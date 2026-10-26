import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { WorkerRow } from "@/ipc/generated";

const NOW = Date.now();
const row = (id: string, title: string, patch: Partial<WorkerRow> = {}): WorkerRow => ({
  id, number: Number(id.slice(1)) || 1, title, agent: "claude", model: null, startedAtMs: 1000,
  working: true, lastEnd: null, gone: false, lastEventAtMs: NOW, limit: null, freshContext: false, waiting: [], mcp: [],
  branch: null, base: null, diff: null, ahead: 0, merged: false, ...patch,
});

// Q87: a line per worker while a few work (its name and, in a soft word, what it does while it
// works, else its state), else the avatars row (S14: its row).
test("the context card's Workers row names each worker at work by its row, with what it does or its state, and +/−", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { WorkersSummary } = await server.ssrLoadModule("/src/app/side-panel/WorkerSummary.tsx");
    const { useCliThread } = await server.ssrLoadModule("/src/state/cliThread.ts");
    const text = (rows: WorkerRow[]) => {
      // A server render reads a store's first state.
      Object.assign(useCliThread.getInitialState().views, { c: { entries: [], turns: [], workers: rows } });
      return ReactDOMServer.renderToStaticMarkup(React.createElement(WorkersSummary, { conversationId: "c" }))
        .replace(/<[^>]+>/g, " ").replace(/\s+/g, " ");
    };

    // One worker at work: its own line, its +/− against the session's branch.
    const one = text([row("t1", "Demo of the fixes", { diff: { files: [], insertions: 12, deletions: 3 } })]);
    assert.match(one, /Demo of the fixes working \+12 −3/);

    // At work on a call: the call, in the thread's words, shimmering.
    const html = (rows: WorkerRow[]) => {
      Object.assign(useCliThread.getInitialState().views, { c: { entries: [], turns: [], workers: rows } });
      return ReactDOMServer.renderToStaticMarkup(React.createElement(WorkersSummary, { conversationId: "c" }));
    };
    const busy = html([row("t1", "Demo of the fixes", { steps: [{ name: "shell", detail: "pnpm build" }] })]);
    assert.match(busy, /<span class="[^"]*shimmer[^"]*"> running pnpm build<\/span>/);
    // Between its calls: what it last did, still shimmering (D39).
    const between = html([row("t1", "Demo of the fixes", { doing: { type: "said", line: "Now wiring the exporter." } })]);
    assert.match(between, /<span class="[^"]*shimmer[^"]*"> Now wiring the exporter\.<\/span>/);

    // A few: a line each with its state; the finished ones in the outcome line, not lines of their own.
    const few = text([
      row("t2", "Find bar fixes", { waiting: [{ kind: "permission", what: "rm -rf dist", card: null }] }),
      row("t1", "Demo of the fixes", { lastEventAtMs: NOW - 34 * 60_000 }),
      row("t4", "Settings fixes", { working: false, lastEnd: "done", merged: true, ahead: 1 }),
      row("t5", "Readme", { working: false, lastEnd: "stopped" }),
    ]);
    assert.match(few, /Find bar fixes needs you.*Demo of the fixes quiet 34m/);
    assert.match(few, /1 done · 1 stopped/);
    assert.doesNotMatch(few, /Settings fixes|rm -rf/);

    // Its turn over but its own background commands still running (its tests, a wait on them):
    // still at work, not done.
    const background = text([
      row("t6", "Plugin page bugs", { working: true, lastEnd: "done", lastEventAtMs: NOW }),
      row("t4", "Settings fixes", { working: false, lastEnd: "done", merged: true, ahead: 1 }),
    ]);
    assert.match(background, /Plugin page bugs working/);
    assert.match(background, /1 done/);
    assert.doesNotMatch(background, /2 done/);

    // More than a few: the one row of avatars with the count.
    const many = text(["t1", "t2", "t3", "t4"].map((id) => row(id, `Worker ${id}`)));
    assert.match(many, /4 working/);
    assert.doesNotMatch(many, /Worker t1/);
  } finally {
    await server.close();
  }
});
