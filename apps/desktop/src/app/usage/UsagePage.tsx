import { ExclamationMarkCircle, Reload, Warning } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useState } from "react";

import { SettingsButton, SettingsCard, SettingsPage } from "@/app/settings/parts";
import { ActivityTab } from "@/app/usage/ActivityTab";
import {
  AccountHeader,
  missingReason,
  ProviderSections,
  useAccountView,
} from "@/app/usage/parts";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useNow } from "@/hooks/use-now";
import type {
  AccountUsage,
  Heat,
  ProviderOverview,
  ProviderUsage,
  WindowState,
} from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { left, leftTone, windowTitle } from "@/lib/quota";
import {
  byLength,
  formatResetAt,
  HEAT_LABELS,
  HEAT_TONES,
  limitPhrase,
  VENDOR_LABELS,
} from "@/lib/routing";
import { cn } from "@/lib/utils";
import { loadAccounts } from "@/state/accounts";
import { useApp } from "@/state/store";
import { refreshUsage, setActivityShown, useUsage, useUsageRefresh } from "@/state/usage";

type Tab = "limits" | "activity";

/**
 * The Usage page: how much of each account's usage is left (Limits), and how it was used over
 * the last days (Activity). Refresh checks every account again.
 */
export function UsagePage() {
  useUsageRefresh();
  const view = useUsage((s) => s.view);
  const error = useUsage((s) => s.error);
  const refreshing = useUsage((s) => s.refreshing);
  const refreshError = useUsage((s) => s.refreshError);
  const [tab, setTab] = useState<Tab>("limits");

  // Who each account is (its name, email and plan) comes from Settings → Providers' accounts.
  useEffect(() => {
    void loadAccounts().catch(() => {});
  }, []);
  useEffect(() => {
    setActivityShown(tab === "activity");
    return () => setActivityShown(false);
  }, [tab]);

  return (
    <SettingsPage
      title="Usage"
      description="How much of each account's usage is left, and how it was used."
      actions={
        <SettingsButton
          aria-label="Check every account again"
          aria-busy={refreshing}
          disabled={refreshing}
          onClick={() => void refreshUsage()}
        >
          <Reload
            data-slot="usage-refresh-spinner"
            data-spinning={refreshing}
            className={refreshing ? "animate-spin motion-reduce:animate-none" : undefined}
          />
          Refresh
        </SettingsButton>
      }
    >
      <Tabs value={tab} onValueChange={(next) => setTab(next as Tab)} className="gap-6">
        <TabsList aria-label="Usage">
          <TabsTrigger value="limits">Limits</TabsTrigger>
          <TabsTrigger value="activity">Activity</TabsTrigger>
        </TabsList>
        {(error || refreshError) && (
          <p role="alert" className="text-destructive text-sm">
            {refreshError ?? `Couldn't read usage: ${error}`}
          </p>
        )}
        {!view && !error ? (
          <p className="text-muted-foreground py-12 text-center text-sm">Reading usage…</p>
        ) : (
          view && (
            <>
              <TabsContent value="limits">
                <LimitsTab providers={view.providers} />
              </TabsContent>
              <TabsContent value="activity">
                <ActivityTab providers={view.providers} />
              </TabsContent>
            </>
          )
        )}
      </Tabs>
    </SettingsPage>
  );
}

// ----- shared ------------------------------------------------------------------------------

/** A heat as a label with its status color and, when it matters, an icon. */
export function HeatBadge({ heat, className }: { heat: Heat; className?: string }) {
  const Icon = heat === "limited" ? ExclamationMarkCircle : heat === "hot" ? Warning : null;
  return (
    <span className={cn("flex shrink-0 items-center gap-1 text-xs", HEAT_TONES[heat].text, className)}>
      {Icon ? (
        <Icon aria-hidden className="size-icon-xs" />
      ) : (
        <span aria-hidden className={cn("size-1.5 rounded-full", HEAT_TONES[heat].fill)} />
      )}
      {HEAT_LABELS[heat]}
    </span>
  );
}

// ----- limits ------------------------------------------------------------------------------

/** Per provider, a card per account: a row per usage window, with how much of it is left. */
function LimitsTab({ providers }: { providers: readonly ProviderUsage[] }) {
  const now = useNow(30_000);
  const overviews = useApp((s) => s.providers.view?.providers);
  return (
    <section aria-label="Usage windows" className="flex flex-col gap-10">
      <ProviderSections providers={providers}>
        {(usage) =>
          usage.accounts.map((account) => (
            <AccountLimits
              key={account.account.account ?? "own"}
              usage={account}
              overviews={overviews}
              now={now}
            />
          ))
        }
      </ProviderSections>
    </section>
  );
}

function AccountLimits({
  usage,
  overviews,
  now,
}: {
  usage: AccountUsage;
  overviews: readonly ProviderOverview[] | undefined;
  now: number;
}) {
  const view = useAccountView(usage);
  const missing = missingReason(view, usage);
  const quota = usage.quota;
  const provider = usage.account.provider;
  const windows = (quota?.windows ?? []).toSorted((a, b) => byLength(a.window, b.window));
  return (
    <SettingsCard>
      <AccountHeader usage={usage} now={now} />
      {quota?.limit && (
        <p className="text-destructive px-4 pt-1 text-xs">
          {VENDOR_LABELS[provider]} {limitPhrase(quota.limit, provider, overviews, now)}
          {quota.limit.resetsAtMs !== null &&
            ` · ${formatCountdown(quota.limit.resetsAtMs, now)} left`}
        </p>
      )}
      {missing ? (
        <p data-slot="usage-missing" className="text-muted-foreground px-4 pt-1 pb-3.5 text-xs">
          {missing}
        </p>
      ) : (
        <div className="flex flex-col pb-1.5">
          {windows.map((state) => (
            <WindowRow key={state.window.id} state={state} now={now} />
          ))}
        </div>
      )}
    </SettingsCard>
  );
}

/**
 * A usage window: its name; when it resets and how much is left; a bar of what is left. Amber
 * at 20% left or less, red at 10% or less or at its limit.
 */
function WindowRow({ state, now }: { state: WindowState; now: number }) {
  const { window, heat } = state;
  const percent = left(window);
  const tone = leftTone(percent, {
    limitReached: heat === "limited",
    normal: { text: "text-muted-foreground", fill: "bg-foreground/70" },
  });
  const title = windowTitle(window);
  return (
    <div data-slot="usage-window" data-level={tone.level} className="flex flex-col gap-1.5 px-4 py-2.5">
      <span className="text-label">{title}</span>
      <span className="text-muted-foreground flex items-baseline justify-between gap-4 text-xs tabular-nums">
        <span title={window.resetsAtMs !== null ? `Resets ${formatResetAt(window.resetsAtMs, now)}` : undefined}>
          {window.resetsAtMs !== null ? `Resets in ${formatCountdown(window.resetsAtMs, now)}` : ""}
        </span>
        <span data-slot="usage-left" className={tone.text}>
          {percent}% left
        </span>
      </span>
      <span
        role="meter"
        aria-label={`${title} left`}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent}
        className="bg-foreground/8 rounded-capsule h-1 w-full overflow-hidden"
      >
        <span
          data-slot="usage-bar"
          className={cn("rounded-capsule block h-full", tone.fill)}
          style={{ width: `${percent}%` }}
        />
      </span>
    </div>
  );
}
