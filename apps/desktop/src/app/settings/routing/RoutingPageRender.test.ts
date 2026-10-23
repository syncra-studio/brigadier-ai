import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The real Routing page in Chromium over a faked daemon and rules file (S32): who does what from
// the file's rows over the shipped ones, a kind made My order and edited (effort, a second model,
// moved up, removed), the rules with the add form (its model, its kind of work, its preview and
// the kind a clash opens), a hand edit of the file shown on reopening, and under More the models'
// strength and how a model is picked.
test("Routing lists the rules file, writes its edits back, and shows a hand edit on reopening", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing.html?drive=1", "routing-result", 40000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(
    seen.note,
    "RoutingWhich agent, model and effort suit each kind of work. The thread follows these when it starts workers; change only what you want to.Applies to new work only",
  );
  assert.deepEqual((seen.sections as string[]).slice(-3), ["Who does what", "Rules", "More"]);
  assert.equal(seen.rulesDescription, "Keep a model away from all work or one kind of work, allow only some models, or cap the effort");
  // Who does what: the shipped rows (Automatic), the user's order replacing its shipped one, then the user's own kind.
  const kinds = [
    "Architecture and broad changesAutomatic · Strong default for work that needs judgment across a codebase.Opus 5.5 · High",
    "Bounded implementationYour order · Sonnet 5.5Sonnet 5.5 · Medium",
    "Mechanical changes and searchAutomatic · A smaller model fits a bounded task with clear checks.Sonnet 5.5 · Medium",
    "Independent reviewAutomatic · A second provider offers a useful independent reading.Default model · High",
    "Release notesYour order · GPT-6GPT-6 · Low",
  ];
  assert.deepEqual(seen.kinds, kinds);
  // Each rule names its model, then where it applies and what it does.
  const rules = [
    "Never give Fable workEverywhere · Chats, sessions and workers can't run on it",
    "Never go above High effortEverywhere · Chats, sessions and workers run at this effort or lower",
  ];
  assert.deepEqual(seen.rules, rules);
  assert.equal(seen.openAtStart, 0);
  assert.equal(seen.readsAtOpen, 1);
  // A shipped kind opened in place: Automatic, the shipped model picked now; My order copies it into the file.
  assert.equal(seen.mode, "Automatic");
  assert.equal(seen.whoPicks, "Who picksAutomatic advice, updated with each releaseAutomaticMy order");
  assert.equal(seen.shipped, "1Opus 5.5Picked nowHigh effortStrong default for work that needs judgment across a codebase.");
  const architecture = {
    work: "Architecture and broad changes",
    agent: "claude",
    model: "opus",
    effort: "high",
    reason: "Strong default for work that needs judgment across a codebase.",
  };
  assert.deepEqual((seen.afterMine as unknown[]).at(-1), architecture);
  assert.deepEqual(seen.mine, ["1Opus 5.5High effort"]);
  // Alone, its grip doesn't drag.
  assert.equal(seen.aloneGripDisabled, true);
  // Its effort menu stops at the ceiling the rules set; a pick writes the row.
  assert.deepEqual(seen.efforts, ["Defaultthe CLI picks", "Low effort", "Medium effort", "High effort"]);
  assert.deepEqual(seen.afterEffort, { ...architecture, effort: "medium" });
  // Add model offers each listed model, the newest of each family and each agent's default.
  assert.deepEqual(seen.addModelItems, [
    "Opus 5.5opus",
    "Sonnet 5.5sonnet",
    "Fable 5.1claude-fable-5-1",
    "Newest Opus familyfollows releases",
    "Newest Sonnet familyfollows releases",
    "Claude's defaultthe CLI picks",
    "GPT-6gpt-6",
    "GPT-6 Lunagpt-6-luna",
    "GPT-5.5gpt-5.5",
    "Codex's defaultthe CLI picks",
  ]);
  assert.deepEqual(seen.order, ["1Opus 5.5Medium effort", "2GPT-6Default effort"]);
  assert.equal(seen.kindLine, "Architecture and broad changesYour order · Opus 5.5, then GPT-6Opus 5.5 · Medium");
  // Moved up from its grip, the order is written first to last.
  assert.deepEqual(seen.afterMove, ["gpt-6", "opus"]);
  // Its last model removed, the kind is Automatic again.
  assert.deepEqual(seen.afterRemove, ["Bounded implementation", "Release notes"]);
  assert.equal(seen.modeAfterRemove, "Automatic");
  // The add form opens on top: a model, then the work it is for; nothing to add until a model is chosen.
  assert.equal(seen.form, "NeveruseChoose a modelforAny workCancelAdd rule");
  assert.equal(seen.addDisabled, true);
  assert.equal(seen.headerAddDisabled, true);
  // Its preview warns of a kind of work the rule would stop, says where to fix it, and opens it.
  assert.equal(
    seen.clash,
    "“Architecture and broad changes” uses Opus 5.5, so starting it would be refused. Pick another model for it in Who does what, or make this rule for one kind of work.Open Architecture and broad changes",
  );
  assert.equal(seen.openAfterClash, 0);
  assert.equal(seen.openedByNote, "Architecture and broad changes");
  assert.equal(
    seen.preview,
    "“Release notes” uses GPT-6, so starting it would be refused. Pick another model for it in Who does what, or make this rule for one kind of work.Open Release notes",
  );
  // For one kind of work only, it stops nothing else.
  assert.equal(
    seen.scoped,
    "GPT-6 won't be used for Independent review: a worker for Independent review that asks for it is refused, quoting this rule.",
  );
  // Added: the file gets the rule for that work, the form closes, the new rule comes first and marked, and Undo takes it back.
  assert.equal(seen.formAfterAdd, false);
  assert.deepEqual(seen.writtenRules, [
    { type: "forbid", names: ["fable"] },
    { type: "effortCeiling", effort: "high" },
    { type: "forbid", names: ["gpt-6"], work: "Independent review" },
  ]);
  assert.deepEqual(seen.rulesAfterAdd, ["Don't use GPT-6 for Independent reviewNewIndependent review only · Its workers can't run on it", ...rules]);
  assert.deepEqual(seen.toast, ["Rule added", "Undo"]);
  assert.deepEqual(seen.rulesAfterUndo, rules);
  // The file edited by hand shows when the page opens again: an order of two, a rule for one work
  // that keeps a kind's only model out, which its row and Automatic say.
  assert.deepEqual(seen.kindsAfterHandEdit, [
    "Architecture and broad changesAutomatic · Your rules keep Opus 5.5 out, so it can't start",
    ...kinds.slice(1),
    "DocsYour order · Sonnet 5.5, then GPT-6GPT-6",
  ]);
  assert.deepEqual(seen.rulesAfterHandEdit, [
    ...rules,
    "Only use GPT-6 on CodexEverywhere · Codex chats, sessions and workers run on nothing else",
    "Don't use Opus 5.5 for Architecture and broad changesArchitecture and broad changes only · Its workers can't run on it",
    "Don't use Sonnet 5.5 for DocsDocs only · Its workers can't run on it",
  ]);
  assert.equal(
    seen.skipped,
    "Skipped: Opus 5.5 is kept out by your rules. Choose My order to pick another model.Strong default for work that needs judgment across a codebase.",
  );
  // How a model is picked, step by step; Model strength stays closed when search shows the steps.
  assert.deepEqual(seen.steps, [
    "1Your choiceIf you or the thread name a model or effort for the worker, that is used.",
    "2Your orderIf not, the worker takes the first model in your order for its kind of work that your rules allow.",
    "3AutomaticWith no order of your own, it takes the first model of the automatic advice that your rules allow.",
    "4Your rulesLast, your rules check the result: a model they keep out, or an effort above your cap, is refused and the worker doesn't start. Your rules win over everything above.",
  ]);
  assert.equal(seen.stepsExample, "For example, Docs: your rules keep Sonnet 5.5 out, so GPT-6, next in your order, runs.");  assert.equal(seen.strengthClosed, true);
  assert.deepEqual(seen.strength, [
    "1Opus 5.5Frontier9.1",
    "2GPT-6Frontier9.0",
    "3Sonnet 5.5Strong8.0",
    "4GPT-6 LunaStrong8.0",
    "5GPT-5.5Strong7.9",
    "6Opus 4.6Standard7.2",
  ]);
  // Refresh models reads again what `brigadier models` shows: each agent's models, every
  // account's usage, the ratings, and the shipped advice beside the rules file.
  assert.deepEqual(seen.refreshAsked, ["checkRegistry", "getModelRules", "getUsage", "refreshAccounts", "refreshProviders"]);
  // Every searchable row can be brought into view.
  assert.deepEqual(seen.unrevealable, []);
});

// Drawn with every Settings page's parts (#385): its cards are Settings › General's (fill,
// corners, edge), its selects General's select and its chosen pill General's.
test("Routing draws its cards, selects and choices as Settings › General does", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing.html?drive=look", "routing-result", 30000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<
    "general" | "routing",
    { cards: unknown[]; select: unknown; chosen: unknown }
  >;
  assert.ok(seen.general.cards[0] && seen.general.select && seen.general.chosen, "General has a card, a select and a choice");
  assert.equal(seen.routing.cards.length, 3);
  for (const card of seen.routing.cards) assert.deepEqual(card, seen.general.cards[0]);
  assert.deepEqual(seen.routing.select, seen.general.select);
  assert.deepEqual(seen.routing.chosen, seen.general.chosen);
});
