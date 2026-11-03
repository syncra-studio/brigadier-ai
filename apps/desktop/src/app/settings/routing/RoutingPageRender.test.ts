import assert from "node:assert/strict";
import { test } from "node:test";

import { renderFixtureLive } from "@/fixtures/headless";

/** A model's line in the picker when it's already in the order. */
const IN_THE_ORDER = "In the order";

// The real Routing page in Chromium over a faked daemon and rules file (S32): who does what from
// the file's rows over the shipped ones, a kind made My order and edited (effort, a second model,
// moved up, removed), the rules with the add form (its model, its kind of work, its preview and
// the kind a clash opens), a hand edit of the file shown on reopening, and under More the models'
// strength and how a model is picked.
test("Routing lists the rules file, writes its edits back, and shows a hand edit on reopening", { timeout: 60000 }, async (t) => {
  const dump = await renderFixtureLive(t, "routing.html?drive=1", "routing-result", 40000);
  const seen = JSON.parse(dump.replaceAll("&gt;", ">").replaceAll("&lt;", "<").replaceAll("&amp;", "&")) as Record<string, unknown>;
  assert.equal(
    String(seen.note).replace(/Ratings updated Oct 21 · checked .*? agoRefresh models$/, ""),
    "RoutingWhich agent, model and effort suit each kind of work. A session follows these when it starts workers; change only what you want to.Applies to new work only",
  );
  assert.deepEqual((seen.sections as string[]).slice(-3), ["Who does what", "Rules", "More"]);
  assert.equal(seen.sources, "Automatic picks ship with the app. Model strength, below, is rated from release notes and benchmarks, checked daily, and adjusted by how each model did in your sessions.");
  assert.equal(seen.rulesDescription, "Keep a model away from all work or one kind of work, allow only some models, or cap the effort");
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
  // The add form opens on top: a model, then the work it is for; nothing to add until a model is chosen.
  assert.equal(seen.form, "NeveruseChoose a modelforAny workCancelAdd rule");
  assert.equal(seen.addDisabled, true);
  assert.equal(seen.headerAddDisabled, true);
  // Its preview warns of a kind of work the rule would stop, says where to fix it, and opens it.
  assert.equal(
    seen.clash,
    "“Architecture and broad changes” and “Fusion · Claude consult” use Opus 5.5, so starting them would be refused. Pick another model for them in Who does what, or make this rule for one kind of work.Open Architecture and broad changesOpen Fusion · Claude consult",
  );
  assert.equal(seen.openAfterClash, 0);
  assert.equal(seen.openedByNote, "Architecture and broad changes");
  // The Codex consult uses its default, GPT-6 here; Release notes also names GPT-6.
  assert.equal(
    seen.preview,
    "“Fusion · Codex consult” and “Release notes” use GPT-6, so starting them would be refused. Pick another model for them in Who does what, or make this rule for one kind of work.Open Fusion · Codex consultOpen Release notes",
  );
  // For one kind of work only, it stops nothing else.
  assert.equal(
    seen.scoped,
    "GPT-6 won't be used for Mechanical changes and search: a worker for Mechanical changes and search that asks for it is refused, quoting this rule.",
  );
  // Added: the file gets the rule for that work, the form closes, the new rule comes first and marked, and Undo takes it back.
  assert.equal(seen.formAfterAdd, false);
  assert.deepEqual(seen.writtenRules, [
    { type: "forbid", names: ["fable"] },
    { type: "effortCeiling", effort: "high" },
    { type: "forbid", names: ["gpt-6"], work: "Mechanical changes and search" },
  ]);
  assert.deepEqual(seen.rulesAfterAdd, ["Don't use GPT-6 for Mechanical changes and searchNewMechanical changes and search only · Its workers can't run on it", ...rules]);
  assert.deepEqual(seen.toast, ["Rule added", "Undo"]);
  assert.deepEqual(seen.rulesAfterUndo, rules);
  // The file edited by hand shows when the page opens again: an order of two, a rule for one work
  // that keeps a kind's only model out, which its row and Automatic say.
  assert.deepEqual(seen.kindsAfterHandEdit, [
    "Architecture and broad changesAutomatic · Your rules keep Opus 5.5 out, so it can't start",
    ...kinds.slice(1).map((line) => line.startsWith("Conversation titles") ? line.replace("GPT-6 Luna · Low", "Haiku") : line),
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
    "1Named for the workerIf you or the session name a model or effort when the worker starts, that is used.",
    "2Your orderIf not, the worker takes the first model in your order for its kind of work that your rules allow.",
    "3AutomaticWith no order of your own, it takes the first model of the automatic advice that your rules allow.",
    "4Your rulesLast, your rules check the result: a model they keep out, or an effort above your cap, is refused and the worker doesn't start. Your rules win over everything above.",
  ]);
  assert.equal(seen.stepsExample, "For example, Conversation titles: your rules keep GPT-6 Luna out, so Haiku, next in the automatic advice, runs.");  assert.equal(seen.strengthClosed, true);
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
