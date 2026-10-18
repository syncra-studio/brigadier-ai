import {
  Book,
  Branch,
  Chat,
  CheckCircle,
  Code,
  Commit,
  Grid,
  Nodes,
  PullRequestOpen,
  Telescope,
} from "@openai/apps-sdk-ui/components/Icon";
import type { ComponentType, SVGProps } from "react";

import { findModel, type ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import type {
  Factor,
  MergedModel,
  OverrideTarget,
  ProviderKind,
  QualityTier,
  Ranking,
  RouteCandidate,
  RoutePreview,
  TaskCategory,
} from "@/ipc/generated";
import { DEFAULT_FLOORS, joinWords, tierBelow, VENDOR_LABELS } from "@/lib/routing";
import { modelName } from "@/lib/setup";

/*
 * The words and small sums Settings › Routing is drawn with: each kind of work's icon and short
 * head, a place's name without its vendor, a list's summary, and what each part of a model's
 * score is.
 */

/** Every kind of work, in the order Routing lists them. */
export const ROUTED_KINDS: readonly TaskCategory[] = [
  "scout",
  "research",
  "implement",
  "review",
  "merge",
  "verify",
  "operate",
  "chat",
  "orchestrate",
  "gitText",
];

export const KIND_ICONS: Record<TaskCategory, ComponentType<SVGProps<SVGSVGElement>>> = {
  scout: Telescope,
  research: Book,
  implement: Code,
  review: PullRequestOpen,
  merge: Branch,
  verify: CheckCircle,
  operate: Grid,
  chat: Chat,
  orchestrate: Nodes,
  gitText: Commit,
};

/** A kind of work's short head, over the ratings' columns. */
export const KIND_HEADS: Record<TaskCategory, string> = {
  scout: "Explore",
  research: "Research",
  implement: "Code",
  review: "Review",
  merge: "Merge",
  verify: "Checks",
  operate: "Apps",
  chat: "Chat",
  orchestrate: "Coordinate",
  gitText: "Commits",
};

/** The tier a model counts as for a kind of work (a model of some kinds only has its own per kind). */
export function tierFor(model: MergedModel, category: TaskCategory): QualityTier {
  return model.only?.[category] ?? model.tier;
}

/** A model's score for a kind of work, when it takes that kind. */
export function strengthFor(model: MergedModel, category: TaskCategory): number | null {
  if (model.only && !(category in model.only)) return null;
  return model.strengths[category] ?? null;
}

/** Whether a model is shown faded for a kind of work: below its floor, or a legacy model. */
export function faded(model: MergedModel, category: TaskCategory): boolean {
  return model.legacy || tierBelow(tierFor(model, category), DEFAULT_FLOORS[category]);
}

/** A model's own name, without its vendor ("Opus 5.5"). */
export function shortModelName(groups: readonly ModelGroup[], provider: RouteCandidate["provider"], model: string): string {
  return findModel(groups, { provider, model, effort: null })?.displayName ?? modelName(groups, { provider, model, effort: null });
}

/** A place of a list in a few words: "Opus 5.5", "Newest Claude opus", "Any Codex model". */
export function shortPlaceName(target: OverrideTarget, groups: readonly ModelGroup[]): string {
  switch (target.type) {
    case "vendor":
      return `Any ${VENDOR_LABELS[target.provider]} model`;
    case "family":
      return `Newest ${VENDOR_LABELS[target.provider]} ${target.family}`;
    case "model":
      return shortModelName(groups, target.provider, target.id);
  }
}

/** A list in force: "Opus 5.5, then GPT-6". */
export function orderSummary(ranking: Ranking, groups: readonly ModelGroup[]): string {
  return ranking.entries.map((entry) => shortPlaceName(entry.target, groups)).join(", then ");
}

/** The kind of work's own row in a scope (not an area override). */
export function rowRanking(
  rankings: readonly Ranking[],
  category: TaskCategory,
  scope: string | null,
): Ranking | null {
  return (
    rankings.find(
      (ranking) =>
        ranking.category === category && ranking.projectId === scope && ranking.areas.length === 0,
    ) ?? null
  );
}

/** The area overrides of a kind of work in a scope. */
export function areaOverrides(
  rankings: readonly Ranking[],
  category: TaskCategory,
  scope: string | null,
): Ranking[] {
  return rankings.filter(
    (ranking) => ranking.category === category && ranking.projectId === scope && ranking.areas.length > 0,
  );
}

/** Whether a list decides: it's on and has a model. */
export function decides(ranking: Ranking | null): ranking is Ranking {
  return ranking !== null && ranking.manual && ranking.entries.length > 0;
}

/** The parts of a candidate's score the ranking shows as columns; the rest is in its score. */
export type ScoreParts = {
  strength: number | null;
  area: number;
  learned: number;
  quota: number;
  load: number;
};

/** What a factor is, by the words the router gives it. */
function factorKind(factor: Factor): keyof Omit<ScoreParts, "strength"> | null {
  const label = factor.label;
  if (label.startsWith("strength for")) return "area";
  if (label.startsWith("learned here") || label.includes("outcome")) return "learned";
  if (label.includes("projected at") || label.includes("window")) return "quota";
  if (label.includes("already runs")) return "load";
  return null;
}

/** A candidate's score in its parts: its strength first, then what added to it or took from it. */
export function scoreParts(candidate: RouteCandidate): ScoreParts {
  const [strength, ...rest] = candidate.factors;
  const parts: ScoreParts = { strength: strength?.delta ?? null, area: 0, learned: 0, quota: 0, load: 0 };
  for (const factor of rest) {
    const kind = factorKind(factor);
    if (kind) parts[kind] += factor.delta;
  }
  return parts;
}

/** The factor about a candidate's quota, if one took from its score. */
export function quotaFactor(candidate: RouteCandidate): Factor | null {
  return candidate.factors.slice(1).find((factor) => factorKind(factor) === "quota") ?? null;
}

/** The factor about the workers its vendor runs, if one took from its score. */
export function loadFactor(candidate: RouteCandidate): Factor | null {
  return candidate.factors.slice(1).find((factor) => factorKind(factor) === "load") ?? null;
}

/** "+0.40", "−3.80", "0": a part of a score, to two decimals. */
export function formatPart(value: number): string {
  const rounded = Math.round(value * 100) / 100;
  if (rounded === 0) return "0";
  return rounded > 0 ? `+${rounded.toFixed(2)}` : `−${Math.abs(rounded).toFixed(2)}`;
}

const short = (effort: string) => (effort === "medium" ? "med" : effort);

/** "low–high", "low–med": the efforts a model takes, from its lowest to its highest. */
export function effortRange(efforts: readonly string[]): string {
  if (efforts.length === 0) return "—";
  if (efforts.length === 1) return short(efforts[0]!);
  return `${short(efforts[0]!)}–${short(efforts.at(-1)!)}`;
}

/** A length of `steps` spacing units, for a table's column template. */
export function spacing(steps: number): string {
  return `calc(var(--spacing) * ${steps})`;
}

/**
 * A review's pick for each vendor's change, as Automatic reviews go to the other vendor from
 * the author: "Claude's changes → GPT-6 · high, Codex's → Opus 5.5 · high". Null without them.
 */
export function authorPicks(
  route: RoutePreview | null,
  groups: readonly ModelGroup[],
  { effort = true }: { effort?: boolean } = {},
): string | null {
  const picks = route?.byAuthor ?? [];
  if (picks.length === 0) return null;
  return picks
    .map(({ author, outcome }, index) => {
      const whose = `${VENDOR_LABELS[author]}'s${index === 0 ? " changes" : ""}`;
      if (outcome.type === "wait") return `${whose} wait`;
      const model = shortModelName(groups, outcome.choice.provider, outcome.choice.model ?? "");
      return `${whose} → ${model}${effort && outcome.choice.effort ? ` · ${outcome.choice.effort}` : ""}`;
    })
    .join(", ");
}

/**
 * When a review's pick depends on who wrote the change (Automatic reviews go to the other vendor
 * from the author): each picked model's key ("codex/gpt-6") with whose changes it reviews. Null
 * when one model takes every review.
 */
export function reviewers(route: RoutePreview | null): Map<string, ProviderKind[]> | null {
  const chosen = (route?.byAuthor ?? []).flatMap(({ author, outcome }) =>
    outcome.type === "chosen" ? [{ author, key: `${outcome.choice.provider}/${outcome.choice.model ?? ""}` }] : [],
  );
  if (new Set(chosen.map(({ key }) => key)).size < 2) return null;
  const byModel = new Map<string, ProviderKind[]>();
  for (const { author, key } of chosen) byModel.set(key, [...(byModel.get(key) ?? []), author]);
  return byModel;
}

/** "Reviews Codex's changes". */
export function reviewsWords(authors: readonly ProviderKind[]): string {
  return `Reviews ${joinWords(authors.map((author) => `${VENDOR_LABELS[author]}'s`))} changes`;
}
