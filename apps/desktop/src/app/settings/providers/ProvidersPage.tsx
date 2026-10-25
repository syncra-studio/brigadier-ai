import { useEffect } from "react";

import { useAction } from "@/app/conversation/useAction";

import { AgentPage, AgentSettingsPage } from "@/app/settings/providers/AgentPage";

import { AgentsOverview, OVERVIEW_ROWS } from "@/app/settings/providers/AgentsOverview";

import { CheckAgainButton } from "@/app/settings/providers/parts";

import { SettingsPage } from "@/app/settings/parts";

import { StateDot } from "@/components/shared";
import { useNow } from "@/hooks/use-now";

import { formatAgo } from "@/lib/format";

import { openSettings, refreshProviders } from "@/state/actions";

import { openAgentPage, useAgentPageAsk } from "@/state/agentPage";

import { accountsOf, loadAccounts, refreshAccounts } from "@/state/accounts";

import { useApp } from "@/state/store";

import { useUsage, useUsageRefresh } from "@/state/usage";

/** The page's rows, for Settings search; its parts render this copy. */
export const PROVIDERS_ROWS = {
  switch: OVERVIEW_ROWS.switch,
  agents: {
    label: "Agents",
    description: "The coding agents Brigadier runs: how each stands, the busiest limit in use, and its switch",
  },
  notSetUp: { label: "Not set up", description: `${OVERVIEW_ROWS.notSetUp}, with Set up` },
  addAgent: { label: "Add another agent", description: "Set up another agent Brigadier can run", target: "Not set up" },
  installation: {
    label: "Installation",
    description: "Each agent's CLI: its version with Update, and where it runs from with Change",
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
  switchAccounts: { ...OVERVIEW_ROWS.switchAccounts, target: "Accounts" },
  add: { label: "Add account", description: "Sign in to another Claude Code or Codex account", target: "Accounts" },
  own: { label: "This computer's login", description: "Also used by your terminal", target: "Accounts" },
  models: {
    label: "Models",
    description: "Each model's switch: on, it shows in the model pickers and Brigadier can give it work",
  },
} as const;

const TITLE = "Providers";
const DESCRIPTION = "The coding agents Brigadier runs, the accounts you use with them, and which models do what.";

/**
 * Settings › Providers, drawn to the user's own design: the agents at a glance, and a page for
 * each (its installation, accounts and models) that a row opens and Providers goes back from.
 */
export function ProvidersPage() {
  // The models' plain descriptions come with the usage view.
  useUsageRefresh();
  const merged = useUsage((s) => s.view?.models ?? null);
  const overviews = useApp((s) => s.providers.view?.providers);
  const accounts = useApp((s) => s.accounts?.accounts);
  const now = useNow(30_000);
  const refresh = useAction();
  const selection = useApp((s) => s.selection);
  const provider = selection.type === "settings" && selection.page === "providers" ? selection.detail?.provider : undefined;
  const ask = useAgentPageAsk((s) => s.ask);
  useEffect(() => {
    loadAccounts().catch((error: unknown) => console.error("reading the accounts failed", error));
  }, []);
  if (provider) {
    const overview = overviews?.find((entry) => entry.provider === provider);
    return (
      <div data-slot="providers" className="contents">
        <AgentSettingsPage provider={provider} overview={overview} onBack={() => openSettings("providers")}>
          <AgentPage
            key={provider}
            provider={provider}
            overview={overview}
            accounts={accounts === undefined ? null : accountsOf(accounts, provider)}
            merged={merged}
            now={now}
            signIn={ask?.under === selection && ask.provider === provider}
          />
        </AgentSettingsPage>
      </div>
    );
  }

  const checkedAtMs = Math.max(
    0,
    ...(overviews ?? []).map((overview) => overview.checkedAtMs ?? 0),
    ...(accounts ?? []).map((account) => account.checkedAtMs ?? 0),
  );
  return (
    <div data-slot="providers" className="contents">
      <SettingsPage
        title={TITLE}
        description={DESCRIPTION}
        actions={
          <>
            {checkedAtMs > 0 && (
              <span className="text-muted-foreground text-label flex items-center gap-1.5">
                <StateDot tone="ready" />
                Checked {formatAgo(checkedAtMs, now)}
              </span>
            )}
            <CheckAgainButton
              busy={refresh.busy}
              aria-label="Check the agents and accounts again"
              onClick={() => refresh.run(() => Promise.all([refreshProviders(), refreshAccounts()]))}
            />
          </>
        }
      >
        <AgentsOverview
          overviews={overviews}
          accounts={accounts}
          now={now}
          onOpen={openAgentPage}
        />
      </SettingsPage>
    </div>
  );
}
