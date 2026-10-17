import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { Task } from "@/ipc/generated";

// A worker's details say why its model ran it, and that which models get each kind of work is
// the user's to change: one plain link to Settings › Routing.
test("Why this model points to Settings › Routing", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { RouteSections } = await server.ssrLoadModule("/src/app/conversation/cards/RouteDetails.tsx");
    const task = {
      route: { choice: { provider: "codex", model: "gpt-6", effort: "high" }, reason: "Top eligible score.", explanation: null },
      floor: "frontier",
      areas: [],
      attempts: [],
      reworkRounds: 0,
      quotaWait: null,
    } as unknown as Task;
    const markup = ReactDOMServer.renderToStaticMarkup(
      React.createElement(RouteSections, { task, model: "GPT-6", groups: [], inThread: true }),
    );
    const text = markup.replace(/<[^>]+>/g, "");
    assert.match(text, /You choose which models get each kind of work in Settings › Routing/);
    assert.match(markup, /<button[^>]*data-slot="routing-hint"[^>]*>Settings › Routing<\/button>/);
  } finally {
    await server.close();
  }
});
