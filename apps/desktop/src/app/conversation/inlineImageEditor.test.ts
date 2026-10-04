import assert from "node:assert/strict";
import { test } from "node:test";
import { ExternalStoreRuntimeCore } from "@assistant-ui/core/internal";
import type { ExternalStoreAdapter } from "@assistant-ui/react";
import type { Unstable_DirectiveFormatter } from "@assistant-ui/react";
import { $createDirectiveNodeWithFormatter, DirectiveNode } from "@assistant-ui/react-lexical";
import { createEmptyHistoryState, registerHistory } from "@lexical/history";
import { registerPlainText } from "@lexical/plain-text";
import { $createParagraphNode, $createTextNode, $getRoot, $getSelection, $isRangeSelection, $nodesOfType, createEditor, HISTORY_PUSH_TAG, REDO_COMMAND, UNDO_COMMAND } from "lexical";

import { registerInlineImages } from "@/app/conversation/inlineImageEditor";
import { IMAGE_UPLOAD, INLINE_IMAGE, InlineImages, imageSegments, imageToken, uploadToken } from "@/lib/inlineImages";
import type { AttachmentRef } from "@/ipc/generated";

const ref = (id: string): AttachmentRef => ({ id, inline: false, pasted: false, bytes: 3, mime: "image/png", name: `${id}.png` });
const tick = () => new Promise<void>((resolve) => queueMicrotask(resolve));

function setup() {
  const images = new InlineImages();
  const formatter: Unstable_DirectiveFormatter = {
    serialize: (item) => item.type === INLINE_IMAGE ? imageToken(item.id) : uploadToken(item.id),
    parse: (value) => imageSegments(value, images, (plain) => plain ? [{ kind: "text", text: plain }] : []),
  };
  const editor = createEditor({ namespace: "inline-image-test", nodes: [DirectiveNode], onError: (error) => { throw error; } });
  registerPlainText(editor);
  registerHistory(editor, createEmptyHistoryState(), 0);
  registerInlineImages(editor, images, formatter);
  const text = () => editor.getEditorState().read(() => $getRoot().getTextContent());
  const insert = (key: string) => {
    images.begin(key, `${key}.png`);
    const selection = $getSelection();
    assert.ok($isRangeSelection(selection));
    selection.insertNodes([$createDirectiveNodeWithFormatter({ id: key, type: IMAGE_UPLOAD, label: `${key}.png` }, formatter)]);
  };
  editor.update(() => {
    const node = $createTextNode("before after");
    $getRoot().append($createParagraphNode().append(node));
    node.select(7, 7);
  }, { discrete: true });
  return { images, formatter, editor, text, insert };
}

test("Lexical inserts pending images at the caret in clipboard order and resolves out of order", async () => {
  const { images, editor, text, insert } = setup();
  editor.update(() => { insert("one"); insert("two"); }, { discrete: true, tag: HISTORY_PUSH_TAG });
  assert.equal(text(), "before [image-upload:one][image-upload:two]after");
  images.complete("two", ref("second"));
  await tick();
  images.complete("one", ref("first"));
  await tick();
  assert.equal(text(), "before [image:first][image:second]after");
  assert.equal(images.blocked, false);
});

test("Lexical undo and redo across completion and deletion restore ready image nodes", async () => {
  const { images, editor, text, insert } = setup();
  editor.update(() => insert("one"), { discrete: true, tag: HISTORY_PUSH_TAG });
  images.complete("one", ref("first"));
  await tick();
  assert.equal(text(), "before [image:first]after");
  editor.dispatchCommand(UNDO_COMMAND, undefined);
  await tick();
  assert.equal(text(), "before after");
  editor.dispatchCommand(REDO_COMMAND, undefined);
  await tick();
  assert.equal(text(), "before [image:first]after");
  editor.update(() => $nodesOfType(DirectiveNode)[0]?.remove(), { discrete: true, tag: HISTORY_PUSH_TAG });
  assert.equal(text(), "before after");
  assert.equal(images.refs.has("first"), true);
  editor.dispatchCommand(UNDO_COMMAND, undefined);
  await tick();
  assert.equal(text(), "before [image:first]after");
  editor.dispatchCommand(REDO_COMMAND, undefined);
  await tick();
  assert.equal(text(), "before after");
});

