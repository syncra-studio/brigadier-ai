import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

test("a turn that paused for more than an hour shows a time line where its work went on, between two groups (issue 130)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-gaps.html", "thread-gaps-result", 90000)) as { error?: string; work: string[]; expected: string };
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.work, ["group: Edited a file, read files", `line: ${seen.expected}`, "group: Edited a file, ran a command"]);
});
