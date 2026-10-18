import { ChevronRight, InfoCircle } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { agentState, SetupTerminal } from "@/app/onboarding/SetupTerminal";
import { ProviderAccounts, type Revealed } from "@/app/providers/AccountRows";
import { bestFor } from "@/app/routing/models";
import { SettingsButton, SettingsCard, SettingsSwitch } from "@/app/settings/parts";
import { revealSetting } from "@/app/settings/reveal";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from "@/components/ui/collapsible";
import type {
  AccountView,
  MergedModel,
  ModelInfo,
  OverrideRule,
  ProviderKind,
  ProviderOverview,
} from "@/ipc/generated";
import {
  isExpensive,
  isNoWorkRule,
  joinWords,
  PLAIN_KIND_NAMES,
  ruleSentence,
  TIER_LABELS,
  VENDOR_LABELS,
} from "@/lib/routing";
import { providerStatusText } from "@/lib/setup";
import { refreshProviders, openSettings } from "@/state/actions";
import {
  isNewModelRule,
  isWorkerRule,
  modelAvailable,
  providerOn,
  setModelAvailable,
} from "@/state/providers";
import { useApp } from "@/state/store";

/** What an agent switched off means, on its status line. */
const OFF_NOTE = "Off: its models are hidden from the model picker, and it gets no work";

/** The line under a model Brigadier gives no tasks to whatever the user chooses. */
export const EXPENSIVE_NOTE =
  "Expensive: Brigadier uses it only where you pick it, in a chat, a session, your order or a rule";

/** The line under a model a "Don't give work" rule keeps from tasks. */
const NO_WORK_NOTE = "Gets no tasks: a rule in Routing keeps it to chats and sessions you pick it for";

/** A heading over a part of the panel ("Accounts"). */
export function PanelHeading({ children }: { children: string }) {
  return (
    <h3 data-setting={children} className="text-label px-1 pb-2 font-medium">
      {children}
    </h3>
  );
}

/**
 * The chosen agent's side of the page: the one step that gets it working when it needs one,
 * then its accounts, then its models with a switch each. Its version, how it stands and its
 * switch are on its line in the list.
 */
export function ProviderPanel({
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
  const state = agentState(overview);
  const needs = on && (state === "install" || state === "signIn");
  // Installing or signing in the agent; its terminal closes by itself once that is done.
  const [setup, setSetup] = useState<{ install: boolean } | null>(null);
  const settingUp = setup && state === (setup.install ? "install" : "signIn") ? setup : null;
  const status = overview?.status;

  return (
    <div data-slot="provider-panel" className="flex flex-col gap-6 p-5">
      {!on && <p className="text-muted-foreground px-1 text-sm">{OFF_NOTE}</p>}
      {needs && (
        <SettingsCard>
          <div className="flex items-center justify-between gap-6 px-4 py-3">
            <span className="text-label flex min-w-0 items-center gap-2">
              <span aria-hidden className="bg-warning size-1.5 shrink-0 rounded-full" />
              {state === "install" ? (status?.guidance ?? providerStatusText(overview)) : providerStatusText(overview)}
            </span>
            <SettingsButton disabled={settingUp !== null} onClick={() => setSetup({ install: state === "install" })}>
              {state === "install" ? "Install" : "Sign in"}
            </SettingsButton>
          </div>
          {settingUp && (
            <div className="px-4 pb-4">
              <SetupTerminal
                key={String(settingUp.install)}
                provider={provider}
                install={settingUp.install}
                onClose={() => {
                  setSetup(null);
                  void refreshProviders(provider).catch(() => {});
                }}
              />
            </div>
          )}
        </SettingsCard>
      )}

      <section className="flex flex-col">
        <PanelHeading>Accounts</PanelHeading>
        <ProviderAccounts wide provider={provider} views={accounts} now={now} revealed={revealed} />
      </section>

      {on && <ProviderModels provider={provider} overview={overview} merged={merged} groups={groups} />}
    </div>
  );
}

/** The agent's models, a switch each; older models folded away. */
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
  const label = VENDOR_LABELS[provider];
  const state = agentState(overview);
  const current = models.filter((model) => !model.legacy);
  const older = models.filter((model) => model.legacy);
  const row = (model: ModelInfo) => (
    <ModelRow key={model.id} provider={provider} model={model} merged={merged} groups={groups} />
  );
  return (
    <section className="flex flex-col">
      <PanelHeading>Models</PanelHeading>
      {models.length === 0 ? (
        <p className="text-muted-foreground px-1 py-2 text-sm">
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
            <Collapsible className="flex flex-col">
              <CollapsibleTrigger className="text-foreground/65 hover:text-foreground group flex w-full items-center gap-2 px-4 py-3 text-sm">
                <ChevronRight
                  aria-hidden
                  className="size-icon-sm transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
                />
                Older models · {older.length}
              </CollapsibleTrigger>
              <CollapsibleContent className="divide-divider border-divider flex flex-col divide-y border-t">
                {older.map(row)}
              </CollapsibleContent>
            </Collapsible>
          )}
        </SettingsCard>
      )}
    </section>
  );
}

