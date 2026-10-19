import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("back-to-back calls of a tool with no words of its own fold into one row that opens to each call (issue 101)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-repeats.html", "thread-repeats-result", 90000)) as {
    error?: string; summary: string; rows: string[]; calls: string[];
  };
  assert.equal(seen.error, undefined, String(seen.error));
  // An integration's calls say the integration, never a raw tool name.
  assert.equal(seen.summary, "Edited a file, read files, used Linear 8 times");
  // The fold's time is its calls' own.
  assert.deepEqual(seen.rows, ["Read tax.ts in 2s", "Used Linear 8 times in 16s", "Edited tax.ts in 2s"]);
  // Opened, each call says what it did and was given.
  assert.deepEqual(seen.calls, [
    ...["checkout tax", "tax rounding", "VAT basket", "tax EU", "discount tax", "tax regression"].map((query) => `Used Linear search issues · ${query} in 2s`),
    "Used Linear get issue · LIN-412 in 2s", "Used Linear get issue · LIN-413 in 2s",
  ]);
});
