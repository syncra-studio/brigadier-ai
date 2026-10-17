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

test("a running worker's lone step row says the live line in its own header", { timeout: 120000 }, async (t) => {
  const { workerLone } = await seen<{ workerLone: { rowLine: string | null; linesUnder: number } }>(t);
  assert.deepEqual(workerLone, { rowLine: "Thinking", linesUnder: 0 });
});

test("an opened edit's diff header stays clear of its group's row as the diff scrolls, at 1280 and 760 px", { timeout: 120000 }, async (t) => {
  const edits = await seen<Record<"edit-wide" | "edit-narrow", Record<string, boolean>>>(t);
  const clear = { scrolled: true, clearOfGroupRow: true, clearOfFade: true, inList: true };
  assert.deepEqual(edits["edit-wide"], clear);
  assert.deepEqual(edits["edit-narrow"], clear);
});
