import { useEffect, useMemo } from "react";

import { DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator } from "@/components/overlays";
import type { ListedModel } from "@/app/settings/routing/ruleWords";
import { capital } from "@/app/settings/routing/words";
import type { OverrideTarget } from "@/ipc/generated";
import { PROVIDERS, VENDOR_LABELS } from "@/lib/routing";
import { useAvailableModelGroups } from "@/lib/setup";
import { providerOn } from "@/state/providers";
import { useApp } from "@/state/store";
import { loadUsage, useUsage } from "@/state/usage";

/*
 * The models a row or a rule may name, as menu items.
 */

/** Whether two picks name the same model. */
export function sameTarget(a: OverrideTarget, b: OverrideTarget): boolean {
  if (a.type !== b.type || a.provider !== b.provider) return false;
  return a.type === "model" ? a.id === (b as typeof a).id : a.type === "family" ? a.family === (b as typeof a).family : true;
}

/** The models the agents list now, as the rules judge them. */
export function useListedModels(): ListedModel[] {
  const models = useUsage((s) => s.view?.models);
  return useMemo(
    () => (models ?? []).map((model) => ({ provider: model.provider, id: model.id, resolved: model.resolved, name: model.displayName })),
    [models],
  );
}

/** Each provider's family words, from the merged model list (read on opening). */
export function useFamilies(): { families: Map<string, string[]>; error: string | null } {
  const models = useUsage((s) => s.view?.models);
  const error = useUsage((s) => s.error);
  useEffect(() => {
    if (!useUsage.getState().view) void loadUsage();
  }, []);
  const families = useMemo(() => {
    const byProvider = new Map<string, string[]>();
    for (const model of models ?? []) {
      if (!model.family || model.excluded) continue;
      const list = byProvider.get(model.provider) ?? [];
      if (!list.includes(model.family)) list.push(model.family);
      byProvider.set(model.provider, list);
    }
    return byProvider;
  }, [models]);
  return { families, error: models ? null : error };
}

/**
 * The models a row or a rule may name, as menu items: each available model, the newest of each
 * family and the agent's own default (`vendors` false leaves that out). Those already `taken`
 * are disabled.
 */
export function TargetItems({
  onPick,
  taken = [],
  vendors = true,
}: {
  onPick: (target: OverrideTarget) => void;
  taken?: readonly OverrideTarget[];
  vendors?: boolean;
}) {
  const { families, error } = useFamilies();
  // Only what the user made available is offered.
  const available = useAvailableModelGroups();
  const availability = useApp((s) => s.settings);
  const listed = (target: OverrideTarget) => taken.some((other) => sameTarget(other, target));
  return (
    <>
      {PROVIDERS.filter((provider) => providerOn(availability, provider)).map((provider, index) => {
        const group = available.find((entry) => entry.provider === provider);
        const models = (group?.models ?? []).filter((model) => !model.legacy);
        const vendor: OverrideTarget = { type: "vendor", provider };
        return (
          <div key={provider}>
            {index > 0 && <DropdownMenuSeparator />}
            <DropdownMenuLabel>{VENDOR_LABELS[provider]}</DropdownMenuLabel>
            {models.map((model) => {
              const target: OverrideTarget = { type: "model", provider, id: model.id };
              return (
                <DropdownMenuItem key={model.id} disabled={listed(target)} onSelect={() => onPick(target)}>
                  <span className="truncate">{model.displayName}</span>
                  <span className="text-muted-foreground ms-auto truncate font-mono text-2xs">{model.id}</span>
                </DropdownMenuItem>
              );
            })}
            {(families.get(provider) ?? []).map((family) => {
              const target: OverrideTarget = { type: "family", provider, family };
              return (
                <DropdownMenuItem key={family} disabled={listed(target)} onSelect={() => onPick(target)}>
                  Newest {capital(family)} family
                  <span className="text-muted-foreground ms-auto text-xs">follows releases</span>
                </DropdownMenuItem>
              );
            })}
            {vendors && (
              <DropdownMenuItem disabled={listed(vendor)} onSelect={() => onPick(vendor)}>
                {VENDOR_LABELS[provider]}'s default
                <span className="text-muted-foreground ms-auto text-xs">the CLI picks</span>
              </DropdownMenuItem>
            )}
          </div>
        );
      })}
      {error && (
        <p role="alert" className="text-destructive px-2 py-1.5 text-xs">
          Couldn't read the model families: {error}
        </p>
      )}
    </>
  );
}
