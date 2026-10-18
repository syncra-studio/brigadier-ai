import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("back-to-back calls of a tool with no words of its own fold into one row that opens to each call (issue 101)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-repeats.html", "thread-repeats-result", 90000)) as {
    error?: string; summary: string; rows: string[]; calls: string[];
  };
  assert.equal(seen.error, undefined, String(seen.error));
  assert.equal(seen.summary, "Edited a file, read files, used search issues 6 times, used get issue 2 times");
  // Two calls of a tool stay two rows; the fold's time is its calls' own.
  assert.deepEqual(seen.rows, ["Read tax.ts in 2s", "Used search issues 6 times in 12s", "Used get issue in 2s", "Used get issue in 2s", "Edited tax.ts in 2s"]);
  assert.deepEqual(seen.calls, ["checkout tax", "tax rounding", "VAT basket", "tax EU", "discount tax", "tax regression"].map((query) => `Used search issues ${query} in 2s`));
});
