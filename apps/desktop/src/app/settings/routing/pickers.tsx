import { type ReactNode, useEffect, useMemo, useState } from "react";

import type { ListedModel } from "@/app/settings/routing/ruleWords";
import { rowModel } from "@/app/settings/routing/words";
import { ModelPicker, type ModelPickerProps } from "@/components/shared";
import type { ModelInfo, OverrideTarget, ProviderKind } from "@/ipc/generated";
import { VENDOR_LABELS } from "@/lib/routing";
import { useAvailableModelGroups, useModelGroups } from "@/lib/setup";
import { loadUsage, useUsage } from "@/state/usage";

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
        };
      }),
    [models, groups],
  );
}

/** Why a model already in the order can't be added again. */
const IN_THE_ORDER = "In the order";

/** A model picker's parts for the models a row or a rule may name (`ModelPicker`, or its panel in a menu). */
type TargetPanel = Pick<ModelPickerProps, "groups" | "checked" | "opensOn" | "onPick" | "agentDefault" | "greyed">;

/**
 * The picker for the models a row or a rule may name: each available model, and the agent's own
 * default (`vendors` false leaves that out). Those already `taken` show greyed, "In the order";
 * a taken family greys the model it runs on now.
 */
export function useTargetPanel({
  onPick,
  taken = [],
  vendors = true,
  opensOn,
}: {
  onPick: (target: OverrideTarget) => void;
  taken?: readonly OverrideTarget[];
  vendors?: boolean;
  /** The agent it opens on (the first otherwise). */
  opensOn?: ProviderKind;
}): TargetPanel {
  // Only what the user made available is offered; a family resolves on the whole list.
  const groups = useAvailableModelGroups();
  const all = useModelGroups();
  const takes = (provider: ProviderKind, model: ModelInfo | null) =>
    taken.some((target) => {
      if (target.provider !== provider) return false;
      if (target.type === "vendor") return model === null;
      if (!model) return false;
      if (target.type === "model") return target.id === model.id;
      const models = all.find((group) => group.provider === provider)?.models ?? [];
      return rowModel(models, target.family)?.id === model.id;
    });
  return {
    groups,
    checked: null,
    opensOn,
    agentDefault: vendors ? (provider) => VENDOR_LABELS[provider] : undefined,
    greyed: (provider, model) => (takes(provider, model) ? IN_THE_ORDER : null),
    onPick: (provider, model) =>
      onPick(model ? { type: "model", provider, id: model.id } : { type: "vendor", provider }),
  };
}

/** The target picker in its popover, opened from `children` (the trigger, as it is). */
export function TargetPicker({
  children,
  label,
  align = "start",
  ...target
}: Parameters<typeof useTargetPanel>[0] & { children: ReactNode; label: string; align?: "start" | "end" }) {
  const [open, setOpen] = useState(false);
  return (
    <ModelPicker label={label} open={open} onOpenChange={setOpen} align={align} {...useTargetPanel(target)}>
      {children}
    </ModelPicker>
  );
}
