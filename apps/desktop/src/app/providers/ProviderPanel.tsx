import { CheckCircle, ChevronRight, InfoCircle, Stop } from "@openai/apps-sdk-ui/components/Icon";
import { type ReactNode, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { agentState, SetupTerminal } from "@/app/onboarding/SetupTerminal";
import { ProviderAccounts, type Revealed } from "@/app/providers/AccountRows";
import { bestFor } from "@/app/routing/models";
import { SettingsButton, SettingsSwitch } from "@/app/settings/parts";
import { revealSetting } from "@/app/settings/reveal";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { Badge } from "@/components/ui/badge";
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
  setProviderOn,
} from "@/state/providers";
import { useApp } from "@/state/store";

/** What an agent switched off means, on its status line. */
const OFF_NOTE = "Off: its models are hidden from the model picker, and it gets no work";

/** The line under a model Brigadier gives no tasks to whatever the user chooses. */
export const EXPENSIVE_NOTE =
  "Expensive: Brigadier uses it only where you pick it, in a chat, a session, your order or a rule";

/** The line under a model a "Don't give work" rule keeps from tasks. */
const NO_WORK_NOTE = "Gets no tasks: a rule in Routing keeps it to chats and sessions you pick it for";

/** A small heading over a part of the panel ("ACCOUNTS"). */
export function PanelHeading({ children }: { children: string }) {
  return (
    <h3
      data-setting={children}
      className="text-muted-foreground px-1 pt-6 pb-1 font-mono text-2xs font-medium tracking-eyebrow uppercase"
    >
      {children}
    </h3>
  );
}

/**
 * One agent's tab: how it stands (the one action that gets it working, its version, plan and
 * where it is installed) with its switch, then its accounts, then its models with a switch each.
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
  const toggle = useAction();
  const state = agentState(overview);
  const label = VENDOR_LABELS[provider];
  const needs = state === "install" || state === "signIn";
  // Installing or signing in the agent; its terminal closes by itself once that is done.
  const [setup, setSetup] = useState<{ install: boolean } | null>(null);
  const settingUp = setup && state === (setup.install ? "install" : "signIn") ? setup : null;
  const status = overview?.status;
  const facts = [
    status?.version && `v${status.version}`,
    status?.plan && `${status.plan.charAt(0).toUpperCase()}${status.plan.slice(1)} plan`,
    status?.path,
  ].filter(Boolean);

  return (
    <div data-slot="provider-panel" className="flex flex-col">
      <div className="border-divider flex min-h-14 items-center justify-between gap-6 border-b px-1 py-3">
        <div className="flex min-w-0 items-center gap-3 text-sm">
          {!on ? (
            <span className="text-muted-foreground">{OFF_NOTE}</span>
          ) : state === "ready" ? (
            <>
              <span className="text-success flex shrink-0 items-center gap-1.5">
                <CheckCircle aria-hidden className="size-icon-sm" />
                Ready
              </span>
              <span className="text-muted-foreground truncate font-mono text-xs" title={status?.path ?? undefined}>
                {facts.join(" · ")}
              </span>
            </>
          ) : (
            <>
              <span className="flex items-center gap-1.5">
                {needs && <span aria-hidden className="bg-warning size-1.5 shrink-0 rounded-full" />}
                {state === "install" ? (status?.guidance ?? providerStatusText(overview)) : providerStatusText(overview)}
              </span>
              {needs && (
                <SettingsButton disabled={settingUp !== null} onClick={() => setSetup({ install: state === "install" })}>
                  {state === "install" ? "Install" : "Sign in"}
                </SettingsButton>
              )}
            </>
          )}
        </div>
        <span className="text-muted-foreground flex shrink-0 items-center gap-3 font-mono text-xs">
          {on ? "On" : "Off"}
          <SettingsSwitch
            label={`Use ${label}`}
            checked={on}
            disabled={toggle.busy}
            onCheckedChange={(next) => toggle.run(() => setProviderOn(provider, next))}
          />
        </span>
      </div>
      {toggle.error && (
        <p role="alert" className="text-destructive px-1 pt-2 text-xs">
          {toggle.error}
        </p>
      )}
      {settingUp && (
        <div className="pt-3">
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

      <PanelHeading>Accounts</PanelHeading>
      <ProviderAccounts wide provider={provider} views={accounts} now={now} revealed={revealed} />

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
    <>
      <PanelHeading>Models</PanelHeading>
      {models.length === 0 ? (
        <p className="text-muted-foreground px-1 py-3.5 text-sm">
          {state === "checking"
            ? "Checking…"
            : state === "install"
              ? `Install ${label} to see its models.`
              : state === "signIn"
                ? `Sign in to ${label} to see its models.`
                : "Reading the models…"}
        </p>
      ) : (
        <div className="divide-divider flex flex-col divide-y">
          {current.map(row)}
          {older.length > 0 && (
            <Collapsible className="flex flex-col">
              <CollapsibleTrigger className="text-foreground/65 hover:text-foreground group flex w-full items-center gap-2 px-1 py-3.5 text-sm">
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
        </div>
      )}
    </>
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
 * What a model's row shows and does: its switch, its rule tags, and its line (New with Allow in
 * Routing, kept from tasks, Expensive, else how good it is and what for). A model Brigadier saw
 * after its agent's first list is tagged New: it gets no tasks until the user allows it in
 * Routing; one the user keeps from tasks says so. Fable models never get tasks.
 */
