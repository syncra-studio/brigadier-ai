import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { ExternalStoreRuntimeCore } from "@assistant-ui/core/internal";
import { clearMocks, mockIPC } from "@/ipc/nativeMocks";

import type { RequestOf } from "@/ipc/client";
import type { AttachmentRef } from "@/ipc/generated";
import { draftRefs } from "@/lib/inlineImages";
import { forgetDraft, loadDraft, NEW_CHAT_SCOPE, saveDraft } from "@/state/drafts";

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

test("a refused direct message is kept whole apart with its worker, its #1 image and paste beside the session draft's own, pinned until dropped", async () => {
  const { dropHeldDirect, heldDraft, holdDirect, useHeldDirect } = await import("@/state/heldDirect");
  useHeldDirect.setState({ held: [] });
  const paste: AttachmentRef = { id: "paste", name: "Pasted text.md", mime: "text/markdown", bytes: 3, pasted: true, inline: 1 };
  const newer: AttachmentRef = { ...image, id: "newer", inline: 1 };
  saveDraft("session", "typed since [Image #1]", [newer], []);
  holdDirect("session", "worker-1", {
    text: "[Image #1] see [Pasted text #1] @src/a.ts",
    attachments: [{ ...image, inline: 1 }, paste],
    mentions: [{ type: "file", path: "src/a.ts" }],
  });
  const [held] = useHeldDirect.getState().held ?? [];
  assert.equal(held?.conversationId, "session");
  assert.equal(held?.taskId, "worker-1");
  // Nothing of it went into the session's draft, nor the other way round.
  assert.deepEqual(loadDraft("session")?.attachments, [newer]);
  assert.deepEqual(heldDraft(held!.scope), {
    text: "[Image #1] see [Pasted text #1] @src/a.ts",
    attachments: [{ ...image, inline: 1 }, paste],
    mentions: [{ type: "file", path: "src/a.ts" }],
  });
  assert.deepEqual(pins.at(-1), { method: "pinDraftAttachments", scope: held!.scope, attachments: [{ ...image, inline: 1 }, paste] });
  dropHeldDirect(held!.scope);
  assert.equal(heldDraft(held!.scope), null);
  assert.deepEqual(useHeldDirect.getState().held, []);
  assert.deepEqual(pins.at(-1), { method: "pinDraftAttachments", scope: held!.scope, attachments: [] });
});

/** Lets the composer's timers run. */
const rest = () => new Promise((resolve) => setTimeout(resolve, 10));

test("a pulled worker draft keeps its slot, mentions and worker across a restart, until sent (issues 672, 673)", async () => {
  const { PulledSlot } = await import("@/app/conversation/composerTarget");
  const mentions = [{ type: "task", id: "task-live" } as const];
  new PulledSlot().set("session", 1, [...mentions], "task-live");
  saveDraft("session", "for the importer", [], []);
  // The app starts again: only what was kept is known.
  const restarted = new PulledSlot();
  assert.equal(loadDraft("session")?.text, "for the importer");
  assert.equal(restarted.worker("session"), "task-live");
  assert.equal(restarted.holds("session"), true);
  // The send takes it, back to its slot; nothing is left for the next one.
  assert.deepEqual(restarted.take("session"), { index: 1, mentions, worker: "task-live" });
  assert.equal(new PulledSlot().holds("session"), false);
});

test("emptying a pulled draft lets its slot and To: go; forgetting its conversation too (issue 720)", async () => {
  const { PulledSlot } = await import("@/app/conversation/composerTarget");
  const { letPulledGo } = await import("@/app/conversation/composerDraft");
  const pulled = new PulledSlot();
  pulled.set("session", 0, [], "task-live");
  // Still words, or an attachment: it is still being edited.
  letPulledGo("session", { text: "for the", attachments: [] });
  letPulledGo("session", { text: "", attachments: [image] });
  assert.equal(pulled.worker("session"), "task-live");
  letPulledGo("session", { text: " \n", attachments: [] });
  assert.equal(pulled.holds("session"), false);
  assert.equal(pulled.worker("session"), null);
  pulled.set("deleted", 2, [], null);
  forgetDraft("deleted");
  assert.equal(pulled.holds("deleted"), false);
});

test("a pulled message sent while its attachment still uploads keeps its slot for that send (issue 720)", async () => {
  const { PulledSlot } = await import("@/app/conversation/composerTarget");
  const { letPulledGo } = await import("@/app/conversation/composerDraft");
  const pulled = new PulledSlot();
  let upload!: () => void;
  const uploaded = new Promise<void>((resolve) => { upload = resolve; });
  const taken: (number | null)[] = [];
  const runtime = new ExternalStoreRuntimeCore({
    messages: [],
    // The view's submit: a pulled message goes back to its slot.
    onNew: async () => { taken.push(pulled.take("session")?.index ?? null); },
    adapters: {
      attachments: {
        accept: "*",
        add: async ({ file }) => ({ id: "upload", type: "file", name: file.name, contentType: file.type, file, status: { type: "requires-action", reason: "composer-send" } }),
        remove: async () => {},
        send: async (attachment) => {
          await uploaded;
          return { ...attachment, status: { type: "complete" }, content: [] };
        },
      },
    },
  });
  const composer = runtime.threads.getMainThreadRuntimeCore().composer;
  // The composer's draft keeping as `useComposerDraft` does it: once its text or attachments
  // change, at once when they are gone.
  const keep = () => {
    saveDraft("session", composer.text, [], []);
    letPulledGo("session", composer);
  };
  let held = "";
  const unsubscribe = composer.subscribe(() => {
    const now = `${composer.text}|${composer.attachments.map((attachment) => attachment.id).join()}`;
    if (now === held) return;
    held = now;
    setTimeout(keep, 0);
  });
  try {
    pulled.set("session", 1, [], null);
    composer.setText("for the importer");
    await composer.addAttachment(new File(["notes"], "notes.txt", { type: "text/plain" }));
    const sending = composer.send();
    await rest();
    // Handed to the send, the composer is empty while the upload finishes, and was kept so.
    assert.equal(composer.text, "");
    assert.notEqual(composer.submission, undefined);
    assert.equal(loadDraft("session"), null);
    assert.equal(pulled.holds("session"), true);
    upload();
    await sending;
    await rest();
    assert.deepEqual(taken, [1]);
    assert.equal(pulled.holds("session"), false);
  } finally {
    unsubscribe();
  }
});
