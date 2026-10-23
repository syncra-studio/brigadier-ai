import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer, type ViteDevServer } from "vite";

import type * as RuleWords from "@/app/settings/routing/ruleWords";
import type { ListedModel, Names } from "@/app/settings/routing/ruleWords";
import type { WorkRow } from "@/app/settings/routing/words";
import type { GuidanceRow, ModelRule, OverrideTarget } from "@/ipc/generated";

// The rules' words need the model names lib/routing reads through the app's modules: loaded by Vite.
let server: ViteDevServer;
let draftRules: typeof RuleWords.draftRules;
let refusingRule: typeof RuleWords.refusingRule;
let matches: typeof RuleWords.matches;
let previewDraft: typeof RuleWords.previewDraft;
let ruleLine: typeof RuleWords.ruleLine;
let ruleText: typeof RuleWords.ruleText;
let ruleTitle: typeof RuleWords.ruleTitle;
before(async () => {
  const globals = globalThis as Record<string, unknown>;
  globals.document ??= { documentElement: {} };
  globals.getComputedStyle ??= () => ({ getPropertyValue: () => "" });
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  ({ draftRules, matches, previewDraft, refusingRule, ruleLine, ruleText, ruleTitle } = (await server.ssrLoadModule(
    "/src/app/settings/routing/ruleWords.ts",
  )) as typeof RuleWords);
});
after(() => server.close());

const DISPLAY: Record<string, string> = { opus: "Opus 5.5", sonnet: "Sonnet 5.5", "gpt-6": "GPT-6", "claude-fable-5-1": "Fable 5.1" };
const names: Names = {
  pattern: (_agent, name) => (name === "*" ? "any model" : (DISPLAY[name] ?? name.charAt(0).toUpperCase() + name.slice(1))),
  target: (target) => (target.type === "model" ? (DISPLAY[target.id] ?? target.id) : target.type === "family" ? `Newest ${target.family}` : "default"),
};
const MODELS: ListedModel[] = [
  { provider: "claude", id: "opus", resolved: "claude-opus-5-5", name: "Opus 5.5" },
  { provider: "claude", id: "sonnet", resolved: "claude-sonnet-5-5", name: "Sonnet 5.5" },
  { provider: "claude", id: "claude-fable-5-1", resolved: null, name: "Fable 5.1" },
  { provider: "codex", id: "gpt-6", resolved: null, name: "GPT-6" },
];
const fable: OverrideTarget = { type: "family", provider: "claude", family: "fable" };
const sonnet: OverrideTarget = { type: "model", provider: "claude", id: "sonnet" };
const opus: OverrideTarget = { type: "model", provider: "claude", id: "opus" };
const row = (work: string, fields: Partial<GuidanceRow>, ...next: Partial<GuidanceRow>[]): WorkRow => {
  const shipped = [fields, ...next].map((own): GuidanceRow => ({ work, agent: "claude", model: "opus", effort: "high", reason: "Shipped.", ...own }));
  return { work, rows: shipped, shipped, mine: [] };
};
const ROWS = [row("Architecture", {}), row("Review", { agent: "codex", model: "default" })];
/** A draft of `effect`: any work, High effort, unless given. */
const draft = (effect: RuleWords.DraftEffect, target: OverrideTarget | null, more: Partial<RuleWords.Draft> = {}): RuleWords.Draft => ({
  effect,
  target,
  effort: "high",
  work: null,
  ...more,
});

test("a rule reads as the file writes it and as a sentence: its model, then where it applies and what it does", () => {
  const rules: ModelRule[] = [
    { type: "forbid", names: ["fable", "claude-fable-5-1"] },
    { type: "allow", agent: "claude", names: ["opus", "sonnet"] },
    { type: "allow", agent: null, names: ["*"] },
    { type: "effortCeiling", effort: "high" },
    { type: "forbid", names: ["sonnet"], work: "Review" },
    { type: "allow", agent: "codex", names: ["gpt-6"], work: "Review" },
    { type: "effortCeiling", effort: "medium", work: "Review" },
  ];
  assert.deepEqual(rules.map(ruleText), [
    "forbid: fable, claude-fable-5-1",
    "allow claude: opus, sonnet",
    "allow: *",
    "effort-ceiling: high",
    "forbid for Review: sonnet",
    "allow codex for Review: gpt-6",
    "effort-ceiling for Review: medium",
  ]);
  assert.deepEqual(
    rules.map((rule) => ruleTitle(rule, names)),
    [
      "Never give Fable or Fable 5.1 work",
      "Only use Opus 5.5 or Sonnet 5.5 on Claude",
      "Only use any model",
      "Never go above High effort",
      "Don't use Sonnet 5.5 for Review",
      "Only use GPT-6 on Codex for Review",
      "Never go above Medium effort for Review",
    ],
  );
  assert.deepEqual(rules.map(ruleLine), [
    "Everywhere · Chats, sessions and workers can't run on them",
    "Everywhere · Claude chats, sessions and workers run on nothing else",
    "Everywhere · Chats, sessions and workers run on nothing else",
    "Everywhere · Chats, sessions and workers run at this effort or lower",
    "Review only · Its workers can't run on it",
    "Review only · Its Codex workers run on nothing else",
    "Review only · Its workers run at this effort or lower",
  ]);
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
  assert.equal(refusingRule(rules, { ...sonnetChoice, effort: "max" }, null), rules[1]);
});

