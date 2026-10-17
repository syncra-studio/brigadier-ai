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

test("the context card's Workers row says what each worker at work is doing", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { WorkersSummary } = await server.ssrLoadModule("/src/app/conversation/WorkerSummary.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const text = (tasks: Task[], activity: Record<string, string>) => {
      const store = createStore(() => ({
        board: { ...emptyBoard("c"), tasks: Object.fromEntries(tasks.map((t) => [t.id, t])), activity },
        orchestrator: null,
      }));
      return ReactDOMServer.renderToStaticMarkup(React.createElement(BoardStoreContext.Provider, { value: store },
        React.createElement(WorkersSummary, { conversationId: "c" }))).replace(/<[^>]+>/g, " ").replace(/\s+/g, " ");
    };

    // One worker at work: its own line, no name needed.
    const one = text([task("t1", "Demo of the fixes")], { t1: "$ pnpm tauri build --debug" });
    assert.match(one, /1 working/);
    assert.match(one, /\$ pnpm tauri build --debug · \d/);

    // Several: each line names its worker; a finished one has none.
    const many = text([
      task("t1", "Demo of the fixes"),
      task("t2", "Find bar fixes", { createdAtMs: 2000 }),
      task("t3", "Settings fixes", { state: "landed" }),
    ], { t1: "$ pnpm tauri build --debug", t2: "Editing ThreadFind.tsx", t3: "Old line" });
    assert.match(many, /2 working/);
    assert.match(many, /Demo of the fixes: \$ pnpm tauri build --debug/);
    assert.match(many, /Find bar fixes: Editing ThreadFind\.tsx/);
    assert.doesNotMatch(many, /Old line/);
  } finally {
    await server.close();
  }
});
