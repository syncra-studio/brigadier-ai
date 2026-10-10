import { ChevronRight } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { agentState, SetupTerminal } from "@/app/onboarding/SetupTerminal";
import { ProviderAccounts, type Revealed } from "@/app/providers/AccountRows";
import { modelSummary } from "@/app/routing/models";
import { SettingsButton, SettingsCard, SettingsRow, SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { revealSetting } from "@/app/settings/reveal";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Badge } from "@/components/ui/badge";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type { AccountView, MergedModel, ModelInfo, ProviderKind, ProviderOverview } from "@/ipc/generated";
import { isFable, ruleSentence } from "@/lib/routing";
import { providerStatusText } from "@/lib/setup";
import { refreshProviders } from "@/state/actions";
import {
  isNewModelRule,
  isWorkerRule,
  modelAvailable,
  providerOn,
  setModelAvailable,
  setProviderOn,
} from "@/state/providers";
import { useApp } from "@/state/store";

/** What an agent switched off means, under its name. */
const OFF_NOTE = "Off: its models are hidden from the model picker, and it gets no work.";

/** The line under a model Brigadier gives no tasks to whatever the user chooses. */
export const FABLE_NOTE =
  "Brigadier never gives work to Fable models. You can still pick one for a chat or session.";

/** The line under a model a "Don't give work" rule keeps from tasks. */
const NO_WORK_NOTE = "Gets no tasks: a rule under Advanced keeps it to chats and sessions you pick it for.";

/**
 * One agent: its name with how it stands, the one action that gets it working (Install or Sign
 * in) and its switch; then its accounts, then its models with "Use this model" each.
 */
export function ProviderCard({
  provider,
  overview,
  accounts,
  merged,
  groups,
  now,
  revealed,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  accounts: AccountView[] | null;
  merged: readonly MergedModel[] | null;
  groups: readonly ModelGroup[];
  now: number;
  revealed: Revealed;
}) {
  const on = useApp((s) => providerOn(s.settings, provider));
  const toggle = useAction();
  const state = agentState(overview);
  const label = PROVIDER_LABELS[provider];
  const needs = state === "install" || state === "signIn";
  // Installing or signing in the agent; its terminal closes by itself once that is done.
  const [setup, setSetup] = useState<{ install: boolean } | null>(null);
  const settingUp = setup && state === (setup.install ? "install" : "signIn") ? setup : null;

  return (
    <SettingsSection
      title={label}
      icon={<ProviderGlyph provider={provider} className="size-icon-md shrink-0" />}
      description={
        <span className="flex items-center gap-1.5">
          {on && needs && <span aria-hidden className="bg-warning size-1.5 shrink-0 rounded-full" />}
          {on ? providerStatusText(overview) : OFF_NOTE}
        </span>
      }
      actions={
        <>
          {on && needs && (
            <SettingsButton disabled={settingUp !== null} onClick={() => setSetup({ install: state === "install" })}>
              {state === "install" ? "Install" : "Sign in"}
            </SettingsButton>
          )}
          <SettingsSwitch
            label={`Use ${label}`}
            checked={on}
            disabled={toggle.busy}
            onCheckedChange={(next) => toggle.run(() => setProviderOn(provider, next))}
          />
        </>
      }
    >
      {toggle.error && (
        <p role="alert" className="text-destructive text-xs">
          {toggle.error}
        </p>
      )}
      {settingUp && (
        <SetupTerminal
          key={String(settingUp.install)}
          provider={provider}
          install={settingUp.install}
          onClose={() => {
            setSetup(null);
            void refreshProviders(provider).catch(() => {});
          }}
        />
      )}
      <ProviderAccounts provider={provider} views={accounts} now={now} revealed={revealed} />
      {on && <ProviderModels provider={provider} overview={overview} merged={merged} groups={groups} />}
    </SettingsSection>
  );
}

/** The agent's models, a "Use this model" switch each; older models folded away. */
function ProviderModels({
  provider,
  overview,
  merged,
  groups,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  merged: readonly MergedModel[] | null;
  groups: readonly ModelGroup[];
}) {
  const models = overview?.models?.models ?? [];
  const label = PROVIDER_LABELS[provider];
  const state = agentState(overview);
  const current = models.filter((model) => !model.legacy);
  const older = models.filter((model) => model.legacy);
  const row = (model: ModelInfo) => (
    <ModelRow key={model.id} provider={provider} model={model} merged={merged} groups={groups} />
  );
  return (
    <>
      <h3 data-setting="Models" className="text-foreground/80 px-1 pt-3 text-xs font-medium">
        Models
      </h3>
      {models.length === 0 ? (
        <p className="text-muted-foreground px-1 text-xs">
          {state === "checking"
            ? "Checking…"
            : state === "install"
              ? `Install ${label} to see its models.`
              : state === "signIn"
                ? `Sign in to ${label} to see its models.`
                : "Reading the models…"}
        </p>
      ) : (
        <SettingsCard>
          {current.map(row)}
          {older.length > 0 && (
            <Collapsible>
              <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex w-full items-center gap-1.5 px-4 py-2.5 text-xs">
                <ChevronRight
                  aria-hidden
                  className="size-icon-xs transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
                />
                {older.length} older {older.length === 1 ? "model" : "models"}
              </CollapsibleTrigger>
              <CollapsibleContent className="flex flex-col">{older.map(row)}</CollapsibleContent>
            </Collapsible>
          )}
        </SettingsCard>
      )}
    </>
  );
}

/**
 * A model: its name, what it's like in a few words, and "Use this model" (in the pickers, and
 * given work). A model Brigadier saw after its agent's first list is tagged New: it gets no
 * tasks until the user removes its rule, which the link opens under Advanced; one the user
 * keeps from tasks says so. Fable models never get tasks.
 */
function ModelRow({
  provider,
  model,
  merged,
  groups,
}: {
  provider: ProviderKind;
  model: ModelInfo;
  merged: readonly MergedModel[] | null;
  groups: readonly ModelGroup[];
}) {
  const ref = { provider, id: model.id };
  const available = useApp((s) => modelAvailable(s.settings, ref));
  const projects = useApp((s) => s.projects);
  const rules = useApp((s) => s.settings.routingOverrides);
  const newRule = rules.find((rule) => isNewModelRule(rule) && isWorkerRule(rule, ref));
  const noWork = rules.some((rule) => isWorkerRule(rule, ref));
  const toggle = useAction();
  const fable = isFable(model);
  const fresh = !fable && newRule !== undefined;
  const known = merged?.find((entry) => entry.provider === provider && entry.id === model.id);
  const description = fable ? (
    FABLE_NOTE
  ) : fresh ? (
    <span className="flex flex-wrap items-center gap-x-2">
      Brigadier gives it no tasks until you allow it.
      <button
        type="button"
        className="text-link hover:underline"
        onClick={() => revealSetting(ruleSentence(newRule, groups, projects))}
      >
        Allow in Rules
      </button>
    </span>
  ) : noWork ? (
    NO_WORK_NOTE
  ) : (
    modelSummary(model, known) || undefined
  );
  return (
    <SettingsRow
      label={
        <span className="flex items-center gap-2">
          {model.displayName}
          {fresh && <Badge variant="warning">New</Badge>}
        </span>
      }
      description={description}
      error={toggle.error}
    >
      <SettingsSwitch
        label={`Use ${model.displayName}`}
        checked={available}
        disabled={toggle.busy}
        onCheckedChange={(next) => toggle.run(() => setModelAvailable(ref, next))}
      />
    </SettingsRow>
  );
}
