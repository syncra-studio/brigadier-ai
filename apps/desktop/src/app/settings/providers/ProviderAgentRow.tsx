import type { KeyboardEvent, MouseEvent } from "react";

import { useAction } from "@/app/conversation/useAction";
import { PlanChip, ROW_TITLE, UsageMeter } from "@/app/settings/providers/parts";
import { SettingsSwitch } from "@/app/settings/parts";
import { ErrorLine } from "@/app/shell/dialogs/fields";
import { agentState } from "@/app/setup/onboarding/SetupTerminal";
import { Button } from "@/components/controls";
import { ChevronRight, ProviderGlyph } from "@/components/icons";
import { StateDot } from "@/components/shared";
import type { AccountView, ProviderKind, ProviderOverview, QuotaWindow } from "@/ipc/generated";
import { used, windowTitle } from "@/lib/quota";
import { VENDOR_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { oneRowPerAccount } from "@/state/accounts";
import { setProviderOn } from "@/state/providers";

/** The window of the account in use that is nearest its limit, for the agent's row. */
export function busiestWindow(views: readonly AccountView[]): QuotaWindow | null {
  const rows = oneRowPerAccount(views);
  const inUse = rows.find(({ view }) => view.inUse)?.view ?? rows[0]?.view;
  const windows = inUse?.status?.loggedIn ? (inUse.quota?.windows ?? []) : [];
  return windows.reduce<QuotaWindow | null>((top, window) => (top && used(top) >= used(window) ? top : window), null);
}

/** One agent: its mark, name and plan, how it stands, its busiest limit, its switch and ›. */
export function ProviderAgentRow({
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
