import { useId, useState } from "react";

import { PickerButton } from "@/components/controls/picker-button";
import { composerPill } from "@/components/conversation/elements/surfaces";
import { Bolt, ChevronDown, ProviderGlyph } from "@/components/icons";
import { effortLabel, ModelPicker, openOnArrows } from "@/components/shared/model-picker";
import type { ModelChoice, ModelInfo, ProviderKind } from "@/ipc/generated";
import { cn } from "@/lib/utils";

export { effortLabel };

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

/**
 * Why the user's rules keep a model out, or one of its efforts (`effort` null: the model
 * itself), in their words ("Never go above High effort"); `null` when they don't.
 */
export type Refused = (provider: ProviderKind, model: ModelInfo, effort: string | null) => string | null;

/**
 * The model and effort picker. The trigger reads "Opus 5.5 High" after its agent's logo: the
 * model and effort the conversation runs with, the CLI's defaults spelled out. It opens the model
 * picker on the agent in use, its model checked and its efforts (and Fast) underneath.
 * `defaultChoice`, what a new conversation starts with when nothing is picked, is marked Default.
 * With `continues` (a started session), another agent's models say a pick continues there; the
 * pick still comes to `onChange`, which moves the session.
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
  automatic,
  refused,
  continues,
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
  /**
   * An "Automatic" choice above the agents (a setting Brigadier may pick for): `chosen` shows
   * it on the button, and `value` is then only where the picker would open on a manual pick.
   * `label` names it, with what it picks now ("Automatic (Opus 5.5)").
   */
  automatic?: { chosen: boolean; hint: string; onChoose: () => void; label?: string } | undefined;
  /** What the user's rules keep out: shown, but it can't be picked, with the rule's words. */
  refused?: Refused | undefined;
  /** A started session: picking another agent's model continues the session there (S38c). */
  continues?: boolean | undefined;
}) {
  const auto = automatic?.chosen === true;
  const [own, setOwn] = useState(false);
  const open = shown ?? own;
  const setOpen = onOpenChange ?? setOwn;
  const current = findModel(groups, value);
  const defaultModel = findModel(groups, defaultChoice);
  const effort = effortFor(current, value.effort);
  const name =
    current?.displayName ?? (value.model && value.model !== "default" ? value.model : "Default model");
  const fast = value.fast === true && Boolean(current?.fast);
  const pick = (provider: ProviderKind, model: ModelInfo) =>
    onChange({
      provider,
      model: model.id,
      effort: effortFor(model, provider === value.provider ? value.effort : null),
      // Fast carries over to a model that has a fast tier too.
      ...(model.fast && value.fast ? { fast: true } : {}),
      // So does the account, to a model of the same agent.
      ...(provider === value.provider && value.account ? { account: value.account } : {}),
    });
  const id = useId();
  return (
    <ModelPicker
      label={label}
      open={open}
      onOpenChange={setOpen}
      side="top"
      align="end"
      groups={groups}
      checked={auto || !current ? null : { provider: value.provider, model: current.id }}
      defaultModel={defaultModel && { provider: defaultChoice.provider, model: defaultModel.id }}
      onPick={(provider, model) => model && pick(provider, model)}
      greyed={refused && ((provider, model) => (model ? refused(provider, model, null) : null))}
      continuesFrom={continues ? value.provider : null}
      effort={
        !auto && current && current.efforts.length > 0
          ? {
              levels: current.efforts,
              value: effort,
              onChange: (level) => onChange({ ...value, model: current.id, effort: level }),
              refused: refused && ((level) => refused(value.provider, current, level)),
            }
          : undefined
      }
      fast={
        !auto && current?.fast
          ? { on: fast, onChange: (on) => onChange({ ...value, model: current.id, fast: on }) }
          : undefined
      }
      automatic={
        automatic && {
          chosen: auto,
          label: automatic.label ?? "Automatic",
          hint: automatic.hint,
          onChoose: automatic.onChoose,
        }
      }
    >
      <PickerButton
        label={label}
        shown={auto ? [`${id}-name`] : [fast && `${id}-fast`, `${id}-name`, effort && `${id}-effort`]}
        disabled={disabled}
        data-slot="model-selector-trigger"
        onKeyDown={openOnArrows(open, setOpen)}
        className={cn(composerPill, "text-muted-foreground gap-1", className)}
      >
        {auto ? (
          <span id={`${id}-name`} className="text-foreground truncate">{automatic?.label ?? "Automatic"}</span>
        ) : (
          <>
            <ProviderGlyph provider={value.provider} className="size-icon-sm shrink-0" />
            {fast && <Bolt id={`${id}-fast`} aria-label="Fast" className="text-foreground" />}
            <span id={`${id}-name`} className="text-foreground truncate">{name}</span>
            {effort && <span id={`${id}-effort`} className="shrink-0">{effortLabel(effort)}</span>}
          </>
        )}
        <ChevronDown className="size-icon-xs!" />
      </PickerButton>
    </ModelPicker>
  );
}
