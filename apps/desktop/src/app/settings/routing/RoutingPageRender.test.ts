import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

/** A model's line in the picker when it's already in the order. */
const IN_THE_ORDER = "In the order";

// The real Routing page in Chromium over a faked daemon and rules file (S32): who does what from
// the file's rows over the shipped ones, a kind made My order and edited (effort, a second model,
// moved up, removed), the rules added in the user's own words (examples, an error, Brigadier
// rewriting them, Cancel, what it will save, Add, Undo), a hand edit of the file shown on
// reopening (with the line under a rule that stops a kind, advice, ×), and under More the models'
// strength and how a model is picked.
test("Routing lists the rules file, writes its edits back, and shows a hand edit on reopening", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing.html?drive=1", "routing-result", 40000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(
    String(seen.note).replace(/Ratings updated Oct 21 · checked .*? ago/, ""),
    "RoutingWhich agent, model and effort suit each kind of work. Changes apply to new work only.Refresh models",
  );
  assert.deepEqual((seen.sections as string[]).slice(-3), ["Who does what", "Rules", "More"]);
  assert.equal(seen.sources, "Automatic uses the best-rated models, from ratings researched every day. Your own order is never changed: a better-rated model only shows up as a suggestion.");
  assert.equal(seen.rulesDescription, "Things every session must follow, in your own words.");
  // Who does what: the shipped rows (Automatic), the user's order replacing its shipped one, then the user's own kind.
  const kinds = [
    "Architecture and broad changesAutomatic · Strong default for work that needs judgment across a codebase.Opus 5.5 · High",
    "Bounded implementationYour order · Sonnet 5.5Sonnet 5.5 · Medium",
    "Mechanical changes and searchAutomatic · A smaller model fits a bounded task with clear checks.Sonnet 5.5 · Medium",
    "Adversarial reviewAutomatic · A model from the other agent reviews the work",
    "FusionAutomatic · Both consults answer at once, then their answers merge",
    "Computer useAutomatic · Seeing and using apps on screen takes strong judgment.Opus 5.5 · High",
    "FallbackAutomatic · The model a session runs on when it moves to the other agent",
    "Conversation titlesAutomatic · A small model for short text; use available quota, cost tier and suitability among shipped title choices.GPT-6 Luna · Low",
    "Rule readingAutomatic · Rewrites the rules you type into exact rules and advice.Sonnet 5.5 · Medium",
    "Release notesYour order · GPT-6GPT-6 · Low",
  ];
  assert.deepEqual(seen.kinds, kinds);
  // Each rule is one plain sentence.
  const rules = ["Never use Fable", "Never go above High effort"];
  assert.deepEqual(seen.rules, rules);
  assert.equal(seen.openAtStart, 0);
  assert.equal(seen.readsAtOpen, 1);
  // A shipped kind opened in place: Automatic, the shipped model picked now; My order copies it into the file.
  assert.equal(seen.mode, "Automatic");
  assert.equal(seen.whoPicks, "Who picksAutomatic: set by the app, always a current modelAutomaticMy order");
  assert.equal(seen.shipped, "1Opus 5.5 · HighPicked nowStrong default for work that needs judgment across a codebase.");
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
  // Add model opens the model picker: each agent, its own default, then its models (no family
  // rows, no ids); one already in the order is greyed with why.
  assert.deepEqual(seen.addModelAgents, ["Claude Code", "Codex"]);
  assert.deepEqual(seen.addModelItems, {
    claude: ["Claude's defaultClaude picks", `Opus 5.5${IN_THE_ORDER}`, "Sonnet 5.5", "Fable 5.1"],
    codex: ["Codex's defaultCodex picks", "GPT-6", "GPT-6 Luna", "GPT-5.5"],
  });
  assert.deepEqual(seen.order, ["1Opus 5.5Medium effort", "2GPT-6Default effort"]);
  assert.equal(seen.kindLine, "Architecture and broad changesYour order · Opus 5.5, then GPT-6Opus 5.5 · Medium");
  // Moved up from its grip, the order is written first to last.
  assert.deepEqual(seen.afterMove, ["gpt-6", "opus"]);
  // Change model opens the picker beside the ⋯ menu, on the row's agent with the search focused;
  // its keys stay in it (← to the agents, ↑ browses, typing searches every agent), Escape clears
  // the search first, and a pick replaces the row and closes the whole menu.
  assert.deepEqual(seen.changeModel, {
    searchFocused: true,
    codex: ["Codex's defaultCodex picks", `GPT-6${IN_THE_ORDER}`, "GPT-6 Luna", "GPT-5.5"],
    typed: "s",
    searched: ["Opus 5.5Claude Code · In the order", "Sonnet 5.5Claude Code", "Opus 4.6Claude Code"],
    afterEscape: { open: true, typed: "" },
    menuClosed: true,
    order: ["1Sonnet 5.5Default effort", "2Opus 5.5Medium effort"],
  });
  // Its last model removed, the kind is Automatic again.
  assert.deepEqual(seen.afterRemove, ["Bounded implementation", "Release notes"]);
  assert.equal(seen.modeAfterRemove, "Automatic");
  // Add rule opens one text box on top, with the hint and examples; nothing to add until words are in it.
  assert.equal(
    seen.form,
    "Brigadier rewrites your words into exact rules and shows them before saving.Never go above High effortNever use SonnetOnly use Codex for reviewsCancelAdd",
  );
  assert.equal(seen.addDisabled, true);
  assert.equal(seen.headerAddShown, false);
  assert.equal(seen.filled, "Never use Sonnet\nNever go above High effort");
  // Add has Brigadier rewrite the words: a spinner line, the box and its button held.
  assert.equal(seen.reading, "Rewriting your rules…");
  assert.deepEqual(seen.readingButton, ["Reading…", true, true]);
  assert.equal(seen.readsAsked, 1);
  assert.equal(seen.readError, "Brigadier couldn't make a rule of that: try other words.");
  assert.equal(seen.keptWords, "gibberish");
  assert.equal(seen.readingAgain, "Rewriting your rules…");
  assert.deepEqual(seen.afterCancel, {
    open: true,
    words: "never go over high effort and keep sonnet off reviews. prefer codex when claude is busy",
    note: "",
  });
  // What Brigadier will save, before it is saved.
  assert.equal(
    seen.read,
    "Brigadier will: Never go above High effortBrigadier will: Never use Sonnet for Adversarial reviewSaved as advice: Prefer Codex when Claude is busy",
  );
  assert.equal(seen.writesBeforeAdd, 7);
  // Added: the file gets the new rules after the old (one it had already, once), the box closes, Undo takes them back.
  assert.equal(seen.formAfterAdd, false);
  assert.deepEqual(seen.writtenRules, [
    { type: "forbid", names: ["fable"] },
    { type: "effortCeiling", effort: "high" },
    { type: "forbid", names: ["sonnet"], work: "Adversarial review" },
    { type: "advice", text: "Prefer Codex when Claude is busy" },
  ]);
  assert.deepEqual(seen.rulesAfterAdd, [...rules, "Never use Sonnet for Adversarial review", "Prefer Codex when Claude is busyadvice"]);
  assert.deepEqual(seen.toast, ["Rules added", "Undo"]);
  assert.deepEqual(seen.rulesAfterUndo, rules);
  // The file edited by hand shows when the page opens again: an order of two, a rule for one work
  // that keeps a kind's only model out, which its row and Automatic say.
  assert.deepEqual(seen.kindsAfterHandEdit, [
    "Architecture and broad changesAutomatic · Your rules keep Opus 5.5 out, so it can't start",
    ...kinds.slice(1).map((line) => line.startsWith("Conversation titles") ? line.replace("GPT-6 Luna · Low", "Haiku") : line),
    "DocsYour order · Sonnet 5.5, then GPT-6GPT-6",
  ]);
  // A rule that stops a kind of work from starting says so, in one line under it.
  const stopsArchitecture = "Architecture and broad changes can't start: this rule keeps out Opus 5.5.";
  const stopsReview = "Reviews Codex's work: it needs High, and this rule keeps Opus at Medium.";
  assert.deepEqual(seen.rulesAfterHandEdit, [
    ...rules,
    "Only use GPT-6 on Codex",
    `Never use Opus for Architecture and broad changes${stopsArchitecture}`,
    "Never use Sonnet for Docs",
    `Never run Opus above Medium${stopsReview}`,
    "Adversarial review never below High",
    "Prefer Codex when Claude is busyadvice",
  ]);
  assert.deepEqual(seen.warnings, [stopsArchitecture, stopsReview]);
  assert.deepEqual(seen.afterRemoveRule, ["forbid", "effortCeiling", "allow", "forbid", "forbid", "effortCeiling", "effortFloor"]);
  assert.equal(
    seen.skipped,
    "Skipped: Opus 5.5 is kept out by your rules. Choose My order to pick another model.Strong default for work that needs judgment across a codebase.",
  );
  // How a model is picked, step by step; Model strength stays closed when search shows the steps.
  assert.deepEqual(seen.steps, [
    "1Named for the workerIf you or the session name a model or effort when the worker starts, that is used.",
    "2Your orderIf not, the worker takes the first model in your order for its kind of work that your rules allow.",
    "3AutomaticWith no order of your own, it takes the first model of the automatic advice that your rules allow.",
    "4Your rulesLast, your rules check the result: an effort above a cap or below a minimum runs at that limit instead, and a model they keep out is refused, so the worker doesn't start. Your rules win over everything above.",
  ]);
  assert.equal(seen.stepsExample, "For example, Conversation titles: your rules keep GPT-6 Luna out, so Haiku, next in the automatic advice, runs.");
  assert.equal(seen.strengthClosed, true);
  // The daily ratings research: when it last ran and on what model.
  assert.equal(seen.researchRow, "Daily ratings researchLast ran 3 hours ago on Sonnet 5.5 · 1 model re-rated");
  // A better-rated model for an order of yours is a line on its row; showing it writes nothing.
  assert.equal(seen.suggestion, "GPT-6 now rates higher for Bounded implementation · 9.0 against Sonnet 5.5's 8.5Use itDismiss");
  assert.equal(seen.suggestionKinds, 1);
  // Dismissed: hidden at this rating, and your order is as it was.
  assert.deepEqual(seen.dismissed, [{ work: "Bounded implementation", agent: "codex", model: "gpt-6", score: 9 }]);
  assert.equal(seen.suggestionAfterDismiss, 0);
  assert.equal(seen.writesAfterDismiss, seen.writesBeforeSuggestion);
  // Its rating changed again: back. Use it puts it first in your order, the one write.
  assert.equal(seen.suggestionBack, "GPT-6 now rates higher for Bounded implementation · 9.5 against Sonnet 5.5's 8.5Use itDismiss");
  assert.equal(seen.writesAfterUse, (seen.writesBeforeSuggestion as number) + 1);
  assert.deepEqual(seen.orderAfterUse, ["codex/gpt-6/high", "claude/sonnet/medium"]);
  assert.equal(seen.suggestionAfterUse, 0);
  assert.deepEqual(seen.strength, [
    "1Opus 5.5Frontier9.1",
    "2GPT-6Frontier9.0",
    "3Sonnet 5.5Strong8.0",
    "4GPT-6 LunaStrong8.0",
    "5GPT-5.5Strong7.9",
    "6Opus 4.6Standard7.2",
    // Unrated: last, with no score (#549).
    "7Claude NewUnrated",
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

// Each role keeps its work and agent when edited, and Automatic removes just that kind's overrides.
test("review, Fusion and Fallback edit their own roles through My order", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing.html?drive=roles", "routing-result", 40000);
  type Row = { work: string; agent: string; model: string; effort: string | null };
  type Role = { work: string; automatic: string[]; reset: string[]; initial: Row[]; afterEffort: Row[]; afterFirstModel: Row[]; afterSecondEffort: Row[]; afterBothModels: Row[]; afterReset: Row[]; agents: string[]; canReorder: boolean; closedHasModel: boolean; noFallbackBadges: boolean };
  const seen = JSON.parse(dump) as { roles: Role[]; noEffort: string[]; fallbackOrder: string[]; refreshOpensStrength: boolean; refreshInHeader: boolean };
  assert.deepEqual(seen.roles.map((role) => role.work), ["Adversarial review", "Fusion", "Fallback"]);
  const expected = [
    ["Reviews Claude's workCodex's default · High", "Reviews Codex's workOpus 5.5 · High"],
    ["Claude consultOpus 5.5 · High", "Codex consultCodex's default · High"],
    ["Moving to Claude CodeOpus 5.5 · High", "Moving to CodexCodex's default · High"],
  ];
  for (const [i, role] of seen.roles.entries()) {
    const works = role.work === "Fusion" ? ["fusion-claude", "fusion-codex"] : [role.work];
    const own = (rows: Row[]) => rows.filter((row) => works.includes(row.work));
    const rest = (rows: Row[]) => rows.filter((row) => !works.includes(row.work));
    assert.deepEqual(role.automatic, expected[i]);
    assert.deepEqual(role.reset, expected[i]);
    assert.equal(role.noFallbackBadges, true);
    assert.equal(role.closedHasModel, false);
    assert.equal(role.canReorder, false);
    assert.deepEqual(role.agents, [i === 0 ? "Codex" : "Claude Code"]);
    const initial = own(role.initial);
    assert.equal(initial.length, 2);
    assert.deepEqual(own(role.afterEffort), [{ ...initial[0], effort: "medium" }, initial[1]]);
    const first = { ...initial[0], model: i === 0 ? "gpt-6-luna" : "sonnet", effort: null, reason: "Your choice." };
    assert.deepEqual(own(role.afterFirstModel), [first, initial[1]]);
    assert.deepEqual(own(role.afterSecondEffort), [first, { ...initial[1], effort: "low" }]);
    assert.deepEqual(own(role.afterBothModels), [first, { ...initial[1], model: i === 0 ? "sonnet" : "gpt-6-luna", effort: null, reason: "Your choice." }]);
    assert.deepEqual(own(role.afterReset), []);
    for (const rows of [role.afterEffort, role.afterFirstModel, role.afterSecondEffort, role.afterBothModels, role.afterReset]) {
      assert.deepEqual(rest(rows), rest(role.initial), "other kinds stay intact");
    }
    assert.ok(role.afterBothModels.every((row) => row.work !== "Fusion"), "Fusion is only a display name");
  }
  assert.equal(seen.noEffort[1], "2HaikuNext if GPT-6 Luna can't run");
  assert.deepEqual(seen.fallbackOrder, ["1Opus 5.5 · HighPicked now", "2Codex's default · HighNext if Opus 5.5 can't run"]);
  assert.equal(seen.refreshOpensStrength, false);
  assert.equal(seen.refreshInHeader, true);
});
