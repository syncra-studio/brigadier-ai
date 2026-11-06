import { type ReactNode, useEffect, useMemo, useState } from "react";

import { type ListedModel, pickRefusal, ruleNames } from "@/app/settings/routing/ruleWords";
import { rowModel } from "@/app/settings/routing/words";
import { effortFor, ModelPicker, type ModelPickerProps } from "@/components/shared";
import type { GuidanceRow, ModelInfo, ModelRule, OverrideTarget, ProviderKind } from "@/ipc/generated";
import { useAvailableModelGroups, useModelGroups } from "@/lib/setup";
import { loadUsage, useUsage } from "@/state/usage";
import { cliFacts } from "@/lib/providerKinds";

/*
 * The models a row or a rule may name, in the model picker.
 */

/** The models the agents list now, as the rules judge them: which is each agent's default, at what effort, from its own list. */
export function useListedModels(): ListedModel[] {
  const models = useUsage((s) => s.view?.models);
  const groups = useModelGroups();
  useEffect(() => {
    if (!useUsage.getState().view) void loadUsage();
  }, []);
  return useMemo(
    () =>
      (models ?? []).map((model) => {
        const own = groups.find((group) => group.provider === model.provider)?.models.find((info) => info.id === model.id);
        return {
          provider: model.provider,
          id: model.id,
          resolved: model.resolved,
          name: model.displayName,
          isDefault: own?.isDefault ?? false,
          defaultEffort: own?.defaultEffort ?? null,
          legacy: model.legacy,
        };
      }),
    [models, groups],
  );
}

/** Why a model already in the order can't be added again. */
const IN_THE_ORDER = "In the order";

/** A row's model and effort, as the picker it opens from shows them. */
export type ChosenRow = Pick<GuidanceRow, "agent" | "model" | "effort">;

/** A model picker's parts for the models a row or a rule may name. */
type TargetPanel = Pick<ModelPickerProps, "groups" | "checked" | "opensOn" | "locked" | "onPick" | "agentDefault" | "greyed" | "effort" | "refused">;

/**
 * The picker for the models a row or a rule may name: each available model, and the agent's own
 * default (`vendors` false leaves that out). What `rules` keep out for `work` shows greyed with
 * their words, and those already `taken` with "In the order"; a taken family greys the model it
 * runs on now. With `chosen`, that row is checked, whatever `taken` says, and `onEffort` sets its
 * effort from the levels its model lists.
 */
export function useTargetPanel({
  onPick,
  taken = [],
  vendors = true,
  opensOn,
  provider: roleProvider,
  chosen,
  onEffort,
  work,
  rules,
}: {
  /** The model picked, with the level pressed to pick it (none: its own default). */
  onPick: (target: OverrideTarget, effort: string | null) => void;
  taken?: readonly OverrideTarget[];
  vendors?: boolean;
  /** The agent it opens on (the first otherwise). */
  opensOn?: ProviderKind;
  /** A role belongs to one agent; offer only that agent's models. */
  provider?: ProviderKind;
  chosen?: ChosenRow | null | undefined;
  onEffort?: ((effort: string) => void) | undefined;
  /** The kind of work and the rules whose limits grey its levels, with their words. */
  work?: string | undefined;
  rules?: readonly ModelRule[] | undefined;
}): TargetPanel {
  // Only what the user made available is offered; a family resolves on the whole list.
  const groups = useAvailableModelGroups();
  const all = useModelGroups();
  const modelsOf = (provider: ProviderKind) => all.find((group) => group.provider === provider)?.models ?? [];
  const takes = (provider: ProviderKind, model: ModelInfo | null) =>
    taken.some((target) => {
      if (target.provider !== provider) return false;
      if (target.type === "vendor") return model === null;
      if (!model) return false;
      if (target.type === "model") return target.id === model.id;
      return rowModel(modelsOf(provider), target.family)?.id === model.id;
    });
  const chosenModel = chosen && chosen.model !== "default" ? rowModel(modelsOf(chosen.agent), chosen.model) : null;
  // An agent's own default row has the levels of the model it runs.
  const levelModel = chosenModel ?? (chosen?.model === "default" ? modelsOf(chosen.agent).find((listed) => listed.isDefault) : null);
  const checked = chosen ? { provider: chosen.agent, model: chosen.model === "default" ? null : (chosenModel?.id ?? chosen.model) } : null;
  const names = ruleNames(all);
  return {
    groups: roleProvider ? groups.filter((group) => group.provider === roleProvider) : groups,
    checked,
    opensOn,
    locked: Boolean(roleProvider),
    agentDefault: vendors ? (provider) => cliFacts(provider).vendor : undefined,
    greyed: (provider, model) => {
      // What the rules keep out, as the composer shows it; an agent's own default by the model it runs.
      const judged = model ?? modelsOf(provider).find((listed) => listed.isDefault);
      const rule = rules && judged ? pickRefusal(rules, names, provider, judged, null, work ?? null) : null;
      if (rule) return rule;
      return takes(provider, model) && !(checked?.provider === provider && checked.model === (model?.id ?? null)) ? IN_THE_ORDER : null;
    },
    effort: chosen && levelModel && onEffort ? { value: effortFor(levelModel, chosen.effort), onChange: onEffort } : undefined,
    refused: rules && ((provider, model, level) => pickRefusal(rules, names, provider, model, level, work ?? null)),
    onPick: (provider, model, also) =>
      onPick(model ? { type: "model", provider, id: model.id } : { type: "vendor", provider }, also?.effort ?? null),
  };
}

/** The target picker in its popover, opened from `children` (the trigger, as it is). */
export function TargetPicker({
  children,
  label,
  align = "start",
  onClosed,
  ...target
}: Parameters<typeof useTargetPanel>[0] & {
  children: ReactNode;
  label: string;
  align?: "start" | "end";
  onClosed?: (() => void) | undefined;
}) {
  const [open, setOpen] = useState(false);
  return (
    <ModelPicker
      label={label}
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) onClosed?.();
      }}
      align={align}
      {...useTargetPanel(target)}
    >
      {children}
    </ModelPicker>
  );
}
