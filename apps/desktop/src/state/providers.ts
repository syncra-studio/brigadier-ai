import type { ModelRef, ProviderKind, Settings } from "@/ipc/generated";
import { editSettings } from "@/state/settings";

/**
 * Which agents and models the user lets Brigadier use (the core's `routing::availability`): an
 * agent switched off (`disabledProviders`) and a model made unavailable (`hiddenModels`) are
 * out of every picker. The Providers page sets these.
 */

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

/** Switches an agent on or off. */
export function setProviderOn(provider: ProviderKind, on: boolean): Promise<Settings> {
  return editSettings((settings) => {
    const others = settings.disabledProviders.filter((entry) => entry !== provider);
    return { ...settings, disabledProviders: on ? others : [...others, provider] };
  });
}

/** Makes a model available (in every picker, and for work) or not: its "Use this model". */
export function setModelAvailable(model: ModelRef, available: boolean): Promise<Settings> {
  return editSettings((settings) => {
    const others = settings.hiddenModels.filter((hidden) => !sameRef(hidden, model));
    return {
      ...settings,
      hiddenModels: available ? others : [...others, { provider: model.provider, id: model.id }],
    };
  });
}
