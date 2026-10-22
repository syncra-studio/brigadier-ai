import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

// The MCP icon follows the text color: the UI kit's copy painted a fixed near-black fill that
// vanished on the dark background.
test("the MCP icon is drawn in the current text color", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { Mcp } = await server.ssrLoadModule("/src/components/icons/index.ts");
    const html = ReactDOMServer.renderToStaticMarkup(React.createElement(Mcp));
    assert.match(html, /<svg[^>]* fill="currentColor"/);
    assert.doesNotMatch(html, /#[0-9a-f]{6}/i);
    assert.equal(html.match(/<path/g)?.length, 2);
  } finally {
    await server.close();
  }
});
