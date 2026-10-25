import assert from "node:assert/strict";
import { test } from "node:test";
import { sendButtonState } from "@/app/conversation/sendButtonState";

const idle = { armed: false, canCancel: false, empty: true, running: false, resumable: false };

test("the submit button has only send, stop and resume looks; queue/steer keeps the arrow", () => {
  assert.equal(sendButtonState(idle), "send");
  assert.equal(sendButtonState({ ...idle, empty: false }), "send");
  assert.equal(sendButtonState({ ...idle, canCancel: true, running: true }), "stop");
  assert.equal(sendButtonState({ ...idle, canCancel: true, running: true, empty: false }), "send");
  assert.equal(sendButtonState({ ...idle, canCancel: true, running: true, empty: false, armed: true }), "stop");
  assert.equal(sendButtonState({ ...idle, resumable: true }), "resume");
  assert.equal(sendButtonState({ ...idle, resumable: true, empty: false }), "send");
  assert.equal(sendButtonState({ ...idle, canCancel: true, resumable: true }), "stop");
});
