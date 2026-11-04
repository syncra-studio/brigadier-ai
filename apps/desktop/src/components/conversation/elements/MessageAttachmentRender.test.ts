import assert from "node:assert/strict";
import { test } from "node:test";
import React from "react";
import ReactDOMServer from "react-dom/server";
import { createServer } from "vite";

import type { AttachmentRef } from "@/ipc/generated";

const ref = (id: string, name: string, mime: string, pasted = false): AttachmentRef => ({ id, name, mime, bytes: 4096, pasted, inline: null });
const pasted = ref("blob-1", "Pasted text.txt", "text/plain", true);
const notes = ref("blob-2", "notes.txt", "text/plain");
const report = ref("blob-3", "report.pdf", "application/pdf");
const script = ref("blob-4", "tool.py", "text/x-python");
const launcher = ref("blob-5", "deploy.command", "application/octet-stream");
const files = [pasted, notes, report, script, launcher];

const newServer = () => createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });

const settle = () => new Promise((resolve) => setImmediate(resolve));

/** Whether a button opens inside another one's markup. */
function nestsButtons(html: string): boolean {
  let depth = 0;
  for (const [tag] of html.matchAll(/<\/?button\b/g)) {
    depth += tag === "<button" ? 1 : -1;
    if (depth > 1) return true;
  }
  return false;
}

// The user's report: a sent message's file cards opened nothing, but for a button on "Pasted text"'s name.
test("a sent message's whole file card is one button, with a hover and focus state, and no button in a button", async () => {
  const server = await newServer();
  try {
    const { MessageAttachments, PastedTextOpenerContext } = await server.ssrLoadModule("/src/components/conversation/index.ts");
    const html = (opener: unknown) => ReactDOMServer.renderToStaticMarkup(
      React.createElement(PastedTextOpenerContext.Provider, { value: opener },
        React.createElement(MessageAttachments, { attachments: files })),
    );

    const shown = html(() => {});
    for (const [name, detail] of [["Pasted text", "TXT text"], ["notes.txt", "TXT text"], ["report.pdf", "PDF"], ["tool.py", "PY text"], ["deploy.command", "COMMAND file"]]) {
      const button = new RegExp(`<button[^>]*aria-label="Open ${name}"[^>]*>.*?</button>`).exec(shown)?.[0];
      assert.ok(button, `${name}'s card is a button`);
      assert.ok(button.includes(`>${name}<`), `${name}'s title is inside it`);
      assert.ok(button.includes(`${detail} · 4.0 KB`), `${name}'s kind and size are inside it`);
      assert.match(button, /hover:bg-foreground\/8/);
      assert.match(button, /focus-visible:ring-2/);
    }
    assert.equal(shown.match(/<button/g)?.length, files.length, "one button a card: the name-only one is gone");
    assert.equal(nestsButtons(shown), false);
    assert.doesNotMatch(shown, /<button[^>]*disabled/);

    // Without a conversation to open them in (a fixture board), what the reader shows stays plain.
    const plain = html(null);
    assert.equal(plain.match(/<button/g)?.length, 2);
    assert.match(plain, /aria-label="Open report\.pdf"/);
    assert.match(plain, /aria-label="Open deploy\.command"/);
    assert.match(plain, />Pasted text</);
    assert.match(plain, />notes\.txt</);
    assert.match(plain, />tool\.py</);
  } finally {
    await server.close();
  }
});

test("each file card does what its kind does: the reader, the default app, or the refusal", async () => {
  const server = await newServer();
  try {
    const { FileTile, MessageAttachments, PastedTextOpenerContext } = await server.ssrLoadModule("/src/components/conversation/index.ts");
    const { useToasts } = await server.ssrLoadModule("/src/state/toasts.ts");

    // The cards' props, as the message's list made them.
    const actions = new Map<string, () => void>();
    const Probe = () => {
      const list = MessageAttachments({ attachments: files }) as React.ReactElement<{ children: React.ReactElement<{ children: React.ReactElement<{ name: string; onOpen: () => void }> }>[] }>;
      for (const item of list.props.children) {
        const card = item.props.children;
        assert.equal(card.type, FileTile);
        actions.set(card.props.name, card.props.onOpen);
      }
      return null;
    };
    const opened: unknown[][] = [];
    ReactDOMServer.renderToStaticMarkup(
      React.createElement(PastedTextOpenerContext.Provider, { value: (...args: unknown[]) => opened.push(args) }, React.createElement(Probe)),
    );
    const open = (file: AttachmentRef) => actions.get(file.name)!();

    const invoked: unknown[][] = [];
    let fail = false;
    const place = globalThis as { window?: unknown };
    place.window = { __TAURI_INTERNALS__: { invoke: async (...args: unknown[]) => {
      invoked.push(args.slice(0, 2));
      if (fail) throw new Error("no app for it");
      return null;
    } } };
    const toasts = () => useToasts.getState().toasts.map((entry: { text: string; tone: string }) => [entry.text, entry.tone]);
    try {
      open(pasted);
      open(notes);
      open(script);
      await settle();
      assert.deepEqual(opened, [[null, pasted], [null, notes], [null, script]], "the reader gets each, under its own name");
      assert.deepEqual(invoked, [], "nothing that reads as text reaches the default app");

      open(report);
      await settle();
      assert.deepEqual(invoked, [["open_artifact", { id: "blob-3", fileName: "report.pdf" }]]);
      assert.deepEqual(toasts(), []);

      fail = true;
      open(report);
      await settle();
      assert.deepEqual(toasts(), [["no app for it", "error"]]);

      useToasts.setState({ toasts: [] });
      invoked.length = 0;
      open(launcher);
      await settle();
      assert.deepEqual(invoked, [], "what launches never reaches the default app");
      assert.deepEqual(toasts(), [["deploy.command wasn't opened: it could run a program on this Mac.", "error"]]);
    } finally {
      delete place.window;
    }
  } finally {
    await server.close();
  }
});

test("FileTile's remove button stays outside its button, and without onOpen the card is as the composer has it", async () => {
  const server = await newServer();
  try {
    const { FileTile, TileRemove } = await server.ssrLoadModule("/src/components/conversation/index.ts");
    const tile = (props: object) => ReactDOMServer.renderToStaticMarkup(
      React.createElement(FileTile, { name: "notes.txt", mime: "text/plain", detail: "TXT text", ...props }, React.createElement(TileRemove, { label: "Remove notes.txt" })),
    );

    const openable = tile({ onOpen: () => {}, openLabel: "Open notes.txt" });
    assert.equal(nestsButtons(openable), false);
    assert.match(openable, /<\/button><button[^>]*aria-label="Remove notes\.txt"/, "the remove button comes after the card's button closes");

    const composer = tile({});
    assert.equal(composer.match(/<button/g)?.length, 1, "only the remove button");
    assert.match(composer, /data-slot="attachment-file"[^>]*class="[^"]*\bgap-2 p-1\.5 pe-7\b/, "the card keeps its padding");
    assert.doesNotMatch(composer, /hover:bg-foreground/);
  } finally {
    await server.close();
  }
});
