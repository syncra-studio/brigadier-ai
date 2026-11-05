import { useId, useRef, useState } from "react";

import { PickerButton } from "@/components/controls/picker-button";
import { composerPill } from "@/components/conversation/elements/surfaces";
import { Bolt, ChevronDown, ProviderGlyph } from "@/components/icons";
import { effortLabel, ModelPicker, openOnArrows, type PickedWith } from "@/components/shared/model-picker";
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
  const word = effortWord(model, effortFor(model, choice.effort));
  return word ? `${name} · ${word}` : name;
}

/**
 * The effort word a model's pill shows: its effort, or Default for a model with efforts that
 * names no default of its own (OpenCode's, which runs its provider's default); none for a model
 * without efforts.
 */
function effortWord(model: ModelInfo | null, effort: string | null): string | null {
  if (effort) return effortLabel(effort);
  return model && model.efforts.length > 0 ? "Default" : null;
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
 * The model and effort picker, the one every place that picks a model uses. The trigger reads
 * "Opus 5.5 High" after its agent's logo: the model and effort the conversation runs with, the
 * CLI's defaults spelled out. A pick comes to `onChange` and the picker stays open on its efforts.
 * With `continues` (a started session), a pick on another agent stays in the picker and comes to
 * `onChange`, which moves the session, as it closes; Escape closes it without moving.
 */
export function ModelSelector({
  groups,
  value,
  onChange,
  label = "Model",
  disabled,
  className,
  open: shown,
  onOpenChange,
  automatic,
  refused,
  continues,
  agentDefault,
  noFast,
}: {
  groups: readonly ModelGroup[];
  value: ModelChoice;
  onChange: (choice: ModelChoice) => void;
  label?: string | undefined;
  disabled?: boolean | undefined;
  className?: string | undefined;
  /** Opens the picker from elsewhere (the composer's `/model`); uncontrolled when absent. */
  open?: boolean | undefined;
  onOpenChange?: ((open: boolean) => void) | undefined;
  /**
   * "Automatic" over the columns (a setting Brigadier may pick for): `chosen` shows it on the
   * button, and `value` is then only where the picker would open on a manual pick, and what
   * turning it off takes. `label` names it, with what it picks now ("Automatic (Opus 5.5)").
   */
  automatic?: { chosen: boolean; hint: string; onChoose: () => void; label?: string } | undefined;
  /** What the user's rules keep out: shown, but it can't be picked, with the rule's words. */
  refused?: Refused | undefined;
  /** A started session: picking another agent's model continues the session there (S38c). */
  continues?: boolean | undefined;
  /** The agent's own default first among its models (a null model), by the agent's name. */
  agentDefault?: ((provider: ProviderKind) => string) | undefined;
  /** What this place runs has no fast tier to offer. */
  noFast?: boolean | undefined;
}) {
  const auto = automatic?.chosen === true;
  const [own, setOwn] = useState(false);
  const open = shown ?? own;
  const setOpen = onOpenChange ?? setOwn;
  // A pick on another agent, waiting for the picker to close (`continues`).
  const [waiting, setWaiting] = useState<ModelChoice | null>(null);
  const escaped = useRef(false);
  const change = (next: boolean) => {
    if (!next && waiting && !escaped.current) onChange(waiting);
    if (!next) setWaiting(null);
    escaped.current = false;
    setOpen(next);
  };
  const shownValue = waiting ?? value;
  const onItsDefault = Boolean(agentDefault) && shownValue.model === null;
  const current = onItsDefault ? null : findModel(groups, shownValue);
  const effort = effortFor(current, shownValue.effort);
  const word = onItsDefault ? (shownValue.effort && effortLabel(shownValue.effort)) : effortWord(current, effort);
  const name = onItsDefault
    ? `${agentDefault?.(shownValue.provider)}'s default`
    : (current?.displayName ?? (shownValue.model && shownValue.model !== "default" ? shownValue.model : "Default model"));
  const fast = shownValue.fast === true && Boolean(current?.fast);
  /** The choice as it changes: held while it waits for the close, else at once. */
  const edit = (next: ModelChoice) => {
    if (continues && next.provider !== value.provider) setWaiting(next);
    else {
      setWaiting(null);
      onChange(next);
    }
  };
  /** The level a model starts at: its default, or the highest the rules allow when they refuse that. */
  const allowedLevel = (provider: ProviderKind, model: ModelInfo, level: string | null) =>
    level && refused?.(provider, model, level)
      ? (model.efforts.toReversed().find((other) => !refused(provider, model, other)) ?? level)
      : level;
  const pick = (provider: ProviderKind, model: ModelInfo | null, also?: PickedWith) => {
    const same = provider === shownValue.provider;
    if (!model) {
      edit({ provider, model: null, effort: also?.effort ?? null, ...(same && shownValue.account ? { account: shownValue.account } : {}) });
      return;
    }
    const wantsFast = also?.fast ?? shownValue.fast;
    edit({
      provider,
      model: model.id,
      effort: also?.effort ?? allowedLevel(provider, model, effortFor(model, same ? shownValue.effort : null)),
      // Fast carries over to a model that has a fast tier too.
      ...(model.fast && wantsFast ? { fast: true } : {}),
      // So does the account, to a model of the same agent.
      ...(same && shownValue.account ? { account: shownValue.account } : {}),
    });
  };
  const id = useId();
  return (
    <ModelPicker
      label={label}
      open={open}
      onOpenChange={change}
      onEscape={() => {
        escaped.current = true;
      }}
      side="top"
      align="end"
      groups={groups}
      checked={auto ? null : onItsDefault ? { provider: shownValue.provider, model: null } : current && { provider: shownValue.provider, model: current.id }}
      opensOn={shownValue.provider}
      agentDefault={agentDefault}
      onPick={pick}
      greyed={refused && ((provider, model) => (model ? refused(provider, model, null) : null))}
      continuesFrom={continues ? value.provider : null}
      effort={
        auto
          ? undefined
          : onItsDefault
            ? { value: shownValue.effort, onChange: (level) => edit({ ...shownValue, effort: level }) }
            : current
              ? { value: effort, onChange: (level) => edit({ ...shownValue, model: current.id, effort: level }) }
              : undefined
      }
      refused={refused}
      fast={
        !noFast && !auto && current?.fast
          ? { on: fast, onChange: (on) => edit({ ...shownValue, model: current.id, fast: on }) }
          : !noFast
            ? { on: false, onChange: () => undefined }
            : undefined
      }
      automatic={
        automatic && {
          chosen: auto,
          label: automatic.label ?? "Automatic",
          hint: automatic.hint,
          onChoose: automatic.onChoose,
          onTurnOff: () => onChange(value),
        }
      }
    >
      <PickerButton
        label={label}
        shown={auto ? [`${id}-name`] : [fast && `${id}-fast`, `${id}-name`, word && `${id}-effort`]}
        disabled={disabled}
        data-slot="model-selector-trigger"
        onKeyDown={openOnArrows(open, change)}
        className={cn(composerPill, "text-muted-foreground gap-1", className)}
      >
        {auto ? (
          <span id={`${id}-name`} className="text-foreground truncate">{automatic?.label ?? "Automatic"}</span>
        ) : (
          <>
            <ProviderGlyph provider={shownValue.provider} className="size-icon-sm shrink-0" />
            {fast && <Bolt id={`${id}-fast`} aria-label="Fast" className="text-foreground" />}
            <span id={`${id}-name`} className="text-foreground truncate">{name}</span>
            {word && <span id={`${id}-effort`} className="shrink-0">{word}</span>}
          </>
        )}
        <ChevronDown className="size-icon-xs!" />
      </PickerButton>
    </ModelPicker>
  );
}
