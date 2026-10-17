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
import type { Factor, OverrideTarget, Ranking, RouteCandidate, TaskCategory } from "@/ipc/generated";
import { VENDOR_LABELS } from "@/lib/routing";
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
