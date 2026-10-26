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

test("Brigadier's /compact between two answers is one divider: no turn, no bubble, no time of its own", { timeout: 120000 }, async (t) => {
  const seen = await see(t, "case=between");
  assert.equal(seen.failed, null, `the thread failed to render: ${seen.failed}`);
  assert.equal(seen.blocks, 2, seen.page ?? "");
  assert.deepEqual(seen.dividers, [REFRESHED]);
  assert.deepEqual(seen.rules, [2]);
  assert.equal(seen.inWork, 0);
  assert.deepEqual(seen.answers, ["Half done: the revoke worked.", "Now it is saved."]);
  assert.deepEqual(
    seen.bubbles.map((bubble) => ["Is the token rotated?", "Here is the terminal."].find((said) => bubble?.startsWith(said)) ?? bubble),
    ["Is the token rotated?", "Here is the terminal."],
  );
  // The answer before it worked 4 s, not until the compaction ended over a minute later.
  assert.deepEqual(seen.order, ["user", "answer", "divider", "user", "answer"]);
  // Each answer's own time: the first stops at its end, not the compaction's a minute later.
  assert.deepEqual(seen.headers, ["Worked for 5s", "Worked for 5s"]);
  assert.equal(seen.interrupted, false);
});

test("Brigadier's /compact as the thread's last turn, read as cut off, is still only its divider: no Interrupted card", { timeout: 120000 }, async (t) => {
  const seen = await see(t, "case=between&last=1");
  assert.equal(seen.failed, null, `the thread failed to render: ${seen.failed}`);
  assert.equal(seen.blocks, 1, seen.page ?? "");
  assert.deepEqual(seen.dividers, [REFRESHED]);
  assert.deepEqual(seen.order, ["user", "answer", "divider"]);
  assert.deepEqual(seen.headers, ["Worked for 5s"]);
  assert.equal(seen.interrupted, false, seen.page ?? "");
});

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
