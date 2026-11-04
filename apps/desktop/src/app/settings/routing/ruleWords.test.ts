import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer, type ViteDevServer } from "vite";

import type * as RuleWords from "@/app/settings/routing/ruleWords";
import type { ListedModel, Names } from "@/app/settings/routing/ruleWords";
import type { Availability } from "@/state/providers";
import type { WorkRow } from "@/app/settings/routing/words";
import type { ModelGroup } from "@/components/shared/model-selector";
import type { GuidanceRow, ModelInfo, ModelRule } from "@/ipc/generated";

// The rules' words need the model names lib/routing reads through the app's modules: loaded by Vite.
let server: ViteDevServer;
let refusingRule: typeof RuleWords.refusingRule;
let ruledEffort: typeof RuleWords.ruledEffort;
let ruleWarnings: typeof RuleWords.ruleWarnings;
let ruleNames: typeof RuleWords.ruleNames;
let matches: typeof RuleWords.matches;
let ruleText: typeof RuleWords.ruleText;
let ruleTitle: typeof RuleWords.ruleTitle;
let allowedPick: typeof RuleWords.allowedPick;
let pickRefusal: typeof RuleWords.pickRefusal;
let refusalText: typeof RuleWords.refusalText;
let rowChoice: typeof RuleWords.rowChoice;
let startsOn: typeof RuleWords.startsOn;
before(async () => {
  const globals = globalThis as Record<string, unknown>;
  globals.document ??= { documentElement: {} };
  globals.getComputedStyle ??= () => ({ getPropertyValue: () => "" });
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  ({ allowedPick, matches, pickRefusal, refusalText, refusingRule, rowChoice, ruledEffort, ruleNames, ruleText, ruleTitle, ruleWarnings, startsOn } = (await server.ssrLoadModule(
    "/src/app/settings/routing/ruleWords.ts",
  )) as typeof RuleWords);
});
after(() => server.close());

const DISPLAY: Record<string, string> = { "gpt-6": "GPT-6", "claude-fable-5-1": "Fable 5.1" };
const names: Names = {
  pattern: (_agent, name) => (name === "*" ? "any model" : (DISPLAY[name] ?? name.charAt(0).toUpperCase() + name.slice(1))),
};
const MODELS: ListedModel[] = [
  { provider: "claude", id: "opus", resolved: "claude-opus-5-5", name: "Opus 5.5" },
  { provider: "claude", id: "sonnet", resolved: "claude-sonnet-5-5", name: "Sonnet 5.5" },
  { provider: "claude", id: "claude-fable-5-1", resolved: null, name: "Fable 5.1" },
  { provider: "codex", id: "gpt-6", resolved: null, name: "GPT-6" },
];
const row = (work: string, fields: Partial<GuidanceRow>, ...next: Partial<GuidanceRow>[]): WorkRow => {
  const shipped = [fields, ...next].map((own): GuidanceRow => ({ work, agent: "claude", model: "opus", effort: "high", reason: "Shipped.", ...own }));
  return { work, rows: shipped, shipped, mine: [] };
};

test("a rule reads as the file writes it and as one plain sentence", () => {
  const rules: ModelRule[] = [
    { type: "forbid", names: ["fable", "claude-fable-5-1"] },
    { type: "allow", agent: "claude", names: ["opus", "sonnet"] },
    { type: "allow", agent: null, names: ["*"] },
    { type: "effortCeiling", effort: "high" },
    { type: "forbid", names: ["sonnet"], work: "Review" },
    { type: "allow", agent: "codex", names: ["gpt-6"], work: "Review" },
    { type: "effortCeiling", effort: "medium", work: "Review" },
    { type: "effortCeiling", effort: "medium", model: "opus" },
    { type: "effortCeiling", effort: "high", agent: "codex" },
    { type: "effortFloor", effort: "high", work: "Adversarial review" },
    { type: "effortFloor", effort: "low" },
    { type: "effortFloor", effort: "xhigh", model: "opus", work: "Review" },
    { type: "advice", text: "Prefer Codex when Claude is busy" },
  ];
  assert.deepEqual(rules.map(ruleText), [
    "forbid: fable, claude-fable-5-1",
    "allow claude: opus, sonnet",
    "allow: *",
    "effort-ceiling: high",
    "forbid for Review: sonnet",
    "allow codex for Review: gpt-6",
    "effort-ceiling for Review: medium",
    "effort-ceiling opus: medium",
    "effort-ceiling codex: high",
    "effort-floor for Adversarial review: high",
    "effort-floor: low",
    "effort-floor opus for Review: xhigh",
    "advice: Prefer Codex when Claude is busy",
  ]);
  assert.deepEqual(
    rules.map((rule) => ruleTitle(rule, names)),
    [
      "Never use Fable or Fable 5.1",
      "Only use Opus or Sonnet on Claude",
      "Only use any model",
      "Never go above High effort",
      "Never use Sonnet for Review",
      "Only use GPT-6 on Codex for Review",
      "Review never above Medium",
      "Never run Opus above Medium",
      "Never run Codex above High",
      "Adversarial review never below High",
      "Never go below Low effort",
      "Never run Opus below Extra high for Review",
      "Prefer Codex when Claude is busy",
    ],
  );
});

