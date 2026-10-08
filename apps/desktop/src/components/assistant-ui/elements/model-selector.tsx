import { Bolt, Check, ChevronDown } from "@openai/apps-sdk-ui/components/Icon";
import { type FC, type ReactNode, useState } from "react";

import { composerPill } from "@/components/assistant-ui/elements/surfaces";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuRadioGroup,
  DropdownMenuRadioItem,
  DropdownMenuSeparator,
  DropdownMenuSub,
  DropdownMenuSubContent,
  DropdownMenuSubTrigger,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { ModelChoice, ModelInfo, ProviderKind } from "@/ipc/generated";
import { tokenPx } from "@/lib/tokens";
import { cn } from "@/lib/utils";

/** "xhigh" → "Extra high", "medium" → "Medium". */
export function effortLabel(effort: string): string {
  const known: Record<string, string> = {
    none: "None",
    minimal: "Minimal",
    low: "Low",
    medium: "Medium",
    high: "High",
    xhigh: "Extra high",
    max: "Max",
  };
  return known[effort] ?? effort.charAt(0).toUpperCase() + effort.slice(1);
}

/** A provider's live model list, as the selector offers it. */
export type ModelGroup = {
  provider: ProviderKind;
  label: string;
  models: readonly ModelInfo[];
  /** Why the provider can't be picked (not installed, not logged in). */
  unavailable: string | null;
};

/**
 * The model a choice names. No model, or the `default` alias of an older list, is the model the
 * CLI runs when not told one.
 */
export function findModel(groups: readonly ModelGroup[], choice: ModelChoice): ModelInfo | null {
  const group = groups.find((entry) => entry.provider === choice.provider);
  if (!group) return null;
  // A concrete id ("claude-sonnet-5-5") names the alias that resolves to it ("sonnet").
  const named =
    choice.model === null
      ? undefined
      : (group.models.find((model) => model.id === choice.model) ??
        group.models.find((model) => model.resolved === choice.model));
  if (named) return named;
  return choice.model === null || choice.model === "default"
    ? (group.models.find((model) => model.isDefault) ?? null)
    : null;
}

/** "Opus 5.5 · High": the choice as the trigger shows it. */
export function choiceLabel(groups: readonly ModelGroup[], choice: ModelChoice): string {
  const model = findModel(groups, choice);
  const name = model?.displayName ?? choice.model ?? "Default model";
  const effort = effortFor(model, choice.effort);
  return effort ? `${name} · ${effortLabel(effort)}` : name;
}

/**
 * Keeps an effort only if the model accepts it; otherwise the model's default (none for a model
 * without efforts). An unknown model keeps it.
 */
export function effortFor(model: ModelInfo | null, effort: string | null): string | null {
  if (!model) return effort;
  if (model.efforts.length === 0) return null;
  if (effort && model.efforts.includes(effort)) return effort;
  return model.defaultEffort;
}

/** A two-line row (a model and what it's for): rounded, not a pill. */
const TALL_ROW = "h-auto rounded-xl";

/** A section heading, in plain grey. */
const Section: FC<{ children: ReactNode }> = ({ children }) => (
  <DropdownMenuLabel className="font-sans text-sm font-normal tracking-normal normal-case">
    {children}
  </DropdownMenuLabel>
);

/** A model row: its name (marked when it's the default), then what it's for in grey. */
const ModelRow: FC<{ model: ModelInfo; isDefault: boolean }> = ({ model, isDefault }) => (
  <span className="flex min-w-0 flex-col py-1">
    <span className="flex min-w-0 gap-2">
      <span className="truncate">{model.displayName}</span>
      {isDefault && <span className="text-muted-foreground shrink-0">Default</span>}
    </span>
    {model.description && (
      <span className="text-muted-foreground truncate text-xs">{model.description}</span>
    )}
  </span>
);

/** A model as a radio row of its provider's list, marked when it's `defaultModel`. */
const modelItem = (defaultModel: ModelInfo | null) => (model: ModelInfo) => (
  <DropdownMenuRadioItem
    key={model.id}
    value={model.id}
    data-slot="model-selector-item"
    indicator={<Check className="size-icon-md" />}
    className={TALL_ROW}
  >
    <ModelRow model={model} isDefault={model === defaultModel} />
  </DropdownMenuRadioItem>
);

/** Whether a model goes under Legacy: an older one, unless it's a default (the CLI's or ours). */
const isLegacy = (defaultModel: ModelInfo | null) => (model: ModelInfo) =>
  model.legacy && !model.isDefault && model !== defaultModel;

/** A flyout's panel: as wide as the menu, scrolling when it runs out of room. */
const FLYOUT =
  "w-xs max-h-(--radix-dropdown-menu-content-available-height) overflow-y-auto p-1";

/**
 * The model and effort picker. The trigger reads "Opus 5.5 High" after its agent's logo: the
 * model and effort the conversation runs with, the CLI's defaults spelled out. The menu lists
 * the model's efforts (its default marked), Fast where the model has a fast tier, then each
 * provider with its model in use, opening to the side on its latest models and a Legacy row
 * opening on the older ones. `defaultChoice`, what a new conversation starts with when nothing
 * is picked, is marked Default, on its model and on its provider while that's the one in use.
 */
