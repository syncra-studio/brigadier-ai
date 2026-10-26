import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { AttachmentRef } from "@/ipc/generated";

// The user's report: "Pasted text.txt" under the context card's Sources opened nothing when clicked.
test("a pasted text under Sources opens in the session's tab for it; another attached file stays a plain row", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { fileSource } = await server.ssrLoadModule("/src/app/conversation/SummaryContent.tsx");
    const { sessionTabs } = await server.ssrLoadModule("/src/state/sessionTabs.ts");
    const { tabTitle } = await server.ssrLoadModule("/src/state/sessionTabTitle.ts");
    const card: AttachmentRef = { id: "blob-1", name: "Pasted text.txt", mime: "text/plain", bytes: 4096, pasted: true, inline: null };
    const chip: AttachmentRef = { id: "blob-2", name: "Pasted text.md", mime: "text/markdown", bytes: 8, pasted: true, inline: 2 };
    const file: AttachmentRef = { id: "blob-3", name: "notes.txt", mime: "text/plain", bytes: 10, pasted: false, inline: null };

    assert.equal(fileSource("s", file).open, undefined);
    fileSource("s", card).open();
    assert.equal(sessionTabs("s").active, "pasted:blob-1");
    fileSource("s", chip).open();
    const { tabs, active } = sessionTabs("s");
    assert.equal(active, "pasted:blob-2:2");
    assert.deepEqual(tabs.map((tab: never) => tabTitle(tab, tabs)), ["Pasted text", "Pasted text #2"]);
  } finally {
    await server.close();
  }
});

// The user's report, in a plain Chat: "Pasted text.txt" under the summary's Attachments opened nothing.
test("a pasted text under a Chat's Attachments opens over the thread through the conversation's opener; another file stays a plain row", async () => {
  const server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  try {
    const { ChatAttachments, pastedOpener } = await server.ssrLoadModule("/src/app/conversation/SummaryContent.tsx");
    const { PastedTextOpenerContext } = await server.ssrLoadModule("/src/components/conversation/index.ts");
    const card: AttachmentRef = { id: "blob-1", name: "Pasted text.txt", mime: "text/plain", bytes: 4096, pasted: true, inline: null };
    const chip: AttachmentRef = { id: "blob-2", name: "Pasted text.md", mime: "text/markdown", bytes: 8, pasted: true, inline: 2 };
    const file: AttachmentRef = { id: "blob-3", name: "notes.txt", mime: "text/plain", bytes: 10, pasted: false, inline: null };
    const opened: unknown[][] = [];
    const open = (...args: unknown[]) => opened.push(args);

    assert.equal(pastedOpener(open, file), undefined);
    assert.equal(pastedOpener(null, card), undefined, "without a conversation to open it in, nothing");
    pastedOpener(open, card)();
    pastedOpener(open, chip)();
    assert.deepEqual(opened, [[null, card], [2, chip]]);

    const html = ReactDOMServer.renderToStaticMarkup(
      React.createElement(PastedTextOpenerContext.Provider, { value: open },
        React.createElement(ChatAttachments, { attachments: [card, file] })),
    );
    assert.match(html, /<button[^>]*data-slot="summary-row"[^>]*>.*Pasted text\.txt/);
    assert.match(html, /<div[^>]*data-slot="summary-row"[^>]*>.*notes\.txt/);
    assert.doesNotMatch(html, /<button[^>]*data-slot="summary-row"[^>]*>(?:(?!<\/button>).)*notes\.txt/);
  } finally {
    await server.close();
  }
});