test("a family word reads as the family, an exact id as its model; every forbid has one form", () => {
  const groups = [
    { provider: "claude", label: "Claude Code", models: [{ id: "opus", displayName: "Opus 5.5", resolved: "claude-opus-5-5", efforts: [] }] },
    { provider: "codex", label: "Codex", models: [{ id: "gpt-6.1-sol", displayName: "GPT-6.1 Sol", resolved: null, efforts: [] }] },
  ] as unknown as ModelGroup[];
  const real = ruleNames(groups);
  const title = (rule: ModelRule) => ruleTitle(rule, real);
  assert.equal(title({ type: "forbid", names: ["opus"] }), "Never use Opus");
  assert.equal(title({ type: "forbid", names: ["sonnet"], work: "Adversarial review" }), "Never use Sonnet for Adversarial review");
  assert.equal(title({ type: "forbid", names: ["claude-opus-5-5"] }), "Never use Opus 5.5");
  assert.equal(title({ type: "forbid", names: ["gpt-6.1-sol"] }), "Never use GPT-6.1 Sol");
  assert.equal(title({ type: "effortCeiling", effort: "medium", model: "opus" }), "Never run Opus above Medium");
  assert.equal(title({ type: "forbid", names: ["*"] }), "Never use any model");
});

test("a pattern matches as the daemon matches it: a whole word, the id, or *", () => {
  assert.ok(matches("fable", "claude-fable-5-1"));
  assert.ok(matches("FABLE", "claude-fable-5-1"));
  assert.ok(matches("claude-fable-5-1", "claude-fable-5-1"));
  assert.ok(matches("*", "anything"));
  assert.ok(!matches("fab", "claude-fable-5-1"));
});

test("a rule for one kind of work refuses only a start of that work, as the daemon judges", () => {
  const rules: ModelRule[] = [{ type: "forbid", names: ["sonnet"], work: "Review" }, { type: "effortCeiling", effort: "high" }];
  const sonnetChoice = { provider: "claude" as const, names: ["sonnet", "claude-sonnet-5-5"], effort: null };
  assert.equal(refusingRule(rules, sonnetChoice, "review"), rules[0]);
  assert.equal(refusingRule(rules, sonnetChoice, "Architecture"), null);
  assert.equal(refusingRule(rules, sonnetChoice, null), null);
  // A cap moves the effort; it refuses nothing by itself.
  assert.equal(refusingRule(rules, { ...sonnetChoice, effort: "max" }, null), null);
  assert.equal(ruledEffort(rules, { ...sonnetChoice, effort: "max" }, null), "high");
});

