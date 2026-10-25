import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The first group reads, then searches: its title says both verbs (D58).
test("a turn that paused for more than an hour shows a time line where its work went on, between two groups (issue 130)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-gaps.html", "thread-gaps-result", 90000)) as { error?: string; work: string[]; expected: string };
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.work, ["group: Edited a file, read files, searched files", `line: ${seen.expected}`, "group: Edited a file, ran a command"]);
});

test("a turn that waited for a usage limit shows where it reset instead of a time line, between two groups (issue 128)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-gaps.html?scene=quota", "thread-gaps-result", 90000)) as { error?: string; work: string[]; expected: string };
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.work, ["group: Edited a file, read files, searched files", `divider: Claude usage limit reset · ${seen.expected}`, "group: Edited a file, ran a command"]);
});

test("a stand-in at a usage limit and the model back are dividers in the turn, in view with its work folded, never cards above the composer (issue 506)", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "thread-gaps.html?scene=moments", "thread-gaps-result", 90000)) as {
    error?: string; moments: string[]; expectedMoments: string[]; notices: number; workOpen: string | null;
  };
  assert.equal(seen.error, undefined, String(seen.error));
  assert.deepEqual(seen.moments, seen.expectedMoments);
  assert.equal(seen.workOpen, "false");
  assert.equal(seen.notices, 0);
});
