import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import type { Task } from "@/ipc/generated";

test("team rows join possessives to names, time stopped work, and omit blank Shell opening lines", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { TeamStep } = await server.ssrLoadModule("/src/app/conversation/activity/TeamSentence.tsx");
    const { ShellBox } = await server.ssrLoadModule("/src/app/conversation/activity/StepDetail.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const task = { id: "t1", number: 1, title: "Add QIF exporter", kind: "implement", state: "stopped",
      createdAtMs: 1000, updatedAtMs: 9000, attempts: [{ startedAtMs: 5000, endedAtMs: 9000 }] } as unknown as Task;
    const store = createStore(() => ({ board: { ...emptyBoard("c"), tasks: { t1: task } }, orchestrator: null }));
    const render = (node: React.ReactNode) => ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: store }, node));
    // The name and its possessive must be one text run, not separate flex items whose gap is
    // closed only visually. This also preserves the worker button's accessible name.
    for (const kind of [
      { type: "landed", taskIds: ["t1"], commits: 1, branch: "main" },
      { type: "reviewed", taskIds: ["t1"], findings: 0 },
      { type: "answered", taskId: "t1", question: "Proceed?", answer: "Yes" },
    ]) {
      const html = render(React.createElement(TeamStep, { kind }));
      assert.match(html, /Add QIF exporter’s (?:change|question)</);
      assert.doesNotMatch(html, /-ms-1/);
      const text = html.replace(/<[^>]+>/g, "");
      assert.equal(text, `${kind.type === "landed" ? "Landed" : kind.type === "reviewed" ? "Reviewed" : "Answered"} Add QIF exporter’s ${kind.type === "answered" ? "question" : "change"}`);
    }
    const stopped = render(React.createElement(TeamStep, { kind: { type: "stopped", taskId: "t1", reason: "Enough" }, atMs: 9000 }));
    assert.match(stopped, /after 4s/);
    // A handoff's earlier attempt counts; a restart after the stop doesn't change the row.
    const handedOff = { ...task, attempts: [{ startedAtMs: 1000, endedAtMs: 3000 }, { startedAtMs: 5000, endedAtMs: 9000 }, { startedAtMs: 20000, endedAtMs: null }] } as unknown as Task;
    const handedOffStore = createStore(() => ({ board: { ...emptyBoard("c"), tasks: { t1: handedOff } }, orchestrator: null }));
    assert.match(ReactDOMServer.renderToStaticMarkup(React.createElement(BoardStoreContext.Provider, { value: handedOffStore },
      React.createElement(TeamStep, { kind: { type: "stopped", taskId: "t1", reason: "Enough" }, atMs: 9000 }))), /after 6s/);
    Object.assign(globalThis, { document: { documentElement: {} }, getComputedStyle: () => ({ getPropertyValue: () => "" }) });
    const shell = render(React.createElement(ShellBox, { command: "echo done", output: "\n  \r\n  done\n", status: "completed", exit: 0 }));
    assert.match(shell, />  done<\/pre>/);
    assert.doesNotMatch(shell, />\n/);
  } finally {
    Reflect.deleteProperty(globalThis, "document");
    Reflect.deleteProperty(globalThis, "getComputedStyle");
    await server.close();
  }
});

test("a worker the user stopped reads that they stopped it; one the lead stopped keeps its reason", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { TeamStep } = await server.ssrLoadModule("/src/app/conversation/activity/TeamSentence.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const task = { id: "t1", number: 1, title: "Add QIF exporter", kind: "implement", state: "stopped",
      createdAtMs: 1000, updatedAtMs: 9000, attempts: [{ startedAtMs: 5000, endedAtMs: 9000 }] } as unknown as Task;
    const store = createStore(() => ({ board: { ...emptyBoard("c"), tasks: { t1: task } }, orchestrator: null }));
    const text = (reason: string) => ReactDOMServer.renderToStaticMarkup(React.createElement(BoardStoreContext.Provider, { value: store },
      React.createElement(TeamStep, { kind: { type: "stopped", taskId: "t1", reason }, atMs: 9000 }))).replace(/<[^>]+>/g, "");
    assert.equal(text("Stopped by the user"), "You stopped Add QIF exporter after 4s");
    assert.equal(text("Enough"), "Stopped Add QIF exporter after 4s");
  } finally {
    await server.close();
  }
});