test("Lexical deleted pending nodes ignore completion and undo exposes a removable error", async () => {
  const { images, editor, text, insert } = setup();
  editor.update(() => insert("one"), { discrete: true, tag: HISTORY_PUSH_TAG });
  editor.update(() => $nodesOfType(DirectiveNode)[0]?.remove(), { discrete: true, tag: HISTORY_PUSH_TAG });
  assert.equal(images.complete("one", ref("late")), false);
  editor.dispatchCommand(UNDO_COMMAND, undefined);
  await tick();
  assert.equal(text(), "before [image-upload:one]after");
  assert.equal(images.uploads.get("one")?.status, "failed");
  assert.equal(images.blocked, true);
});

test("Lexical draft reload and plain-text cut/paste restore only known inline nodes", () => {
  const { images, editor, text } = setup();
  images.recall("[image:known]", [{ ...ref("known"), inline: true }]);
  editor.update(() => {
    $getRoot().clear().append($createParagraphNode().append($createTextNode("a[image:known]b[image:unknown]")));
  }, { discrete: true });
  assert.equal(text(), "a[image:known]b[image:unknown]");
  assert.equal(editor.getEditorState().read(() => $nodesOfType(DirectiveNode).length), 1);
});

test("the shared runtime guard preserves pending drafts for normal, queue, steer and programmatic sends", async () => {
  const images = new InlineImages();
  const sent: string[] = [];
  const adapter = (): ExternalStoreAdapter => ({
    messages: [],
    isRunning: true,
    isSendDisabled: images.blocked,
    onNew: async () => { sent.push("new"); },
    queue: {
      items: [], steerItems: [],
      enqueue: () => { sent.push("queue"); },
      steer: () => { sent.push("steer"); },
      remove: () => {}, move: () => {}, edit: () => {},
    },
  });
  const runtime = new ExternalStoreRuntimeCore(adapter());
  const composer = runtime.threads.getMainThreadRuntimeCore().composer;
  images.subscribe(() => runtime.setAdapter(adapter()));
  images.begin("local", "shot.png");
  composer.setText("before[image-upload:local]after");
  for (const options of [undefined, { steer: false }, { steer: true }, { startRun: true }]) {
    await composer.send(options);
    assert.equal(composer.text, "before[image-upload:local]after");
    assert.equal(composer.canSend, false);
  }
  assert.deepEqual(sent, []);
  images.complete("local", ref("ready"));
  composer.setText("before[image:ready]after");
  assert.equal(composer.canSend, true);
  await composer.send({ steer: true });
  assert.deepEqual(sent, ["steer"]);
});

test("Lexical undo into an older pending snapshot resolves it using the retained completion", async () => {
  const { images, editor, text, insert } = setup();
  editor.update(() => insert("one"), { discrete: true, tag: HISTORY_PUSH_TAG });
  editor.update(() => {
    const selection = $getSelection();
    assert.ok($isRangeSelection(selection));
    selection.insertText("typed ");
  }, { discrete: true, tag: HISTORY_PUSH_TAG });
  images.complete("one", ref("first"));
  await tick();
  assert.equal(text(), "before [image:first]typed after");
  editor.dispatchCommand(UNDO_COMMAND, undefined);
  await tick();
  await tick();
  assert.equal(text(), "before [image:first]after");
  assert.equal(images.blocked, false);
  editor.dispatchCommand(REDO_COMMAND, undefined);
  await tick();
  assert.equal(text(), "before [image:first]typed after");
});
