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
