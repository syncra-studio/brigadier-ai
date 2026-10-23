import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

test("Reports reads the latest artifact, shows both arms and skip reasons, refreshes and keeps numbers on a read error", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "reports.html?drive=1", "reports-result", 15000)) as {
    initial: string; refreshed: string; failed: string; calls: number;
  };
  for (const value of ["abcdef1234", "First baseline", "Decisions survive · long · seam", "Decisions survive · long · plain",
    "Recall: 90%", "Applied: 9 of 10 planted", "Brief contradictions: 0", "Unmarked additions: 0", "Open items raised: 2 of 2",
    "420,000", "$2.3456", "0.940× plain CLI", "Cold return: $0.1234", "needs the user: secrets", "real-session-42", "1 compactions"]) {
    assert.ok(seen.initial.includes(value), `Missing ${value}: ${seen.initial}`);
  }
  assert.ok(seen.refreshed.includes("fedcba9876"));
  assert.ok(!seen.refreshed.includes("abcdef1234"));
  assert.ok(seen.failed.includes("GitHub CLI is offline. Try Refresh."));
  assert.ok(seen.failed.includes("Applied: 9 of 10 planted"));
  assert.equal(seen.calls, 3);
});
