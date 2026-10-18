import { Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { agentState } from "@/app/onboarding/SetupTerminal";
import { useRevealedEmails } from "@/app/providers/AccountRows";
import { ProviderPanel } from "@/app/providers/ProviderPanel";
import { SettingsButton, SettingsPage, SettingsSwitch } from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useNow } from "@/hooks/use-now";
import type { ProviderKind, ProviderOverview } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { PROVIDERS, VENDOR_LABELS } from "@/lib/routing";
import { useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { refreshProviders } from "@/state/actions";
import { accountsOf, loadAccounts, refreshAccounts } from "@/state/accounts";
import { providerOn } from "@/state/providers";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";
import { useUsage, useUsageRefresh } from "@/state/usage";

/** The Providers page's rows, for Settings search; the page renders this copy. */
export const PROVIDERS_ROWS = {
  switch: {
    label: "Switch to another account when one runs out",
    description:
      "A chat that hits its account's limit carries on with your next account, then with the other provider.",
  },
  agents: {
    label: "Agents",
    description:
      "The coding agents that do Brigadier's work with your own subscriptions, each with a switch",
    target: "Accounts",
  },
  accounts: {
    label: "Accounts",
    description: "Each agent's accounts: who is signed in, the plan, how much of each limit is used, and Use",
  },
  usage: {
    label: "Usage",
    description: "How much of each account's 5-hour, weekly and model limits is used, and when each resets",
    target: "Accounts",
  },
  add: {
    label: "Add account",
    description: "Sign in to another Claude Code or Codex account",
    target: "Accounts",
  },
  own: {
    label: "This computer's login",
    description: "Also used by your terminal",
    target: "Accounts",
  },
  installed: {
    label: "Installed agents",
    description: "Where each agent is installed, and its version",
    target: "Accounts",
  },
  models: {
    label: "Models",
    description:
      "Each model's switch: on, it shows in the model pickers and Brigadier can give it work",
  },
} as const;

/** A dot for how an agent stands: ready, needing a step, or off. */
function stateDot(on: boolean, overview: ProviderOverview | undefined): string {
  if (!on) return "bg-muted-foreground/50";
  const state = agentState(overview);
  if (state === "ready") return "bg-success";
  if (state === "checking") return "bg-muted-foreground/50";
  return "bg-warning";
}

/**
 * Settings › Providers: the switch for moving chats between accounts, then a tab per agent with
 * how it stands and its switch, its accounts with how much of each limit is used, and its
 * models. Who does which work is on Routing.
 */
export function ProvidersPage() {
  // The models' plain descriptions come with the usage view.
  useUsageRefresh();
  const view = useUsage((s) => s.view);
  const overviews = useApp((s) => s.providers.view?.providers);
  const accounts = useApp((s) => s.accounts?.accounts);
  const settings = useApp((s) => s.settings);
  const groups = useModelGroups();
  const revealed = useRevealedEmails();
  const now = useNow(30_000);
  const refresh = useAction();
  const switching = useAction();
  const [tab, setTab] = useState<ProviderKind>("claude");
  // Search's rows are on every tab; the one shown keeps them.
  const revealing = useReveal((s) => s.label);
  useEffect(() => {
    loadAccounts().catch((error: unknown) => console.error("reading the accounts failed", error));
  }, []);
  const revealedTab = revealing
    ? overviews?.find((entry) => entry.models?.models.some((model) => model.displayName === revealing))?.provider
    : undefined;
  if (revealedTab && revealedTab !== tab) setTab(revealedTab);

  const checkedAtMs = Math.max(
    0,
    ...(overviews ?? []).map((overview) => overview.checkedAtMs ?? 0),
    ...(accounts ?? []).map((account) => account.checkedAtMs ?? 0),
  );
  const merged = view?.models ?? null;

  return (
    <SettingsPage
      title="Providers"
      description="The coding agents Brigadier runs, your accounts with them, and which of their models do what."
      actions={
        <>
          {checkedAtMs > 0 && (
            <span className="text-muted-foreground flex items-center gap-1.5 text-label">
              <span aria-hidden className="bg-success size-1.5 rounded-full" />
              Checked {formatAgo(checkedAtMs, now)}
            </span>
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
      <div className="-mt-3 flex flex-col">
        <div
          data-setting={PROVIDERS_ROWS.switch.label}
          className="border-divider flex items-center justify-between gap-6 border-b px-1 pb-4"
        >
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="text-sm">{PROVIDERS_ROWS.switch.label}</span>
            <span className="text-foreground/65 text-xs">{PROVIDERS_ROWS.switch.description}</span>
            {switching.error && (
              <p role="alert" className="text-destructive text-xs">
                {switching.error}
              </p>
            )}
          </div>
          <SettingsSwitch
            label={PROVIDERS_ROWS.switch.label}
            checked={settings.switchAccounts}
            disabled={switching.busy}
            onCheckedChange={(next) => switching.run(() => setSetting("switchAccounts", next))}
          />
        </div>

        <Tabs value={tab} onValueChange={(next) => setTab(next as ProviderKind)} className="gap-0 pt-5">
          <TabsList variant="line" className="border-divider h-auto w-full justify-start gap-6 rounded-none border-b p-0">
            {PROVIDERS.map((provider) => {
              const overview = overviews?.find((entry) => entry.provider === provider);
              const plan = overview?.status?.plan;
              return (
                <TabsTrigger
                  key={provider}
                  value={provider}
                  className="h-auto flex-none gap-2 rounded-none px-1 pb-3 text-base font-normal after:-bottom-px data-[state=active]:font-medium"
                >
                  <span className="bg-foreground/8 rounded-nav flex size-6 items-center justify-center">
                    <ProviderGlyph provider={provider} className="size-icon-sm" />
                  </span>
                  {VENDOR_LABELS[provider]}
                  {plan && (
                    <Badge variant="secondary" className="text-2xs font-normal">
                      {plan.charAt(0).toUpperCase()}
                      {plan.slice(1)}
                    </Badge>
                  )}
                  <span
                    aria-hidden
                    className={cn("size-1.5 rounded-full", stateDot(providerOn(settings, provider), overview))}
                  />
                </TabsTrigger>
              );
            })}
          </TabsList>
          {PROVIDERS.map((provider) => (
            <TabsContent key={provider} value={provider}>
              <ProviderPanel
                provider={provider}
                overview={overviews?.find((entry) => entry.provider === provider)}
                accounts={accounts === undefined ? null : accountsOf(accounts, provider)}
                merged={merged}
                groups={groups}
                now={now}
                revealed={revealed}
              />
            </TabsContent>
          ))}
        </Tabs>
      </div>
    </SettingsPage>
  );
}
