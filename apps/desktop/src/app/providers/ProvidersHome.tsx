import { useEffect, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { useRevealedEmails } from "@/app/providers/AccountRows";
import { AgentPage, AgentSettingsPage } from "@/app/providers/AgentPage";
import { AgentsOverview } from "@/app/providers/AgentsOverview";
import { CheckAgainButton, CheckedNote } from "@/app/providers/parts";
import { SettingsPage } from "@/app/settings/parts";
import { useReveal } from "@/app/settings/reveal";
import { useNow } from "@/hooks/use-now";
import type { ProviderKind } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { useModelGroups } from "@/lib/setup";
import { refreshProviders } from "@/state/actions";
import { accountsOf, loadAccounts, refreshAccounts } from "@/state/accounts";
import { useApp } from "@/state/store";
import { useUsage, useUsageRefresh } from "@/state/usage";

const TITLE = "Providers";
const DESCRIPTION = "The coding agents Brigadier runs, the accounts you use with them, and which models do what.";

/**
 * Settings › Providers, drawn to the user's own design: the agents at a glance, and a page for
 * each (its installation, accounts and models) that a row opens and Providers goes back from.
 */
export function ProvidersHome() {
  // The models' plain descriptions come with the usage view.
  useUsageRefresh();
  const merged = useUsage((s) => s.view?.models ?? null);
  const overviews = useApp((s) => s.providers.view?.providers);
  const accounts = useApp((s) => s.accounts?.accounts);
  const groups = useModelGroups();
  const revealed = useRevealedEmails();
  const now = useNow(30_000);
  const refresh = useAction();
  const [open, setOpen] = useState<{ provider: ProviderKind; signIn: boolean } | null>(null);
  useEffect(() => {
    loadAccounts().catch((error: unknown) => console.error("reading the accounts failed", error));
  }, []);
  // Search's rows for a model are on its agent's page: that page opens.
  const revealing = useReveal((s) => s.label);
  const revealedAgent = revealing
    ? overviews?.find((entry) => entry.models?.models.some((model) => model.displayName === revealing))?.provider
    : undefined;
  if (revealedAgent && revealedAgent !== open?.provider) setOpen({ provider: revealedAgent, signIn: false });

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
            groups={groups}
            now={now}
            revealed={revealed}
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
            {checkedAtMs > 0 && <CheckedNote>Checked {formatAgo(checkedAtMs, now)}</CheckedNote>}
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
