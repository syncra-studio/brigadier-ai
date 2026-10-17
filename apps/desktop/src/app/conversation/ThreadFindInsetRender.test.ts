import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

for (const mode of ["short", "top", "welcome", "embedded"]) {
  test(`Find inset: ${mode}`, { timeout: 180000 }, async (t) => {
    const seen = JSON.parse(await renderFixtureLive(t, `thread-find.html?drive=${mode}`, "thread-find-result", 150000));
    assert.equal(seen.error, undefined);
    if (mode === "welcome" || mode === "embedded") {
      assert.equal(parseFloat(seen.padding), 32);
      return;
    }
    assert.equal(seen.clear, true, `${mode}: first line clears the Find bar`);
    assert.equal(seen.openingMoved, 0, `${mode}: opening keeps the reader’s place`);
    assert.equal(seen.closingMoved, 0, `${mode}: closing keeps the reader’s place`);
  });
}
