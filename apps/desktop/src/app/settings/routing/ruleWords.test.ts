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
  ({ draftRules, matches, previewDraft, ruleLine, ruleText, ruleTitle } = (await server.ssrLoadModule(
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
const row = (work: string, fields: Partial<GuidanceRow>): WorkRow => {
  const shipped: GuidanceRow = { work, agent: "claude", model: "opus", effort: "high", reason: "Shipped.", ...fields };
  return { work, row: shipped, shipped, mine: null };
};
const ROWS = [row("Architecture", {}), row("Review", { agent: "codex", model: "default" })];

test("a rule reads as the file writes it and as a sentence", () => {
  const rules: ModelRule[] = [
    { type: "forbid", names: ["fable", "claude-fable-5-1"] },
    { type: "allow", agent: "claude", names: ["opus", "sonnet"] },
    { type: "allow", agent: null, names: ["*"] },
    { type: "effortCeiling", effort: "high" },
  ];
  assert.deepEqual(rules.map(ruleText), [
    "forbid: fable, claude-fable-5-1",
    "allow claude: opus, sonnet",
    "allow: *",
    "effort-ceiling: high",
  ]);
  assert.deepEqual(
    rules.map((rule) => ruleTitle(rule, names)),
    ["Never use Fable or Fable 5.1", "Only use Opus 5.5 or Sonnet 5.5 on Claude", "Only use any model", "Never go above High effort"],
  );
  assert.deepEqual(rules.map(ruleLine), [
    "Everywhere · Chats and workers can't use them",
    "Claude only · Chats and workers use nothing else",
    "Everywhere · Chats and workers use nothing else",
    "Everywhere · Chats and workers stay at or below it",
  ]);
});

test("a pattern matches as the daemon matches it: a whole word, the id, or *", () => {
  assert.ok(matches("fable", "claude-fable-5-1"));
  assert.ok(matches("FABLE", "claude-fable-5-1"));
  assert.ok(matches("claude-fable-5-1", "claude-fable-5-1"));
  assert.ok(matches("*", "anything"));
  assert.ok(!matches("fab", "claude-fable-5-1"));
});

test("a draft adds its rule: Only joins its agent's allowlist, a ceiling replaces the old one", () => {
  const old: ModelRule[] = [{ type: "allow", agent: "claude", names: ["opus"] }, { type: "effortCeiling", effort: "max" }];
  assert.deepEqual(draftRules({ effect: "never", target: fable, effort: "high" }, old), [...old, { type: "forbid", names: ["fable"] }]);
  assert.deepEqual(draftRules({ effect: "only", target: sonnet, effort: "high" }, old), [
    { type: "allow", agent: "claude", names: ["opus", "sonnet"] },
    old[1],
  ]);
  assert.deepEqual(draftRules({ effect: "only", target: { type: "model", provider: "codex", id: "gpt-6" }, effort: "high" }, old), [
    ...old,
    { type: "allow", agent: "codex", names: ["gpt-6"] },
  ]);
  assert.deepEqual(draftRules({ effect: "cap", target: null, effort: "high" }, old), [old[0], { type: "effortCeiling", effort: "high" }]);
});

const preview = (draft: RuleWords.Draft, rules: ModelRule[] = [], rows: WorkRow[] = ROWS) =>
  previewDraft(draft, { rules, rows, models: MODELS, names });

test("a draft's preview says what it changes, and warns of a rule already there or a row it would refuse", () => {
  assert.equal(preview({ effect: "never", target: null, effort: "high" }), null);
  assert.deepEqual(preview({ effect: "never", target: fable, effort: "high" }), {
    tone: "info",
    text: "Fable 5.1 won't be used: a chat or worker that asks for it is refused, quoting this rule.",
  });
  assert.deepEqual(preview({ effect: "never", target: fable, effort: "high" }, [{ type: "forbid", names: ["fable"] }]), {
    tone: "warning",
    text: "You already have this rule.",
  });
  // A row naming the model, as written or as the listed model it resolves to.
  assert.deepEqual(preview({ effect: "never", target: { type: "model", provider: "claude", id: "claude-opus-5-5" }, effort: "high" }), {
    tone: "warning",
    text: "“Architecture” uses claude-opus-5-5, so starting it would be refused. Set your own model there too.",
  });
  assert.deepEqual(preview({ effect: "only", target: opus, effort: "high" }), {
    tone: "info",
    text: "Claude chats and workers will use only Opus 5.5. Sonnet 5.5 and Fable 5.1 are refused.",
  });
  assert.deepEqual(preview({ effect: "only", target: sonnet, effort: "high" }, [{ type: "allow", agent: "claude", names: ["opus"] }]), {
    tone: "info",
    text: "Sonnet 5.5 joins your Only rule for Claude. Claude chats and workers will use only Opus 5.5 or Sonnet 5.5. Fable 5.1 is refused.",
  });
  assert.deepEqual(preview({ effect: "only", target: sonnet, effort: "high" }), {
    tone: "warning",
    text: "“Architecture” uses another Claude model, so starting it would be refused. Set your own model there too.",
  });
  assert.deepEqual(preview({ effect: "cap", target: null, effort: "medium" }), {
    tone: "warning",
    text: "“Architecture” and “Review” ask for more than Medium effort, so starting them would be refused. Lower the effort there too.",
  });
  assert.deepEqual(preview({ effect: "cap", target: null, effort: "high" }, [{ type: "effortCeiling", effort: "max" }]), {
    tone: "info",
    text: "Chats and workers will run at High effort or lower; asking for more is refused. It replaces your Max ceiling.",
  });
  assert.deepEqual(preview({ effect: "cap", target: null, effort: "high" }, [{ type: "effortCeiling", effort: "high" }]), {
    tone: "warning",
    text: "You already have this rule.",
  });
});
