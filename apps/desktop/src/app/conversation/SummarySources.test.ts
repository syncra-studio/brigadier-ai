import assert from "node:assert/strict";
import { test } from "node:test";
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
