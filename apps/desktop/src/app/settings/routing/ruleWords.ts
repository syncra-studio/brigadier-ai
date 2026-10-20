import type { OverrideRule, OverrideTarget, ProviderKind, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { CATEGORIES, joinWords, PLAIN_KIND_NAMES, sameTarget } from "@/lib/routing";
import { isNoWorkRule, noWorkRule, WORKER_CATEGORIES } from "@/lib/workRules";

/*
 * The Rules card's words and sums: a rule's title and line, the rule a draft makes, and what a
 * draft would change before it is added (or why it would change nothing).
 */

/** What a new rule does: keeps a model away, or puts it first. */
export type DraftEffect = "never" | "prefer";

/** A rule being written: what it does, to which model, for which kind of work ("" any). */
export type Draft = { effect: DraftEffect; target: OverrideTarget | null; kind: TaskCategory | "" };

/** The names the words need: a rule's model, a routed model, a project. */
export type Names = {
  target: (target: OverrideTarget) => string;
  model: (provider: ProviderKind, model: string) => string;
  project: (id: string) => string | null;
};

/** A routed model's family ("opus"), for a rule about the newest of one. */
export type FamilyOf = (provider: ProviderKind, model: string) => string | null;

/** "review changes": a kind of work as the end of "Don't use … to". */
function kindVerb(category: TaskCategory): string {
  const name = PLAIN_KIND_NAMES[category];
  return name.charAt(0).toLowerCase() + name.slice(1);
}

const kindNames = (categories: readonly TaskCategory[], last = "and") =>
  joinWords(categories.map((category) => PLAIN_KIND_NAMES[category]), last);

/** Every kind of work a rule naming no kind applies to. */
const EVERY_KIND: readonly TaskCategory[] = [...CATEGORIES, "gitText"];

/** Whether a rule applies to a kind of work: "no work" covers the workers' kinds only. */
function keepsIn(rule: OverrideRule, category: TaskCategory): boolean {
  return (
    rule.categories.length === 0 ||
    rule.categories.includes(category) ||
    (isNoWorkRule(rule) && WORKER_CATEGORIES.includes(category))
  );
}

/** A never rule over all work: the "no work" rule, or one naming no kind. */
function neverAll(rule: OverrideRule): boolean {
  return rule.effect === "never" && (isNoWorkRule(rule) || rule.categories.length === 0);
}

/** "Never give Sonnet 5.5 work", "Don't use Sonnet 5.5 to review changes", "Prefer GPT-6 for Write commit messages". */
export function ruleTitle(rule: OverrideRule, names: Names): string {
  const model = names.target(rule.target);
  if (neverAll(rule)) return `Never give ${model} work`;
  if (rule.effect === "never") return `Don't use ${model} to ${joinWords(rule.categories.map(kindVerb), "or")}`;
  const kinds = rule.categories.length > 0 ? kindNames(rule.categories, "or") : "any work";
  return rule.effect === "prefer" ? `Prefer ${model} for ${kinds}` : `Only use ${model} for ${kinds}`;
}

/** Where a rule applies and what it leaves: "Everywhere · You can still pick it yourself in a chat or session". */
export function ruleLine(rule: OverrideRule, names: Names, isNewModel: boolean): string {
  const where = rule.projectId ? `In ${names.project(rule.projectId) ?? "a removed project"}` : null;
  if (neverAll(rule)) {
    return `${where ?? "Everywhere"} · ${
      isNewModel ? "New model, it gets nothing until you allow it" : "You can still pick it yourself in a chat or session"
    }`;
  }
  const kinds = rule.categories.length > 0 ? kindNames(rule.categories) : null;
  if (rule.effect === "never") return [where, `${kinds} only`].filter(Boolean).join(" · ");
  const what = rule.effect === "prefer" ? "Goes first whenever it can run" : "Only it takes this work";
  return [where, kinds ?? (where ? null : "Everywhere"), what].filter(Boolean).join(" · ");
}

/** The rule a finished draft adds: Never for any work is the "no work" rule. */
export function draftRule(draft: Draft & { target: OverrideTarget }, id: string, createdAtMs: number): OverrideRule {
  if (draft.effect === "never" && draft.kind === "") return noWorkRule(draft.target, id, createdAtMs);
  return {
    id,
    effect: draft.effect,
    target: draft.target,
    categories: draft.kind === "" ? [] : [draft.kind],
    areas: [],
    projectId: null,
    createdAtMs,
  };
}

/** Whether a rule's target takes in a routed model (a vendor takes in all of its models). */
function takesIn(target: OverrideTarget, provider: ProviderKind, model: string, family: FamilyOf | null): boolean {
  if (target.provider !== provider) return false;
  if (target.type === "vendor") return true;
  if (target.type === "model") return target.id === model;
  return family !== null && family(provider, model) === target.family;
}

/** Whether rule `wide`'s target takes in all `narrow` names. */
function covers(wide: OverrideTarget, narrow: OverrideTarget): boolean {
  return sameTarget(wide, narrow) || (wide.type === "vendor" && wide.provider === narrow.provider);
}

/** What a draft would change, said before it is added; a warning when it would change nothing. */
export type DraftPreview = { tone: "info" | "warning"; text: string };

/**
 * What adding the draft would do, from what routing picks for each kind now (All projects); a
 * warning when another rule or your order already decides there. `family` names a routed
 * model's family, for a rule about the newest of one.
 */
export function previewDraft(
  draft: Draft,
  {
    rules,
    rankings,
    routes,
    names,
    family = null,
  }: {
    rules: readonly OverrideRule[];
    rankings: readonly Ranking[];
    routes: readonly RoutePreview[] | null;
    names: Names;
    family?: FamilyOf | null;
  },
): DraftPreview | null {
  const target = draft.target;
  if (!target) return null;
  const model = names.target(target);
  const made = draftRule({ ...draft, target }, "", 0);
  // Rules for all projects and any area: one kept to an area still leaves the rest to the draft.
  const everywhere = rules.filter((rule) => rule.projectId === null && rule.areas.length === 0);
  if (
    everywhere.some(
      (rule) =>
        rule.effect === made.effect &&
        sameTarget(rule.target, made.target) &&
        rule.categories.length === made.categories.length &&
        rule.categories.every((category) => made.categories.includes(category)),
    )
  ) {
    return { tone: "warning", text: "You already have this rule." };
  }
  // A never rule already keeping the model from this work wins over the draft. "No work" covers
  // the workers' kinds only, so not a chat, coordinating or commit messages.
  const keepsFrom = (rule: OverrideRule) => {
    if (draft.kind !== "") return keepsIn(rule, draft.kind);
    // Any work: a rule the draft would match on every kind it covers.
    return rule.categories.length === 0 || (draft.effect === "never" && neverAll(rule));
  };
  const never = everywhere.filter((rule) => rule.effect === "never" && covers(rule.target, target));
  const blocking = never.find(keepsFrom);
  if (blocking) {
    const from = neverAll(blocking) ? "everything" : joinWords(blocking.categories.map(kindVerb));
    const title = ruleTitle(blocking, names);
    return {
      tone: "warning",
      text:
        draft.effect === "prefer"
          ? `“${title}” already keeps ${model} away from ${from}. Never rules win, so this rule won't change anything until you remove that one.`
          : `“${title}” already keeps ${model} away from ${from}, so this rule won't change anything.`,
    };
  }
  const ordered = (category: TaskCategory) =>
    rankings.some(
      (ranking) =>
        ranking.category === category &&
        ranking.projectId === null &&
        ranking.areas.length === 0 &&
        ranking.manual &&
        ranking.entries.length > 0,
    );
  if (draft.effect === "prefer" && draft.kind !== "" && ordered(draft.kind)) {
    return {
      tone: "warning",
      text: `${PLAIN_KIND_NAMES[draft.kind]} follows your order, so this rule won't change anything there until you set it back to Automatic.`,
    };
  }
  const route = (category: TaskCategory) => routes?.find((found) => found.category === category) ?? null;
  const picked = (category: TaskCategory) => {
    const outcome = route(category)?.outcome;
    return outcome?.type === "chosen" ? outcome.choice : null;
  };
  const onIt = (category: TaskCategory) => {
    const choice = picked(category);
    return choice !== null && takesIn(target, choice.provider, choice.model ?? "", family);
  };

  if (draft.effect === "prefer") {
    if (draft.kind !== "") {
      const name = PLAIN_KIND_NAMES[draft.kind];
      const choice = picked(draft.kind);
      if (choice && onIt(draft.kind)) return { tone: "info", text: `${name} already goes to ${model}. This rule keeps it first whenever it can run.` };
      const instead = choice ? ` instead of ${names.model(choice.provider, choice.model ?? "")}` : "";
      return { tone: "info", text: `${name} will go to ${model}${instead} whenever ${model} can run. Everything else stays the same.` };
    }
    const moving = EVERY_KIND.filter(
      (category) => !ordered(category) && !onIt(category) && !never.some((rule) => keepsIn(rule, category)),
    );
    if (moving.length === 0) return { tone: "info", text: `Every kind of work already goes to ${model} when it can.` };
    const which = moving.length > 3 ? `${moving.length} kinds of work` : kindNames(moving);
    return { tone: "info", text: `${which} will go to ${model} whenever it can run. Everything else stays the same.` };
  }

  if (draft.kind !== "") {
    const name = PLAIN_KIND_NAMES[draft.kind];
    if (!onIt(draft.kind)) {
      return { tone: "info", text: `${name} doesn't use ${model} right now, so nothing changes yet. It won't be picked for it.` };
    }
    const next = (route(draft.kind)?.candidates ?? []).find(
      (candidate) => candidate.blocked === null && !takesIn(target, candidate.provider, candidate.model, family),
    );
    return {
      tone: "info",
      text: next
        ? `${name} will go to ${names.model(next.provider, next.model)} instead of ${model}. Everything else stays the same.`
        : `${name} will wait until another model can run.`,
    };
  }
  const moving = WORKER_CATEGORIES.filter(onIt);
  if (moving.length === 0) {
    return { tone: "info", text: `Nothing runs on ${model} right now. It won't be given work; you can still pick it yourself in a chat or session.` };
  }
  return {
    tone: "info",
    text: `${kindNames(moving)} will move to the next model. You can still pick ${model} yourself in a chat or session.`,
  };
}
