import type { OverrideRule, OverrideTarget, ProviderKind, Ranking, RoutePreview, TaskCategory } from "@/ipc/generated";
import { CATEGORIES, joinWords, PLAIN_KIND_NAMES, sameTarget } from "@/lib/routing";
import { isNoWorkRule, noWorkRule, WORKER_CATEGORIES } from "@/lib/workRules";

/*
 * The Rules card's words and sums: a rule's title and line, the rule a draft makes, and what a
 * draft would change before it is added (or why it would change nothing).
 */

/** What a new rule does: keeps a model away, or puts it first. */
export type DraftEffect = "never" | "prefer";

/** A rule being written: what it does, to which model, for which kinds of work (none: any work). */
export type Draft = { effect: DraftEffect; target: OverrideTarget | null; kinds: readonly TaskCategory[] };

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

/** The "for" menu's words for the kinds a draft names: "Any work", "Run checks and Write commit messages". */
export function draftKindsName(kinds: readonly TaskCategory[]): string {
  if (kinds.length === 0) return "Any work";
  return kinds.length > 3 ? `${kinds.length} kinds of work` : kindNames(kinds);
}

/** "Write code doesn't", "Run checks and Write commit messages don't": a verb fitted to how many kinds. */
const kindsDo = (kinds: readonly TaskCategory[], one: string, several: string) => `${kindNames(kinds)} ${kinds.length === 1 ? one : several}`;

/** Every kind of work a rule naming no kind applies to. */
const EVERY_KIND: readonly TaskCategory[] = [...CATEGORIES, "gitText"];

/** The kinds a draft names once `kind` is ticked or unticked, in Routing's order; none is any work. */
export function toggleKind(kinds: readonly TaskCategory[], kind: TaskCategory): TaskCategory[] {
  const next = kinds.includes(kind) ? kinds.filter((found) => found !== kind) : [...kinds, kind];
  return EVERY_KIND.filter((found) => next.includes(found));
}

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
  if (draft.effect === "never" && draft.kinds.length === 0) return noWorkRule(draft.target, id, createdAtMs);
  return {
    id,
    effect: draft.effect,
    target: draft.target,
    categories: [...draft.kinds],
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
  const kinds = draft.kinds;
  const keepsFrom = (rule: OverrideRule) => {
    if (kinds.length > 0) return kinds.every((category) => keepsIn(rule, category));
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
  const route = (category: TaskCategory) => routes?.find((found) => found.category === category) ?? null;
  const picked = (category: TaskCategory) => {
    const outcome = route(category)?.outcome;
    return outcome?.type === "chosen" ? outcome.choice : null;
  };
  const onIt = (category: TaskCategory) => {
    const choice = picked(category);
    return choice !== null && takesIn(target, choice.provider, choice.model ?? "", family);
  };
  // The kinds no never rule already keeps the model from: those the draft still changes.
  const open = kinds.filter((category) => !never.some((rule) => keepsIn(rule, category)));
  if (kinds.length > 0 && open.length === 0) {
    const from = joinWords(kinds.map(kindVerb));
    return {
      tone: "warning",
      text:
        draft.effect === "prefer"
          ? `Your rules already keep ${model} away from ${from}. Never rules win, so this rule won't change anything until you remove them.`
          : `Your rules already keep ${model} away from ${from}, so this rule won't change anything.`,
    };
  }

  if (draft.effect === "prefer") {
    if (kinds.length > 0) {
      const free = open.filter((category) => !ordered(category));
      if (free.length === 0) {
        return {
          tone: "warning",
          text: `${kindsDo(open, "follows", "follow")} your order, so this rule won't change anything there until you set it back to Automatic.`,
        };
      }
      // Kinds that follow an order are said apart.
      const followed = open.filter(ordered);
      const rest = followed.length > 0 ? ` ${kindsDo(followed, "follows", "follow")} your order instead.` : "";
      const moving = free.filter((category) => !onIt(category));
      if (moving.length === 0) {
        return free.length === 1
          ? { tone: "info", text: `${kindNames(free)} already goes to ${model}. This rule keeps it first whenever it can run.${rest}` }
          : { tone: "info", text: `${kindNames(free)} already go to ${model}. This rule keeps them first whenever it can run.${rest}` };
      }
      if (moving.length === 1) {
        const [choice] = moving.map(picked);
        const instead = choice ? ` instead of ${names.model(choice.provider, choice.model ?? "")}` : "";
        return { tone: "info", text: `${kindNames(moving)} will go to ${model}${instead} whenever ${model} can run.${rest} Everything else stays the same.` };
      }
      return { tone: "info", text: `${kindNames(moving)} will go to ${model} whenever it can run.${rest} Everything else stays the same.` };
    }
    const moving = EVERY_KIND.filter(
      (category) => !ordered(category) && !onIt(category) && !never.some((rule) => keepsIn(rule, category)),
    );
    if (moving.length === 0) return { tone: "info", text: `Every kind of work already goes to ${model} when it can.` };
    const which = moving.length > 3 ? `${moving.length} kinds of work` : kindNames(moving);
    return { tone: "info", text: `${which} will go to ${model} whenever it can run. Everything else stays the same.` };
  }

  if (kinds.length > 0) {
    const on = open.filter(onIt);
    const off = open.filter((category) => !onIt(category));
    if (on.length === 0) {
      return {
        tone: "info",
        text: `${kindsDo(off, "doesn't", "don't")} use ${model} right now, so nothing changes yet. It won't be picked for ${off.length === 1 ? "it" : "them"}.`,
      };
    }
    const next = (category: TaskCategory) =>
      (route(category)?.candidates ?? []).find(
        (candidate) => candidate.blocked === null && !takesIn(target, candidate.provider, candidate.model, family),
      ) ?? null;
    const nexts = on.map(next);
    const first = nexts[0];
    const same = nexts.every((candidate) => candidate?.provider === first?.provider && candidate?.model === first?.model);
    const rest = off.length > 0 ? ` ${kindsDo(off, "doesn't", "don't")} use ${model} right now.` : "";
    if (same && !first) return { tone: "info", text: `${kindNames(on)} will wait until another model can run.${rest}` };
    const moves = same && first
      ? `${kindNames(on)} will go to ${names.model(first.provider, first.model)} instead of ${model}.`
      : `${kindNames(on)} will move from ${model} to the next model.`;
    return { tone: "info", text: `${moves}${rest} Everything else stays the same.` };
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
