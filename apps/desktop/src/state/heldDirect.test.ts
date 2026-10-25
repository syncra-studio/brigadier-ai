import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import type { AttachmentRef, Conversation, Request } from "@/ipc/generated";
import { deleteAll, removeProject } from "@/state/actions";
import { loadDraft } from "@/state/drafts";
import { heldDraft, holdDirect, useHeldDirect } from "@/state/heldDirect";
import { useApp } from "@/state/store";

const file: AttachmentRef = { id: "file", name: "notes.txt", mime: "text/plain", bytes: 3, pasted: false, inline: null };
const draft = { text: "Use blue", attachments: [file], mentions: [] };
const conversation = (id: string, projectId: string | null = null) => ({ id, kind: "session", projectId }) as unknown as Conversation;
const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
let pins: { scope: string; attachments: AttachmentRef[] }[];
/** The delete outcome's error per conversation id (none: deleted). */
let refused: Record<string, string>;

beforeEach(() => {
  const values = new Map<string, string>();
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  Object.defineProperty(globalThis, "localStorage", {
    configurable: true,
    value: { getItem: (key: string) => values.get(key) ?? null, setItem: (key: string, value: string) => { values.set(key, value); } },
  });
  useHeldDirect.setState({ held: [] });
  pins = [];
  refused = {};
  mockIPC((_command, payload) => {
    const req = (payload as { request: Request }).request;
    switch (req.method) {
      case "pinDraftAttachments":
        pins.push({ scope: req.scope, attachments: req.attachments });
        return { method: req.method };
      case "delete":
        return { method: req.method, outcomes: req.ids.map((id) => ({ id, error: refused[id] ?? null })) };
      case "removeProject":
        return { method: req.method, report: {} };
      default:
        return { method: req.method };
    }
  });
});

afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
  if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

const heldScopes = (id: string) => (useHeldDirect.getState().held ?? []).filter((entry) => entry.conversationId === id).map((entry) => entry.scope);

test("deleting conversations lets their held direct messages and pins go, keeps a failed delete's, and a refusal heard later holds nothing", async () => {
  useApp.setState({ conversations: { gone: conversation("gone"), kept: conversation("kept") } });
  holdDirect("gone", "w1", draft);
  holdDirect("kept", "w1", draft);
  const [goneScope] = heldScopes("gone");
  const [keptScope] = heldScopes("kept");
  refused = { kept: "it is busy" };
  await deleteAll(["gone", "kept"]);
  assert.deepEqual(heldScopes("gone"), []);
  assert.equal(loadDraft(goneScope!), null);
  assert.deepEqual(pins.at(-1), { scope: goneScope, attachments: [] });
  // The one whose delete failed keeps its message, pinned.
  assert.deepEqual(heldScopes("kept"), [keptScope]);
  assert.deepEqual(heldDraft(keptScope!), draft);
  // A send to the deleted conversation's worker refused only now keeps nothing.
  const before = pins.length;
  holdDirect("gone", "w1", draft);
  assert.deepEqual(heldScopes("gone"), []);
  assert.equal(pins.length, before);
});

test("removing a project lets its conversations' held direct messages and pins go", async () => {
  useApp.setState({ conversations: { inside: conversation("inside", "p1"), outside: conversation("outside", "p2") }, projects: {} });
  holdDirect("inside", "w1", draft);
  holdDirect("outside", "w1", draft);
  const [insideScope] = heldScopes("inside");
  await removeProject("p1", [], false);
  assert.deepEqual(heldScopes("inside"), []);
  assert.equal(loadDraft(insideScope!), null);
  assert.deepEqual(pins.at(-1), { scope: insideScope, attachments: [] });
  assert.equal(heldScopes("outside").length, 1);
  holdDirect("inside", "w1", draft);
  assert.deepEqual(heldScopes("inside"), []);
});
