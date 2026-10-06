import {
  Analytics,
  ChevronRight,
  Clock,
  ExclamationMarkCircle,
  Reload,
  Shuffle,
  Warning,
} from "@openai/apps-sdk-ui/components/Icon";
import { useState } from "react";

import { useAction } from "@/app/conversation/useAction";
import { SettingsButton, SettingsPage } from "@/app/settings/parts";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { WindowChart } from "@/app/usage/WindowChart";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible";
import { useNow } from "@/hooks/use-now";
import type {
  Heat,
  ProviderOverview,
  ProviderUsage,
  RoutingActivity,
  WindowHistory,
  WindowState,
  WindowTokens,
} from "@/ipc/generated";
import { formatAgo, formatCountdown, formatTokens } from "@/lib/format";
import {
  byLength,
  choiceName,
  formatResetAt,
  HEAT_LABELS,
  HEAT_TONES,
  limitPhrase,
  PROVIDERS,
  VENDOR_LABELS,
  windowScope,
} from "@/lib/routing";
import { modelName, useModelGroups } from "@/lib/setup";
import { cn } from "@/lib/utils";
import { openConversation, refreshProviders } from "@/state/actions";
import { useApp } from "@/state/store";
import { loadUsage, useUsage, useUsageRefresh } from "@/state/usage";

/**
 * The Usage page: each agent's usage windows, a row each (how much is used, when it resets,
 * where it's heading; opening one shows its history and Brigadier's own use of it), what
 * routing did about them lately.
 */
export function UsagePage() {
  useUsageRefresh();
  const view = useUsage((s) => s.view);
  const error = useUsage((s) => s.error);
  const loading = useUsage((s) => s.loading);
  const overviews = useApp((s) => s.providers.view?.providers);
  const groups = useModelGroups();
  const now = useNow(30_000);
  const refresh = useAction();

  return (
    <SettingsPage
      wide
      title="Usage"
      description="How much of each agent's usage is left, and where it's heading."
      actions={
        <>
          {view && (
            <span className="text-muted-foreground text-xs">
              Updated {formatAgo(view.atMs, now)}
            </span>
          )}
          <SettingsButton
            aria-label="Read usage again"
            disabled={loading || refresh.busy}
            onClick={() =>
              // A fresh read from each agent; its results arrive as provider checks.
              refresh.run(() => Promise.all([refreshProviders(), loadUsage()]))
            }
          >
            <Reload className={loading ? "animate-spin motion-reduce:animate-none" : undefined} />
            Refresh
          </SettingsButton>
        </>
      }
    >
      <div className="@container flex flex-col gap-8">
        {error && (
          <p role="alert" className="text-destructive text-sm">
            Couldn't read usage: {error}
          </p>
        )}
        {!view && !error && (
          <div className="text-muted-foreground flex flex-col items-center gap-2 py-12 text-sm">
            <Analytics className="size-icon-lg" />
            Reading usage…
          </div>
        )}

        {view && (
          <>
            <section aria-label="Usage windows" className="flex flex-col gap-4">
              {PROVIDERS.map((provider) => (
                <ProviderSection
                  key={provider}
                  usage={view.providers.find((entry) => entry.provider === provider)}
                  overview={overviews?.find((entry) => entry.provider === provider)}
                  overviews={overviews}
                  groups={groups}
                  now={now}
                  provider={provider}
                />
              ))}
            </section>
            <ActivitySection activity={view.activity} groups={groups} now={now} />
          </>
        )}
      </div>
    </SettingsPage>
  );
}

// ----- providers -------------------------------------------------------------------------

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

function missingReason(overview: ProviderOverview | undefined, usage: ProviderUsage | undefined) {
  const status = overview?.status;
  if (status && !status.path) return "Not installed.";
  if (status && !status.loggedIn) return "Not signed in.";
  if (!usage?.quota) return "No usage read yet.";
  if (usage.quota.windows.length === 0) return "It reports no usage windows.";
  return null;
}

