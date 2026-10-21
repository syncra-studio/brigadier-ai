import assert from "node:assert/strict";
import { after, before, test } from "node:test";
import { createServer, type ViteDevServer } from "vite";

import type * as RuleWords from "@/app/settings/routing/ruleWords";
import type { Names } from "@/app/settings/routing/ruleWords";
import type { OverrideRule, OverrideTarget, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { noWorkRule } from "@/lib/workRules";

// The rules' words need the model names lib/routing reads through the app's modules: loaded by Vite.
let server: ViteDevServer;
let draftRule: typeof RuleWords.draftRule;
let previewDraft: typeof RuleWords.previewDraft;
let ruleLine: typeof RuleWords.ruleLine;
let ruleTitle: typeof RuleWords.ruleTitle;
let toggleKind: typeof RuleWords.toggleKind;
before(async () => {
  const globals = globalThis as Record<string, unknown>;
  globals.document ??= { documentElement: {} };
  globals.getComputedStyle ??= () => ({ getPropertyValue: () => "" });
  server = await createServer({ server: { middlewareMode: true, ws: false }, appType: "custom", ssr: { noExternal: ["@openai/apps-sdk-ui"] } });
  ({ draftRule, previewDraft, ruleLine, ruleTitle, toggleKind } = (await server.ssrLoadModule("/src/app/settings/routing/ruleWords.ts")) as typeof RuleWords);
});
after(() => server.close());

const DISPLAY: Record<string, string> = { opus: "Opus 5.5", sonnet: "Sonnet 5.5", "gpt-6": "GPT-6", "gpt-6-luna": "GPT-6 Luna", "gpt-6-astra": "GPT-6-Astra" };
const names: Names = {
  target: (target) => (target.type === "model" ? (DISPLAY[target.id] ?? target.id) : `Any ${target.provider} model`),
  model: (_provider, model) => DISPLAY[model] ?? model,
  project: (id) => (id === "p" ? "Brigadier Desktop" : null),
};
const opus: OverrideTarget = { type: "model", provider: "claude", id: "opus" };
const sonnet: OverrideTarget = { type: "model", provider: "claude", id: "sonnet" };
const gpt6: OverrideTarget = { type: "model", provider: "codex", id: "gpt-6" };
const astra: OverrideTarget = { type: "model", provider: "codex", id: "gpt-6-astra" };

const rule = (fields: Partial<OverrideRule>): OverrideRule => ({
  id: "r",
  effect: "never",
  target: sonnet,
  categories: [],
  areas: [],
  projectId: null,
  createdAtMs: 0,
  ...fields,
});

/** Every kind picking Opus 5.5 now, GPT-6 next. */
const routes = (["scout", "research", "implement", "review", "merge", "verify", "operate", "design", "chat", "orchestrate", "gitText"] as TaskCategory[]).map(
  (category): RoutePreview => ({
    category,
    areas: [],
    outcome: { type: "chosen", choice: { provider: "claude", model: "opus", effort: "high" }, reason: "", explanation: null },
    candidates: [
      { provider: "claude", model: "opus", tier: "frontier", score: 9.2, factors: [], effort: "high", heat: null, listed: null, chosen: true, trial: false, new: false, blocked: null, resetsAtMs: null },
      { provider: "codex", model: "gpt-6", tier: "frontier", score: 9, factors: [], effort: "high", heat: null, listed: null, chosen: false, trial: false, new: false, blocked: null, resetsAtMs: null },
    ],
    rankingId: null,
    places: [],
    trialSlot: false,
    byAuthor: [],
  }),
);

test("A rule reads as a sentence with where it applies", () => {
  const noWork = noWorkRule(sonnet, "x", 0);
  assert.equal(ruleTitle(noWork, names), "Never give Sonnet 5.5 work");
  assert.equal(ruleLine(noWork, names, false), "Everywhere · You can still pick it yourself in a chat or session");
  assert.equal(ruleLine(noWork, names, true), "Everywhere · New model, it gets nothing until you allow it");
  const review = rule({ categories: ["review"] });
  assert.equal(ruleTitle(review, names), "Don't use Sonnet 5.5 to review changes");
  assert.equal(ruleLine(review, names, false), "Review changes only");
  const prefer = rule({ effect: "prefer", target: gpt6, categories: ["gitText"] });
  assert.equal(ruleTitle(prefer, names), "Prefer GPT-6 for Write commit messages");
  assert.equal(ruleLine(prefer, names, false), "Write commit messages · Goes first whenever it can run");
  assert.equal(ruleLine({ ...prefer, projectId: "p" }, names, false), "In Brigadier Desktop · Write commit messages · Goes first whenever it can run");
  assert.equal(ruleTitle(rule({ effect: "only", target: gpt6, categories: ["implement"] }), names), "Only use GPT-6 for Write code");
});

test("Never for any work adds the no-work rule; a kind adds a rule for it", () => {
  assert.deepEqual(draftRule({ effect: "never", target: sonnet, kinds: [] }, "a", 1), noWorkRule(sonnet, "a", 1));
  assert.deepEqual(draftRule({ effect: "prefer", target: gpt6, kinds: ["gitText"] }, "b", 2), {
    id: "b",
    effect: "prefer",
    target: gpt6,
    categories: ["gitText"],
    areas: [],
    projectId: null,
    createdAtMs: 2,
  });
});

test("A draft says what it would change, or warns when another rule or an order decides", () => {
  const rules = [noWorkRule(sonnet, "old", 1), rule({ id: "rev", categories: ["review"] })];
  const ask = (effect: "never" | "prefer", target: OverrideTarget | null, kind: TaskCategory | "", rankings: Ranking[] = []) =>
    previewDraft({ effect, target, kinds: kind === "" ? [] : [kind] }, { rules, rankings, routes, names });
  assert.equal(ask("never", null, ""), null);
  assert.deepEqual(ask("prefer", gpt6, "gitText"), {
    tone: "info",
    text: "Write commit messages will go to GPT-6 instead of Opus 5.5 whenever GPT-6 can run. Everything else stays the same.",
  });
  assert.deepEqual(ask("prefer", sonnet, "review"), {
    tone: "warning",
    text: "“Never give Sonnet 5.5 work” already keeps Sonnet 5.5 away from everything. Never rules win, so this rule won't change anything until you remove that one.",
  });
  // No work covers the workers' kinds only: commit messages can still prefer it.
  assert.equal(ask("prefer", sonnet, "gitText")?.tone, "info");
  assert.deepEqual(ask("never", sonnet, "review"), { tone: "warning", text: "You already have this rule." });
  assert.deepEqual(ask("never", opus, "implement"), {
    tone: "info",
    text: "Write code will go to GPT-6 instead of Opus 5.5. Everything else stays the same.",
  });
  assert.deepEqual(ask("never", gpt6, "implement"), {
    tone: "info",
    text: "Write code doesn't use GPT-6 right now, so nothing changes yet. It won't be picked for it.",
  });
  assert.equal(
    ask("never", opus, "")?.text,
    "Explore the code, Research, Write code, Review changes, Resolve conflicts, Run checks, Use apps and Make designs and images will move to the next model. You can still pick Opus 5.5 yourself in a chat or session.",
  );
  const ordered: Ranking = { id: "o", category: "implement", areas: [], projectId: null, manual: true, entries: [{ target: opus, effort: null }], only: false, updatedAtMs: 0 };
  assert.deepEqual(ask("prefer", gpt6, "implement", [ordered]), {
    tone: "warning",
    text: "Write code follows your order, so this rule won't change anything there until you set it back to Automatic.",
  });
});

const ask = (rules: OverrideRule[], effect: "never" | "prefer", target: OverrideTarget, kind: TaskCategory | "" = "") =>
  previewDraft({ effect, target, kinds: kind === "" ? [] : [kind] }, { rules, rankings: [], routes, names });

test("A draft for any work weighs each kind a rule covers, and a rule kept to an area doesn't count", () => {
  // No work leaves a chat, coordinating and commit messages to a preference for any work.
  assert.deepEqual(ask([noWorkRule(sonnet, "old", 1)], "prefer", sonnet), {
    tone: "info",
    text: "Take over a chat, Coordinate work and Write commit messages will go to Sonnet 5.5 whenever it can run. Everything else stays the same.",
  });
  // A rule naming no kind keeps it from all of them.
  assert.equal(ask([rule({})], "prefer", sonnet)?.tone, "warning");
  // Every kind counts, coordinating and commit messages too.
  assert.equal(ask([], "prefer", gpt6)?.text, "11 kinds of work will go to GPT-6 whenever it can run. Everything else stays the same.");
  // A rule kept to an area isn't the same rule, and doesn't block one for everywhere.
  const frontend = rule({ categories: ["review"], areas: ["frontend"] });
  assert.equal(ask([frontend], "never", sonnet, "review")?.tone, "info");
  assert.equal(ask([{ ...frontend, categories: [] }], "prefer", sonnet, "review")?.tone, "info");
});

// #502: one rule for several kinds of work, its sentence and note reading naturally.
test("A rule takes several kinds of work at once", () => {
  assert.deepEqual(toggleKind(toggleKind([], "gitText"), "verify"), ["verify", "gitText"]);
  assert.deepEqual(toggleKind(["verify", "gitText"], "verify"), ["gitText"]);
  assert.deepEqual(toggleKind(["gitText"], "gitText"), []);
  const draft = { effect: "never" as const, target: astra, kinds: ["verify", "gitText"] as TaskCategory[] };
  const made = draftRule(draft, "c", 3);
  assert.deepEqual(made.categories, ["verify", "gitText"]);
  assert.equal(ruleTitle(made, names), "Don't use GPT-6-Astra to run checks or write commit messages");
  assert.equal(ruleLine(made, names, false), "Run checks and Write commit messages only");
  const preview = (kinds: TaskCategory[], rules: OverrideRule[] = []) =>
    previewDraft({ ...draft, kinds }, { rules, rankings: [], routes, names })?.text;
  assert.equal(
    preview(["verify", "gitText"]),
    "Run checks and Write commit messages don't use GPT-6-Astra right now, so nothing changes yet. It won't be picked for them.",
  );
  assert.equal(preview(["verify", "gitText"], [made]), "You already have this rule.");
  // Stored rules naming one kind still read as before.
  assert.equal(ruleTitle(rule({ target: astra, categories: ["design"] }), names), "Don't use GPT-6-Astra to make designs and images");
  // A model on some of the kinds: they move, the others are said apart.
  assert.equal(
    previewDraft({ effect: "never", target: opus, kinds: ["verify", "gitText"] }, { rules: [], rankings: [], routes, names })?.text,
    "Run checks and Write commit messages will go to GPT-6 instead of Opus 5.5. Everything else stays the same.",
  );
  const onlyChecks = routes.map((route) =>
    route.category === "gitText"
      ? { ...route, outcome: { type: "chosen" as const, choice: { provider: "codex" as const, model: "gpt-6", effort: "high" }, reason: "", explanation: null } }
      : route,
  );
  assert.equal(
    previewDraft({ effect: "never", target: opus, kinds: ["verify", "gitText"] }, { rules: [], rankings: [], routes: onlyChecks, names })?.text,
    "Run checks will go to GPT-6 instead of Opus 5.5. Write commit messages doesn't use Opus 5.5 right now. Everything else stays the same.",
  );
  assert.equal(
    previewDraft({ effect: "prefer", target: gpt6, kinds: ["verify", "gitText"] }, { rules: [], rankings: [], routes, names })?.text,
    "Run checks and Write commit messages will go to GPT-6 whenever it can run. Everything else stays the same.",
  );
  // Each kind kept off by another rule: nothing changes.
  const apart = [rule({ id: "v", target: opus, categories: ["verify"] }), rule({ id: "g", target: opus, categories: ["gitText"] })];
  assert.equal(
    previewDraft({ effect: "never", target: opus, kinds: ["verify", "gitText"] }, { rules: apart, rankings: [], routes, names })?.tone,
    "warning",
  );
});
