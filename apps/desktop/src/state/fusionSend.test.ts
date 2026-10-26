import assert from "node:assert/strict";
import { test } from "node:test";

import { fusionProviders } from "@/fixtures/fusionProviders";
import { armFusionSend, fusionQuestion, fusionReady, isFusionArmed, isFusionChat, moveFusionSend } from "@/state/fusionSend";
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
