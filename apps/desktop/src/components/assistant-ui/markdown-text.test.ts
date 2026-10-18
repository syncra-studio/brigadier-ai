import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import React from "react";
import { renderToReadableStream } from "react-dom/server";
import { createServer, type ViteDevServer } from "vite";

let server: ViteDevServer;
let render: (text: string) => Promise<string>;
const PR = "https://github.com/syncra-studio/brigadier-ai/pull/286";

before(async () => {
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  const { MarkdownText } = await server.ssrLoadModule("/src/components/assistant-ui/markdown-text.tsx");
  const { TextMessagePartProvider } = await server.ssrLoadModule("@assistant-ui/react");
  const { TooltipProvider } = await server.ssrLoadModule("/src/components/ui/tooltip.tsx");
  render = async (text) => {
    const stream = await renderToReadableStream(React.createElement(TooltipProvider, null,
      React.createElement(TextMessagePartProvider, { text, isRunning: false }, React.createElement(MarkdownText))));
    await stream.allReady;
    return new Response(stream).text();
  };
});

after(async () => { await server?.close(); });

test("a merge reply's PR link has a clean URL and text, with no joiner or spacing element", async () => {
  for (const markdown of [PR, `[\u2060${PR}\u2060](\u2060${PR}\u2060)`, `[\u200b${PR}\ufeff](${PR})`, `[\u2060${PR}\u2060][pr]\n\n[pr]: \u2060${PR}\u2060`]) {
    const html = await render(`Merged: ${markdown}`);
    const link = /<a\b[^>]*href="([^"]*)"[^>]*>(.*?)<\/a>/s.exec(html);
    assert.ok(link, html);
    assert.equal(link[1], PR);
    assert.equal(link[2]!.replace(/<[^>]*>/g, ""), PR);
    assert.doesNotMatch(link[2]!, /[\u200b-\u200d\u2060\ufeff]|class="select-none"/);
    assert.doesNotMatch(link[2]!, /\bme-0\.75\b/);
  }
});

test("link cleanup preserves formatting, ordinary spacing, prose and code", async () => {
  const html = await render(`Kept\u2060here and \`\u2060code\u2060\`. [\u2060**\u2060Pull request\u2060**\u2060](${PR}) after.`);
  assert.match(html, /Kept\u2060here/);
  assert.match(html, />\u2060code\u2060<\/code>/);
  assert.match(html, /<strong[^>]*>Pull request<\/strong>/);
  assert.match(html, /<\/a> after\./);
});
