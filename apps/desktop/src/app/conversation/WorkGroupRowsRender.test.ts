import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

async function seen<T>(t: test.TestContext): Promise<T> {
  const result = JSON.parse(await renderFixtureLive(t, "work-group-rows.html", "work-group-rows-result", 90000)) as T & { error?: string };
  assert.equal(result.error, undefined);
  return result;
}

test("a row opened inside a work group scrolls its state line into the group's list", { timeout: 120000 }, async (t) => {
  const { openInGroup } = await seen<{ openInGroup: { scrolls: boolean; endingShown: boolean } }>(t);
  assert.deepEqual(openInGroup, { scrolls: true, endingShown: true });
});
