import type { KeyboardEvent, MouseEvent } from "react";

import { useAction } from "@/app/conversation/useAction";

import { ErrorLine } from "@/app/shell/dialogs/fields";

import { ConnectStatus } from "@/app/setup/onboarding/ConnectStatus";

import { agentState } from "@/app/setup/onboarding/SetupTerminal";

import { PlanChip, ROW_LINE, ROW_TITLE, UsageMeter } from "@/app/settings/providers/parts";
import { SettingsRow, SettingsSection, SettingsSwitch } from "@/app/settings/parts";

import { Button } from "@/components/controls";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/overlays";

import { ArrowUpRight, ChevronRight, Plus, ProviderGlyph } from "@/components/icons";

import { SettingsCard, StateDot } from "@/components/shared";

import type { AccountView, ProviderKind, ProviderOverview, QuotaWindow } from "@/ipc/generated";

import { used, windowTitle } from "@/lib/quota";

import { PROVIDERS, VENDOR_LABELS } from "@/lib/routing";

import { cn } from "@/lib/utils";

import { openSettings } from "@/state/actions";

import { accountsOf, oneRowPerAccount } from "@/state/accounts";

import { connectAgent, connecting, useConnect } from "@/state/connect";

import { providerOn, setProviderOn } from "@/state/providers";

import { setSetting } from "@/state/settings";

import { useApp } from "@/state/store";

/** The overview's copy, for Settings search; the page renders this. */
export const OVERVIEW_ROWS = {
  switch: {
    label: "Switch to another agent automatically",
    description:
      "When every account of an agent is at its limit, work carries on with another agent. Off, it waits for the limit to reset.",
  },
  switchAccounts: {
    label: "Switch accounts automatically",
    description:
      "New work starts on the account whose weekly limit resets soonest, and moves on when one hits its limit. Use picks one account and turns this off.",
  },
  notSetUp: "Agents Brigadier can run that aren't installed yet",
  routing: "Who does what is set in Routing",
} as const;

/** The CLI each agent runs, as its rows name it. */
export const CLI_NAMES: Record<ProviderKind, string> = { claude: "Claude Code CLI", codex: "Codex CLI" };

/**
 * Settings › Providers' first page: the fallback switch, the agents Brigadier runs (how each
 * stands, the busiest limit of the account in use, its switch; a row opens the agent's page),
 * the agents not installed yet with Set up (Connect: installed by their own installer, then
 * signed in), and Add another agent.
 */
export function AgentsOverview({
  overviews,
  accounts,
  now,
  onOpen,
}: {
  overviews: readonly ProviderOverview[] | undefined;
  accounts: readonly AccountView[] | undefined;
  now: number;
  /** Opens an agent's page; `signIn` starts its sign-in there. */
  onOpen: (provider: ProviderKind, signIn?: boolean) => void;
}) {
  const settings = useApp((s) => s.settings);
  const switching = useAction();
  const overviewOf = (provider: ProviderKind) => overviews?.find((entry) => entry.provider === provider);
  const installed = PROVIDERS.filter((provider) => agentState(overviewOf(provider)) !== "install");
  const missing = PROVIDERS.filter((provider) => agentState(overviewOf(provider)) === "install");
  // The agents Connect is installing or signing in now.
  const connects = useConnect((s) => s.view?.agents);
  const settingUp = (provider: ProviderKind) =>
    connects?.some((agent) => agent.provider === provider && connecting(agent.progress)) ?? false;

  return (
    <div data-slot="agents-overview" className="flex flex-col gap-10">
      <SettingsSection title="Fallback" setting="Fallback">
        <SettingsCard>
          <SettingsRow
            label={OVERVIEW_ROWS.switch.label}
            description={OVERVIEW_ROWS.switch.description}
            error={switching.error}
          >
            <SettingsSwitch
              label={OVERVIEW_ROWS.switch.label}
              checked={settings.switchProviders}
              disabled={switching.busy}
              onCheckedChange={(next) => switching.run(() => setSetting("switchProviders", next))}
            />
          </SettingsRow>
        </SettingsCard>
      </SettingsSection>

      <SettingsSection
        title="Agents"
        setting="Agents"
        aside={
          <button
            type="button"
            className="text-link flex items-center gap-1 hover:underline"
            onClick={() => openSettings("routing")}
          >
            {OVERVIEW_ROWS.routing}
            <ArrowUpRight aria-hidden className="size-3" />
          </button>
        }
      >
        <SettingsCard>
          {installed.length === 0 ? (
            <p className="text-foreground/65 px-4 py-3 text-sm">
              No agent is installed yet.
            </p>
          ) : (
            installed.map((provider) => (
              <AgentRow
                key={provider}
                provider={provider}
                overview={overviewOf(provider)}
                accounts={accountsOf(accounts, provider)}
                on={providerOn(settings, provider)}
                now={now}
                onOpen={onOpen}
              />
            ))
          )}
        </SettingsCard>
      </SettingsSection>

      <SettingsSection title="Not set up" setting="Not set up" aside={OVERVIEW_ROWS.notSetUp}>
        <SettingsCard>
          {missing.map((provider) => (
            <div key={provider} data-slot="agent-not-set-up" className="flex flex-col">
              <div className="flex items-center gap-3.5 px-4 py-3">
                <ProviderGlyph provider={provider} tile="md" idle />
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className={ROW_TITLE}>{VENDOR_LABELS[provider]}</span>
                  <span className={ROW_LINE}>
                    {CLI_NAMES[provider]} · not installed
                  </span>
                </div>
                <Button size="sm" disabled={settingUp(provider)} onClick={() => void connectAgent(provider)}>
                  Set up
                </Button>
              </div>
              <ConnectStatus provider={provider} className="px-4 pb-3" />
            </div>
          ))}
          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className="text-link text-label flex w-full items-center gap-2 px-4 py-3 font-medium hover:underline"
              >
                <Plus aria-hidden className="size-icon-sm" />
                Add another agent
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {PROVIDERS.map((provider) => {
                const added = installed.includes(provider);
                return (
                  <DropdownMenuItem
                    key={provider}
                    disabled={added || settingUp(provider)}
                    onSelect={() => void connectAgent(provider)}
                  >
                    <ProviderGlyph provider={provider} tile="md" idle />
                    <span className="flex-1">{VENDOR_LABELS[provider]}</span>
                    {added && <span className="text-muted-foreground text-xs">Added</span>}
                  </DropdownMenuItem>
                );
              })}
            </DropdownMenuContent>
          </DropdownMenu>
        </SettingsCard>
      </SettingsSection>
    </div>
  );
}