test("caps and minimums move the effort, for one model or agent; only a cap and a minimum that can't both be met refuse", () => {
  const opusChoice = { provider: "claude" as const, names: ["opus", "claude-opus-5-5"], effort: "max" };
  const gpt = { provider: "codex" as const, names: ["gpt-6"], effort: "low" };
  const rules: ModelRule[] = [
    { type: "effortCeiling", effort: "medium", model: "opus" },
    { type: "effortCeiling", effort: "high", agent: "codex" },
    { type: "effortFloor", effort: "high", work: "Adversarial review" },
  ];
  assert.equal(ruledEffort(rules, opusChoice, null), "medium");
  assert.equal(ruledEffort(rules, { ...opusChoice, names: ["sonnet"] }, null), "max");
  assert.equal(ruledEffort(rules, { ...gpt, effort: "max" }, null), "high");
  assert.equal(ruledEffort(rules, gpt, "Adversarial review"), "high");
  // No effort named: a minimum names one; a cap leaves the model's default.
  assert.equal(ruledEffort(rules, { ...gpt, effort: null }, "Adversarial review"), "high");
  assert.equal(ruledEffort(rules, { ...opusChoice, effort: null }, null), null);
  assert.equal(refusingRule(rules, opusChoice, null), null);
  assert.equal(refusingRule(rules, gpt, "Adversarial review"), null);
  // Opus for a review: kept at Medium, needing High.
  assert.equal(refusingRule(rules, opusChoice, "Adversarial review"), rules[0]);
  // Advice never refuses or moves anything.
  const advice: ModelRule[] = [{ type: "advice", text: "never use opus" }];
  assert.equal(refusingRule(advice, opusChoice, null), null);
  assert.equal(ruledEffort(advice, opusChoice, null), "max");
});

const MODEL_NAMES: Record<string, string> = { opus: "Opus 5.5", "gpt-6": "GPT-6" };
const modelName = (found: GuidanceRow) => MODEL_NAMES[found.model] ?? found.model;

test("a rule that stops a kind of work says so in one line; one that only moves its effort says nothing", () => {
  const kinds = [
    row("Architecture", {}),
    row("Adversarial review", { agent: "codex", model: "gpt-6" }, {}),
    row("Computer use", {}, { agent: "codex", model: "gpt-6" }),
  ];
  const cap: ModelRule = { type: "effortCeiling", effort: "medium", model: "opus" };
  const floor: ModelRule = { type: "effortFloor", effort: "high", work: "Adversarial review" };
  const never: ModelRule = { type: "forbid", names: ["opus"] };
  assert.deepEqual([...ruleWarnings([cap], kinds, MODELS, names, modelName)], []);
  assert.deepEqual([...ruleWarnings([cap, floor], kinds, MODELS, names, modelName)], [
    ["effort-ceiling opus: medium", "Reviews Codex's work: it needs High, and this rule keeps Opus at Medium."],
  ]);
  // Never: Architecture has no other model; Computer use's next one runs; a review's other side still can.
  assert.deepEqual([...ruleWarnings([never], kinds, MODELS, names, modelName)], [
    ["forbid: opus", "Architecture can't start: this rule keeps out Opus 5.5. Reviews Codex's work: this rule keeps out Opus 5.5."],
  ]);
  // A kind of roles with no side left can't start; Fallback's side is named as Fallback names it.
  const roles = [row("Adversarial review", { agent: "codex", model: "gpt-6" }, {}), row("Fallback", {}, { agent: "codex", model: "gpt-6" })];
  assert.deepEqual([...ruleWarnings([{ type: "forbid", names: ["opus", "gpt-6"] }], roles, MODELS, names, modelName)], [
    ["forbid: opus, gpt-6", "Adversarial review can't start: this rule keeps out GPT-6 and Opus 5.5. Fallback can't start: this rule keeps out Opus 5.5 and GPT-6."],
  ]);
  assert.deepEqual([...ruleWarnings([never], roles.slice(1), MODELS, names, modelName)], [["forbid: opus", "Moving to Claude Code: this rule keeps out Opus 5.5."]]);
});

test("a row of the agent's default is judged as its default model at that model's effort, as the daemon resolves it", () => {
  const listed: ListedModel[] = [
    { provider: "codex", id: "gpt-6", resolved: null, name: "GPT-6", isDefault: true, defaultEffort: "medium" },
    { provider: "codex", id: "gpt-6-luna", resolved: null, name: "GPT-6 Luna", isDefault: false, defaultEffort: "low" },
  ];
  const review: GuidanceRow = { work: "Adversarial review", agent: "codex", model: "default", effort: null, reason: "Shipped." };
  assert.deepEqual(rowChoice(review, listed), { provider: "codex", names: ["gpt-6"], effort: "medium" });
  // A rule that keeps the default model out keeps the row out, so the page never shows it as the pick.
  const rules: ModelRule[] = [{ type: "forbid", names: ["gpt-6"] }];
  assert.equal(refusingRule(rules, rowChoice(review, listed), "Adversarial review"), rules[0]);
  assert.equal(ruledEffort([{ type: "effortCeiling", effort: "low" }], rowChoice(review, listed), null), "low");
  // A named model with no effort starts at its own default effort.
  assert.equal(rowChoice({ ...review, model: "gpt-6-luna" }, listed).effort, "low");
});

