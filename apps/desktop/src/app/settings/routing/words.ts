import { findModel, type ModelGroup } from "@/components/shared/model-selector";
import type {
  GuidanceRow,
  MergedModel,
  ModelInfo,
  ModelRules,
  OverrideTarget,
  ProviderKind,
  QualityTier,
  TaskCategory,
} from "@/ipc/generated";
import { VENDOR_LABELS } from "@/lib/routing";

/*
 * The words and small sums Settings › Routing is drawn with: the rows of the rules file merged
 * over the shipped ones, a row's model in a few words, and a model's tier and strength for a
 * kind of work.
 */

/** Every kind of work the models' strength is rated for, in the order Routing lists them. */
export const ROUTED_KINDS: readonly TaskCategory[] = [
  "scout",
  "research",
  "implement",
  "review",
  "merge",
  "verify",
  "operate",
  "design",
  "chat",
  "orchestrate",
  "gitText",
];

/**
 * A kind of work in Who does what: its rows in force (an order: the first that can run takes
 * it), the shipped ones and the user's own (empty: Automatic, the shipped rows).
 */
export type WorkRow = { work: string; rows: GuidanceRow[]; shipped: GuidanceRow[]; mine: GuidanceRow[] };

/** The kinds of work: the shipped ones in order, each in force as the user's order if they set one; then the user's own kinds. */
export function workRows(rules: ModelRules | null): WorkRow[] {
  if (!rules) return [];
  const works = [...new Set([...rules.shipped, ...rules.rows].map((row) => row.work))];
  return works.map((work) => {
    const shipped = rules.shipped.filter((row) => row.work === work);
    const mine = rules.rows.filter((row) => row.work === work);
    return { work, rows: mine.length > 0 ? mine : shipped, shipped, mine };
  });
}

/** The user's rows with `work`'s order replaced by `order`, in the place of its first row (a new kind's last; empty drops it). */
export function withOrder(rows: readonly GuidanceRow[], work: string, order: readonly GuidanceRow[]): GuidanceRow[] {
  const at = rows.findIndex((row) => row.work === work);
  const rest = rows.filter((row) => row.work !== work);
  const place = at < 0 ? rest.length : rows.slice(0, at).filter((row) => row.work !== work).length;
  return [...rest.slice(0, place), ...order, ...rest.slice(place)];
}

/**
 * A kind of work as Routing names it: as written, but a fusion's consult row (`fusion-claude`,
 * the name `brigadier start --work` takes) in plain words ("Fusion · Claude consult").
 */
export function workName(work: string): string {
  const agent = /^fusion-(.+)$/.exec(work)?.[1];
  if (!agent) return work;
  return `Fusion · ${VENDOR_LABELS[agent as ProviderKind] ?? capital(agent)} consult`;
}

/** "Opus" from "opus": a family word as a name. */
export const capital = (word: string) => word.charAt(0).toUpperCase() + word.slice(1);

/** A model id's version numbers, as the engine reads them: "gpt-6.1-sol" is [6, 1]. */
const versionOf = (id: string) =>
  id
    .split(/[^0-9]+/)
    .filter((part) => part !== "")
    .map(Number)
    .filter((number) => number < 1000);

/** Which of two versions is higher, the way Rust orders them: number by number, a prefix first. */
function compareVersions(a: readonly number[], b: readonly number[]): number {
  for (let index = 0; index < Math.min(a.length, b.length); index++) {
    if (a[index] !== b[index]) return a[index]! - b[index]!;
  }
  return a.length - b.length;
}

/**
 * The listed model a row's model word runs on, as the engine picks it (`listed_model`): a listed
 * id or alias as it is; else a family word's newest model, the one of the highest version among
 * the current models with the word as a part of their id (the last of equals); null when none.
 */
export function rowModel(models: readonly ModelInfo[], word: string): ModelInfo | null {
  const lower = word.toLowerCase();
  const listed = models.find((model) => model.id.toLowerCase() === lower || model.resolved?.toLowerCase() === lower);
  if (listed) return listed;
  let newest: ModelInfo | null = null;
  for (const model of models) {
    if (model.legacy || !model.id.split(/[-_ ]/).some((part) => part.toLowerCase() === lower)) continue;
    if (!newest || compareVersions(versionOf(model.id), versionOf(newest.id)) >= 0) newest = model;
  }
  return newest;
}

/**
 * A row's model in a few words: the listed model it runs on ("Opus 5.5", a family word's newest
 * model too), the agent's default ("Codex's default"), or the word as written when nothing listed
 * matches it ("Opus").
 */
export function rowModelName(groups: readonly ModelGroup[], agent: ProviderKind, model: string): string {
  if (model === "default") return `${VENDOR_LABELS[agent]}'s default`;
  const models = groups.find((group) => group.provider === agent)?.models ?? [];
  return (findModel(groups, { provider: agent, model, effort: null }) ?? rowModel(models, model))?.displayName ?? capital(model);
}

/** The target a row's model is, for the model menus: a listed id, the CLI's default, or a family. */
export function rowTarget(groups: readonly ModelGroup[], agent: ProviderKind, model: string): OverrideTarget {
  if (model === "default") return { type: "vendor", provider: agent };
  return findModel(groups, { provider: agent, model, effort: null })
    ? { type: "model", provider: agent, id: model }
    : { type: "family", provider: agent, family: model };
}

/** What a row names as its model, from a model menu's pick. */
export function targetModel(target: OverrideTarget): string {
  switch (target.type) {
    case "vendor":
      return "default";
    case "family":
      return target.family;
    case "model":
      return target.id;
  }
}

/** A model of a rule or a row in a few words: "Opus 5.5" (a family as its newest model), "Claude's default". */
export function shortPlaceName(target: OverrideTarget, groups: readonly ModelGroup[]): string {
  switch (target.type) {
    case "vendor":
      return `${VENDOR_LABELS[target.provider]}'s default`;
    case "family":
      return rowModelName(groups, target.provider, target.family);
    case "model":
      return rowModelName(groups, target.provider, target.id);
  }
}

/** The tier a model counts as for a kind of work (a model of some kinds only has its own per kind). */
export function tierFor(model: MergedModel, category: TaskCategory): QualityTier {
  return model.only?.[category] ?? model.tier;
}

/** A model's score for a kind of work, when it takes that kind. */
export function strengthFor(model: MergedModel, category: TaskCategory): number | null {
  if (model.only && !(category in model.only)) return null;
  return model.strengths[category] ?? null;
}

/**
 * The models ranked for a kind of work, strongest first; an unrated model's score is only a
 * placeholder, so the unrated come last, in the order given.
 */
export function rankedFor<M extends { model: MergedModel; strength: number }>(models: readonly M[], category: TaskCategory): M[] {
  const unrated = (entry: M) => tierFor(entry.model, category) === "unrated";
  return models.toSorted((a, b) => Number(unrated(a)) - Number(unrated(b)) || (unrated(a) ? 0 : b.strength - a.strength));
}

/** Whether a model is shown faded: a legacy model. */
export function faded(model: MergedModel): boolean {
  return model.legacy;
}
