import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { AttachmentRef } from "@/ipc/generated";

const ref = (id: string, name: string, mime: string, pasted = false, inline: number | null = null): AttachmentRef =>
  ({ id, name, mime, bytes: 10, pasted, inline });
const long = (end: string, tail: string) => `${"a".repeat(80 - end.length)}${end}${tail}`;
const newServer = () => createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });

// The user's report: "Pasted text.txt" under the context card's Sources opened nothing when clicked.
test("a pasted text or an attached text file under Sources opens in the session's tab for it, titled as it is called", async () => {
  const server = await newServer();
  try {
    const { attachmentSources } = await server.ssrLoadModule("/src/app/conversation/SummaryContent.tsx");
    const { openPastedTab, sessionTabs } = await server.ssrLoadModule("/src/state/sessionTabs.ts");
    const { tabTitle } = await server.ssrLoadModule("/src/state/sessionTabTitle.ts");
    const card = ref("blob-1", "Pasted text.txt", "text/plain", true);
    const chip = ref("blob-2", "Pasted text.md", "text/markdown", true, 2);
    const file = ref("blob-3", "notes.txt", "text/plain");
    const [one, two, three] = attachmentSources([card, chip, file], (n: number | null, paste: AttachmentRef) => openPastedTab("s", n, paste));

    one.open();
    assert.equal(sessionTabs("s").active, "pasted:blob-1");
    two.open();
    assert.equal(sessionTabs("s").active, "pasted:blob-2:2");
    three.open();
    const { tabs, active } = sessionTabs("s");
    assert.equal(active, "pasted:blob-3");
    assert.deepEqual(tabs.map((tab: never) => tabTitle(tab, tabs)), ["Pasted text", "Pasted text #2", "notes.txt"]);
  } finally {
    await server.close();
  }
});

// The user's report, in a plain Chat: "Pasted text.txt" under the summary's Attachments opened nothing.
test("a Chat's Attachments open through the conversation's opener, and every kind of file is a button", async () => {
  const server = await newServer();
  try {
    const { attachmentSources, ChatAttachments } = await server.ssrLoadModule("/src/app/conversation/SummaryContent.tsx");
    const { PastedTextOpenerContext } = await server.ssrLoadModule("/src/components/conversation/index.ts");
    const card = ref("blob-1", "Pasted text.txt", "text/plain", true);
    const chip = ref("blob-2", "Pasted text.md", "text/markdown", true, 2);
    const file = ref("blob-3", "notes.txt", "text/plain");
    const image = ref("blob-4", "image.png", "image/png");
    const pdf = ref("blob-5", "report.pdf", "application/pdf");
    const opened: unknown[][] = [];
    const open = (...args: unknown[]) => opened.push(args);

    const [one, two, three] = attachmentSources([card, chip, file], open);
    one.open();
    two.open();
    three.open();
    assert.deepEqual(opened, [[null, card], [2, chip], [null, file]]);
    assert.equal(attachmentSources([card], null)[0].open, undefined, "without a conversation to open it in, nothing");

    const html = ReactDOMServer.renderToStaticMarkup(
      React.createElement(PastedTextOpenerContext.Provider, { value: open },
        React.createElement(ChatAttachments, { attachments: [card, file, image, pdf] })),
    );
    for (const name of ["Pasted text", "notes\\.txt", "image\\.png", "report\\.pdf"]) {
      assert.match(html, new RegExp(`<button[^>]*data-slot="summary-row"[^>]*>(?:(?!</button>).)*${name}`));
    }
    assert.doesNotMatch(html, /<div[^>]*data-slot="summary-row"/);
  } finally {
    await server.close();
  }
});

test("rows are named as their message names them, numbers restarting per message, a repeat with (2) after it", async () => {
  const server = await newServer();
  try {
    const { attachmentLabels } = await server.ssrLoadModule("/src/app/conversation/SummaryContent.tsx");
    const labels = attachmentLabels([
      ref("a", "image.png", "image/png", false, 1),
      ref("b", "image.png", "image/png", false, 2),
      ref("c", "Pasted text.txt", "text/plain", true, 1),
      ref("d", "image.png", "image/png", false, 1),
      ref("e", "Pasted text.txt", "text/plain", true),
      ref("f", "Pasted text.txt", "text/plain", true),
      ref("g", "photo.png", "image/png"),
      ref("h", "photo.png", "image/png"),
      ref("i", "photo.png", "image/png"),
      ref("j", "notes.txt", "text/plain"),
    ]);
    assert.deepEqual([...labels.values()], [
      "Image #1", "Image #2", "Pasted text #1", "Image #1 (2)", "Pasted text", "Pasted text (2)",
      "photo.png", "photo.png (2)", "photo.png (3)", "notes.txt",
    ]);
  } finally {
    await server.close();
  }
});

