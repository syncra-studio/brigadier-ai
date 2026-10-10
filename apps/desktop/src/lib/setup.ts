import { useEffect, useMemo } from "react";

import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import {
  effortFor,
  findModel,
  type ModelGroup,
} from "@/components/assistant-ui/elements/model-selector";
import type {
  Environment,
  ModelChoice,
  ModelInfo,
  PermissionLevel,
  Project,
  ProviderOverview,
  Settings,
  Setup,
} from "@/ipc/generated";
import { loadProviders } from "@/state/actions";
import { type Availability, modelAvailable, providerOn } from "@/state/providers";
import { useApp } from "@/state/store";

export const PERMISSION_LEVELS: readonly PermissionLevel[] = [
  "askForApproval",
  "approveForMe",
  "fullAccess",
];

export const PERMISSION_LABELS: Record<PermissionLevel, string> = {
  askForApproval: "Ask for approval",
  approveForMe: "Approve for me",
  fullAccess: "Full access",
};

/** One line each. */
export const PERMISSION_DETAILS: Record<PermissionLevel, string> = {
  askForApproval: "Ask before plans, landings and each step outside the sandbox",
  approveForMe: "Only ask what only you can answer",
  fullAccess: "Workers run like your own terminal, without asking",
};

/** The permission menu's line in a folder the user doesn't trust. */
export const UNTRUSTED_NOTE =
  "This folder isn't trusted, so sessions here ask first. Change it in the project's settings.";

/** The README's section on permission levels ("Learn more"). */
export const PERMISSIONS_HELP_URL =
  "https://github.com/stephen-golban/brigadier-ai#permission-levels";

/** At every level workers leave pushes, deploys and publishing to the user. */
export const NEVER_PUSHES_NOTE = "Workers never push, deploy or publish; you start those.";

/** The Full access confirmation's line on what workers may do. */
export const FULL_ACCESS_NOTE =
  "Workers run commands like your own terminal, without asking. They never push, deploy or publish on their own: you start those.";

function unavailable(overview: ProviderOverview): string | null {
  const status = overview.status;
  if (!status) return null;
  if (!status.path) return "not installed";
  if (!status.loggedIn) return "not logged in";
  return null;
}

function groupsOf(providers: readonly ProviderOverview[]): ModelGroup[] {
  return providers.map((overview) => ({
    provider: overview.provider,
    label: PROVIDER_LABELS[overview.provider],
    models: overview.models?.models ?? [],
    unavailable: unavailable(overview),
  }));
}

const NO_GROUPS: ModelGroup[] = [];

/** The live model lists of the installed CLIs, loaded on first use. */
export function useModelGroups(): ModelGroup[] {
  const providers = useApp((s) => s.providers.view?.providers);
  const connected = useApp((s) => s.connection.status === "connected");
  useEffect(() => {
    if (providers || !connected) return;
    void loadProviders().catch((error: unknown) => console.error("loading providers failed", error));
  }, [providers, connected]);
  return useMemo(() => (providers ? groupsOf(providers) : NO_GROUPS), [providers]);
}

/**
 * The groups with only what the user made available on Providers: an agent switched off is
 * left out, and so is each model made unavailable, and an agent with none of its listed models
 * left. An agent whose list isn't read yet stays, with no models.
 */
export function availableGroups(
  groups: readonly ModelGroup[],
  settings: Availability,
): ModelGroup[] {
  return groups
    .filter((group) => providerOn(settings, group.provider))
    .map((group) => ({
      ...group,
      models: group.models.filter((model) =>
        modelAvailable(settings, { provider: group.provider, id: model.id }),
      ),
    }))
    .filter(
      (group) =>
        group.models.length > 0 ||
        !groups.some((entry) => entry.provider === group.provider && entry.models.length > 0),
    );
}

/**
 * Nothing can start a conversation: every agent is switched off or has none of its listed
 * models available. An agent on whose list isn't read yet may still run its default model.
 */
export function nothingAvailable(listed: readonly ModelGroup[], available: readonly ModelGroup[]): boolean {
  return listed.length > 0 && available.length === 0;
}

/**
 * The live model lists with only the available models: what every picker offers. Names of
 * past work come from [`useModelGroups`], which keeps everything.
 */
export function useAvailableModelGroups(): ModelGroup[] {
  const groups = useModelGroups();
  const disabledProviders = useApp((s) => s.settings.disabledProviders);
  const hiddenModels = useApp((s) => s.settings.hiddenModels);
  return useMemo(
    () => availableGroups(groups, { disabledProviders, hiddenModels }),
    [groups, disabledProviders, hiddenModels],
  );
}

/**
 * `groups` with `choice`'s model put back from `all` when it isn't available any more, so a
 * running conversation's picker still names the model it runs on.
 */
export function withChoice(
  groups: readonly ModelGroup[],
  all: readonly ModelGroup[],
  choice: ModelChoice,
): readonly ModelGroup[] {
  const model = findModel(groups, choice) ? null : findModel(all, choice);
  const group = all.find((entry) => entry.provider === choice.provider);
  if (!model || !group) return groups;
  const shown = groups.find((entry) => entry.provider === choice.provider);
  return shown
    ? groups.map((entry) => (entry === shown ? { ...entry, models: [...entry.models, model] } : entry))
    : [...groups, { ...group, models: [model] }];
}