export function useModelRow(
  provider: ProviderKind,
  model: ModelInfo,
  merged: readonly MergedModel[] | null,
  groups: readonly ModelGroup[],
) {
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
  // What it is: its tier and what it's best for, else the agent's own words.
  const description =
    known && known.tier !== "unrated"
      ? [TIER_LABELS[known.tier], bestFor(known)].filter(Boolean).join(" · ")
      : model.description || null;
  const line: { kind: "new"; rule: OverrideRule } | { kind: "expensive" } | { kind: "text"; text: string } | null =
    newRule
      ? { kind: "new", rule: newRule }
      : noWork
        ? { kind: "text", text: NO_WORK_NOTE }
        : expensive
          ? { kind: "expensive" }
          : description
            ? { kind: "text", text: description }
            : null;
  return {
    available,
    toggle,
    fresh,
    tags,
    line,
    expensive,
    description,
    showRule,
    ruleTitle: (rule: OverrideRule) => `${ruleSentence(rule, groups, projects)}: open it in Routing`,
    tagText: ruleTag,
    setAvailable: (next: boolean) => toggle.run(() => setModelAvailable(ref, next)),
  };
}

/** A model: its name, id and whether it is the agent's default, its tags, its line, its switch. */
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
  const { available, toggle, fresh, tags, line, showRule, ruleTitle, setAvailable } = useModelRow(
    provider,
    model,
    merged,
    groups,
  );
  let description: ReactNode = null;
  if (line?.kind === "new") {
    description = (
      <span className="flex flex-wrap items-center gap-x-2">
        Brigadier gives it no tasks until you allow it
        <button type="button" className="text-link hover:underline" onClick={() => showRule(line.rule)}>
          Allow in Routing
        </button>
      </span>
    );
  } else if (line?.kind === "expensive") {
    description = (
      <span className="flex items-center gap-1.5">
        <InfoCircle aria-hidden className="size-icon-xs shrink-0" />
        {EXPENSIVE_NOTE}
      </span>
    );
  } else if (line) {
    description = line.text;
  }

  return (
    <div
      data-slot="provider-model"
      data-setting={model.displayName}
      className="flex items-center justify-between gap-6 px-1 py-3.5"
    >
      <div className="flex min-w-0 flex-col gap-1">
        <span className="flex min-w-0 flex-wrap items-center gap-x-2.5 gap-y-1">
          <span className="text-sm font-medium">{model.displayName}</span>
          <span className="text-muted-foreground font-mono text-xs">
            {model.id}
            {model.isDefault && " · default"}
          </span>
          {fresh && <Badge variant="warning">New</Badge>}
          {tags.map((rule) => (
            <button
              key={rule.id}
              type="button"
              title={ruleTitle(rule)}
              className="bg-foreground/6 text-foreground/65 hover:text-foreground rounded-nav flex items-center gap-1 px-1.5 py-0.5 text-2xs"
              onClick={() => showRule(rule)}
            >
              <Stop aria-hidden className="size-icon-xs" />
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
        onCheckedChange={setAvailable}
      />
    </div>
  );
}
