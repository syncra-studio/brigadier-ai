import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

// The redesigned pages' title: larger and tighter, its description quieter; other pages keep theirs.
test("A large Settings page title uses the redesigned pages' type", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { SettingsPage } = await server.ssrLoadModule("/src/app/settings/parts.tsx");
    const html = (large: boolean) =>
      ReactDOMServer.renderToStaticMarkup(
        React.createElement(SettingsPage, { title: "Routing", description: "Which model", large }, null),
      );
    assert.match(html(true), /<h1 class="font-medium text-settings-title text-settings-text tracking-section">Routing/);
    assert.match(html(true), /<p class="text-settings-body text-settings-text-secondary">Which model/);
    assert.match(html(false), /<h1 class="font-medium text-page-title">Routing/);
  } finally {
    await server.close();
  }
});