export function ModelSelector({
  groups,
  value,
  defaultChoice,
  onChange,
  label = "Model",
  disabled,
  className,
  open: shown,
  onOpenChange,
}: {
  groups: readonly ModelGroup[];
  value: ModelChoice;
  defaultChoice: ModelChoice;
  onChange: (choice: ModelChoice) => void;
  label?: string | undefined;
  disabled?: boolean | undefined;
  className?: string | undefined;
  /** Opens the picker from elsewhere (the composer's `/model`); uncontrolled when absent. */
  open?: boolean | undefined;
  onOpenChange?: ((open: boolean) => void) | undefined;
}) {
  const [own, setOwn] = useState(false);
  const open = shown ?? own;
  const setOpen = onOpenChange ?? setOwn;
  const current = findModel(groups, value);
  const defaultModel = findModel(groups, defaultChoice);
  const effort = effortFor(current, value.effort);
  const name =
    current?.displayName ?? (value.model && value.model !== "default" ? value.model : "Default model");
  const fast = value.fast === true && Boolean(current?.fast);
  const pick = (model: ModelInfo, provider: ProviderKind) =>
    onChange({
      provider,
      model: model.id,
      effort: effortFor(model, provider === value.provider ? value.effort : null),
      // Fast carries over to a model that has a fast tier too.
      ...(model.fast && value.fast ? { fast: true } : {}),
      // So does the account, to a model of the same agent.
      ...(provider === value.provider && value.account ? { account: value.account } : {}),
    });
  const models = (group: ModelGroup) => {
    const chosen = group.provider === value.provider ? current?.id : undefined;
    const legacy = group.models.filter(isLegacy(defaultModel));
    return (
      <DropdownMenuRadioGroup
        value={chosen ?? ""}
        onValueChange={(id) => {
          const model = group.models.find((entry) => entry.id === id);
          if (model) pick(model, group.provider);
        }}
      >
        {group.models
          .filter((model) => !isLegacy(defaultModel)(model))
          .map(modelItem(defaultModel))}
        {legacy.length > 0 && (
          <DropdownMenuSub>
            <DropdownMenuSubTrigger data-slot="model-selector-legacy">
              Legacy
              <span className="text-muted-foreground min-w-0 flex-1 truncate">
                {legacy.find((model) => model.id === chosen)?.displayName}
              </span>
            </DropdownMenuSubTrigger>
            <DropdownMenuSubContent sideOffset={tokenPx("--spacing")} className={FLYOUT}>
              {legacy.map(modelItem(defaultModel))}
            </DropdownMenuSubContent>
          </DropdownMenuSub>
        )}
      </DropdownMenuRadioGroup>
    );
  };
  const single = groups.length === 1 ? groups[0] : undefined;
  return (
    <DropdownMenu modal={false} open={open} onOpenChange={setOpen}>
      <DropdownMenuTrigger asChild>
        <button
          type="button"
          aria-label={label}
          disabled={disabled}
          data-slot="model-selector-trigger"
          className={cn(composerPill, "text-muted-foreground gap-1", className)}
        >
          <ProviderGlyph provider={value.provider} className="size-icon-sm shrink-0" />
          {fast && <Bolt aria-label="Fast" className="text-foreground" />}
          <span className="text-foreground truncate">{name}</span>
          {effort && <span className="shrink-0">{effortLabel(effort)}</span>}
          <ChevronDown className="size-icon-xs!" />
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent
        side="top"
        align="end"
        data-slot="model-selector"
        className="w-xs p-1"
      >
        {current && current.efforts.length > 0 && (
          <>
            <Section>Effort</Section>
            <DropdownMenuRadioGroup
              value={effort ?? ""}
              onValueChange={(next) => onChange({ ...value, model: current.id, effort: next })}
            >
              {current.efforts.map((level) => (
                <DropdownMenuRadioItem
                  key={level}
                  value={level}
                  data-slot="model-selector-effort"
                  indicator={<Check className="size-icon-md" />}
                >
                  {effortLabel(level)}
                  {level === current.defaultEffort && (
                    <span className="text-muted-foreground">Default</span>
                  )}
                </DropdownMenuRadioItem>
              ))}
            </DropdownMenuRadioGroup>
          </>
        )}
        {current?.fast && (
          <DropdownMenuCheckboxItem
            checked={fast}
            data-slot="model-selector-fast"
            onCheckedChange={(on) => onChange({ ...value, model: current.id, fast: on })}
          >
            <Bolt />
            Fast
            <span className="text-muted-foreground min-w-0 truncate">{current.fast}</span>
          </DropdownMenuCheckboxItem>
        )}
        {current && (current.efforts.length > 0 || current.fast) && <DropdownMenuSeparator />}
        <Section>Model</Section>
        {single ? (
          models(single)
        ) : (
          groups.map((group) => {
            const inUse =
              group.provider === value.provider
                ? current
                : (group.models.find((model) => model.isDefault) ?? group.models[0]);
            return (
              <DropdownMenuSub key={group.provider}>
                <DropdownMenuSubTrigger
                  disabled={group.unavailable !== null || group.models.length === 0}
                  data-slot="model-selector-provider"
                >
                  <ProviderGlyph provider={group.provider} className="size-icon-md shrink-0" />
                  <span className={cn(group.provider === value.provider && "font-medium")}>
                    {group.label}
                  </span>
                  <span className="text-muted-foreground min-w-0 truncate">
                    {group.unavailable ??
                      (group.models.length === 0 ? "No models listed yet" : inUse?.displayName)}
                  </span>
                  {group.unavailable === null && inUse && inUse === defaultModel && (
                    <span className="text-muted-foreground/60 shrink-0">Default</span>
                  )}
                </DropdownMenuSubTrigger>
                <DropdownMenuSubContent sideOffset={tokenPx("--spacing")} className={FLYOUT}>
                  {models(group)}
                </DropdownMenuSubContent>
              </DropdownMenuSub>
            );
          })
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
