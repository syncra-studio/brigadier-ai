import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive, renderFixturePage } from "@/fixtures/headless";

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

test("Brigadier's compaction as the app hears it moves nothing at the thread's bottom: no /compact bubble or block shows for a frame and goes", { timeout: 120000 }, async (t) => {
  const seen = JSON.parse(await renderFixtureLive(t, "compaction-divider.html?case=live", "compaction-divider-result", 90000)) as {
    failed: string | null;
    steps: { step: string; top: number; bottom: number; asked: boolean }[];
    dividers: (string | null)[];
  };
  assert.equal(seen.failed, null);
  const [before] = seen.steps;
  // At its bottom, following: the divider coming in moves what is above it up, once; nothing comes and goes.
  assert.ok(before && before.bottom <= 24, JSON.stringify(seen.steps));
  for (const step of seen.steps) {
    assert.equal(step.asked, false, `a /compact bubble at "${step.step}": ${JSON.stringify(seen.steps)}`);
  }
  seen.steps.slice(1).forEach((step, index) => assert.ok(step.top >= seen.steps[index]!.top, `the view went back up at "${step.step}": ${JSON.stringify(seen.steps)}`));
  assert.deepEqual(seen.dividers, [REFRESHED]);
});
