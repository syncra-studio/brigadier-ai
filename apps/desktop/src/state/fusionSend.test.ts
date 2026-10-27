import assert from "node:assert/strict";
import { afterEach, beforeEach, test } from "node:test";
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";

import { fusionProviders } from "@/fixtures/fusionProviders";
import { armFusionSend, fusionQuestion, fusionReady, isFusionArmed, isFusionChat, moveFusionSend } from "@/state/fusionSend";
import { useApp } from "@/state/store";
import { deleteAll, removeProject } from "@/state/actions";
import { NEW_CHAT_SCOPE } from "@/state/drafts";
import type { Conversation, Request } from "@/ipc/generated";
import type * as FusionSend from "@/state/fusionSend";

const originalWindow = Object.getOwnPropertyDescriptor(globalThis, "window");
const originalStorage = Object.getOwnPropertyDescriptor(globalThis, "localStorage");
let values: Map<string, string>;
let reloads = 0;
// A fresh module has no in-memory toggles, just as after the webview restarts.
const reload = (): Promise<typeof FusionSend> =>
  import(new URL(`./fusionSend.ts?reload=${++reloads}`, import.meta.url).href);
const conversation = (id: string, projectId: string | null) => ({ id, kind: "chat", projectId }) as Conversation;

beforeEach(() => {
  values = new Map();
  Object.defineProperty(globalThis, "window", { configurable: true, value: {} });
  Object.defineProperty(globalThis, "localStorage", { configurable: true, value: {
    getItem: (key: string) => values.get(key) ?? null,
    setItem: (key: string, value: string) => { values.set(key, value); },
  } });
});

afterEach(() => {
  clearMocks();
  if (originalWindow) Object.defineProperty(globalThis, "window", originalWindow);
  else Reflect.deleteProperty(globalThis, "window");
  if (originalStorage) Object.defineProperty(globalThis, "localStorage", originalStorage);
  else Reflect.deleteProperty(globalThis, "localStorage");
});

test("Fusion on and off survive a restart independently per Chat", async () => {
  const first = await reload();
  first.armFusionSend("one", true);
  first.armFusionSend("two", true);
  const reopened = await reload();
  assert.equal(reopened.isFusionArmed("one"), true);
  assert.equal(reopened.isFusionArmed("two"), true);
  assert.equal(reopened.isFusionArmed("other"), false);
  reopened.armFusionSend("one", false);
  const again = await reload();
  assert.equal(again.isFusionArmed("one"), false);
  assert.equal(again.isFusionArmed("two"), true);
});

test("a restored draft moves its saved toggle to the created Chat", async () => {
  (await reload()).armFusionSend(NEW_CHAT_SCOPE, true);
  const reopened = await reload();
  assert.equal(reopened.isFusionArmed(NEW_CHAT_SCOPE), true);
  reopened.moveFusionSend(NEW_CHAT_SCOPE, "created");
  reopened.moveFusionSend("created", "created");
  reopened.moveFusionSend(NEW_CHAT_SCOPE, "next");
  const again = await reload();
  assert.equal(again.isFusionArmed("created"), true);
  assert.equal(again.isFusionArmed(NEW_CHAT_SCOPE), false);
  assert.equal(again.isFusionArmed("next"), false);
});

test("bad or unavailable storage leaves Fusion usable in memory", async () => {
  for (const stored of ["{", "null", "{}", "true"]) {
    values.set("brigadier.fusionSend", stored);
    const fusion = await reload();
    assert.equal(fusion.isFusionArmed("one"), false);
    fusion.armFusionSend("one", true);
    assert.equal(fusion.isFusionArmed("one"), true);
  }
  Object.defineProperty(globalThis, "localStorage", { configurable: true, get: () => { throw new Error("unavailable"); } });
  const fusion = await reload();
  fusion.armFusionSend("one", true);
  assert.equal(fusion.isFusionArmed("one"), true);
  fusion.armFusionSend("one", false);
  assert.equal(fusion.isFusionArmed("one"), false);
});

