import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Seen = { steps: string[]; workers: string[]; background: string[] | null; panel: string | null };

test("a subagent is a row of its block that opens its transcript, never a worker; a background one is a Background row while it runs (D96, D97)", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "subagent-rows.html", "subagent-rows-result", 30000)) as {
    running: Seen; foreground: string; background: string; ended: Seen; forgotten: Seen; reopened: string;
  };
  const { running, ended, forgotten } = seen;
  // Rows of the block, no workers (D96).
  assert.deepEqual(running.workers, []);
  assert.match(running.steps[0]!, /^Ran a subagent: Carry out TASK\.md steps\s*· 15s$/);
  assert.match(running.steps[1]!, /^Running a subagent/);
  // Its row and its panel read one time (D94); the panel shows its transcript.
  assert.match(seen.foreground, /^Worked for 15s.*All TASK\.md steps are done\./);
  // The background one is a Background row while it runs, and opens its transcript (D97).
  assert.ok(running.background?.some((row) => /^Subagent: Explore the parser\d+s$/.test(row)), JSON.stringify(running.background));
  assert.match(seen.background, /Reading the parser now\./);
  // Over, both leave for Background history, even once the CLI lists them no more.
  assert.deepEqual(ended.background, []);
  assert.deepEqual(forgotten.background, ended.background);
  assert.match(seen.reopened, /Reading the parser now\./);
});
