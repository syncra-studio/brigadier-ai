import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

// The real Routing page in Chromium over a faked daemon and rules file (S32): who does what from
// the file's rows over the shipped ones, a kind made yours and edited, the rules with the add
// form and its preview, a hand edit of the file shown on reopening, and under More the models'
// strength and how a model is picked.
test("Routing lists the rules file, writes its edits back, and shows a hand edit on reopening", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing.html?drive=1", "routing-result", 40000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(
    seen.note,
    "RoutingWhich agent, model and effort suit each kind of work. The thread follows these when it starts workers; change only what you want to.Applies to new work only",
  );
  assert.deepEqual((seen.sections as string[]).slice(-3), ["Who does what", "Rules", "More"]);
  // Who does what: the shipped rows, the user's row replacing its shipped one, then the user's own kind.
  const kinds = [
    "Architecture and broad changesShipped · Strong default for work that needs judgment across a codebase.Opus 5.5 · High",
    "Bounded implementationYours · My preference.Sonnet 5.5 · Medium",
    "Mechanical changes and searchShipped · A smaller model fits a bounded task with clear checks.Sonnet 5.5 · Medium",
    "Independent reviewShipped · A second provider offers a useful independent reading.Default model · High",
    "Release notesYours · Short and cheap.GPT-6 · Low",
  ];
  assert.deepEqual(seen.kinds, kinds);
  const rules = [
    "Never use FableEverywhere · Chats and workers can't use it",
    "Never go above High effortEverywhere · Chats and workers stay at or below it",
  ];
  assert.deepEqual(seen.rules, rules);
  assert.equal(seen.openAtStart, 0);
  assert.equal(seen.readsAtOpen, 1);
  // A shipped kind opened in place, then made yours: the shipped row is copied into the file.
  assert.equal(seen.mode, "Shipped");
  assert.equal(seen.whoPicks, "Who picksThe shipped row, updated with each releaseShippedMine");
  assert.equal(seen.shipped, "Opus 5.5High effortStrong default for work that needs judgment across a codebase.");
  const architecture = {
    work: "Architecture and broad changes",
    agent: "claude",
    model: "opus",
    effort: "high",
    reason: "Strong default for work that needs judgment across a codebase.",
  };
  assert.deepEqual((seen.afterMine as unknown[]).at(-1), architecture);
  assert.deepEqual(seen.mine, ["Opus 5.5High effort"]);
  // Its effort menu stops at the ceiling the rules set; a pick writes the row.
  assert.deepEqual(seen.efforts, ["Defaultthe CLI picks", "Low", "Medium", "High"]);
  assert.deepEqual(seen.afterEffort, { ...architecture, effort: "medium" });
  assert.equal(seen.kindLine, "Architecture and broad changesYours · Strong default for work that needs judgment across a codebase.Opus 5.5 · Medium");
  // Remove gives the kind back to the shipped row.
  assert.deepEqual(seen.afterRemove, ["Bounded implementation", "Release notes"]);
  assert.equal(seen.modeAfterRemove, "Shipped");
  // The add form opens on top; nothing to add until a model is chosen.
  assert.equal(seen.form, "NeveruseChoose a modelCancelAdd rule");
  assert.equal(seen.addDisabled, true);
  assert.equal(seen.headerAddDisabled, true);
  // Its preview warns of a row the rule would refuse.
  assert.equal(seen.clash, "“Architecture and broad changes” uses Opus 5.5, so starting it would be refused. Set your own model there too.");
  assert.equal(seen.preview, "“Release notes” uses GPT-6, so starting it would be refused. Set your own model there too.");
  // Added: the file gets the rule, the form closes, the new rule comes first and marked, and Undo takes it back.
  assert.equal(seen.formAfterAdd, false);
  assert.deepEqual(seen.writtenRules, [
    { type: "forbid", names: ["fable"] },
    { type: "effortCeiling", effort: "high" },
    { type: "forbid", names: ["gpt-6"] },
  ]);
  assert.deepEqual(seen.rulesAfterAdd, ["Never use GPT-6NewEverywhere · Chats and workers can't use it", ...rules]);
  assert.deepEqual(seen.toast, ["Rule added", "Undo"]);
  assert.deepEqual(seen.rulesAfterUndo, rules);
  // The file edited by hand shows when the page opens again.
  assert.deepEqual(seen.kindsAfterHandEdit, [...kinds, "DocsYours · Hand edit.Sonnet 5.5 · Low"]);
  assert.deepEqual(seen.rulesAfterHandEdit, [...rules, "Only use GPT-6 on CodexCodex only · Chats and workers use nothing else"]);
  // How a model is picked, step by step; Model strength stays closed when search shows the steps.
  assert.deepEqual(seen.steps, [
    "1Your choiceAn agent, model or effort the thread names for a worker is used as given",
    "2Your rowOtherwise the row you set for its kind of work picks the rest",
    "3Shipped rowOtherwise the shipped row for that kind of work",
    "4Your rulesLast, a choice your rules forbid is refused, quoting the rule",
  ]);
  assert.equal(seen.strengthClosed, true);
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