test("successful chat and project deletion forget saved toggles; failed deletes keep them", async () => {
  useApp.setState({ conversations: {
    gone: conversation("gone", null), kept: conversation("kept", null), inside: conversation("inside", "p1"),
  }, projects: {} });
  for (const id of ["gone", "kept", "inside"]) armFusionSend(id, true);
  mockIPC((_command, payload) => {
    const req = (payload as { request: Request }).request;
    if (req.method === "delete") return { method: req.method, outcomes: req.ids.map((id) => ({ id, error: id === "kept" ? "busy" : null })) };
    return { method: req.method, report: {} };
  });
  await deleteAll(["gone", "kept"]);
  await removeProject("p1", [], false);
  assert.equal(isFusionArmed("gone"), false);
  assert.equal(isFusionArmed("inside"), false);
  assert.equal(isFusionArmed("kept"), true);
  const reopened = await reload();
  assert.equal(reopened.isFusionArmed("gone"), false);
  assert.equal(reopened.isFusionArmed("inside"), false);
  assert.equal(reopened.isFusionArmed("kept"), true);
  armFusionSend("kept", false);
});

test("Fusion readiness follows installed, signed-in and allowed CLIs", () => {
  const settings = useApp.getState().settings;
  assert.equal(fusionReady(fusionProviders, settings), true);
  assert.equal(fusionReady(fusionProviders.slice(0, 1), settings), false);
  assert.equal(fusionReady(fusionProviders, { ...settings, disabledProviders: ["codex"] }), false);
  assert.equal(fusionReady(fusionProviders.map((p) => p.provider === "codex" ? { ...p, status: { ...p.status!, loggedIn: false } } : p), settings), false);
  assert.equal(fusionReady(fusionProviders.map((p) => p.provider === "codex" ? { ...p, status: { ...p.status!, path: null } } : p), settings), false);
});

test("Fusion belongs only to active top-level Chats", () => {
  const chat = { kind: "chat", sideOf: null, lifecycle: "active" } as Conversation;
  assert.equal(isFusionChat(chat), true);
  assert.equal(isFusionChat({ ...chat, kind: "session" }), false);
  assert.equal(isFusionChat({ ...chat, sideOf: "session" }), false);
  assert.equal(isFusionChat({ ...chat, lifecycle: "archived" }), false);
  assert.equal(isFusionChat({ ...chat, tabChat: { tabId: "browser:1", target: { type: "browser" } } }), false);
});

test("Fusion stays on per Chat across sends until toggled off", () => {
  armFusionSend("first", true);
  assert.equal(isFusionArmed("second"), false);
  assert.equal(isFusionArmed("first"), true);
  assert.equal(isFusionArmed("first"), true);
  armFusionSend("first", false);
  assert.equal(isFusionArmed("first"), false);
});

test("a new Chat inherits its draft's Fusion state without arming the next draft", () => {
  armFusionSend("new", true);
  moveFusionSend("new", "created");
  assert.equal(isFusionArmed("created"), true);
  assert.equal(isFusionArmed("created"), true);
  assert.equal(isFusionArmed("new"), false);
  moveFusionSend("new", "next");
  assert.equal(isFusionArmed("next"), false);
  moveFusionSend("created", "created");
  assert.equal(isFusionArmed("created"), true);
  armFusionSend("created", false);
});

test("typed Fusion hides only its whole invocation prefix", () => {
  for (const prefix of ["/fusion", "$fusion"]) {
    assert.equal(fusionQuestion(`${prefix} Compare\nthe options.`), "Compare\nthe options.");
    assert.equal(fusionQuestion(prefix), "");
    assert.equal(fusionQuestion(`${prefix}ation`), null);
  }
  assert.equal(fusionQuestion("Please explain /fusion"), null);
});
