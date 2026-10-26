import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixturePage } from "@/fixtures/headless";

type Seen = {
  failed: string | null;
  blocks: number;
  headers: (string | null)[];
  dividers: (string | null)[];
  rules: number[];
  inWork: number;
  answers: (string | null)[];
  bubbles: (string | null)[];
  order: string[];
  interrupted: boolean;
  page: string | null;
};

const see = async (t: Parameters<typeof renderFixturePage>[0], query: string) =>
  JSON.parse(await renderFixturePage(t, `compaction-divider.html?${query}`, "compaction-divider-result", 15000)) as Seen;

const REFRESHED = "Context refreshed · everything important kept";

test("a compaction mid-turn is a divider in the turn's place, outside its folded work, under one header", { timeout: 120000 }, async (t) => {
  const seen = await see(t, "case=midturn");
  assert.equal(seen.failed, null, `the thread failed to render: ${seen.failed}`);
  assert.equal(seen.blocks, 1, seen.page ?? "");
  assert.deepEqual(seen.headers, ["Worked for 45s"]);
  assert.deepEqual(seen.order, ["user", "divider", "answer"]);
  assert.deepEqual(seen.dividers, [REFRESHED]);
  assert.deepEqual(seen.rules, [2]);
  assert.equal(seen.inWork, 0);
  assert.deepEqual(seen.answers, ["The release notes are tidy."]);
  assert.equal(seen.interrupted, false);
});
