import { findModel, type ModelGroup } from "@/components/shared/model-selector";
import type {
  GuidanceRow,
  MergedModel,
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

/** A kind of work in Who does what: its row in force, the shipped one and the user's own. */
export type WorkRow = { work: string; row: GuidanceRow; shipped: GuidanceRow | null; mine: GuidanceRow | null };

/** The rows in force: the shipped ones in order, each replaced by the user's row of the same work; then the user's own kinds. */
export function workRows(rules: ModelRules | null): WorkRow[] {
  if (!rules) return [];
  const mine = (work: string) => rules.rows.find((row) => row.work === work) ?? null;
  const shipped = rules.shipped.map((row) => ({ work: row.work, row: mine(row.work) ?? row, shipped: row, mine: mine(row.work) }));
  const own = rules.rows
    .filter((row) => !rules.shipped.some((found) => found.work === row.work))
    .map((row) => ({ work: row.work, row, shipped: null, mine: row }));
  return [...shipped, ...own];
}

/** "Opus" from "opus": a family word as a name. */
const capital = (word: string) => word.charAt(0).toUpperCase() + word.slice(1);

/** A row's model in a few words: a listed model's name ("Opus 5.5"), a family ("Opus"), or "Default model". */
export function rowModelName(groups: readonly ModelGroup[], agent: ProviderKind, model: string): string {
  if (model === "default") return "Default model";
  return findModel(groups, { provider: agent, model, effort: null })?.displayName ?? capital(model);
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

/** A model of a rule or a row in a few words: "Opus 5.5", "Newest Claude opus", "Claude's default". */
export function shortPlaceName(target: OverrideTarget, groups: readonly ModelGroup[]): string {
  switch (target.type) {
    case "vendor":
      return `${VENDOR_LABELS[target.provider]}'s default`;
    case "family":
      return `Newest ${VENDOR_LABELS[target.provider]} ${target.family}`;
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

/** Whether a model is shown faded: a legacy model. */
export function faded(model: MergedModel): boolean {
  return model.legacy;
}
