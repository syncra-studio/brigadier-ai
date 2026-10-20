import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import night from "@/fixtures/boards/overnight-2026-10-03.json" with { type: "json" };
import type { Task } from "@/ipc/generated";

const base = Object.values(night.tasks)[0] as unknown as Task;
const task = (id: string, title: string, patch: Partial<Task> = {}): Task => ({ ...base, id, title, state: "running",
  createdAtMs: 1000, updatedAtMs: 1000, attempts: [], quotaWait: null, blockedReason: null, candidate: null, run: null, ...patch });

// Q87: a line per worker while a few work (its name and a soft "working"), else the avatars
// row; what a worker runs shows in the Workers panel, never in the card.
test("the context card's Workers row names each worker at work, never what it runs", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { WorkersSummary } = await server.ssrLoadModule("/src/app/side-panel/WorkerSummary.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const text = (tasks: Task[], activity: Record<string, string>) => {
      const store = createStore(() => ({
        board: { ...emptyBoard("c"), tasks: Object.fromEntries(tasks.map((t) => [t.id, t])), activity },
        orchestrator: null,
      }));
      return ReactDOMServer.renderToStaticMarkup(React.createElement(BoardStoreContext.Provider, { value: store },
        React.createElement(WorkersSummary, { conversationId: "c" }))).replace(/<[^>]+>/g, " ").replace(/\s+/g, " ");
    };

    // One worker at work: its own line.
    const one = text([task("t1", "Demo of the fixes")], { t1: "$ pnpm tauri build --debug" });
    assert.match(one, /Demo of the fixes working/);
    assert.doesNotMatch(one, /pnpm tauri build/);

    // A few: a line each; a finished one is in the outcome line, not a line of its own.
    const few = text([
      task("t1", "Demo of the fixes"),
      task("t2", "Find bar fixes", { createdAtMs: 2000 }),
      task("t3", "Settings fixes", { state: "landed" }),
    ], { t1: "$ pnpm tauri build --debug", t2: "Editing ThreadFind.tsx", t3: "Old line" });
    assert.match(few, /Find bar fixes working.*Demo of the fixes working/);
    assert.doesNotMatch(few, /Settings fixes|Editing ThreadFind|pnpm tauri build|Old line/);

    // More than a few: the one row of avatars with the count.
    const many = text(["a", "b", "c", "d"].map((id, at) => task(id, `Worker ${id}`, { createdAtMs: at })), {});
    assert.match(many, /4 working/);
    assert.doesNotMatch(many, /Worker a/);
  } finally {
    await server.close();
  }
});
