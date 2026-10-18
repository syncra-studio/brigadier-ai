import { findModel, type ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import type {
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
 * The words and small sums Settings › Routing is drawn with: a model's tier and strength for a
 * kind of work, a place's name without its vendor, a list's summary and a review's pick for each
 * vendor's change.
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

/** Whether a list decides: it's on and has a model. */
export function decides(ranking: Ranking | null): ranking is Ranking {
  return ranking !== null && ranking.manual && ranking.entries.length > 0;
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
