import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";

import { agentState } from "@/app/onboarding/SetupTerminal";

import { AgentPage, AgentSettingsPage } from "@/app/providers/AgentPage";

import { AgentsOverview, OVERVIEW_ROWS } from "@/app/providers/AgentsOverview";

import { CheckAgainButton } from "@/app/providers/parts";

import { SettingsPage } from "@/app/settings/parts";

import { useReveal } from "@/app/settings/reveal";

import { StateDot } from "@/components/shared/state-dot";
import { useNow } from "@/hooks/use-now";

import type { ProviderKind } from "@/ipc/generated";

import { formatAgo } from "@/lib/format";

import { PROVIDERS } from "@/lib/routing";

import { refreshProviders } from "@/state/actions";

import { useAgentPageAsk } from "@/state/agentPage";

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

/** The rows on an agent's page: revealing one opens an agent's page. */
const AGENT_PAGE_ROWS: readonly string[] = [
  PROVIDERS_ROWS.installation.label,
  PROVIDERS_ROWS.accounts.label,
  PROVIDERS_ROWS.models.label,
];

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
  const [open, setOpen] = useState<{ provider: ProviderKind; signIn: boolean } | null>(null);
  useEffect(() => {
    loadAccounts().catch((error: unknown) => console.error("reading the accounts failed", error));
  }, []);
  // An agent's page asked for from elsewhere (the usage list's Sign in) opens once, then the
  // ask is cleared.
  const ask = useAgentPageAsk((s) => s.ask);
  const [taken, setTaken] = useState<typeof ask>(null);
  if (ask && ask !== taken) {
    setTaken(ask);
    setOpen(ask);
  }
  useEffect(() => {
    if (ask) useAgentPageAsk.setState({ ask: null });
  }, [ask]);
  // A search row on an agent's page opens it: a model's, its agent's; Installation, Accounts or
  // Models, the open one's, else the first agent installed. One on the overview goes back to it.
  const revealing = useReveal((s) => s.label);
  if (revealing) {
    const modelsAgent = overviews?.find((entry) =>
      entry.models?.models.some((model) => model.displayName === revealing),
    )?.provider;
    const installed = PROVIDERS.find((provider) => {
      const state = agentState(overviews?.find((entry) => entry.provider === provider));
      return state !== "install";
    });
    const agent =
      modelsAgent ?? (AGENT_PAGE_ROWS.includes(revealing) ? (open?.provider ?? installed ?? PROVIDERS[0]) : null);
    if (agent && agent !== open?.provider) setOpen({ provider: agent, signIn: false });
    else if (!agent && open) setOpen(null);
  }

  if (open) {
    const overview = overviews?.find((entry) => entry.provider === open.provider);
    return (
      <div data-slot="providers" className="contents">
        <AgentSettingsPage provider={open.provider} overview={overview} onBack={() => setOpen(null)}>
          <AgentPage
            key={open.provider}
            provider={open.provider}
            overview={overview}
            accounts={accounts === undefined ? null : accountsOf(accounts, open.provider)}
            merged={merged}
            now={now}
            signIn={open.signIn}
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
          onOpen={(provider, signIn = false) => setOpen({ provider, signIn })}
        />
      </SettingsPage>
    </div>
  );
}
