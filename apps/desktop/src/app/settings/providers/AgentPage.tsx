import type { ReactNode } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/components/controls";
import { agentState } from "@/app/setup/onboarding/SetupTerminal";
import { ProviderAccounts, useOpenRows, useRevealedEmails } from "@/app/settings/providers/AccountRows";
import { OVERVIEW_ROWS } from "@/app/settings/providers/AgentsOverview";
import { CheckAgainButton, PlanChip } from "@/app/settings/providers/parts";
import { AgentModels } from "@/app/settings/providers/AgentModels";
import { Installation } from "@/app/settings/providers/Installation";
import { SettingsPage, SettingsSection, SettingsSwitch } from "@/app/settings/parts";
import { ProviderGlyph } from "@/components/icons";
import { SettingsCard, StateDot, SettingsRow } from "@/components/shared";
import type { AccountView, MergedModel, ProviderKind, ProviderOverview } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { VENDOR_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { refreshProviders } from "@/state/actions";
import { refreshAccounts, setSwitchAccounts, switchesAccounts } from "@/state/accounts";
import { providerOn, setProviderOn } from "@/state/providers";
import { useApp } from "@/state/store";

/** Who makes each agent's CLI, on its page's line. */
const MAKERS: Record<ProviderKind, string> = { claude: "Claude Code by Anthropic", codex: "Codex CLI" };

/** What an agent switched off means, on its page's line. */
const OFF_NOTE = "Off: its models are hidden from the model picker, and it gets no work";

/**
 * An agent's Settings page: Providers to go back, its logo, its name and plan, its line, Enabled
 * and Check again; `children` under it.
 */
export function AgentSettingsPage({
  provider,
  overview,
  onBack,
  children,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  onBack: () => void;
  children: ReactNode;
}) {
  const on = useApp((s) => providerOn(s.settings, provider));
  const toggle = useAction();
  const refresh = useAction();
  const state = agentState(overview);
  const label = VENDOR_LABELS[provider];
  const plan = overview?.status?.plan;
  const [tone, line] = !on
    ? (["off", OFF_NOTE] as const)
    : state === "ready"
      ? (["ready", `Ready · ${MAKERS[provider]}`] as const)
      : state === "checking"
        ? (["off", "Checking…"] as const)
        : (["needs", state === "install" ? "Not installed" : "Not signed in"] as const);
  return (
    <SettingsPage
      back={{ label: "Providers", onBack }}
      mark={<ProviderGlyph provider={provider} tile="lg" idle={state === "install"} dim={!on} />}
      title={
        <span className="flex items-center gap-2.5">
          {label}
          {plan && <PlanChip plan={plan} large />}
        </span>
      }
      description={
        <span data-slot="agent-line" className="flex flex-col gap-1">
          <span className="flex min-w-0 items-center gap-1.75">
            <StateDot tone={tone} />
            <span className={cn("truncate", tone === "needs" && "text-warning")}>{line}</span>
          </span>
          <ErrorLine error={toggle.error ?? refresh.error} />
        </span>
      }
      actions={
        <>
          <span className="text-foreground/65 text-label flex items-center gap-2.5">
            Enabled
            <SettingsSwitch
              label={`Use ${label}`}
              checked={on}
              disabled={toggle.busy}
              onCheckedChange={(next) => toggle.run(() => setProviderOn(provider, next))}
            />
          </span>
          <CheckAgainButton
            busy={refresh.busy}
            aria-label={`Check ${label} and its accounts again`}
            onClick={() => refresh.run(() => Promise.all([refreshProviders(provider), refreshAccounts()]))}
          />
        </>
      }
    >
      {children}
    </SettingsPage>
  );
}

/**
 * An agent's page under its header: where its CLI is installed (its version with Update, which
 * runs when a newer one is out, Install or Sign in when it needs one, and where it runs from with Change),
 * its accounts with every limit and when each resets, and its models with a switch each.
 */
export function AgentPage({
  provider,
  overview,
  accounts,
  merged,
  now,
  signIn = false,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  accounts: AccountView[] | null;
  merged: readonly MergedModel[] | null;
  now: number;
  /** Starts its sign-in when the page opens (the agents list's Sign in). */
  signIn?: boolean;
}) {
  const on = useApp((s) => providerOn(s.settings, provider));
  const switches = useApp((s) => switchesAccounts(s.settings, provider));
  const switching = useAction();
  // Its own: going back to the agents hides the emails again.
  const revealed = useRevealedEmails();
  const opened = useOpenRows();
  const checkedAtMs = overview?.checkedAtMs ?? null;
  return (
    <div data-slot="agent-page" className="flex flex-col gap-10">
      <SettingsSection
        title="Installation"
        setting="Installation"
        aside={checkedAtMs !== null ? `Checked ${formatAgo(checkedAtMs, now)}` : undefined}
      >
        <Installation provider={provider} overview={overview} signIn={signIn} />
      </SettingsSection>
      <SettingsSection title="Accounts" setting="Accounts" aside="Usage refreshes every few minutes">
        <SettingsCard>
          <SettingsRow
            label={OVERVIEW_ROWS.switchAccounts.label}
            description={OVERVIEW_ROWS.switchAccounts.description}
            error={switching.error}
          >
            <SettingsSwitch
              label={OVERVIEW_ROWS.switchAccounts.label}
              checked={switches}
              disabled={switching.busy}
              onCheckedChange={(next) => switching.run(() => setSwitchAccounts(provider, next))}
            />
          </SettingsRow>
          <ProviderAccounts provider={provider} views={accounts} now={now} revealed={revealed} opened={opened} />
        </SettingsCard>
      </SettingsSection>
      {on && <AgentModels provider={provider} overview={overview} merged={merged} />}
    </div>
  );
}
