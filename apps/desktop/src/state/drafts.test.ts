import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { ExternalStoreRuntimeCore } from "@assistant-ui/core/internal";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { RequestOf } from "@/ipc/client";
import type { AttachmentRef } from "@/ipc/generated";
import { draftRefs } from "@/lib/inlineImages";
import { loadDraft, NEW_CHAT_SCOPE, saveDraft } from "@/state/drafts";

const image: AttachmentRef = { id: "image", name: "shot.png", mime: "image/png", bytes: 4, pasted: false, inline: 0 };
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
let pins: RequestOf<"pinDraftAttachments">[];

beforeEach(() => {
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: {
      getItem: (key: string) => values.get(key) ?? null,
      setItem: (key: string, value: string) => { values.set(key, value); },
    },
  });
  pins = [];
  mockIPC((command, payload) => {
    assert.equal(command, "ipc_request");
    const pin = (payload as { request: RequestOf<"pinDraftAttachments"> }).request;
    assert.equal(pin.method, "pinDraftAttachments");
    pins.push(pin);
    return { method: pin.method };
  });
});

afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
  if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

test("sending removes the saved inline draft and releases its daemon pins even with retained refs", async () => {
  const retained = [image];
  const runtime = new ExternalStoreRuntimeCore({ messages: [], onNew: async () => {} });
  const composer = runtime.threads.getMainThreadRuntimeCore().composer;
  const keep = () => saveDraft(NEW_CHAT_SCOPE, composer.text, draftRefs(composer.text, [], retained), []);
  const unsubscribe = composer.subscribe(keep);
  try {
    composer.setText("look [image:image]");
    assert.deepEqual(loadDraft(NEW_CHAT_SCOPE)?.attachments, [image]);
    await composer.send();
    assert.equal(composer.text, "");
    assert.equal(loadDraft(NEW_CHAT_SCOPE), null);
    assert.deepEqual(pins.at(-1), { method: "pinDraftAttachments", scope: NEW_CHAT_SCOPE, attachments: [] });
    assert.deepEqual(retained, [image]);
    keep();
    assert.equal(loadDraft(NEW_CHAT_SCOPE), null);
  } finally {
    unsubscribe();
  }
});

test("clearing history recall releases all retained pins and keeps the new scope empty on reload", () => {
  const second = { ...image, id: "second" };
  const retained = [image, second];
  saveDraft(NEW_CHAT_SCOPE, "[image:second]", retained, []);
  assert.equal(pins.at(-1)?.attachments.length, 2);
  saveDraft(NEW_CHAT_SCOPE, " \n ", retained, []);
  assert.equal(loadDraft(NEW_CHAT_SCOPE), null);
  assert.deepEqual(pins.at(-1)?.attachments, []);
  saveDraft(NEW_CHAT_SCOPE, "", [], []);
  assert.equal(loadDraft(NEW_CHAT_SCOPE), null);
});

test("empty drafts preserve row attachments sharing an inline id", () => {
  const row = { ...image, inline: null };
  saveDraft("conversation", "[image:image]", [row, image], []);
  saveDraft("conversation", "", [row, image], []);
  assert.deepEqual(loadDraft("conversation")?.attachments, [row]);
  assert.deepEqual(pins.at(-1)?.attachments, [row]);
});

test("nonempty drafts retain and pin deleted images for undo and reload", () => {
  saveDraft("conversation", "image removed while still editing", [image], []);
  const draft = loadDraft("conversation");
  assert.deepEqual(draft?.attachments, [image]);
  assert.deepEqual(pins.at(-1)?.attachments, [image]);
  assert.deepEqual(draftRefs(draft!.text, [], draft!.attachments), [image]);
});
