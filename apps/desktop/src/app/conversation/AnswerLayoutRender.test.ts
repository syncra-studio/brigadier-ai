import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

type Fold = { slot: string | null; title: string | null; open: boolean; chevronFirst: boolean; chevronShown: boolean };
type Seen = {
  layout: boolean;
  bottomLine: string | null;
  steps: (string | null)[];
  picks: { badge: string | null; row: string | null; tinted: string }[];
  avoid: number;
  avoidTitle: string | null;
  chance: string | null;
  folds: Fold[];
  askNext: (string | null)[];
  stars: boolean;
  markdown: string[];
  citations: number;
  spoken: string | null;
  sent: string[];
};

const see = async (t: Parameters<typeof renderFixtureLive>[0], query: string) =>
  JSON.parse(await renderFixtureLive(t, `answer-layout.html?drive=1&${query}`, "answer-layout-result")) as Seen;

const sourcesFold = (count: number): Fold => ({ slot: "answer-sources", title: `Sources · ${count}`, open: false, chevronFirst: true, chevronShown: true });

// Q86: the trading answer with every part: the bottom line as a card, steps, the pick's badge,
// the avoid card, the odds in their fold, the folds in one list with Sources last, Ask next chips.
test("the trading answer in a Chat shows every part, reads aloud what's in view, and a chip sends its question", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=chat&a=trading&open=Why");
  assert.equal(seen.layout, true);
  assert.match(seen.bottomLine ?? "", /^Don't trade for quick money/);
  assert.equal(seen.steps.length, 5);
  assert.match(seen.steps[0] ?? "", /^1 ?Practise first\. ?Open the free Trading 212/);
  assert.equal(seen.picks.length, 1);
  assert.equal(seen.picks[0]!.badge, "My pick for you");
  assert.match(seen.picks[0]!.row ?? "", /^Trading 212 Invest ?My pick for you$/);
  // The pick's row is tinted: its background isn't transparent.
  assert.doesNotMatch(seen.picks[0]!.tinted, /, 0\)$|transparent/);
  assert.equal(seen.stars, false);
  assert.equal(seen.avoidTitle, "Avoid these");
  assert.equal(seen.avoid, 4);
  assert.match(seen.chance ?? "", /^80 out of 100 ?small CFD accounts lose money$/);
  assert.deepEqual(
    seen.folds.map((fold) => [fold.title, fold.open]),
    [
      ["Why not fast income", true],
      ["Taxes in Moldova", false],
      ["How to spot a scam", false],
      ["Who can help, for free", false],
      ["What I couldn't check", false],
      ["Sources · 12", false],
    ],
  );
  assert.ok(seen.folds.every((fold) => fold.chevronFirst && fold.chevronShown));
  assert.deepEqual(seen.askNext, ["Walk me through the practice account", "How much should I put in each month?", "What is an index fund?"]);
  assert.match(seen.spoken ?? "", /^Don't trade for quick money/);
  assert.ok(seen.spoken?.endsWith("More details are folded below."), seen.spoken ?? "");
  assert.ok(!seen.spoken?.includes("Taxes"));
  assert.deepEqual(seen.sent, ["Walk me through the practice account"]);
});

test("a how-to in a Chat: its bottom line card and steps, nothing else, Sources on the same fold row", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=chat&a=howto");
  assert.equal(seen.layout, true);
  assert.match(seen.bottomLine ?? "", /^It takes about 5 minutes/);
  assert.equal(seen.steps.length, 4);
  assert.deepEqual(seen.picks, []);
  assert.deepEqual(seen.folds, [sourcesFold(1)]);
  assert.ok(!seen.spoken?.includes("folded"));
});

// No markers: today's look, exactly. The same Markdown, its Sources on the fold row.
for (const [name, query, markdown, folds] of [
  ["a factual answer in a Chat", "kind=chat&a=fact", ["p", "p"], [sourcesFold(1)]],
  ["a free-form answer in a Chat", "kind=chat&a=free", ["p", "ul", "div", "h3", "p"], []],
  ["a how-to's steps in a session", "kind=session&a=howto", ["p", "ol", "p"], [sourcesFold(1)]],
  ["a coding reply in a session that changed no code", "kind=session&a=coding", ["p", "ol", "div", "hr"], []],
] as const) {
  test(`${name} renders as today: no parts`, { timeout: 90000 }, async (t) => {
    const seen = await see(t, query);
    assert.equal(seen.layout, false);
    assert.equal(seen.bottomLine, null);
    assert.deepEqual(seen.steps, []);
    assert.deepEqual(seen.markdown, markdown);
    assert.deepEqual(seen.folds, folds);
    assert.deepEqual(seen.askNext, []);
  });
}

test("a session's coding turn keeps its Details and today's look, the trading answer's markers too", { timeout: 90000 }, async (t) => {
  const coding = await see(t, "kind=session&a=coding&changed=1");
  assert.equal(coding.layout, false);
  assert.deepEqual(coding.markdown, ["p", "ol", "div", "hr"]);
  assert.deepEqual(coding.folds.map((fold) => fold.title), ["Details"]);
  // Even with every marker, a turn that changed code keeps today's look.
  const trading = await see(t, "kind=session&a=trading&changed=1");
  assert.equal(trading.layout, false);
  assert.equal(trading.stars, true);
});

test("the trading answer in a session shows its parts: it has explicit markers", { timeout: 90000 }, async (t) => {
  const seen = await see(t, "kind=session&a=trading");
  assert.equal(seen.layout, true);
  assert.equal(seen.steps.length, 5);
  assert.equal(seen.picks.length, 1);
});
