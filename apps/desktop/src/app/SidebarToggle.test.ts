import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

test("the sidebar toggle is one plain button, with no tip", async () => {
  // Menus measure spacing tokens while rendering; give them a root to read.
  const globals = globalThis as Record<string, unknown>;
  const saved = { document: globals.document, getComputedStyle: globals.getComputedStyle };
  globals.document = { documentElement: {} };
  globals.getComputedStyle = () => ({ getPropertyValue: () => "" });
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { SidebarToggle } = await server.ssrLoadModule("/src/app/SidebarToggle.tsx");
    const render = (open: boolean) => ReactDOMServer.renderToStaticMarkup(
      React.createElement(SidebarToggle, { open, onToggle: () => {} }),
    );
    const html = render(true);
    // One button, nothing beside it: how the sidebar closes is chosen in Settings.
    assert.equal(html.match(/<button/g)?.length, 1);
    assert.match(html, /data-slot="sidebar-toggle"/);
    assert.doesNotMatch(html, /aria-haspopup/);
    // It shows or hides the sidebar.
    assert.match(html, /aria-label="Hide sidebar" aria-expanded="true"/);
    assert.match(render(false), /aria-label="Show sidebar" aria-expanded="false"/);
    // No tooltip.
    assert.doesNotMatch(html, /tooltip/);
  } finally {
    await server.close();
    Object.assign(globals, saved);
  }
});
