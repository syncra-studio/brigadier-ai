import assert from "node:assert/strict";
import { test } from "node:test";

import { bound, editing, rebound } from "./pasteBinding";

test("a view of pasted text keeps its own edit's attachment, and reads afresh another text that took its number", () => {
  let binding = bound("a1");
  assert.equal(rebound(binding, "a1"), binding);
  // Its edit: the attachment goes, and a new one comes under the same number.
  binding = editing(binding);
  binding = rebound(binding, undefined);
  binding = rebound(binding, "a2");
  assert.deepEqual(binding, { id: "a2", editing: false, generation: 0 });
  // Sent: the attachment goes; a later paste takes the number.
  binding = rebound(binding, undefined);
  assert.equal(binding.generation, 1);
  binding = rebound(binding, "b1");
  assert.deepEqual(binding, { id: "b1", editing: false, generation: 2 });
  // Removed and pasted again, with no edit between.
  assert.equal(rebound(binding, "c1").generation, 3);
});
