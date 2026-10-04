import { ChevronRight, Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { agentState, SetupTerminal } from "@/app/onboarding/SetupTerminal";
import { modelSummary } from "@/app/routing/models";
import {
  SettingsAdvanced,
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SettingsSwitch,
} from "@/app/settings/parts";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { useNow } from "@/hooks/use-now";
import type { MergedModel, ModelInfo, ProviderKind, ProviderOverview } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { PROVIDERS } from "@/lib/routing";
import { providerStatusText } from "@/lib/setup";
import { refreshProviders } from "@/state/actions";
import { reopenOnboarding } from "@/state/onboarding";
import { modelAvailable, providerOn, setModelAvailable, setProviderOn } from "@/state/providers";
import { useApp } from "@/state/store";
import { useUsage, useUsageRefresh } from "@/state/usage";

/** The Providers page's rows, for Settings search; the page renders this copy. */
export const PROVIDERS_ROWS = {
  agents: {
    label: "Agents",
    description:
      "The coding agents on this computer that do Brigadier's work, with your own subscriptions.",
  },
  models: {
    label: "Available models",
    description:
      "Available models show in the model picker when you start a chat or session, and can be used on Routing.",
  },
  advanced: {
    label: "Advanced",
    description: "Where each agent is installed, its version, and the first-run setup.",
  },
  setup: {
    label: "Run the first-run setup again",
    description: "Walks through connecting the agents and adding projects.",
  },
} as const;

/** What an agent switched off means, on its row. */
const OFF_NOTE = "Off: its models are hidden from the model picker and Routing, and it gets no work.";

/**
 * The Providers page: the agents Brigadier runs, each with how it stands, the one action that
 * gets it working (Install or Sign in) and a switch; then which of their models are available.
 * Where they're installed and the first-run setup wait under Advanced.
 */
export function ProvidersPage() {
  // The models' plain descriptions ("Most capable · Best for …") come with the usage view.
  useUsageRefresh();
  const merged = useUsage((s) => s.view?.models);
  const overviews = useApp((s) => s.providers.view?.providers);
  const now = useNow(30_000);
  const refresh = useAction();
  // The agent being installed or signed in.
  const [setup, setSetup] = useState<{ provider: ProviderKind; install: boolean } | null>(null);
  const checkedAtMs = Math.max(0, ...(overviews ?? []).map((overview) => overview.checkedAtMs ?? 0));
  const overviewOf = (provider: ProviderKind) =>
    overviews?.find((entry) => entry.provider === provider);
  // Its terminal closes by itself once the agent is installed or signed in.
  const settingUp =
    setup && agentState(overviewOf(setup.provider)) === (setup.install ? "install" : "signIn")
      ? setup
      : null;

  return (
    <SettingsPage
      wide
      title="Providers"
      description="The coding agents Brigadier runs, such as Claude Code and Codex. Connect them and choose which of their models you use."
      actions={
        <>
          {checkedAtMs > 0 && (
            <span className="text-muted-foreground text-xs">Checked {formatAgo(checkedAtMs, now)}</span>
          )}
          <SettingsButton
            aria-label="Check the agents again"
            disabled={refresh.busy}
            onClick={() => refresh.run(refreshProviders)}
          >
            <Reload className={refresh.busy ? "animate-spin motion-reduce:animate-none" : undefined} />
            Check again
          </SettingsButton>
        </>
      }
    >
      <SettingsSection title={PROVIDERS_ROWS.agents.label} description={PROVIDERS_ROWS.agents.description}>
        <SettingsCard>
          {PROVIDERS.map((provider) => (
            <AgentRow
              key={provider}
              provider={provider}
              overview={overviewOf(provider)}
              busy={settingUp?.provider === provider}
              onSetUp={(install) => setSetup({ provider, install })}
            />
          ))}
        </SettingsCard>
        {settingUp && (
          <SetupTerminal
            key={`${settingUp.provider}:${settingUp.install}`}
            provider={settingUp.provider}
            install={settingUp.install}
            onClose={() => {
              setSetup(null);
              void refreshProviders(settingUp.provider).catch(() => {});
            }}
          />
        )}
      </SettingsSection>

      <SettingsSection title={PROVIDERS_ROWS.models.label} description={PROVIDERS_ROWS.models.description}>
        <AvailableModels overviews={overviews} merged={merged ?? null} />
      </SettingsSection>

      <SettingsAdvanced description={PROVIDERS_ROWS.advanced.description}>
        <SettingsSection title="Installed agents">
          <SettingsCard>
            {PROVIDERS.map((provider) => {
              const status = overviewOf(provider)?.status;
              return (
                <SettingsRow
                  key={provider}
                  label={PROVIDER_LABELS[provider]}
                  description={
                    status?.path ? (
                      <span className="font-mono" title={status.path}>
                        {status.path}
                      </span>
                    ) : (
                      // How to install it by hand, where the agent says.
                      (status?.guidance ?? "Not installed")
                    )
                  }
                >
                  {status?.version && (
                    <span className="text-muted-foreground font-mono text-xs">v{status.version}</span>
                  )}
                </SettingsRow>
              );
            })}
          </SettingsCard>
        </SettingsSection>
        <SettingsSection>
          <SettingsCard>
            <SettingsRow label={PROVIDERS_ROWS.setup.label} description={PROVIDERS_ROWS.setup.description}>
              <SettingsButton onClick={() => reopenOnboarding()}>Run setup</SettingsButton>
            </SettingsRow>
          </SettingsCard>
        </SettingsSection>
      </SettingsAdvanced>
    </SettingsPage>
  );
}

/**
 * One agent: its logo and name, how it stands in words, Install or Sign in when that's what it
 * needs, and "Use Claude Code".
 */
function AgentRow({
  provider,
  overview,
  busy,
  onSetUp,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  busy: boolean;
  onSetUp: (install: boolean) => void;
}) {
  const on = useApp((s) => providerOn(s.settings, provider));
  const toggle = useAction();
  const state = agentState(overview);
  const label = PROVIDER_LABELS[provider];
  const needs = state === "install" || state === "signIn";
  return (
    <SettingsRow
      label={
        <span className="flex items-center gap-2">
          <ProviderGlyph provider={provider} className="size-icon-md shrink-0" />
          {label}
        </span>
      }
      description={
        on ? (
          <span className="flex items-center gap-1.5">
            {needs && <span aria-hidden className="bg-warning size-1.5 shrink-0 rounded-full" />}
            {providerStatusText(overview)}
          </span>
        ) : (
          OFF_NOTE
        )
      }
      error={toggle.error}
    >
      {on && needs && (
        <SettingsButton disabled={busy} onClick={() => onSetUp(state === "install")}>
          {state === "install" ? "Install" : "Sign in"}
        </SettingsButton>
      )}
      <SettingsSwitch
        label={`Use ${label}`}
        checked={on}
        disabled={toggle.busy}
        onCheckedChange={(next) => toggle.run(() => setProviderOn(provider, next))}
      />
    </SettingsRow>
  );
}

/** Each agent that's on, with its models and a switch each; older models folded away. */
function AvailableModels({
  overviews,
  merged,
}: {
  overviews: readonly ProviderOverview[] | undefined;
  merged: readonly MergedModel[] | null;
}) {
  const disabled = useApp((s) => s.settings.disabledProviders);
  const on = PROVIDERS.filter((provider) => !disabled.includes(provider));
  if (on.length === 0) {
    return <p className="text-muted-foreground text-xs">Turn an agent on to choose its models.</p>;
  }
  return (
    <div className="flex flex-col gap-4 pt-2">
      {on.map((provider) => {
        const overview = overviews?.find((entry) => entry.provider === provider);
        const models = overview?.models?.models ?? [];
        const label = PROVIDER_LABELS[provider];
        const state = agentState(overview);
        const current = models.filter((model) => !model.legacy);
        const older = models.filter((model) => model.legacy);
        return (
          <div key={provider} className="flex flex-col gap-1.5">
            <h3 className="text-foreground/80 flex items-center gap-1.5 px-1 text-xs font-medium">
              <ProviderGlyph provider={provider} className="size-icon-sm shrink-0" />
              {label}
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
                {current.map((model) => (
                  <ModelRow key={model.id} provider={provider} model={model} merged={merged} />
                ))}
                {older.length > 0 && (
                  <Collapsible>
                    <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex w-full items-center gap-1.5 px-4 py-2.5 text-xs">
                      <ChevronRight
                        aria-hidden
                        className="size-icon-xs transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
                      />
                      {older.length} older {older.length === 1 ? "model" : "models"}
                    </CollapsibleTrigger>
                    <CollapsibleContent className="flex flex-col">
                      {older.map((model) => (
                        <ModelRow key={model.id} provider={provider} model={model} merged={merged} />
                      ))}
                    </CollapsibleContent>
                  </Collapsible>
                )}
              </SettingsCard>
            )}
          </div>
        );
      })}
    </div>
  );
}

/** A model: its name, what it's like in a few words, and whether it's available. */
function ModelRow({
  provider,
  model,
  merged,
}: {
  provider: ProviderKind;
  model: ModelInfo;
  merged: readonly MergedModel[] | null;
}) {
  const ref = { provider, id: model.id };
  const available = useApp((s) => modelAvailable(s.settings, ref));
  const toggle = useAction();
  const known = merged?.find((entry) => entry.provider === provider && entry.id === model.id);
  const summary = modelSummary(model, known);
  return (
    <SettingsRow label={model.displayName} description={summary || undefined} error={toggle.error}>
      <SettingsSwitch
        label={`${model.displayName} available`}
        checked={available}
        disabled={toggle.busy}
        onCheckedChange={(next) => toggle.run(() => setModelAvailable(ref, next))}
      />
    </SettingsRow>
  );
}
