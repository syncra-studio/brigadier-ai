import assert from "node:assert/strict";
import { test } from "node:test";

import { darkMark } from "@/components/conversation/site-icons";

test("a near-black mark on clear is told from an icon that shows on the dark thread", () => {
  const clear = [0, 0, 0, 0];
  // GitHub's favicon, as measured: its visible pixels are at most (28, 25, 25).
  assert.equal(darkMark([...clear, 28, 25, 25, 255]), true);
  assert.equal(darkMark([...clear, 28, 25, 25, 255, 240, 240, 240, 255]), false);
  assert.equal(darkMark([0, 0, 0, 255, 200, 40, 40, 255]), false);
  assert.equal(darkMark(clear), false);
});