test("a draft adds its rule: Only joins its agent's allowlist of the same work, a ceiling replaces the one of its work", () => {
  const old: ModelRule[] = [{ type: "allow", agent: "claude", names: ["opus"] }, { type: "effortCeiling", effort: "max" }];
  assert.deepEqual(draftRules(draft("never", fable), old), [...old, { type: "forbid", names: ["fable"] }]);
  assert.deepEqual(draftRules(draft("only", sonnet), old), [{ type: "allow", agent: "claude", names: ["opus", "sonnet"] }, old[1]]);
  assert.deepEqual(draftRules(draft("only", { type: "model", provider: "codex", id: "gpt-6" }), old), [
    ...old,
    { type: "allow", agent: "codex", names: ["gpt-6"] },
  ]);
  assert.deepEqual(draftRules(draft("cap", null), old), [old[0], { type: "effortCeiling", effort: "high" }]);
  // For one kind of work: its own line, beside the rules for every work.
  assert.deepEqual(draftRules(draft("only", sonnet, { work: "Review" }), old), [
    ...old,
    { type: "allow", agent: "claude", names: ["sonnet"], work: "Review" },
  ]);
  assert.deepEqual(draftRules(draft("cap", null, { effort: "low", work: "Review" }), old), [
    ...old,
    { type: "effortCeiling", effort: "low", work: "Review" },
  ]);
});

const preview = (next: RuleWords.Draft, rules: ModelRule[] = [], rows: WorkRow[] = ROWS) =>
  previewDraft(next, { rules, rows, models: MODELS, names });

test("a draft's preview says what it changes, and warns of a rule already there or a kind of work it would stop", () => {
  assert.equal(preview(draft("never", null)), null);
  assert.deepEqual(preview(draft("never", fable)), {
    tone: "info",
    text: "Fable 5.1 won't be used: a chat, session or worker that asks for it is refused, quoting this rule.",
  });
  assert.deepEqual(preview(draft("never", fable, { work: "Review" })), {
    tone: "info",
    text: "Fable 5.1 won't be used for Review: a worker for Review that asks for it is refused, quoting this rule.",
  });
  assert.deepEqual(preview(draft("never", fable), [{ type: "forbid", names: ["fable"] }]), {
    tone: "warning",
    text: "You already have this rule.",
  });
  // A kind of work whose model the rule names, as written or as the listed model it resolves to:
  // the warning names where to fix it, and the kinds to open.
  assert.deepEqual(preview(draft("never", { type: "model", provider: "claude", id: "claude-opus-5-5" })), {
    tone: "warning",
    text: "“Architecture” uses claude-opus-5-5, so starting it would be refused. Pick another model for it in Who does what, or make this rule for one kind of work.",
    works: ["Architecture"],
  });
  // For another kind of work only, it stops nothing.
  assert.equal(preview(draft("never", opus, { work: "Review" }))?.tone, "info");
  // A kind whose order has another model the rules allow still starts.
  assert.equal(preview(draft("never", opus), [], [row("Architecture", {}, { agent: "codex", model: "gpt-6" })])?.tone, "info");
  assert.deepEqual(preview(draft("only", opus)), {
    tone: "info",
    text: "Claude chats, sessions and workers will use only Opus 5.5. Sonnet 5.5 and Fable 5.1 are refused.",
  });
  assert.deepEqual(preview(draft("only", sonnet), [{ type: "allow", agent: "claude", names: ["opus"] }]), {
    tone: "info",
    text: "Sonnet 5.5 joins your Only rule for Claude. Claude chats, sessions and workers will use only Opus 5.5 or Sonnet 5.5. Fable 5.1 is refused.",
  });
  assert.deepEqual(preview(draft("only", sonnet)), {
    tone: "warning",
    text: "“Architecture” uses another Claude model, so starting it would be refused. Pick another model for it in Who does what, or make this rule for one kind of work.",
    works: ["Architecture"],
  });
  assert.deepEqual(preview(draft("only", sonnet, { work: "Architecture" })), {
    tone: "warning",
    text: "“Architecture” uses another Claude model, so starting it would be refused. Pick another model for it in Who does what.",
    works: ["Architecture"],
  });
  // Never rules win: Only of a model a Never rule keeps out.
  assert.deepEqual(preview(draft("only", sonnet), [{ type: "forbid", names: ["sonnet"] }]), {
    tone: "warning",
    text: "“Never give Sonnet 5.5 work” already keeps Sonnet 5.5 out, and Never rules win, so with this rule no Claude model could run. Remove that rule first.",
  });
  assert.deepEqual(preview(draft("cap", null, { effort: "medium" })), {
    tone: "warning",
    text: "“Architecture” and “Review” ask for more than Medium effort, so starting them would be refused. Lower their effort in Who does what, or make this rule for one kind of work.",
    works: ["Architecture", "Review"],
  });
  assert.deepEqual(preview(draft("cap", null), [{ type: "effortCeiling", effort: "max" }]), {
    tone: "info",
    text: "Chats, sessions and workers will run at High effort or lower; asking for more is refused. It replaces your Max ceiling.",
  });
  assert.deepEqual(preview(draft("cap", null), [{ type: "effortCeiling", effort: "high" }]), {
    tone: "warning",
    text: "You already have this rule.",
  });
});
