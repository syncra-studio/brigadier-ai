import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

test("an eval card shows the script exactly, backticks and blank lines too", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { BrowserScript } = await server.ssrLoadModule("/src/app/conversation/cards/BrowserScript.tsx");
    const render = (details: string) => ReactDOMServer.renderToStaticMarkup(React.createElement(BrowserScript, { details }));
    const script = "const a = `x`;\n\nreturn `${a}`;";
    const html = render(`On https://shop.example/.\n\n${script}`);
    assert.equal(/<pre[^>]*>([\s\S]*)<\/pre>/.exec(html)?.[1], script);
    assert.match(html, /<p[^>]*>On https:\/\/shop.example\/\.<\/p>/);
    assert.doesNotMatch(html, /<code/);
    // With no first paragraph it is all script.
    const bare = render("return 1");
    assert.doesNotMatch(bare, /<p[ >]/);
    assert.match(bare, /<pre[^>]*>return 1<\/pre>/);
  } finally {
    await server.close();
  }
});