// The composer's model picker (issue 545): what the rules keep out, in the Rules card's words.
const info = (id: string, displayName: string, resolved: string | null, efforts: string[], defaultEffort: string | null): ModelInfo => ({
  id, displayName, description: "", resolved, efforts, defaultEffort, isDefault: id === "opus", inputModalities: [], fast: null, legacy: false,
});
const OPUS = info("opus", "Opus 5.5", "claude-opus-5-5", ["low", "medium", "high", "xhigh", "max"], "high");
const SONNET = info("sonnet", "Sonnet 5.5", "claude-sonnet-5-5", ["low", "medium", "high"], "medium");
const GROUPS: ModelGroup[] = [{ provider: "claude", label: "Claude", models: [OPUS, SONNET], unavailable: null }];
const CEILING: ModelRule[] = [{ type: "effortCeiling", effort: "high" }];

test("the picker shows an effort or model the rules keep out with the rule's title; others are free (issue 545)", () => {
  assert.equal(pickRefusal(CEILING, names, "claude", OPUS, "max"), "Never go above High effort");
  assert.equal(pickRefusal(CEILING, names, "claude", OPUS, "xhigh"), "Never go above High effort");
  assert.equal(pickRefusal(CEILING, names, "claude", OPUS, "high"), null);
  // The model alone: a ceiling doesn't keep it out.
  assert.equal(pickRefusal(CEILING, names, "claude", OPUS, null), null);
  // A rule names a model by its id or what it resolves to.
  assert.equal(pickRefusal([{ type: "forbid", names: ["claude-opus-5-5"] }], names, "claude", OPUS, null), "Never use Claude-opus-5-5");
  assert.equal(pickRefusal([{ type: "allow", agent: "claude", names: ["sonnet"] }], names, "claude", OPUS, null), "Only use Sonnet on Claude");
  // A minimum keeps lower efforts out; a cap and a minimum on one model that can't both be met keep the model out.
  assert.equal(pickRefusal([{ type: "effortFloor", effort: "medium" }], names, "claude", OPUS, "low"), "Never go below Medium effort");
  assert.equal(pickRefusal([{ type: "effortFloor", effort: "medium" }], names, "claude", OPUS, "high"), null);
  assert.equal(
    pickRefusal([{ type: "effortCeiling", effort: "low", model: "opus" }, { type: "effortFloor", effort: "high" }], names, "claude", OPUS, null),
    "Never run Opus above Low",
  );
  // A rule for one kind of work never judges a chat's or session's pick.
  assert.equal(pickRefusal([{ type: "effortCeiling", effort: "low", work: "Review" }], names, "claude", OPUS, "max"), null);
});

test("a pick above the ceiling comes down to its model's highest allowed effort; any other pick stays (issue 545)", () => {
  const max = { provider: "claude" as const, model: "opus", effort: "max" };
  assert.deepEqual(allowedPick(CEILING, GROUPS, max), { ...max, effort: "high" });
  assert.deepEqual(allowedPick([{ type: "effortCeiling", effort: "medium" }], GROUPS, max), { ...max, effort: "medium" });
  assert.equal(allowedPick(CEILING, GROUPS, { ...max, effort: "medium" }).effort, "medium");
  assert.deepEqual(allowedPick([], GROUPS, max), max);
  // No effort picked: the model's default is what the daemon judges.
  assert.deepEqual(allowedPick([{ type: "effortCeiling", effort: "low" }], GROUPS, { provider: "claude", model: "sonnet", effort: null }), {
    provider: "claude", model: "sonnet", effort: "low",
  });
  // Below a minimum: up to its model's lowest effort within the rules.
  assert.deepEqual(allowedPick([{ type: "effortFloor", effort: "xhigh" }], GROUPS, { ...max, effort: "low" }), { ...max, effort: "xhigh" });
  assert.deepEqual(allowedPick([{ type: "effortCeiling", effort: "medium", agent: "codex" }], GROUPS, max), max);
  // A model the rules keep out stays as it is: the picker shows why, the daemon refuses it.
  const forbid: ModelRule[] = [{ type: "forbid", names: ["opus"] }];
  assert.deepEqual(allowedPick(forbid, GROUPS, max), max);
});

