import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

test("the trust question names the folder and offers exactly Trust and Don't trust", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { TrustForm } = await server.ssrLoadModule("/src/app/dialogs/TrustDialog.tsx");
    const { Dialog } = await server.ssrLoadModule("/src/components/ui/dialog.tsx");
    const project = { id: "p", name: "p", createdAtMs: 1, repos: [{ path: "/code/p", name: "p" }], prefs: {}, trust: [] };
    const html = ReactDOMServer.renderToStaticMarkup(
      React.createElement(Dialog, { open: true },
        React.createElement(TrustForm, { project, folder: "/code/p" })),
    );
    assert.match(html, />Do you trust this folder\?</);
    assert.match(html, />\/code\/p</);
    assert.match(html, /Only trust folders whose contents you know\./);
    assert.match(html, /You can change this in the project(&#x27;|')s settings\./);
    const buttons = [...html.matchAll(/<button[^>]*>([^<]*)<\/button>/g)].map((match) => match[1]);
    assert.deepEqual(buttons, ["Don&#x27;t trust", "Trust"]);
  } finally {
    await server.close();
  }
});
