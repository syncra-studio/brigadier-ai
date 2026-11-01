import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("a working block's header and the composer read the same time at every moment, from the message sent, not the CLI's later start (D94)", { timeout: 60000 }, async (t) => {
  const reads = JSON.parse(await renderFixturePage(t, "two-timers.html", "two-timers-result", 10000)) as [string | null, string | null][];
  for (const [header, composer] of reads) {
    assert.match(header ?? "", /^Working for 1[2-6]s$/, JSON.stringify(reads));
    assert.equal(composer, header, JSON.stringify(reads));
  }
});