/** Whether a Chat's CLI can compact its context on request; a session's never does. */
export function useCanCompact(setup: Setup | null | undefined): boolean {
  return useApp(
    (s) =>
      setup?.type === "chat" &&
      (s.providers.view?.providers.find((overview) => overview.provider === setup.model.provider)
        ?.status?.compacts ??
        false),
  );
}

/** Whether a name has "fable" as a word, as routing reads model names. */
function namesFable(name: string): boolean {
  return name.split(/[^A-Za-z0-9.]+/).some((word) => word.toLowerCase() === "fable");
}

/** A Fable model: routing never uses one, so it is never offered for a ranking. */
export function isFable(model: ModelInfo): boolean {
  return namesFable(model.id) || namesFable(model.displayName) || (model.resolved !== null && namesFable(model.resolved));
}

/**
 * An agent's own default model, else its first current one that isn't Fable (a Fable model is
 * only used when picked), else its first one.
 */
function agentDefault(group: ModelGroup): ModelInfo | undefined {
  return (
    group.models.find((entry) => entry.isDefault) ??
    group.models.find((entry) => !entry.legacy && !isFable(entry)) ??
    group.models.find((entry) => !entry.legacy) ??
    group.models[0]
  );
}

/** The first ready provider's default model: the last step of the resolution order. */
export function builtInDefault(groups: readonly ModelGroup[]): ModelChoice {
  const ready = groups.find((group) => group.unavailable === null && group.models.length > 0);
  const model = ready && agentDefault(ready);
  if (!ready || !model) return { provider: "claude", model: null, effort: null };
  return { provider: ready.provider, model: model.id, effort: model.defaultEffort };
}

/**
 * A saved choice as it can be used now, given the available `groups`: kept when its model is
 * there (or its agent's list isn't known yet), else that agent's default model; `null` when
 * the agent is switched off or has no model left, so the next choice in line is tried.
 */
function availableChoice(groups: readonly ModelGroup[], choice: ModelChoice): ModelChoice | null {
  const group = groups.find((entry) => entry.provider === choice.provider);
  if (!group) return null;
  if (group.models.length === 0 || findModel(groups, choice)) return choice;
  const model = agentDefault(group);
  return model ? { provider: group.provider, model: model.id, effort: model.defaultEffort } : null;
}

/**
 * The model a new conversation starts with: the session choice, then the project's remembered
 * choice, then the global default in Settings (a Chat's own default first), then the first
 * ready provider's default. `groups` are the available ones: a saved choice that isn't
 * available any more stands for its agent's default model (the saved preference stays).
 */
export function resolveModel(
  explicit: ModelChoice | null,
  kind: "session" | "chat",
  project: Project | null,
  settings: Settings,
  groups: readonly ModelGroup[],
): ModelChoice {
  const chain = [
    explicit,
    kind === "session" ? project?.prefs.orchestrator : settings.defaultChatModel,
    settings.defaultOrchestrator,
  ];
  for (const choice of chain) {
    const usable = choice ? availableChoice(groups, choice) : null;
    if (usable) return completeChoice(groups, usable);
  }
  return completeChoice(groups, builtInDefault(groups));
}

/**
 * A choice with its model and effort spelled out ("the CLI's default" becomes the model and
 * effort the CLI would pick), so what the picker shows is what the conversation runs with.
 * Left as is while the provider's model list is unknown.
 */
export function completeChoice(groups: readonly ModelGroup[], choice: ModelChoice): ModelChoice {
  const model = findModel(groups, choice);
  if (!model) return choice;
  return { ...choice, model: model.id, effort: effortFor(model, choice.effort) };
}

/**
 * A new session's permission level: the draft's own pick, else the default from Settings >
 * Configuration. Projects don't remember one (an earlier version's remembered level is ignored).
 */
export function resolvePermission(explicit: PermissionLevel | null, settings: Settings): PermissionLevel {
  return explicit ?? settings.defaultPermission;
}

/** "Local checkout · main" or "New worktree · brigadier/x from main". */
export function environmentLabel(environment: Environment): string {
  return environment.type === "localCheckout"
    ? `Local checkout · ${environment.branch}`
    : `New worktree · ${environment.branch} from ${environment.base}`;
}

/** "Opus 5.5" for a choice, from the live lists (the raw id when unknown). */
export function modelName(groups: readonly ModelGroup[], choice: ModelChoice): string {
  return (
    findModel(groups, choice)?.displayName ??
    (choice.model && choice.model !== "default" ? choice.model : null) ??
    `${PROVIDER_LABELS[choice.provider]} default`
  );
}

/** Whether two choices name the same model, an alias and the id it resolves to included. */
export function sameModel(groups: readonly ModelGroup[], a: ModelChoice, b: ModelChoice): boolean {
  if (a.provider !== b.provider) return false;
  if (a.model === b.model) return true;
  const model = findModel(groups, a);
  return model !== null && model === findModel(groups, b);
}

/** How an agent stands, in a few words: "Signed in with chatgpt · Pro plan", "Not installed". */
export function providerStatusText(overview: ProviderOverview | undefined): string {
  const status = overview?.status;
  if (!status) return "Checking…";
  if (!status.path) return "Not installed";
  if (!status.loggedIn) return "Not signed in";
  const method = status.authMethod ? `Signed in with ${status.authMethod}` : "Signed in";
  const plan = status.plan ? ` · ${status.plan.charAt(0).toUpperCase()}${status.plan.slice(1)} plan` : "";
  return method + plan;
}
