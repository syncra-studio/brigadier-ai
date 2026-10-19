import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Job = { state: string; text: string; value: string; fill: number; spinning: boolean };

test("job progress fills its bar by the share done, and settles into how the job ended", { timeout: 60000 }, async (t) => {
  const seen = JSON.parse(
    await renderFixtureLive(t, "job-progress.html", "job-progress-result", 30000, { reducedMotion: true }),
  ) as Job[];
  assert.deepEqual(
    seen.map((job) => job.state),
    ["running", "success", "partial", "failed", "cancelled", "running"],
  );
  assert.equal(seen[0]!.text, "Downloading…47.0 MB of 141.0 MB");
  assert.equal(seen[0]!.value, "33");
  assert.equal(seen[0]!.fill, 33);
  assert.equal(seen[0]!.spinning, true);
  // A job that succeeded is full; the rest stop where they ended.
  assert.deepEqual(
    seen.slice(1).map((job) => job.value),
    ["100", "67", "33", "17", "0"],
  );
  assert.equal(seen[2]!.text, "Windows supportFour of six phases are verified.");
  assert.equal(seen[1]!.spinning, false);
});
