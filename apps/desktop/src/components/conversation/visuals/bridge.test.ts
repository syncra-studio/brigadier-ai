import assert from "node:assert/strict";
import { test } from "node:test";
import { canDrawVisuals, readFrameMessage, receiveFrameMessage } from "./bridge";

test("only the current frame port is accepted", () => {
  const own = new MessageChannel();
  const other = new MessageChannel();
  assert.deepEqual(receiveFrameMessage(own.port1, own.port1, { type: "resize", height: 100 }), { type: "resize", height: 100 });
  assert.equal(receiveFrameMessage(other.port1, own.port1, { type: "sendPrompt", text: "Forged" }), null);
  assert.equal(receiveFrameMessage(own.port1, null, { type: "ready" }), null);
  for (const port of [own.port1, own.port2, other.port1, other.port2]) port.close();
});
test("height is finite and bounded", () => {
  assert.deepEqual(readFrameMessage({ type: "resize", height: -10 }), { type: "resize", height: 50 });
  assert.deepEqual(readFrameMessage({ type: "resize", height: 9000 }), { type: "resize", height: 4000 });
  for (const height of [NaN, Infinity, "100", null]) assert.equal(readFrameMessage({ type: "resize", height }), null);
});
test("follow-ups have a text cap and links use HTTPS only", () => {
  for (const text of ["", "   ", "x".repeat(4001), 3]) assert.equal(readFrameMessage({ type: "sendPrompt", text }), null);
  assert.deepEqual(readFrameMessage({ type: "sendPrompt", text: "x".repeat(4000) }), { type: "sendPrompt", text: "x".repeat(4000) });
  for (const url of ["http://example.com", "javascript:alert(1)", "file:///tmp/a", "brigadier-ui://localhost/frame", "/relative", "https://" ]) assert.equal(readFrameMessage({ type: "openLink", url }), null);
  assert.deepEqual(readFrameMessage({ type: "openLink", url: "https://example.com" }), { type: "openLink", url: "https://example.com" });
  for (const value of [null, 3, [], {}, { type: "invoke" }, { type: "render" }]) assert.equal(readFrameMessage(value), null);
});

test("only top-level Chats can draw, including when a side chat is opened on its own", () => {
  assert.equal(canDrawVisuals({kind: "chat"}, false), true);
  for (const [conversation, embedded] of [
    [{kind: "session"}, false], [{kind: "chat", sideOf: "parent"}, false],
    [{kind: "chat", tabChat: {}}, false], [{kind: "chat"}, true],
    [{kind: "worker"}, false], [null, false],
  ] as const) assert.equal(canDrawVisuals(conversation, embedded), false);
});
