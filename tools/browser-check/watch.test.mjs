import assert from "node:assert/strict";
import test from "node:test";

import { classifyLogLine } from "./watch.mjs";

test("a front change to the watched app is the watched app's", () => {
  const row = { eventMessage: "SETFRONT: pid=24731 asn=0x-0x1925924 foreground=0 oldFrontPid=55615 oldFrontASN=0x-0x1" };
  assert.deepEqual(classifyLogLine(row, 24731), { kind: "log-front", to: 24731, foreground: 0, from: 55615, isWatched: true });
  assert.equal(classifyLogLine(row, 1).isWatched, false);
});

test("a system dialog process coming forward is a dialog", () => {
  const row = { eventMessage: "advicePolicy: .frontmost; frontmostProcess: 0x0-0x21021 (UserNotificationCenter) mainConnectionID: 69F0F;" };
  assert.deepEqual(classifyLogLine(row, 1), { kind: "log-front-name", name: "UserNotificationCenter", isDialog: true });
  const other = { eventMessage: "frontmostProcess: 0x0-0x21 (Finder) mainConnectionID: 1;" };
  assert.equal(classifyLogLine(other, 1).isDialog, false);
});

test("other log lines mean nothing", () => {
  assert.equal(classifyLogLine({ eventMessage: "something else" }, 1), null);
});
