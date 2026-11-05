import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

test("an agent that isn't set up names the models it would offer, from the shipped registry (issue 593)", async () => {
  // Menus measure spacing tokens while rendering; give them a root to read.
  const globals = globalThis as Record<string, unknown>;
  const saved = { document: globals.document, getComputedStyle: globals.getComputedStyle };
  globals.document = { documentElement: {} };
  globals.getComputedStyle = () => ({ getPropertyValue: () => "" });
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ModelPickerPanel } = await server.ssrLoadModule("/src/components/shared/model-picker.tsx");
    const notSetUp = (provider: string, label: string, unavailable: string) =>
      ReactDOMServer.renderToStaticMarkup(
        React.createElement(ModelPickerPanel, { groups: [{ provider, label, models: [], unavailable }], checked: null, opensOn: provider, onPick: () => {} }),
      );
    assert.match(notSetUp("claude", "Claude", "not logged in"), /Sign in and its models show here: Opus 5\.5, Sonnet 5\.5 and Haiku 5\.5\./);
    assert.match(notSetUp("codex", "Codex", "not installed"), /Install it and its models show here: Astra 6, Sol 6\.1, Luna 6 and Terra 5\.6\./);
    // The registry names none of OpenCode's: the line stays as it was.
    assert.match(notSetUp("opencode", "OpenCode", "not installed"), /Install it and its models show here\./);
  } finally {
    await server.close();
    Object.assign(globals, saved);
  }
});
