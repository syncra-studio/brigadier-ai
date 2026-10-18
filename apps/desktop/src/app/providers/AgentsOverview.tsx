import { ArrowUpRight, ChevronRight, Plus } from "@openai/apps-sdk-ui/components/Icon";
import { type KeyboardEvent, type MouseEvent, useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { ErrorLine } from "@/app/dialogs/fields";
import { agentState, SetupTerminal } from "@/app/onboarding/SetupTerminal";
import {
  AgentMark,
  PlanChip,
  ProvidersButton,
  ProvidersCard,
  ProvidersSection,
  SetUpButton,
  StateDot,
  UsageMeter,
} from "@/app/providers/parts";
import { SettingsSwitch } from "@/app/settings/parts";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import type { AccountView, ProviderKind, ProviderOverview, QuotaWindow } from "@/ipc/generated";
import { used, windowTitle } from "@/lib/quota";
import { PROVIDERS, VENDOR_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { openSettings, refreshProviders } from "@/state/actions";
import { accountsOf, oneRowPerAccount } from "@/state/accounts";
import { providerOn, setProviderOn } from "@/state/providers";
import { setSetting } from "@/state/settings";
import { useApp } from "@/state/store";

/** The overview's copy, for Settings search; the page renders this. */
export const OVERVIEW_ROWS = {
  switch: {
    label: "Switch to another account automatically",
    description:
      "A chat that hits its account's limit carries on with your next account, then with the other provider.",
  },
  notSetUp: "Agents Brigadier can run that aren't installed yet",
  routing: "Who does what is set in Routing",
} as const;

/** The CLI each agent runs, as its rows name it. */
export const CLI_NAMES: Record<ProviderKind, string> = { claude: "Claude Code CLI", codex: "Codex CLI" };

/**
 * Settings › Providers' first page: the fallback switch, the agents Brigadier runs (how each
 * stands, the busiest limit of the account in use, its switch; a row opens the agent's page),
 * the agents not installed yet with Set up, and Add another agent.
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
  // The agent being installed; its terminal closes by itself once the agent is found.
  const [installing, setInstalling] = useState<ProviderKind | null>(null);
  const overviewOf = (provider: ProviderKind) => overviews?.find((entry) => entry.provider === provider);
  const installed = PROVIDERS.filter((provider) => agentState(overviewOf(provider)) !== "install");
  const missing = PROVIDERS.filter((provider) => agentState(overviewOf(provider)) === "install");
  const settingUp = installing && missing.includes(installing) ? installing : null;

  return (
    <div data-slot="agents-overview" className="flex flex-col gap-10">
      <ProvidersSection title="Fallback">
        <ProvidersCard>
          <div data-setting={OVERVIEW_ROWS.switch.label} className="flex items-center gap-6 py-4">
            <div className="flex min-w-0 flex-1 flex-col gap-1">
              <span className="text-settings-text text-settings-body font-medium">
                {OVERVIEW_ROWS.switch.label}
              </span>
              <span className="text-settings-text-secondary text-settings-small">
                {OVERVIEW_ROWS.switch.description}
              </span>
              <ErrorLine error={switching.error} />
            </div>
            <SettingsSwitch
              label={OVERVIEW_ROWS.switch.label}
              checked={settings.switchAccounts}
              disabled={switching.busy}
              onCheckedChange={(next) => switching.run(() => setSetting("switchAccounts", next))}
            />
          </div>
        </ProvidersCard>
      </ProvidersSection>

      <ProvidersSection
        title="Agents"
        aside={
          <button
            type="button"
            className="text-settings-link flex items-center gap-1 hover:underline"
            onClick={() => openSettings("routing")}
          >
            {OVERVIEW_ROWS.routing}
            <ArrowUpRight aria-hidden className="size-3" />
          </button>
        }
      >
        <ProvidersCard inset="list">
          {installed.length === 0 ? (
            <p className="text-settings-text-secondary px-3 py-4 text-settings-body">
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
        </ProvidersCard>
      </ProvidersSection>

      <ProvidersSection title="Not set up" aside={OVERVIEW_ROWS.notSetUp}>
        <ProvidersCard inset="list">
          {missing.map((provider) => (
            <div key={provider} data-slot="agent-not-set-up" className="flex flex-col">
              <div className="flex items-center gap-3.5 px-3 py-3.5">
                <AgentMark provider={provider} idle />
                <div className="flex min-w-0 flex-1 flex-col gap-1">
                  <span className="text-settings-text text-settings-body font-medium">
                    {VENDOR_LABELS[provider]}
                  </span>
                  <span className="text-settings-text-secondary text-settings-small">
                    {CLI_NAMES[provider]} · not installed
                  </span>
                </div>
                <SetUpButton disabled={settingUp !== null} onClick={() => setInstalling(provider)}>
                  Set up
                </SetUpButton>
              </div>
              {settingUp === provider && (
                <div className="px-3 pb-3.5">
                  <SetupTerminal
                    provider={provider}
                    install
                    onClose={() => {
                      setInstalling(null);
                      void refreshProviders(provider).catch(() => {});
                    }}
                  />
                </div>
              )}
            </div>
          ))}
          <DropdownMenu modal={false}>
            <DropdownMenuTrigger asChild>
              <button
                type="button"
                className="text-settings-link flex w-full items-center gap-2 px-3 py-3.5 text-settings-body font-medium hover:underline"
              >
                <Plus aria-hidden className="size-3.5" />
                Add another agent
              </button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="start">
              {PROVIDERS.map((provider) => {
                const added = installed.includes(provider);
                return (
                  <DropdownMenuItem
                    key={provider}
                    disabled={added || settingUp !== null}
                    onSelect={() => setInstalling(provider)}
                  >
                    <AgentMark provider={provider} idle />
                    <span className="flex-1">{VENDOR_LABELS[provider]}</span>
                    {added && <span className="text-muted-foreground text-settings-meta">Added</span>}
                  </DropdownMenuItem>
                );
              })}
            </DropdownMenuContent>
          </DropdownMenu>
        </ProvidersCard>
      </ProvidersSection>
    </div>
  );
}

/** The window of the account in use that is nearest its limit, for the agent's row. */
export function busiestWindow(views: readonly AccountView[]): QuotaWindow | null {
  const rows = oneRowPerAccount(views);
  const inUse = rows.find(({ view }) => view.default)?.view ?? rows[0]?.view;
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
      className="hover:bg-settings-raised/40 flex cursor-default items-center gap-3.5 px-3 py-3.5 transition-colors"
    >
      <AgentMark provider={provider} dim={!on} />
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <span className="flex min-w-0 items-center gap-2">
          <span
            className={cn(
              "text-settings-body font-medium",
              on ? "text-settings-text" : "text-settings-text-secondary",
            )}
          >
            {label}
          </span>
          {status?.plan && <PlanChip plan={status.plan} />}
        </span>
        <span className="flex min-w-0 items-center gap-1.75">
          <StateDot tone={tone} />
          <span
            data-slot="agent-state"
            className={cn(
              "truncate text-settings-small",
              tone === "needs" ? "text-settings-warning" : "text-settings-text-secondary",
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
            <ProvidersButton
              onClick={(event) => {
                event.stopPropagation();
                onOpen(provider, true);
              }}
            >
              Sign in
            </ProvidersButton>
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
      <ChevronRight aria-hidden className="text-settings-text-tertiary size-4 shrink-0" />
    </div>
  );
}
