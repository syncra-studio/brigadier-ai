import { Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useMemo, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { type Revealed } from "@/app/providers/AccountRows";
import { ProviderCard } from "@/app/providers/ProviderCard";
import { learnedKey, ModelRow, ProjectPicker } from "@/app/routing/models";
import { RANKINGS_ROWS, RatingsSection } from "@/app/routing/RankingsSection";
import { AdvancedRouting, ROUTING_ROWS, RoutingKinds } from "@/app/routing/routing";
import {
  SettingsAdvanced,
  SettingsButton,
  SettingsCard,
  SettingsPage,
  SettingsRow,
  SettingsSection,
  SwitchSetting,
} from "@/app/settings/parts";
import { useNow } from "@/hooks/use-now";
import type { Learned } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { PROVIDERS, ruleSentence } from "@/lib/routing";
import { useModelGroups } from "@/lib/setup";
import { refreshProviders } from "@/state/actions";
import { accountsOf, loadAccounts, refreshAccounts } from "@/state/accounts";
import { reopenOnboarding } from "@/state/onboarding";
import { modelAvailable } from "@/state/providers";
import { useRankingsRefresh } from "@/state/rankings";
import { useApp } from "@/state/store";
import { useUsage, useUsageRefresh } from "@/state/usage";

/**
 * The Providers page's rows, for Settings search; the page renders this copy. It holds what the
 * Providers, Accounts and Routing pages did: a row whose place moved says where (`target`).
 */
export const PROVIDERS_ROWS = {
  switch: {
    label: "Switch to another account when one runs out",
    description:
      "A chat that hits its account's limit carries on with your next account. When every account is used up, Brigadier uses the other provider.",
  },
  agents: {
    label: "Agents",
    description:
      "The coding agents on this computer that do Brigadier's work, with your own subscriptions. Each has a switch.",
    target: PROVIDER_LABELS.claude,
  },
  accounts: {
    label: "Accounts",
    description: "Each agent's accounts: who is signed in, the plan, how much is used, and Use.",
    target: PROVIDER_LABELS.claude,
  },
  add: {
    label: "Add account",
    description: "Sign in to another Claude Code or Codex account.",
    target: PROVIDER_LABELS.claude,
  },
  own: {
    label: "This computer's login",
    description: "Also used by your terminal.",
    target: PROVIDER_LABELS.claude,
  },
  models: {
    label: "Use this model",
    description:
      "Each model's switch, formerly Available models: on, it shows in the model pickers and Brigadier can give it work.",
    target: "Models",
  },
  workers: {
    label: "Models Brigadier can give work to",
    description: "Now a rule under Advanced › Rules: “Don't give work” keeps a model from tasks.",
    target: ROUTING_ROWS.rules.label,
  },
  whoDoesWhat: ROUTING_ROWS.simple,
  advanced: {
    label: "Advanced",
    description:
      "Where each agent is installed, the first-run setup, the full routing order and backup models, settings per project or area, rules, each model's scores, and where the ratings come from.",
    target: "Installed agents",
  },
  installed: {
    label: "Installed agents",
    description: "Where each agent is installed, and its version.",
  },
  setup: {
    label: "Run the first-run setup again",
    description: "Walks through connecting the agents and adding projects.",
  },
  kinds: ROUTING_ROWS.kinds,
  modes: { ...ROUTING_ROWS.modes, target: ROUTING_ROWS.kinds.label },
  areas: { ...ROUTING_ROWS.areas, target: ROUTING_ROWS.kinds.label },
  reset: { ...ROUTING_ROWS.reset, target: ROUTING_ROWS.kinds.label },
  rules: ROUTING_ROWS.rules,
  details: {
    label: "Model details",
    description:
      "Brigadier scores each model 0–10 per kind of work from the model registry, then adjusts the scores from how its tasks went.",
  },
  ratings: RANKINGS_ROWS.ratings,
  refresh: { ...RANKINGS_ROWS.refresh, target: RANKINGS_ROWS.ratings.label },
  registry: {
    label: "Model registry",
    description: "The published list the models' ratings start from.",
    target: RANKINGS_ROWS.ratings.label,
  },
} as const;

/** What Advanced holds, by the labels Settings may bring into view there. */
const ADVANCED_LABELS: readonly string[] = [
  PROVIDERS_ROWS.installed.label,
  PROVIDERS_ROWS.setup.label,
  PROVIDERS_ROWS.kinds.label,
  PROVIDERS_ROWS.rules.label,
  PROVIDERS_ROWS.details.label,
  PROVIDERS_ROWS.ratings.label,
];

/** The account rows showing their email, kept only while the page is open. */
function useRevealed(): Revealed {
  const [shown, setShown] = useState<ReadonlySet<string>>(() => new Set());
  return useMemo(
    () => ({
      has: (key: string) => shown.has(key),
      toggle: (key: string) =>
        setShown((current) => {
          const next = new Set(current);
          if (!next.delete(key)) next.add(key);
          return next;
        }),
    }),
    [shown],
  );
}

/**
 * Settings › Providers: the switch for moving chats between accounts, then each agent (how it
 * stands and its switch, its accounts, its models), then who does each kind of work. Where the
 * agents are installed, the first-run setup and routing in full wait under Advanced.
 */
export function ProvidersPage() {
  // The models' plain descriptions and scores come with the usage view.
  useUsageRefresh();
  const view = useUsage((s) => s.view);
  const picked = useUsage((s) => s.projectId);
  const overviews = useApp((s) => s.providers.view?.providers);
  const accounts = useApp((s) => s.accounts?.accounts);
  const settings = useApp((s) => s.settings);
  const projects = useApp((s) => s.projects);
  // Names of every model (a rule may name one that isn't available).
  const groups = useModelGroups();
  const rankings = useRankingsRefresh();
  const revealed = useRevealed();
  const now = useNow(30_000);
  const refresh = useAction();
  useEffect(() => {
    loadAccounts().catch((error: unknown) => console.error("reading the accounts failed", error));
  }, []);

  const checkedAtMs = Math.max(
    0,
    ...(overviews ?? []).map((overview) => overview.checkedAtMs ?? 0),
    ...(accounts ?? []).map((account) => account.checkedAtMs ?? 0),
  );
  const merged = view?.models ?? null;
  // Adjustments read for another project than the one picked are not shown while it is read.
  const forPicked = view?.projectId === picked;
  const learned = useMemo(
    () =>
      new Map<string, Learned>(
        forPicked && view
          ? view.learned.map((entry) => [learnedKey(entry.provider, entry.model, entry.category), entry])
          : [],
      ),
    [view, forPicked],
  );
  const detailed = (merged ?? []).filter((model) =>
    modelAvailable(settings, { provider: model.provider, id: model.id }),
  );
  // A model's "Allow in Rules" brings its rule into view, opening Advanced for it.
  const advancedLabels = [
    ...ADVANCED_LABELS,
    ...settings.routingOverrides.map((rule) => ruleSentence(rule, groups, projects)),
  ];

  return (
    <SettingsPage
      wide
      title="Providers"
      description="The coding agents Brigadier runs, your accounts with them, and which of their models do what."
      actions={
        <>
          {checkedAtMs > 0 && (
            <span className="text-muted-foreground text-xs">Checked {formatAgo(checkedAtMs, now)}</span>
          )}
          <SettingsButton
            aria-label="Check the agents and accounts again"
            disabled={refresh.busy}
            onClick={() => refresh.run(() => Promise.all([refreshProviders(), refreshAccounts()]))}
          >
            <Reload className={refresh.busy ? "animate-spin motion-reduce:animate-none" : undefined} />
            Check again
          </SettingsButton>
        </>
      }
    >
      <SettingsCard>
        <SwitchSetting setting="switchAccounts" row={PROVIDERS_ROWS.switch} />
      </SettingsCard>

      {PROVIDERS.map((provider) => (
        <ProviderCard
          key={provider}
          provider={provider}
          overview={overviews?.find((entry) => entry.provider === provider)}
          accounts={accounts === undefined ? null : accountsOf(accounts, provider)}
          merged={merged}
          groups={groups}
          now={now}
          revealed={revealed}
        />
      ))}

      <RoutingKinds groups={groups} />

      <SettingsAdvanced description={PROVIDERS_ROWS.advanced.description} labels={advancedLabels}>
        <SettingsSection title={PROVIDERS_ROWS.installed.label} description={PROVIDERS_ROWS.installed.description}>
          <SettingsCard>
            {PROVIDERS.map((provider) => {
              const status = overviews?.find((entry) => entry.provider === provider)?.status;
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
            <SettingsRow label={PROVIDERS_ROWS.setup.label} description={PROVIDERS_ROWS.setup.description}>
              <SettingsButton onClick={() => reopenOnboarding()}>Run setup</SettingsButton>
            </SettingsRow>
          </SettingsCard>
        </SettingsSection>
        <AdvancedRouting groups={groups} />
        <SettingsSection title={PROVIDERS_ROWS.details.label} description={PROVIDERS_ROWS.details.description}>
          <SettingsCard>
            <SettingsRow
              label="Learned in"
              description="The project the details show the adjustments for, or all of them."
            >
              <ProjectPicker projectId={picked} />
            </SettingsRow>
          </SettingsCard>
          {merged === null ? (
            <p className="text-muted-foreground text-xs">Reading the models…</p>
          ) : (
            <SettingsCard>
              {detailed.map((model) => (
                <ModelRow
                  key={`${model.provider}/${model.id}`}
                  model={model}
                  learned={learned}
                  projectId={picked}
                  groups={groups}
                />
              ))}
            </SettingsCard>
          )}
        </SettingsSection>
        <RatingsSection rankings={rankings} merged={merged} registry={view?.registry ?? null} now={now} />
      </SettingsAdvanced>
    </SettingsPage>
  );
}