/** The window of the account in use that is nearest its limit, for the agent's row. */
export function busiestWindow(views: readonly AccountView[]): QuotaWindow | null {
  const rows = oneRowPerAccount(views);
  const inUse = rows.find(({ view }) => view.inUse)?.view ?? rows[0]?.view;
  const windows = inUse?.status?.loggedIn ? (inUse.quota?.windows ?? []) : [];
  return windows.reduce<QuotaWindow | null>((top, window) => (top && used(top) >= used(window) ? top : window), null);
}

/** One agent: its mark, name and plan, how it stands, its busiest limit, its switch and ›. */
function AgentRow({
  provider,
  overview,
  accounts,
  on,
  now,
  onOpen,
}: {
  provider: ProviderKind;
  overview: ProviderOverview | undefined;
  accounts: AccountView[];
  on: boolean;
  now: number;
  onOpen: (provider: ProviderKind, signIn?: boolean) => void;
}) {
  const toggle = useAction();
  const state = agentState(overview);
  const status = overview?.status;
  const signedIn = oneRowPerAccount(accounts).filter(({ view }) => view.status?.loggedIn).length;
  const count = `${signedIn} ${signedIn === 1 ? "account" : "accounts"}`;
  const busiest = on && state === "ready" ? busiestWindow(accounts) : null;
  const label = VENDOR_LABELS[provider];

  let tone: "ready" | "needs" | "off" = "ready";
  let line: string;
  if (!on) {
    tone = "off";
    line = `Off · ${count}`;
  } else if (state === "checking") {
    tone = "off";
    line = "Checking…";
  } else if (state === "signIn") {
    tone = "needs";
    line = "Not signed in";
  } else {
    line = ["Ready", count, status?.version && `v${status.version}`].filter(Boolean).join(" · ");
  }
  const open = () => onOpen(provider);
  // The row's switch toggles the agent; it doesn't open its page.
  const onClick = (event: MouseEvent) => {
    if (event.target instanceof Element && event.target.closest('[data-slot="switch"]')) return;
    open();
  };
  const onKeyDown = (event: KeyboardEvent) => {
    if (event.target !== event.currentTarget || (event.key !== "Enter" && event.key !== " ")) return;
    event.preventDefault();
    open();
  };

  return (
    <div
      role="button"
      tabIndex={0}
      aria-label={`${label}: open its accounts and models`}
      data-slot="agent-row"
      data-setting={label}
      onClick={onClick}
      onKeyDown={onKeyDown}
      className="hover:bg-foreground/5 flex cursor-default items-center gap-3.5 px-4 py-3 transition-colors"
    >
      <ProviderGlyph provider={provider} tile="md" dim={!on} />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="flex min-w-0 items-center gap-2">
          <span className={cn(ROW_TITLE, !on && "text-foreground/65")}>
            {label}
          </span>
          {status?.plan && <PlanChip plan={status.plan} />}
        </span>
        <span className="flex min-w-0 items-center gap-1.75">
          <StateDot tone={tone} />
          <span
            data-slot="agent-state"
            className={cn(
              "truncate text-xs",
              tone === "needs" ? "text-warning" : "text-foreground/65",
            )}
          >
            {line}
          </span>
        </span>
        <ErrorLine error={toggle.error} />
      </div>
      <div className="flex w-37.5 shrink-0 items-center justify-end">
        {busiest ? (
          <UsageMeter window={busiest} now={now} compact label={`${windowTitle(busiest)} · in use`} />
        ) : (
          on &&
          state === "signIn" && (
            <Button
              variant="soft"
              size="sm"
              onClick={(event) => {
                event.stopPropagation();
                onOpen(provider, true);
              }}
            >
              Sign in
            </Button>
          )
        )}
      </div>
      <span aria-hidden className="w-2 shrink-0" />
      <SettingsSwitch
        label={`Use ${label}`}
        checked={on}
        disabled={toggle.busy}
        onCheckedChange={(next) => toggle.run(() => setProviderOn(provider, next))}
      />
      <ChevronRight aria-hidden className="text-muted-foreground size-icon-md shrink-0" />
    </div>
  );
}
