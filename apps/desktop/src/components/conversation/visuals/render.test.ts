import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import React from "react";
import { renderToReadableStream } from "react-dom/server";
import { createServer, type ViteDevServer } from "vite";

let server: ViteDevServer;
let renderReply: (text: string) => Promise<string>;
let render: (code: string, language: string, chat: boolean, writing?: boolean) => Promise<string>;
before(async () => {
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  const { CodeBlock } = await server.ssrLoadModule("/src/components/conversation/code-block.tsx");
  const { MarkdownText } = await server.ssrLoadModule("/src/components/conversation/markdown-text.tsx");
  const { TextMessagePartProvider } = await server.ssrLoadModule("@assistant-ui/react");
  const { VisualsContext } = await server.ssrLoadModule("/src/components/conversation/visuals/bridge.ts");
  const { TooltipProvider } = await server.ssrLoadModule("/src/components/overlays/tooltip.tsx");
  renderReply = async (text) => {
    const stream = await renderToReadableStream(React.createElement(TooltipProvider, null,
      React.createElement(VisualsContext.Provider, { value: { running: false, prefill() {}, openLink() {} } },
        React.createElement(TextMessagePartProvider, { text, isRunning: false }, React.createElement(MarkdownText)))));
    await stream.allReady;
    return new Response(stream).text();
  };
  render = async (code, language, chat, writing = false) => {
    const stream = await renderToReadableStream(React.createElement(TooltipProvider, null,
      React.createElement(VisualsContext.Provider, { value: chat ? { running: false, prefill() {}, openLink() {} } : null },
        React.createElement(CodeBlock, { code, language, node: { properties: { dataOpen: writing } } }))));
    await stream.allReady;
    return new Response(stream).text();
  };
});
after(async () => { await server?.close(); });
const table = JSON.stringify({ title: "Travel times", columns: ["Route", "Minutes"], rows: [["Train", "35"]], source: "Illustrative" });

test("a Chat draws a table with the existing copy and expand controls", async () => {
  const html = await render(table, "brigadier-table", true);
  assert.match(html, /<table/);
  assert.match(html, /Travel times/);
  assert.match(html, /Copy table/);
  assert.match(html, /Expand table/);
  assert.doesNotMatch(html, /data-slot="code-block"/);
});
test("excluded conversations keep the code card for every visual format", async () => {
  for (const language of ["brigadier-table", "brigadier-chart", "brigadier-html"]) {
    const html = await render(table, language, false);
    assert.match(html, /data-slot="code-block"/);
    assert.doesNotMatch(html, /<iframe|<table|<figure/);
  }
});
test("an open data fence draws only a placeholder, and invalid data is explained", async () => {
  for (const language of ["brigadier-table", "brigadier-chart"]) {
    assert.match(await render("{", language, true, true), /Drawing…/);
    assert.match(await render("{", language, true), /role="alert"/);
  }
  assert.match(await render('{"title":"Bad","columns":["a"],"rows":[[]],"source":"test"}', "brigadier-table", true), /invalid rows or columns/);
  assert.match(await render('{"kind":"bar","title":"Bad","description":"test","data":[{"label":"x","value":"5"}]}', "brigadier-chart", true), /invalid data/);
});
test("HTML previews and final frames keep an opaque sandbox on the custom scheme", async () => {
  const html = await render("<p>hello</p><script>secret()</script>", "brigadier-html", true, true);
  assert.match(html, /<iframe/);
  assert.match(html, /sandbox="allow-scripts"/);
  assert.match(html, /brigadier-ui:\/\/localhost\/frame/);
  assert.match(html, /title="Drawing…"/);
  assert.doesNotMatch(html, /secret\(\)|allow-same-origin|srcdoc/);
  assert.match(await render("<p>hello</p>", "brigadier-html", true), /title="Interactive visual"/);
});
test("all chart kinds expose their data in words", async () => {
  for (const kind of ["bar", "line", "pie"]) {
    const html = await render(JSON.stringify({ kind, title: "Time", description: "Minutes; illustrative", data: [{label: "Train", value: 35}] }), "brigadier-chart", true);
    assert.match(html, /<figure/);
    assert.match(html, /Train: 35/);
    assert.match(html, /Minutes; illustrative/);
  }
});

test("a recovered complete reply draws every visual fence without live stream state", async () => {
  const reply = [
    "```brigadier-table", table, "```", "",
    "```brigadier-chart", JSON.stringify({ kind: "bar", title: "Time", description: "Minutes", data: [{ label: "Train", value: 35 }] }), "```", "",
    "```brigadier-html", "<p>Recovered visual</p>", "```",
  ].join("\n");
  const html = await renderReply(reply);
  assert.match(html, /<table/);
  assert.match(html, /<figure/);
  assert.match(html, /<iframe/);
  assert.match(html, /title="Interactive visual"/);
  assert.doesNotMatch(html, /Drawing…|data-slot="code-block"/);
});
