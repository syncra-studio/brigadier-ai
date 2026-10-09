import type { ModelRef, OverrideRule, ProviderKind, Settings, TaskCategory } from "@/ipc/generated";
import { editSettings } from "@/state/settings";

/**
 * Which agents and models the user lets Brigadier use (the core's `routing::availability`):
 *
 * - An agent switched off (`disabledProviders`) and a model made unavailable (`hiddenModels`)
 *   are out of every picker and get no work at all. The Providers page sets these.
 * - Whether the orchestrator may give a model worker tasks is a `never` rule for the worker
 *   kinds of work only, so a model can still run a chat or a session. The Routing page sets it.
 */

/** The kinds of work the orchestrator hands to workers (not a chat or the orchestrator itself). */
export const WORKER_CATEGORIES: readonly TaskCategory[] = [
  "scout",
  "research",
  "implement",
  "review",
  "merge",
  "verify",
  "operate",
];

/** The part of the settings that says what's available. */
export type Availability = Pick<Settings, "disabledProviders" | "hiddenModels">;

const sameRef = (a: ModelRef, b: ModelRef) => a.provider === b.provider && a.id === b.id;

/** The agent isn't switched off. */
export function providerOn(settings: Availability, provider: ProviderKind): boolean {
  return !settings.disabledProviders.includes(provider);
}

/** The model may be used at all: its agent is on and the model isn't made unavailable. */
export function modelAvailable(settings: Availability, model: ModelRef): boolean {
  return (
    providerOn(settings, model.provider) &&
    !settings.hiddenModels.some((hidden) => sameRef(hidden, model))
  );
}

/** The rule the Routing page's switch writes: `model` gets no worker tasks, anywhere. */
export function isWorkerRule(rule: OverrideRule, model: ModelRef): boolean {
  return (
    rule.effect === "never" &&
    rule.target.type === "model" &&
    sameRef(rule.target, model) &&
    rule.categories.length === WORKER_CATEGORIES.length &&
    WORKER_CATEGORIES.every((category) => rule.categories.includes(category)) &&
    rule.areas.length === 0 &&
    rule.projectId === null
  );
}

/**
 * How the id of a worker rule the core added for a model it just saw starts (the core's
 * `availability::NEW_MODEL_RULE`): Routing tags that model "New".
 */
const NEW_MODEL_RULE = "new-model-";

/** A model Brigadier saw after its agent's first list, not given work until the user allows it. */
export function isNewModel(rules: readonly OverrideRule[], model: ModelRef): boolean {
  return rules.some((rule) => rule.id.startsWith(NEW_MODEL_RULE) && isWorkerRule(rule, model));
}

/** A rule keeping a model from all work everywhere (what the old model switch wrote). */
export function blocksModel(rule: OverrideRule, model: ModelRef): boolean {
  return (
    rule.effect === "never" &&
    rule.target.type === "model" &&
    sameRef(rule.target, model) &&
    rule.categories.length === 0 &&
    rule.areas.length === 0 &&
    rule.projectId === null
  );
}

/** Whether the orchestrator may give `model` worker tasks (Brigadier's own rules aside). */
export function mayWork(rules: readonly OverrideRule[], model: ModelRef): boolean {
  return !rules.some((rule) => isWorkerRule(rule, model) || blocksModel(rule, model));
}

/** Switches an agent on or off. */
export function setProviderOn(provider: ProviderKind, on: boolean): Promise<Settings> {
  return editSettings((settings) => {
    const others = settings.disabledProviders.filter((entry) => entry !== provider);
    return { ...settings, disabledProviders: on ? others : [...others, provider] };
  });
}

/** Makes a model available (in every picker, and on Routing) or not. */
export function setModelAvailable(model: ModelRef, available: boolean): Promise<Settings> {
  return editSettings((settings) => {
    const others = settings.hiddenModels.filter((hidden) => !sameRef(hidden, model));
    return {
      ...settings,
      hiddenModels: available ? others : [...others, { provider: model.provider, id: model.id }],
    };
  });
}

/**
 * Lets the orchestrator give `model` worker tasks, or not. Allowing it also drops a model-wide
 * `never` rule the old switch left, so the switch always does what it shows.
 */
export function setModelWorks(model: ModelRef, works: boolean): Promise<Settings> {
  const rule: OverrideRule = {
    id: crypto.randomUUID(),
    effect: "never",
    target: { type: "model", provider: model.provider, id: model.id },
    categories: [...WORKER_CATEGORIES],
    areas: [],
    projectId: null,
    createdAtMs: Date.now(),
  };
  return editSettings((settings) => {
    const rules = settings.routingOverrides;
    const next = works
      ? rules.filter((existing) => !isWorkerRule(existing, model) && !blocksModel(existing, model))
      : rules.some((existing) => isWorkerRule(existing, model))
        ? rules
        : [...rules, rule];
    return { ...settings, routingOverrides: next };
  });
}
