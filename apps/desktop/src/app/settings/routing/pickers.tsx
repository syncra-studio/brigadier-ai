import { useEffect, useMemo } from "react";

import { Badge } from "@/components/controls";
import { DropdownMenuItem, DropdownMenuLabel, DropdownMenuSeparator } from "@/components/overlays";
import type { OverrideTarget, RouteCandidate } from "@/ipc/generated";
import { PROVIDERS, sameTarget, VENDOR_LABELS } from "@/lib/routing";
import { useAvailableModelGroups } from "@/lib/setup";
import { providerOn } from "@/state/providers";
import { useApp } from "@/state/store";
import { loadUsage, useUsage } from "@/state/usage";

/*
 * The models an order or a rule may name, as menu items.
 */

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
 * The models a list or a rule may name, as menu items: each available model, the newest of each
 * family and any model of a vendor (`vendors` and `families` false leave those out). Those
 * already `taken` are disabled.
 */
export function TargetItems({
  onPick,
  taken = [],
  candidates = [],
  families: withFamilies = true,
}: {
  onPick: (target: OverrideTarget) => void;
  taken?: readonly OverrideTarget[];
  candidates?: readonly RouteCandidate[];
  families?: boolean;
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
        // Expensive models (Fable, Mythos) are offered too: picked here, routing uses them.
        const models = (group?.models ?? []).filter((model) => !model.legacy);
        const vendor: OverrideTarget = { type: "vendor", provider };
        return (
          <div key={provider}>
            {index > 0 && <DropdownMenuSeparator />}
            <DropdownMenuLabel>{VENDOR_LABELS[provider]}</DropdownMenuLabel>
            {models.map((model) => {
              const target: OverrideTarget = { type: "model", provider, id: model.id };
              const isNew = candidates.some(
                (candidate) => candidate.provider === provider && candidate.model === model.id && candidate.new,
              );
              return (
                <DropdownMenuItem key={model.id} disabled={listed(target)} onSelect={() => onPick(target)}>
                  <span className="truncate">{model.displayName}</span>
                  {isNew && <Badge variant="warning">New</Badge>}
                  <span className="text-muted-foreground ms-auto truncate font-mono text-2xs">{model.id}</span>
                </DropdownMenuItem>
              );
            })}
            {withFamilies &&
              (families.get(provider) ?? []).map((family) => {
                const target: OverrideTarget = { type: "family", provider, family };
                return (
                  <DropdownMenuItem key={family} disabled={listed(target)} onSelect={() => onPick(target)}>
                    Newest {VENDOR_LABELS[provider]} {family}
                    <span className="text-muted-foreground ms-auto text-xs">follows releases</span>
                  </DropdownMenuItem>
                );
              })}
            <DropdownMenuItem disabled={listed(vendor)} onSelect={() => onPick(vendor)}>
              Any {VENDOR_LABELS[provider]} model
              <span className="text-muted-foreground ms-auto text-xs">best scored</span>
            </DropdownMenuItem>
          </div>
        );
      })}
      {error && (
        <p className="text-muted-foreground px-2 py-1.5 text-xs">
          Families appear once the model list can be read ({error}).
        </p>
      )}
    </>
  );
}
