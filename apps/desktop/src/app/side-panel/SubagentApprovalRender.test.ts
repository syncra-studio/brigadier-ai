import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

// #668: a subagent's command that waits on the user's approval reads "Awaiting approval" in its
// panel, as the lead's does, not "Running …"; once allowed it runs.
test("a subagent's panel says its command awaits approval while its approval waits, and runs it once allowed", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixturePage(t, "subagent-approval.html", "subagent-approval-result", 15000)) as Record<string, unknown>;
  assert.equal(seen.error, undefined, `${String(seen.error)}: ${String(seen.text)}`);
  assert.deepEqual(seen.waiting, { awaiting: true, running: false });
  assert.deepEqual(seen.allowed, { awaiting: false, running: true });
});
