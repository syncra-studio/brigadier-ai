import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

test("the trust question names the folder, its own name apart, and offers Don't trust, then Trust folder", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { TrustForm } = await server.ssrLoadModule("/src/app/dialogs/TrustDialog.tsx");
    const { Dialog } = await server.ssrLoadModule("/src/components/ui/dialog.tsx");
    const project = { id: "p", name: "p", createdAtMs: 1, repos: [{ path: "/code/p", name: "p" }], prefs: {}, trust: [] };
    const html = ReactDOMServer.renderToStaticMarkup(
      React.createElement(Dialog, { open: true },
        React.createElement(TrustForm, { project, folder: "/code/p" })),
    );
    assert.match(html, />Trust this folder\?</);
    assert.match(html, /<span[^>]*>\/code\/<\/span><span[^>]*>p<\/span>/);
    assert.match(html, /Agents can edit and run files here without asking, and the folder(&#x27;|')s own settings can run code\./);
    const buttons = [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((match) => match[1]);
    assert.deepEqual(buttons, ["Don&#x27;t trust", "Trust folder"]);
  } finally {
    await server.close();
  }
});

/** A line `chars` characters wide. */
const within = (chars: number) => (text: string) => text.length <= chars;

test("a path too long for its line loses home to ~, then its middle, never its ends", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { fitFolder } = await server.ssrLoadModule("/src/app/dialogs/TrustDialog.tsx");
    const long =
      "/Users/you/Library/Mobile Documents/com~apple~CloudDocs/Work/clients/acme-corporation/2026/q4-launch/repositories/marketing-website-v3";
    // It fits: shown whole, from the root.
    assert.deepEqual(fitFolder("/private/tmp/repo/hello", within(51)), { parent: "/private/tmp/repo/", name: "hello" });
    assert.deepEqual(fitFolder(long, within(500)).parent, "/Users/you/Library/Mobile Documents/com~apple~CloudDocs/Work/clients/acme-corporation/2026/q4-launch/repositories/");
    // Too long: home becomes ~ first, then the middle goes, the first folder and the nearest kept.
    assert.deepEqual(fitFolder("/Users/you/Development/a-rather-long-folder", within(40)), {
      parent: "~/Development/",
      name: "a-rather-long-folder",
    });
    assert.deepEqual(fitFolder(long, within(51)), { parent: "~/Library/…/repositories/", name: "marketing-website-v3" });
    assert.deepEqual(fitFolder(long, within(57)).parent, "~/Library/…/q4-launch/repositories/");
    // Nothing fits: only where it starts and the name, which always stays whole.
    assert.deepEqual(fitFolder(long, within(10)), { parent: "~/…/", name: "marketing-website-v3" });
    assert.deepEqual(fitFolder("/opt/very/deep/tree/of/folders/name", within(20)).parent, "/opt/…/folders/");
  } finally {
    await server.close();
  }
});
