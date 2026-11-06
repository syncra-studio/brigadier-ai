import assert from "node:assert/strict";
import test from "node:test";

import { fadeValue, shouldHold } from "./introRules.ts";

test("a fade starts and ends on its values, eased at the middle", () => {
  assert.equal(fadeValue(0, 0.85, 0), 0);
  assert.equal(fadeValue(0, 0.85, 1), 0.85);
  assert.equal(fadeValue(1, 0, 0.5), 0.5);
  assert.equal(fadeValue(0, 1, 2), 1);
});

test("only a first launch on macOS, not under --smoke, holds the window", () => {
  assert.equal(shouldHold("darwin", true, false), true);
  assert.equal(shouldHold("darwin", false, false), false);
  assert.equal(shouldHold("darwin", true, true), false);
  assert.equal(shouldHold("win32", true, false), false);
});
