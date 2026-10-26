import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { AttachmentRef } from "@/ipc/generated";

// The user's report: a sent message's "Pasted text" card opened nothing when clicked.
test("a sent message's \"Pasted text\" card opens its text, read only, through the conversation's opener", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { MessageAttachments, PastedTextOpenerContext } = await server.ssrLoadModule("/src/components/conversation/index.ts");
    const card: AttachmentRef = { id: "blob-1", name: "Pasted text.txt", mime: "text/plain", bytes: 4096, pasted: true, inline: null };
    const file: AttachmentRef = { id: "blob-2", name: "notes.txt", mime: "text/plain", bytes: 10, pasted: false, inline: null };
    const html = (opener: unknown) => ReactDOMServer.renderToStaticMarkup(
      React.createElement(PastedTextOpenerContext.Provider, { value: opener },
        React.createElement(MessageAttachments, { attachments: [card, file] })),
    );

    const opened: unknown[][] = [];
    const shown = html((...args: unknown[]) => opened.push(args));
    assert.equal(shown.match(/<button[^>]*aria-label="Open Pasted text"[^>]*>Pasted text<\/button>/g)?.length, 1, "the card's title opens it; another file's doesn't");
    assert.doesNotMatch(shown, /<button[^>]*disabled/);
    assert.match(shown, /TXT text · 4\.0 KB/, "it looks as it did");

    // Without a conversation to open it in (a fixture board), it stays a plain label.
    assert.doesNotMatch(html(null), /Open Pasted text/);
  } finally {
    await server.close();
  }
});
