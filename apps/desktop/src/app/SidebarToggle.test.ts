import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

test("the sidebar toggle is one shape holding the toggle and the chevron, with no tips", async () => {
  // The hide choices measure spacing tokens while rendering; give them a root to read.
  const globals = globalThis as Record<string, unknown>;
  const saved = { document: globals.document, getComputedStyle: globals.getComputedStyle };
  globals.document = { documentElement: {} };
  globals.getComputedStyle = () => ({ getPropertyValue: () => "" });
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { SidebarToggle } = await server.ssrLoadModule("/src/app/SidebarToggle.tsx");
    const render = (open: boolean) => ReactDOMServer.renderToStaticMarkup(
      React.createElement(SidebarToggle, { open, onToggle: () => {}, mode: "strip", onModeChange: () => {} }),
    );
    const html = render(true);
    // One wrapper, and both buttons inside it.
    assert.equal(html.match(/data-slot="sidebar-toggle"/g)?.length, 1);
    assert.match(html, /^<div data-slot="sidebar-toggle"/);
    assert.match(html, /<\/button><\/div>$/);
    assert.equal(html.match(/<button/g)?.length, 2);
    // The glyph toggles the sidebar; the chevron opens the hide choices.
    assert.match(html, /aria-label="Hide sidebar" aria-expanded="true"/);
    assert.match(render(false), /aria-label="Show sidebar" aria-expanded="false"/);
    assert.match(html, /aria-label="How the sidebar hides"[^>]*aria-haspopup="menu"/);
    // No tooltips on either part.
    assert.doesNotMatch(html, /tooltip/);
  } finally {
    await server.close();
    Object.assign(globals, saved);
  }
});
