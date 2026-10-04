import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";
import { createStore } from "zustand/vanilla";

import type { Plan } from "@/ipc/generated";

test("the plan card keeps progress alongside the durable small-plan approval reason", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { PlanSection } = await server.ssrLoadModule("/src/app/conversation/cards/PlanSection.tsx");
    const { emptyBoard, BoardStoreContext } = await server.ssrLoadModule("/src/state/board.ts");
    const plan: Plan = {
      id: "p", conversationId: "c", requestId: "r", position: 0, title: "Small plan",
      steps: [{ title: "Do it", detail: null, taskId: null }], risky: false,
      state: { type: "approved", by: "brigadier" }, gate: null, revises: null,
      responses: [], reviewNotes: [], reviewSkipReason: "small plan",
      createdAtMs: 0, decidedAtMs: 1,
    };
    const store = createStore(() => ({ board: { ...emptyBoard("c"), plans: { p: plan } }, orchestrator: null }));
    const render = () => ReactDOMServer.renderToStaticMarkup(
      React.createElement(BoardStoreContext.Provider, { value: store },
        React.createElement(PlanSection, { planIds: ["p"] })),
    );
    assert.match(render(), /Approved without review: small plan/);
    assert.match(render(), /Approved, not started/);
    plan.state = { type: "proposed" };
    assert.doesNotMatch(render(), /Approved without review/);
    assert.match(render(), /Plan proposed/);
    plan.state = { type: "approved", by: "user" };
    assert.match(render(), /Approved by you/);
    plan.state = { type: "approved", by: "brigadier" };
    plan.reviewSkipReason = "another recorded reason";
    assert.match(render(), /Approved without review: another recorded reason/);
    plan.reviewSkipReason = null;
    assert.match(render(), /Auto-approved by Brigadier/);
  } finally {
    await server.close();
  }
});