test("each kind opens its own way: images the viewer, text and scripts the reader, others the default app; what launches is refused", async () => {
  const server = await newServer();
  try {
    const { attachmentSources } = await server.ssrLoadModule("/src/app/conversation/SummaryContent.tsx");
    const { attachmentOpens } = await server.ssrLoadModule("/src/components/conversation/elements/attachment-open.ts");
    const { useToasts } = await server.ssrLoadModule("/src/state/toasts.ts");
    const kind = (name: string, mime = "application/octet-stream") => attachmentOpens(ref("x", name, mime));

    assert.equal(kind("image.png", "image/png"), "viewer");
    assert.equal(attachmentOpens(ref("x", "Pasted text.txt", "text/plain", true)), "reader");
    assert.equal(kind("notes.txt", "text/plain"), "reader");
    assert.equal(kind("data.json", "application/json"), "reader");
    assert.equal(kind("report.pdf", "application/pdf"), "app");
    assert.equal(kind("archive.zip", "application/zip"), "app");

    // Scripts read as text, whatever type the browser gave them, and never reach the default app.
    for (const script of ["deploy.sh", "tool.py", "RUN.BAT", "build.cmd", "app.js", "task.ps1", "x.vbs", "y.applescript"]) {
      assert.equal(kind(script), "reader", script);
      assert.equal(kind(script, "text/x-script"), "reader", script);
      assert.equal(kind(script, ""), "reader", script);
    }
    for (const launcher of ["Evil.app", "run.command", "RUN.COMMAND", "run.command.", "a.tool", "a.pyc", "setup.exe", "setup.pkg", "x.webloc"]) {
      assert.equal(kind(launcher), "refused", launcher);
    }

    // The daemon writes the first 80 characters: what's left is what would open.
    assert.equal(kind(long(".command", ".pdf"), "application/pdf"), "refused");
    assert.equal(kind(long(".command", ".txt"), "text/plain"), "refused");
    assert.equal(kind(long(".sh", ".pdf"), "application/pdf"), "refused", "cut down to a script, it must not reach the default app");
    assert.equal(kind(`a.command${"b".repeat(90)}.sh`), "reader", "the copy is never written: the reader reads the blob");

    const pdf = ref("hash-pdf", "report.pdf", "application/pdf");
    const refused = ref("hash-cmd", "deploy.command", "application/octet-stream");
    const script = ref("hash-sh", "deploy.sh", "");
    const image = ref("hash-img", "image.png", "image/png", false, 1);
    const other = ref("hash-img2", "shot.png", "image/png");
    const opened: unknown[][] = [];
    const [pdfRow, refusedRow, scriptRow, imageRow, otherRow] = attachmentSources([pdf, refused, script, image, other], (...args: unknown[]) => opened.push(args));

    const invoked: unknown[][] = [];
    let fail = false;
    const place = globalThis as { window?: unknown };
    place.window = { __TAURI_INTERNALS__: { invoke: async (...args: unknown[]) => {
      invoked.push(args.slice(0, 2));
      if (fail) throw new Error("no app for it");
      return null;
    } } };
    try {
      pdfRow.open();
      await new Promise((resolve) => setImmediate(resolve));
      assert.deepEqual(invoked, [["open_artifact", { id: "hash-pdf", fileName: "report.pdf" }]]);
      assert.deepEqual(useToasts.getState().toasts, []);

      fail = true;
      pdfRow.open();
      await new Promise((resolve) => setImmediate(resolve));
      assert.deepEqual(useToasts.getState().toasts.map((entry: { text: string; tone: string }) => [entry.text, entry.tone]), [["no app for it", "error"]]);

      useToasts.setState({ toasts: [] });
      invoked.length = 0;
      refusedRow.open();
      scriptRow.open();
      await new Promise((resolve) => setImmediate(resolve));
      assert.deepEqual(invoked, [], "neither reaches the default app");
      assert.deepEqual(useToasts.getState().toasts.map((entry: { text: string; tone: string }) => [entry.text, entry.tone]), [["deploy.command wasn't opened: it could run a program on this Mac.", "error"]]);
      assert.deepEqual(opened, [[null, script]]);
    } finally {
      delete place.window;
    }

    // An image's row opens the viewer on it, among every image of the conversation.
    assert.equal(imageRow.open, undefined);
    assert.deepEqual(imageRow.viewer.gallery.images.map((entry: { name: string }) => entry.name), ["Image #1", "shot.png"]);
    assert.equal(imageRow.viewer.gallery.index, 0);
    assert.equal(otherRow.viewer.gallery.index, 1);
  } finally {
    await server.close();
  }
});
