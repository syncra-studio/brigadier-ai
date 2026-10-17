import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The real Routing page in Chromium over a faked daemon: who does what, a kind opened in place
// with its order, the rules, and under Advanced the ranking right now (asked about the change's
// author), the area overrides and the models' ratings with a profile; editing the order and
// switching it to Automatic and back.
test("Routing shows each kind's order, ranking, rules and the models' ratings, and edits the order", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing.html?drive=1", "routing-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  // How a model is picked, the ranking, overrides and ratings wait under a closed Advanced, which
  // opens when Settings search brings one of them into view.
  assert.equal(seen.advancedClosed, true);
  assert.equal(seen.advancedRevealed, true);
  assert.deepEqual(seen.steps, [
    "1. Skip what can't run",
    "2. Follow your rules",
    "3. Keep important work on strong models",
    "4. Use your order",
    "5. Otherwise, the best score",
  ]);
  // Who does what: a row per kind of work saying what it runs on, none open; one opens in place
  // to Automatic or My order, its models as rows, and what it runs on right now.
  assert.deepEqual(seen.kinds, [
    "Explore the codeAutomatic · GPT-6 now",
    "ResearchAutomatic · Opus 5.5 now",
    "Write codeYour order · Opus 5.5, then GPT-6",
    "Review changesYour order · Opus 5.5, then GPT-6",
    "Resolve conflictsAutomatic · Opus 5.5 now",
    "Run checksAutomatic · Opus 5.5 now",
    "Use appsYour order · GPT-6, then Opus 5.5 · 1 area override",
    "Take over a chatThe chat's own model, then Automatic",
    "Coordinate workAutomatic · Opus 5.5 now",
    "Write commit messagesAutomatic · Opus 5.5 now",
  ]);
  assert.equal(seen.openAtStart, 0);
  assert.equal(seen.heading, "Review changes");
  assert.equal(
    seen.order,
    "AutomaticMy order⠿1Opus 5.5high effort⠿2GPT-6high effortAdd modelIf neither can runpick automaticallyRight now: Claude's changes → GPT-6 · high, Codex's → Opus 5.5 · high. Why",
  );
  // Each part of the score in its column; the pick's reason under it; the four first of those
  // that can't take it, then how many more.
  assert.deepEqual(seen.ranked, [
    "1GPT-6Frontier9.00+0.400−0.259.15highPickedTop eligible score. Claude wrote this change, so Codex reviews it.",
    "2Opus 5.5Frontier9.10+0.60−3.8005.90high",
  ]);
  assert.equal(seen.blocked, 4);
  assert.equal(seen.more, "1 more that can't take it");
  // Every rule, page-wide, as a plain sentence with where it applies or what it leaves.
  assert.deepEqual(seen.rules, [
    "Don't give Claude Sonnet 5.5 work",
    "Don't give Codex GPT-6 Luna work",
    "Never use Claude Sonnet 5.5 for review",
  ]);
  assert.deepEqual(seen.ruleRows, [
    "Don't give Sonnet 5.5 workYou can still pick it for a chat or session",
    "Don't give GPT-6 Luna workA new model: it gets no tasks until you remove this rule",
    "Never use Sonnet 5.5 for reviewing changesEverywhere",
  ]);
  // Every kind's area overrides are listed together under Advanced.
  assert.deepEqual(seen.overrides, [
    "In All projects, Use apps touchingfrontendtriesGPT-6, thenOpus 5.5, andwaitsif neither can run.",
  ]);
  assert.equal(seen.rankingFor, "Review changes");
  assert.deepEqual(seen.unrevealable, []);
  // A review is ranked as if Claude wrote the change, then as Codex did once asked.
  assert.deepEqual(seen.authors, ["claude"]);
  assert.equal(seen.askedCodex, true);
  assert.equal(seen.reasonAfter, "Top eligible score.");
  // × removes a model; the order may wait; Automatic keeps the order, My order brings it back.
  assert.deepEqual(seen.afterRemove, ["opus"]);
  assert.equal(seen.orderAfterRemove, "⠿1Opus 5.5high effort");
  assert.equal(seen.waits, true);
  assert.deepEqual(seen.afterReset, { manual: false, kept: 1 });
  // Automatic reviews are adversarial: each vendor's change goes to the other, both picks shown.
  assert.equal(
    seen.automatic,
    "AutomaticMy orderPicks the strongest model that can run now.Add modelRight now: the other vendor from the author reviews — Claude's changes → GPT-6 · high, Codex's → Opus 5.5 · high. Why",
  );
  assert.equal(seen.reviewRow, "Review changesAutomatic · adversarial: Claude's changes → GPT-6, Codex's → Opus 5.5");
  assert.deepEqual(seen.afterMyOrder, { manual: true, kept: 1 });
  // A project without an order of its own follows All projects' (My order); Automatic gives it
  // an Automatic of its own.
  assert.equal(seen.projectMode, "My order");
  assert.match(seen.projectOrder as string, /Follows All projects: Opus 5\.5\./);
  assert.deepEqual(seen.projectAutomatic, { manual: false, entries: 0 });
  assert.equal(seen.projectModeAfter, "Automatic");
  // Ratings: per provider, then those not in the registry; Fable left out; the best unfaded score
  // of each kind marked; a row opens its profile with what it learned.
  assert.deepEqual(seen.ratingGroups, ["Claude", "Codex", "Not in the registry"]);
  assert.deepEqual(seen.ratingRows, ["Opus 5.5", "Sonnet 5.5", "Opus 4.6", "GPT-6", "GPT-5.5", "GPT-6 Luna"]);
  assert.deepEqual(seen.best, ["9.2", "9.1", "9.0", "8.4", "9.0", "8.2", "7.6", "8.6", "6.8", "8.9", "6.8", "7.6", "6.8"]);
  assert.deepEqual(seen.profile, ["Profile", "Strength by kind", "Area adjustments", "In all projects"]);
  assert.equal(seen.learned, "+0.40");
  assert.equal(seen.form, true);
  assert.equal(seen.formAfterCancel, false);
  // The page's column is the one every Settings page has (#259).
  assert.equal(seen.settingsColumn, true);
  // Nothing over the page's top takes a click meant for what scrolled under it (#260).
  assert.equal(seen.addRuleNearTop, true);
  assert.equal(seen.addRuleHit, true);
  // A model switched off, or its agent, is out of the ratings; switched back on, it returns (#261).
  assert.deepEqual(seen.ratingRowsModelOff, ["Opus 5.5", "Sonnet 5.5", "GPT-6", "GPT-5.5", "GPT-6 Luna"]);
  assert.deepEqual(seen.ratingRowsAgentOff, ["Opus 5.5", "Sonnet 5.5"]);
  assert.ok((seen.bestAgentOff as number) > 0);
  assert.deepEqual(seen.ratingRowsBack, ["Opus 5.5", "Sonnet 5.5", "Opus 4.6", "GPT-6", "GPT-5.5", "GPT-6 Luna"]);
});