test("the daemon's refusal reads in the rule's words; other errors stay as they are (issue 545)", () => {
  assert.equal(refusalText('refused by your model rule: "effort-ceiling: high"', CEILING, names), "Refused by your rule “Never go above High effort”");
  // As the user wrote it in the file: case and spaces don't matter.
  assert.equal(refusalText('refused by your model rule: "forbid:  Opus,sonnet"', [{ type: "forbid", names: ["opus", "sonnet"] }], names), "Refused by your rule “Never use Opus or Sonnet”");
  assert.equal(refusalText('refused by your model rule: "forbid: fable"', CEILING, names), 'refused by your model rule: "forbid: fable"');
  assert.equal(refusalText("the daemon closed the connection", CEILING, names), "the daemon closed the connection");
  // Two that can't both be met, both named.
  const both: ModelRule[] = [{ type: "effortCeiling", effort: "medium", model: "opus" }, { type: "effortFloor", effort: "high", work: "Adversarial review" }];
  assert.equal(
    refusalText('refused by your model rules: "effort-ceiling opus: medium" and "effort-floor for Adversarial review: high" can\'t both be met', both, names),
    "Refused: your rules “Never run Opus above Medium” and “Adversarial review never below High” can't both be met",
  );
});

// Who does what's "now" (#761): the first row your rules let start that Settings › Providers lets
// run, as a start by its Work picks it, so the row agrees with the kind's own Automatic list.
test("a kind starts on the first row whose agent is on and whose model isn't made unavailable", () => {
  const models: ListedModel[] = [
    ...MODELS,
    { provider: "claude", id: "claude-sonnet-4-5", resolved: null, name: "Sonnet 4.5", legacy: true },
    { provider: "codex", id: "gpt-6-luna", resolved: null, name: "GPT-6 Luna", isDefault: true },
  ];
  const order: GuidanceRow[] = [
    { work: "Docs", agent: "claude", model: "opus", effort: "high", reason: "Mine." },
    { work: "Docs", agent: "codex", model: "gpt-6", effort: null, reason: "Mine." },
  ];
  const all: Availability = { disabledProviders: [], hiddenModels: [] };
  const on = (availability: Partial<Availability>) => startsOn(order, [], models, { ...all, ...availability }, "Docs");
  assert.equal(on({}), order[0]);
  // Claude switched off: the order starts on Codex, as a start would.
  assert.equal(on({ disabledProviders: ["claude"] }), order[1]);
  // Claude on, its model made unavailable (as its listed id): Codex too.
  assert.equal(on({ hiddenModels: [{ provider: "claude", id: "opus" }] }), order[1]);
  // Nothing on: nothing starts.
  assert.equal(on({ disabledProviders: ["claude", "codex"] }), undefined);
  // A rule still comes first: refused rows never start, whatever is on.
  assert.equal(startsOn(order, [{ type: "forbid", names: ["gpt-6"] }], models, { ...all, disabledProviders: ["claude"] }, "Docs"), undefined);
  // A family word starts on its newest current model, the agent's default on its default: those are what's judged.
  const family: GuidanceRow[] = [
    { work: "Docs", agent: "claude", model: "sonnet", effort: null, reason: "Mine." },
    { work: "Docs", agent: "codex", model: "default", effort: null, reason: "Mine." },
  ];
  const familyOn = (availability: Partial<Availability>) => startsOn(family, [], models, { ...all, ...availability }, "Docs");
  assert.equal(familyOn({ hiddenModels: [{ provider: "claude", id: "claude-sonnet-4-5" }] }), family[0]);
  assert.equal(familyOn({ hiddenModels: [{ provider: "claude", id: "sonnet" }] }), family[1]);
  assert.equal(familyOn({ hiddenModels: [{ provider: "claude", id: "sonnet" }, { provider: "codex", id: "gpt-6-luna" }] }), undefined);
});
