import assert from "node:assert/strict";
import { test } from "node:test";

import { fusionProviders } from "@/fixtures/fusionProviders";
import { armFusionSend, fusionQuestion, fusionReady, isFusionChat, takeFusionSend } from "@/state/fusionSend";
import { useApp } from "@/state/store";
import type { Conversation } from "@/ipc/generated";

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

test("Fusion is armed per composer and consumed once", () => {
  armFusionSend("first", true);
  assert.equal(takeFusionSend("second"), false);
  assert.equal(takeFusionSend("first"), true);
  assert.equal(takeFusionSend("first"), false);
  armFusionSend("first", true);
  armFusionSend("first", false);
  assert.equal(takeFusionSend("first"), false);
});

test("typed Fusion hides only its whole invocation prefix", () => {
  for (const prefix of ["/fusion", "$fusion"]) {
    assert.equal(fusionQuestion(`${prefix} Compare\nthe options.`), "Compare\nthe options.");
    assert.equal(fusionQuestion(prefix), "");
    assert.equal(fusionQuestion(`${prefix}ation`), null);
  }
  assert.equal(fusionQuestion("Please explain /fusion"), null);
});