/** An agent's card: its plan and heat, then a row per usage window. */
function ProviderSection({
  provider,
  usage,
  overview,
  overviews,
  groups,
  now,
}: {
  provider: ProviderUsage["provider"];
  usage: ProviderUsage | undefined;
  overview: ProviderOverview | undefined;
  overviews: readonly ProviderOverview[] | undefined;
  groups: readonly ModelGroup[];
  now: number;
}) {
  const quota = usage?.quota ?? null;
  const plan = overview?.status?.plan;
  const missing = missingReason(overview, usage);
  const windows = (quota?.windows ?? []).toSorted((a, b) => byLength(a.window, b.window));
  return (
    <div className="bg-card border-divider rounded-settings flex flex-col border">
      <div className="flex items-center gap-2 px-4 pt-3.5 pb-1">
        <ProviderGlyph provider={provider} className="size-icon-md shrink-0" />
        <h2 className="text-label min-w-0 flex-1 truncate font-medium">
          {PROVIDER_LABELS[provider]}
          {plan && (
            <span className="text-muted-foreground font-normal">
              {" "}
              · {plan.charAt(0).toUpperCase()}
              {plan.slice(1)}
            </span>
          )}
        </h2>
        {quota?.observedAtMs != null && (
          <span className="text-muted-foreground text-2xs">
            Read {formatAgo(quota.observedAtMs, now)}
          </span>
        )}
        {quota && <HeatBadge heat={quota.heat} />}
      </div>
      {quota?.limit && (
        <p className="text-destructive px-4 pt-1 text-sm">
          {VENDOR_LABELS[provider]} {limitPhrase(quota.limit, provider, overviews, now)}
          {quota.limit.resetsAtMs !== null &&
            ` · ${formatCountdown(quota.limit.resetsAtMs, now)} left`}
          .
        </p>
      )}
      {usage?.balancing && (
        <p className="text-muted-foreground flex items-start gap-1.5 px-4 pt-1 text-xs">
          <Shuffle aria-hidden className="size-icon-xs mt-0.5 shrink-0" />
          <span>{usage.balancing}</span>
        </p>
      )}
      {missing ? (
        <p className="text-muted-foreground px-4 pt-1 pb-3.5 text-xs">{missing}</p>
      ) : (
        <div className="flex flex-col py-1.5">
          {windows.map((state) => (
            <WindowRow
              key={state.window.id}
              provider={provider}
              state={state}
              history={usage?.history.find((entry) => entry.windowId === state.window.id)}
              tokens={usage?.tokens.find((entry) => entry.windowId === state.window.id)}
              groups={groups}
              now={now}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * "≈72% at reset", "Runs out ≈20:40, 30m before reset", at the recent rate. None once the
 * window is at its limit: it has run out already, and its percentage says so.
 */
function forecastLine(state: WindowState, now: number): string | null {
  const { forecast, window, heat } = state;
  if (!forecast || heat === "limited") return null;
  if (forecast.runsOutAtMs !== null && window.resetsAtMs !== null) {
    return `Runs out ≈${formatResetAt(forecast.runsOutAtMs, now)}, ${formatCountdown(
      window.resetsAtMs,
      forecast.runsOutAtMs,
    )} before reset, at the recent rate`;
  }
  return `≈${Math.round(forecast.projectedAtReset)}% at reset at the recent rate`;
}

/**
 * A usage window on one line: its name, a bar, how much is used and when it resets; under it
 * where it's heading. Opening it shows the window's history and Brigadier's own use of it.
 */
function WindowRow({
  provider,
  state,
  history,
  tokens,
  groups,
  now,
}: {
  provider: ProviderUsage["provider"];
  state: WindowState;
  history: WindowHistory | undefined;
  tokens: WindowTokens | undefined;
  groups: readonly ModelGroup[];
  now: number;
}) {
  const [open, setOpen] = useState(false);
  const { window, forecast, heat } = state;
  const used = Math.round(Math.min(100, Math.max(0, window.usedPercent)));
  const scope = windowScope(window);
  const estimate = forecastLine(state, now);
  const warm = heat === "hot" || heat === "limited";
  const span =
    window.resetsAtMs !== null && window.windowMinutes !== null
      ? { start: window.resetsAtMs - window.windowMinutes * 60_000, end: window.resetsAtMs }
      : null;
  return (
    <Collapsible open={open} onOpenChange={setOpen} data-slot="usage-window">
      <CollapsibleTrigger className="hover:bg-foreground/4 group flex w-full flex-col gap-1 px-4 py-2 text-start">
        <span className="flex w-full items-center gap-4">
          <span className="flex w-36 min-w-0 shrink items-center gap-1.5">
            <ChevronRight
              aria-hidden
              className="text-muted-foreground size-icon-xs shrink-0 transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
            />
            <span className="text-label truncate" title={window.label}>
              {window.label}
            </span>
          </span>
          <span
            role="meter"
            aria-label={`${window.label} used`}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={used}
            className="bg-muted rounded-capsule h-1.5 min-w-12 flex-1 overflow-hidden"
          >
            <span
              className={cn("block h-full", warm ? HEAT_TONES[heat].fill : "bg-muted-foreground/60")}
              style={{ width: `${used}%` }}
            />
          </span>
          <span className="flex shrink-0 items-baseline justify-end gap-2 text-xs tabular-nums">
            <span
              className={cn("text-label w-20 text-end", warm ? HEAT_TONES[heat].text : "text-foreground")}
            >
              {used}% used
            </span>
            {window.resetsAtMs !== null && (
              <span
                className="text-muted-foreground w-28 text-end"
                title={`Resets ${formatResetAt(window.resetsAtMs, now)}`}
              >
                resets in {formatCountdown(window.resetsAtMs, now)}
              </span>
            )}
          </span>
        </span>
        {(scope || estimate) && (
          <span className="text-muted-foreground flex min-w-0 max-w-full gap-1.5 ps-5 text-xs">
            {scope && <span className="truncate">{scope}</span>}
            {scope && estimate && <span aria-hidden>·</span>}
            {estimate && (
              <span className={cn("truncate", forecast?.runsOutAtMs != null && "text-warning")}>
                {estimate}
              </span>
            )}
          </span>
        )}
      </CollapsibleTrigger>
      <CollapsibleContent className="flex flex-col gap-3 px-4 pt-1 pb-3 ps-9">
        {window.resetsAtMs !== null && (
          <p className="text-muted-foreground text-xs">
            Resets {formatResetAt(window.resetsAtMs, now)}
          </p>
        )}
        {span && (
          <WindowChart
            samples={history?.samples ?? []}
            startMs={span.start}
            endMs={span.end}
            nowMs={now}
            usedPercent={window.usedPercent}
            forecast={heat === "limited" ? null : forecast}
            label={window.label}
          />
        )}
        {tokens && <TokensView provider={provider} tokens={tokens} groups={groups} />}
      </CollapsibleContent>
    </Collapsible>
  );
}

/** Rows shown per list of Brigadier's use. */
const TOP = 5;

/** Brigadier's own tokens in the window: a total that opens on the split by model and conversation. */
function TokensView({
  provider,
  tokens,
  groups,
}: {
  provider: ProviderUsage["provider"];
  tokens: WindowTokens;
  groups: readonly ModelGroup[];
}) {
  const [open, setOpen] = useState(false);
  const total = tokens.byModel.reduce((sum, entry) => sum + entry.tokens, 0);
  if (total === 0) {
    return <p className="text-muted-foreground text-xs">Brigadier used none of it yet.</p>;
  }
  const models = tokens.byModel.slice(0, TOP).map((entry) => ({
    key: entry.model,
    label: modelName(groups, { provider, model: entry.model, effort: null }),
    detail: `${entry.turns} turns · ${formatTokens(entry.outputTokens)} out`,
    tokens: entry.tokens,
  }));
  const conversations = tokens.byConversation.slice(0, TOP).map((entry) => ({
    key: entry.conversationId,
    label: entry.title,
    detail: null,
    tokens: entry.tokens,
    onOpen: () => openConversation(entry.conversationId),
  }));
  return (
    <Collapsible open={open} onOpenChange={setOpen}>
      <CollapsibleTrigger className="text-muted-foreground hover:text-foreground group flex items-center gap-1 text-xs">
        <ChevronRight
          aria-hidden
          className="size-icon-xs transition-transform group-data-[state=open]:rotate-90 motion-reduce:transition-none"
        />
        Brigadier used {formatTokens(total)} tokens in this window
      </CollapsibleTrigger>
      <CollapsibleContent className="mt-2 grid gap-4 @md:grid-cols-2">
        <ShareList title="By model" rows={models} total={total} />
        <ShareList title="By conversation" rows={conversations} total={total} />
      </CollapsibleContent>
    </Collapsible>
  );
}

type ShareRow = {
  key: string;
  label: string;
  detail: string | null;
  tokens: number;
  onOpen?: () => void;
};

function ShareList({ title, rows, total }: { title: string; rows: ShareRow[]; total: number }) {
  return (
    <div className="flex min-w-0 flex-col gap-1.5">
      <h3 className="text-muted-foreground text-2xs">{title}</h3>
      {rows.length === 0 ? (
        <p className="text-muted-foreground text-xs">None.</p>
      ) : (
        <ul className="flex flex-col gap-1.5">
          {rows.map((row) => (
            <li key={row.key} className="flex flex-col gap-0.5" title={row.detail ?? undefined}>
              <div className="flex items-baseline gap-2 text-xs">
                {row.onOpen ? (
                  <button
                    type="button"
                    className="min-w-0 flex-1 truncate text-start hover:underline"
                    onClick={row.onOpen}
                  >
                    {row.label}
                  </button>
                ) : (
                  <span className="min-w-0 flex-1 truncate">{row.label}</span>
                )}
                <span className="text-muted-foreground shrink-0 tabular-nums">
                  {formatTokens(row.tokens)}
                </span>
              </div>
              <div aria-hidden className="bg-muted rounded-capsule h-1 overflow-hidden">
                <div
                  className="bg-chart-2 h-full"
                  style={{ width: `${Math.min(100, (row.tokens / total) * 100)}%` }}
                />
              </div>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ----- routing activity ------------------------------------------------------------------

function ActivitySection({
  activity,
  groups,
  now,
}: {
  activity: readonly RoutingActivity[];
  groups: readonly ModelGroup[];
  now: number;
}) {
  return (
    <section aria-labelledby="usage-activity" className="flex flex-col gap-2">
      <h2 id="usage-activity" className="text-sm font-medium">
        Recent activity
      </h2>
      {activity.length === 0 ? (
        <p className="text-muted-foreground text-sm">
          No hand-offs in the last 7 days, and nothing is waiting for quota.
        </p>
      ) : (
        <ul className="flex flex-col">
          {activity.map((entry) => (
            <ActivityRow
              key={`${entry.type}-${entry.conversationId}-${"taskId" in entry ? entry.taskId : ""}-${"atMs" in entry ? entry.atMs : ""}`}
              entry={entry}
              groups={groups}
              now={now}
            />
          ))}
        </ul>
      )}
    </section>
  );
}

function ActivityRow({
  entry,
  groups,
  now,
}: {
  entry: RoutingActivity;
  groups: readonly ModelGroup[];
  now: number;
}) {
  const Icon = entry.type === "waiting" ? Clock : Shuffle;
  let what: string;
  let detail: string;
  let when: string;
  switch (entry.type) {
    case "handoff":
      what = `Handed off: ${choiceName(groups, entry.from)} → ${choiceName(groups, entry.to)}`;
      detail = entry.cause;
      when = formatAgo(entry.atMs, now);
      break;
    case "waiting":
      what = "Waiting for quota";
      detail = entry.reason;
      when =
        entry.resetsAtMs !== null
          ? `resets in ${formatCountdown(entry.resetsAtMs, now)}`
          : `since ${formatAgo(entry.sinceMs, now)}`;
      break;
    case "fallback":
      what = `On ${choiceName(groups, entry.fallback.choice)} instead of ${choiceName(groups, entry.fallback.replaces)}`;
      detail = entry.fallback.reason;
      when =
        entry.fallback.untilMs !== null
          ? `until ${formatResetAt(entry.fallback.untilMs, now)}`
          : `since ${formatAgo(entry.fallback.sinceMs, now)}`;
      break;
  }
  return (
    <li
      data-slot="usage-activity"
      className="hover:bg-accent/50 rounded-control flex items-start gap-2 px-2 py-1.5"
    >
      <Icon
        aria-hidden
        className={cn(
          "size-icon-md mt-0.5 shrink-0",
          entry.type === "waiting" ? "text-warning" : "text-muted-foreground",
        )}
      />
      <div className="flex min-w-0 flex-1 flex-col">
        <button
          type="button"
          className="min-w-0 truncate text-start text-sm hover:underline"
          title={`Open ${entry.title}`}
          onClick={() => openConversation(entry.conversationId)}
        >
          {entry.title}
        </button>
        <span className="text-xs">{what}</span>
        <span className="text-muted-foreground truncate text-xs" title={detail}>
          {detail}
        </span>
      </div>
      <span className="text-muted-foreground shrink-0 text-xs tabular-nums">{when}</span>
    </li>
  );
}
