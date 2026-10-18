import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The new Routing page (not shown yet) in Chromium over a faked daemon: who does what, a kind opened in place
// with its order or its automatic picks, the rules with the add form and its preview, and under
// More the models' strength and how a model is picked.
test("Routing shows who does what, edits an order, adds a rule with Undo, and ranks the models", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing-home.html?drive=1", "routing-result", 40000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(
    seen.note,
    "RoutingWhich model takes each kind of work. Brigadier picks automatically; change only what you want to.Applies to new work only",
  );
  assert.deepEqual((seen.sections as string[]).slice(-3), ["Who does what", "Rules", "More"]);
  // Who does what: a row per kind saying how it's picked and what runs it now, none open.
  assert.deepEqual(seen.kinds, [
    "Explore the codeAutomaticNowGPT-6",
    "ResearchAutomaticNowOpus 5.5",
    "Write codeYour order · Opus 5.5, then GPT-6NowOpus 5.5",
    "Review changesYour order · Opus 5.5, then GPT-6Varies",
    "Resolve conflictsAutomaticNowOpus 5.5",
    "Run checksAutomaticNowOpus 5.5",
    "Use appsYour order · GPT-6, then Opus 5.5NowGPT-6",
    "Take over a chatThe chat's own model, then AutomaticVaries",
    "Coordinate workAutomaticNowOpus 5.5",
    "Write commit messagesAutomaticNowOpus 5.5",
  ]);
  assert.equal(seen.openAtStart, 0);
  // A kind opened in place: My order, its models with their effort; removing one, then switching
  // to Automatic keeps the order for later.
  assert.equal(seen.mode, "My order");
  assert.equal(seen.whoPicks, "Who picksBrigadier, or the order you setAutomaticMy order");
  assert.deepEqual(seen.order, ["1Opus 5.5High effort", "2GPT-6High effort"]);
  assert.deepEqual(seen.afterRemove, ["opus"]);
  assert.deepEqual(seen.afterReset, { manual: false, kept: 1 });
  // A review goes to the other vendor from the change's author, so its pick varies.
  assert.equal(seen.reviewRow, "Review changesAutomaticVaries");
  // Automatic shows the models it would try, whose changes each reviews, and what it skips and why.
  assert.deepEqual(seen.picks, ["1GPT-6Reviews Claude's changes9.2", "2Opus 5.5Reviews Codex's changes5.9"]);
  assert.equal(
    seen.skipped,
    "Skipped: GPT-6 Luna, Sonnet 5.5, GPT-5.5, Opus 4.6 and gpt-5 are below the Frontier tier this work needs. See strengths",
  );
  // See strengths opens Model strength on that kind, each vendor's reviewer marked.
  assert.equal(seen.strengthFor, "Review changes");
  assert.deepEqual(seen.strength, [
    "1Opus 5.5Frontier9.1Reviews Codex's changes",
    "2GPT-6Frontier9.0Reviews Claude's changes",
    "3Sonnet 5.5Strong8.0",
    "4GPT-6 LunaStrong8.0",
    "5GPT-5.5Strong7.9",
    "6Opus 4.6Standard7.2",
  ]);
  assert.match(String(seen.ratingsFooter), /^Ratings updated Oct 13 · checked .+ agoRefresh ratings$/);
  assert.deepEqual(seen.afterMyOrder, { manual: true, kept: 1 });
  // Rules: each as a title and where it applies.
  const rules = [
    "Never give Sonnet 5.5 workEverywhere · You can still pick it yourself in a chat or session",
    "Never give GPT-6 Luna workEverywhere · New model, it gets nothing until you allow it",
    "Don't use Sonnet 5.5 to review changesReview changes only",
  ];
  assert.deepEqual(seen.rules, rules);
  // The add form opens on top; nothing to add until a model is chosen.
  assert.equal(seen.form, "NeveruseChoose a modelforAny workCancelAdd rule");
  assert.equal(seen.addDisabled, true);
  assert.equal(seen.headerAddDisabled, true);
  // Its preview warns of a rule that won't change anything, or says what will change.
  assert.equal(
    seen.clash,
    "“Never give Sonnet 5.5 work” already keeps Sonnet 5.5 away from everything. Never rules win, so this rule won't change anything until you remove that one.",
  );
  assert.equal(
    seen.preview,
    "Write commit messages will go to GPT-6 instead of Opus 5.5 whenever GPT-6 can run. Everything else stays the same.",
  );
  // Added: the form closes, the new rule comes first and marked, and Undo takes it back.
  assert.equal(seen.formAfterAdd, false);
  assert.deepEqual(seen.rulesAfterAdd, [
    "Prefer GPT-6 for Write commit messagesNewWrite commit messages · Goes first whenever it can run",
    ...rules,
  ]);
  assert.deepEqual(seen.toast, ["Rule added", "Undo"]);
  assert.deepEqual(seen.rulesAfterUndo, rules);
  // How a model is picked, step by step; Model strength stays closed when search shows the steps.
  assert.deepEqual(seen.steps, [
    "1Hard limitsSkips models that are signed out, at a limit, just failed or can't read images",
    "2Never rulesSkips models your rules keep away from this work",
    "3Tier floorSkips models below the tier this work needs, e.g. Frontier for code, review and merge",
    "4Your orderIf you set one, the first model that can run takes it. Skips the next two steps",
    "5Prefer rulesMoves models your rules prefer to the front",
    "6Orchestrator pinCoordinate work stays with the model that started it",
    "7Best scoreOtherwise the highest of strength + learned − quota − load",
  ]);
  assert.equal(seen.strengthClosed, true);
  // Every searchable row can be brought into view; the column matches every other page's; Add
  // rule at the top edge still takes a click.
  assert.deepEqual(seen.unrevealable, []);
  assert.equal(seen.settingsColumn, true);
  assert.equal(seen.addRuleNearTop, true);
  assert.equal(seen.addRuleHit, true);
  // A model switched off on Providers, then its whole agent, leave Model strength; back on, back.
  assert.deepEqual(seen.strengthModelOff, ["Opus 5.5", "GPT-6", "Sonnet 5.5", "GPT-6 Luna", "GPT-5.5"]);
  assert.deepEqual(seen.strengthAgentOff, ["Opus 5.5", "Sonnet 5.5"]);
  assert.deepEqual(seen.strengthBack, ["Opus 5.5", "GPT-6", "Sonnet 5.5", "GPT-6 Luna", "GPT-5.5", "Opus 4.6"]);
});

// A kind that waits with no order of its own still opens My order to add one; a ratings refresh
// that failed after it started says why.
test("Routing starts an empty order when nothing runs, and says why a refresh failed", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing-home.html?drive=1&edge=1", "routing-result", 40000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(seen.mode, "My order");
  assert.equal(seen.addModel, true);
  assert.equal(seen.failure, "The refresh failed, so the ratings didn't change: the research model timed out");
});
