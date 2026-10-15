import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import type { Plan, Task, UserRequest } from "@/ipc/generated";

test("the plan section shows its phases and state, without approval or review words", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { PlanSection } = await server.ssrLoadModule("/src/app/conversation/cards/PlanSection.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const plan: Plan = {
      id: "p", conversationId: "c", requestId: "r", position: 0, title: "Small plan", body: null,
      steps: [{ title: "Do it", detail: null, taskId: null, stage: "pending", startedAtMs: null, endedAtMs: null, outline: null }],
      state: { type: "approved", by: "orchestrator" },
      createdAtMs: 0, decidedAtMs: 1,
    };
    const store = createStore(() => ({ board: { ...emptyBoard("c"), plans: { p: plan } }, orchestrator: null }));
    const render = () => ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: store },
        React.createElement(PlanSection, { planIds: ["p"] })),
    );
    assert.match(render(), /Not started/);
    assert.match(render(), />Pending</);
    assert.doesNotMatch(render(), /Approved without review|Auto-approved|Approved by you|Earlier plans/);
    plan.state = { type: "proposed" };
    assert.match(render(), /Plan proposed/);
    plan.state = { type: "approved", by: "user" };
    assert.doesNotMatch(render(), /Approved by you/);
  } finally {
    await server.close();
  }
});

const request = (type: "working" | "done"): UserRequest => ({ id: "r", conversationId: "c", preview: "Docs", state: { type }, startedAtMs: 0, endedAtMs: null, steeredInto: null, steeredAfter: null, undo: null, worked: [], waited: [], quotaWait: false });

test("a stopped phase reads Stopped, quietly, and an ended plan folds to one line (UI audit M3)", async (t) => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { PlanSection } = await server.ssrLoadModule("/src/app/conversation/cards/PlanSection.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const { TooltipProvider } = await server.ssrLoadModule("/src/components/ui/tooltip.tsx");
    const night = JSON.parse(readFileSync(new URL("../../../fixtures/boards/overnight-2026-10-03.json", import.meta.url), "utf8")) as { tasks: Record<string, Task> };
    const task: Task = { ...Object.values(night.tasks)[0]!, id: "t", conversationId: "c", state: "stopped", requestId: "r" };
    const plan: Plan = {
      id: "p", conversationId: "c", requestId: "r", position: 0, title: "Write module docs", body: null,
      steps: [{ title: "Write module docs", detail: null, taskId: "t", stage: "pending", startedAtMs: null, endedAtMs: null, outline: null },
        { title: "Review them", detail: null, taskId: null, stage: "pending", startedAtMs: null, endedAtMs: null, outline: null }],
      state: { type: "approved", by: "orchestrator" },
      createdAtMs: 0, decidedAtMs: 1,
    };
    const render = (type: "working" | "done") => ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: createStore(() => ({ board: { ...emptyBoard("c"), plans: { p: plan }, tasks: { t: task }, requests: { r: request(type) } }, orchestrator: null })) },
        React.createElement(TooltipProvider, null, React.createElement(PlanSection, { planIds: ["p"] }))),
    );
    // The worker chip's tooltip reads a size token from the page: none here, on the server.
    Object.assign(globalThis, { document: { documentElement: {} }, getComputedStyle: () => ({ getPropertyValue: () => "" }) });
    t.after(() => {
      Reflect.deleteProperty(globalThis, "document");
      Reflect.deleteProperty(globalThis, "getComputedStyle");
    });
    const live = render("working");
    assert.match(live, /Phase 1 stopped/);
    assert.match(live, />Stopped</);
    assert.doesNotMatch(live, /failed|Failed|text-destructive/);
    assert.doesNotMatch(live, /data-folded/);
    // Its request ended: the plan folds to its header and one quiet line.
    const ended = render("done");
    assert.match(ended, /data-folded="true"/);
    assert.match(ended, /· Phase 1 stopped/);
    assert.doesNotMatch(ended, /text-destructive/);
  } finally {
    await server.close();
  }
});
