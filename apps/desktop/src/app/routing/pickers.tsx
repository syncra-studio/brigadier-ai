import { Check, ChevronDown } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useEffect, useMemo } from "react";

import { selectTrigger } from "@/app/settings/parts";
import { Badge } from "@/components/ui/badge";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { OverrideTarget, RouteCandidate } from "@/ipc/generated";
import { PROVIDERS, sameTarget, VENDOR_LABELS } from "@/lib/routing";
import { useAvailableModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { providerOn } from "@/state/providers";
import { useApp } from "@/state/store";
import { loadUsage, useUsage } from "@/state/usage";

/*
 * The menus Settings › Routing's sentences are made of: a chip that opens a choice, the models
 * a list or a rule may name, and where a setting applies.
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

/** A word of a sentence that opens a menu: its value with a chevron. */
export function ChipTrigger({
  label,
  children,
  large = false,
  className,
  ...props
}: {
  label: string;
  children: ReactNode;
  /** The size of the try-order sentence's words. */
  large?: boolean | undefined;
  className?: string;
} & Omit<React.ComponentProps<"button">, "children">) {
  return (
    <button
      type="button"
      aria-label={label}
      className={cn(selectTrigger, "inline-flex gap-1.5 align-middle", large && "h-8 text-sm", className)}
      {...props}
    >
      <span className="min-w-0 truncate">{children}</span>
      <ChevronDown aria-hidden className="text-muted-foreground size-icon-sm shrink-0" />
    </button>
  );
}

/** One choice of a few, as a word of a sentence. */
export function ChipSelect<T extends string>({
  label,
  value,
  options,
  onChange,
  large,
  disabled,
}: {
  label: string;
  value: T;
  options: readonly { value: T; label: string }[];
  onChange: (value: T) => void;
  large?: boolean;
  disabled?: boolean;
}) {
  const chosen = options.find((option) => option.value === value);
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild disabled={disabled}>
        <ChipTrigger label={label} large={large}>
          {chosen?.label}
        </ChipTrigger>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-96 overflow-y-auto">
        <DropdownMenuRadioGroup value={value} onValueChange={(next) => onChange(next as T)}>
          {options.map((option) => (
            <DropdownMenuRadioItem
              key={option.value}
              value={option.value}
              indicator={<Check className="size-icon-md" />}
            >
              {option.label}
            </DropdownMenuRadioItem>
          ))}
        </DropdownMenuRadioGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** "All projects" and each project by name, for where a setting applies. */
export function useScopeOptions(): { value: string; label: string }[] {
  const projects = useApp((s) => s.projects);
  return [
    { value: "", label: "All projects" },
    ...Object.values(projects)
      .toSorted((a, b) => a.name.localeCompare(b.name))
      .map((project) => ({ value: project.id, label: project.name })),
  ];
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

/** A model (or a family or vendor) as a word of a sentence, changed from its menu. */
export function TargetSelect({
  label,
  value,
  name,
  onChange,
  large,
}: {
  label: string;
  value: OverrideTarget | null;
  /** The value in words; shown when there is one. */
  name: string;
  onChange: (target: OverrideTarget) => void;
  large?: boolean;
}) {
  return (
    <DropdownMenu modal={false}>
      <DropdownMenuTrigger asChild>
        <ChipTrigger label={label} large={large}>
          {value ? name : "Choose a model"}
        </ChipTrigger>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="max-h-96 w-xs overflow-y-auto">
        <TargetItems onPick={onChange} />
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
