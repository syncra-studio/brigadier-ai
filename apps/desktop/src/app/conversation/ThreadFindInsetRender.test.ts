import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("Find leaves the first line of a short thread clear", { timeout: 180000 }, async (t) => {
  for (const mode of ["short", "top"]) {
    const seen = JSON.parse(await renderFixtureLive(t, `thread-find.html?drive=${mode}`, "thread-find-result", 150000));
    assert.equal(seen.error, undefined);
    assert.equal(seen.clear, true, `${mode}: first line clears the Find bar`);
    assert.equal(seen.openingMoved, 0, `${mode}: opening keeps the reader’s place`);
    assert.equal(seen.closingMoved, 0, `${mode}: closing keeps the reader’s place`);
  }
});
