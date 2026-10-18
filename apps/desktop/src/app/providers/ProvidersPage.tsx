import { Reload } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { agentState } from "@/app/onboarding/SetupTerminal";
import { useRevealedEmails } from "@/app/providers/AccountRows";
import { ProviderPanel } from "@/app/providers/ProviderPanel";
import { SettingsButton, SettingsCard, SettingsPage, SettingsRow, SettingsSwitch } from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { useNow } from "@/hooks/use-now";
import type { ProviderKind, ProviderOverview } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { PROVIDERS, VENDOR_LABELS } from "@/lib/routing";
import { providerStatusText, useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { refreshProviders } from "@/state/actions";
import { accountsOf, loadAccounts, refreshAccounts } from "@/state/accounts";
import { providerOn, setProviderOn } from "@/state/providers";
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
    description: "Each agent's version, and whether it is signed in",
    target: "Agents",
  },
  models: {
    label: "Models",
    description:
      "Each model's switch: on, it shows in the model pickers and Brigadier can give it work",
  },
} as const;

/** How an agent stands, on its line in the list: "Signed in · Max plan", "Not installed", or Off. */
export function providerLine(on: boolean, overview: ProviderOverview | undefined): string {
  if (!on) return "Off";
  if (agentState(overview) !== "ready") return providerStatusText(overview);
  const plan = overview?.status?.plan;
  return plan ? `Signed in · ${plan.charAt(0).toUpperCase()}${plan.slice(1)} plan` : "Signed in";
}

/**
 * An agent in the page's list: its logo, name and version, how it stands, and its switch.
 * Pressing it shows its accounts and models beside the list.
 */
function ProviderItem({
  provider,
  overview,
  selected,
  onSelect,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  selected: boolean;
  onSelect: () => void;
}) {
  const on = useApp((s) => providerOn(s.settings, provider));
  const toggle = useAction();
  const label = VENDOR_LABELS[provider];
  const version = overview?.status?.version;
  const ready = on && agentState(overview) === "ready";
  return (
    <div
      data-slot="provider-item"
      className={cn(
        "border-divider flex items-start gap-3 border-b px-4 py-3.5 transition-colors",
        selected ? "bg-foreground/5" : "hover:bg-foreground/3",
      )}
    >
      <button
        type="button"
        aria-current={selected || undefined}
        onClick={onSelect}
        className="flex min-w-0 flex-1 items-start gap-3 text-start"
      >
        <ProviderGlyph provider={provider} className="size-icon-md mt-0.5 shrink-0" />
        <span className="flex min-w-0 flex-1 flex-col gap-0.5">
          <span className="flex min-w-0 items-baseline gap-2">
            <span className="text-label font-medium">{label}</span>
            {version && <span className="text-muted-foreground truncate font-mono text-2xs">v{version}</span>}
          </span>
          <span className={cn("text-xs", ready || !on ? "text-foreground/65" : "text-warning")}>
            {providerLine(on, overview)}
          </span>
          {toggle.error && (
            <span role="alert" className="text-destructive text-xs">
              {toggle.error}
            </span>
          )}
        </span>
      </button>
      <SettingsSwitch
        label={`Use ${label}`}
        checked={on}
        disabled={toggle.busy}
        onCheckedChange={(next) => toggle.run(() => setProviderOn(provider, next))}
      />
    </div>
  );
}

/**
 * Settings › Providers: the switch for moving chats between accounts, then one card with the
 * agents listed on its start (each with its version, how it stands and its switch) and the chosen
 * one's accounts and models beside them. Who does which work is on Routing.
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
      <SettingsCard>
        <SettingsRow
          label={PROVIDERS_ROWS.switch.label}
          description={PROVIDERS_ROWS.switch.description}
          error={switching.error}
        >
          <SettingsSwitch
            label={PROVIDERS_ROWS.switch.label}
            checked={settings.switchAccounts}
            disabled={switching.busy}
            onCheckedChange={(next) => switching.run(() => setSetting("switchAccounts", next))}
          />
        </SettingsRow>
      </SettingsCard>

      {/* The list beside the chosen agent's side where there is room for both, above it where not. */}
      <div data-setting={PROVIDERS_ROWS.agents.label} className="@container -mt-4">
        <div className="bg-card border-divider rounded-settings @providers-split:max-h-providers-card @providers-split:flex-row flex flex-col overflow-hidden border">
          <div className="border-divider @providers-split:w-providers-list @providers-split:border-e flex shrink-0 flex-col overflow-y-auto">
            {PROVIDERS.map((provider) => (
              <ProviderItem
                key={provider}
                provider={provider}
                overview={overviews?.find((entry) => entry.provider === provider)}
                selected={provider === tab}
                onSelect={() => setTab(provider)}
              />
            ))}
          </div>
          <div className="min-w-0 flex-1 overflow-y-auto">
            <ProviderPanel
              key={tab}
              provider={tab}
              overview={overviews?.find((entry) => entry.provider === tab)}
              accounts={accounts === undefined ? null : accountsOf(accounts, tab)}
              merged={merged}
              groups={groups}
              now={now}
              revealed={revealed}
            />
          </div>
        </div>
      </div>
    </SettingsPage>
  );
}