/** A rule about one model in a few words, as its row's tag says it ("Never gets Review changes"). */
function ruleTag(rule: OverrideRule): string {
  const kinds =
    rule.categories.length > 0
      ? joinWords(rule.categories.map((category) => PLAIN_KIND_NAMES[category]), "or")
      : "any work";
  switch (rule.effect) {
    case "never":
      return `Never gets ${kinds}`;
    case "prefer":
      return `Preferred for ${kinds}`;
    case "only":
      return `The only model for ${kinds}`;
  }
}

/** The rules naming this model (or its family), but the "Don't give work" one its line says. */
function rulesAbout(rules: readonly OverrideRule[], provider: ProviderKind, model: ModelInfo, family: string | null) {
  return rules.filter(
    (rule) =>
      !isNoWorkRule(rule) &&
      rule.target.provider === provider &&
      ((rule.target.type === "model" && rule.target.id === model.id) ||
        (rule.target.type === "family" && family !== null && rule.target.family === family)),
  );
}

/**
 * A model: its name, id and whether it is the agent's default, a tag for each rule about it, what
 * it's like in a few words, and its switch (in the pickers, and given work). A model Brigadier saw
 * after its agent's first list is tagged New: it gets no tasks until the user allows it in
 * Routing; one the user keeps from tasks says so. Fable models never get tasks.
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
  const expensive = isExpensive(model);
  // A rule kept from before still shows, so it can be lifted.
  const fresh = newRule !== undefined;
  const known = merged?.find((entry) => entry.provider === provider && entry.id === model.id);
  const tags = rulesAbout(rules, provider, model, known?.family ?? null);
  // Opens Routing at the rule the link names.
  const showRule = (rule: OverrideRule) => {
    openSettings("routing");
    revealSetting(ruleSentence(rule, groups, projects));
  };

  let description: ReactNode;
  if (fresh) {
    description = (
      <span className="flex flex-wrap items-center gap-x-2">
        Brigadier gives it no tasks until you allow it
        <button type="button" className="text-link hover:underline" onClick={() => showRule(newRule)}>
          Allow in Routing
        </button>
      </span>
    );
  } else if (noWork) {
    description = NO_WORK_NOTE;
  } else if (expensive) {
    description = (
      <span className="flex items-start gap-1.5">
        <InfoCircle aria-hidden className="size-icon-xs mt-0.5 shrink-0" />
        {EXPENSIVE_NOTE}
      </span>
    );
  } else if (known && known.tier !== "unrated") {
    description = [TIER_LABELS[known.tier], bestFor(known)].filter(Boolean).join(" · ");
  } else {
    description = model.description || null;
  }

  return (
    <div
      data-slot="provider-model"
      data-setting={model.displayName}
      className="flex items-center justify-between gap-6 px-4 py-3"
    >
      <div className="flex min-w-0 flex-col gap-1">
        <span className="flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-1">
          <span className="text-sm font-medium">{model.displayName}</span>
          <span className="text-muted-foreground font-mono text-xs">
            {model.id}
            {model.isDefault && " · default"}
          </span>
          {fresh && <span className="text-warning text-xs">New</span>}
          {tags.map((rule) => (
            <button
              key={rule.id}
              type="button"
              title={`${ruleSentence(rule, groups, projects)}: open it in Routing`}
              className="text-foreground/65 hover:text-foreground text-xs hover:underline"
              onClick={() => showRule(rule)}
            >
              {ruleTag(rule)} · rule in Routing
            </button>
          ))}
        </span>
        {description && <span className="text-foreground/65 text-label">{description}</span>}
        {toggle.error && (
          <p role="alert" className="text-destructive text-xs">
            {toggle.error}
          </p>
        )}
      </div>
      <SettingsSwitch
        label={`Use ${model.displayName}`}
        checked={available}
        disabled={toggle.busy}
        onCheckedChange={(next) => toggle.run(() => setModelAvailable(ref, next))}
      />
    </div>
  );
}
