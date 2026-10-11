import { Segmented, SettingsCard } from "@/app/settings/parts";
import { UsageChart } from "@/app/usage/UsageChart";
import {
  AccountHeader,
  missingReason,
  ProviderSections,
  useAccountView,
} from "@/app/usage/parts";
import type { ModelGroup } from "@/components/assistant-ui/elements/model-selector";
import { useNow } from "@/hooks/use-now";
import type { AccountActivity, AccountTokens, AccountUsage, ProviderUsage, WindowState } from "@/ipc/generated";
import { formatCountdown, formatTokens } from "@/lib/format";
import { windowTitle } from "@/lib/quota";
import { byLength, formatResetAt } from "@/lib/routing";
import { modelName, useModelGroups } from "@/lib/setup";
import { openConversation } from "@/state/actions";
import { type ActivityDays, setActivityDays, useUsage } from "@/state/usage";

const RANGES = [
  { value: "7", label: "7 days" },
  { value: "30", label: "30 days" },
] as const;

/**
 * How each account was used over the last 7 or 30 days: its windows' share used over time,
 * where each is heading at the recent rate, and Brigadier's own tokens on it.
 */
export function ActivityTab({ providers }: { providers: readonly ProviderUsage[] }) {
  const activity = useUsage((s) => s.activity);
  const days = useUsage((s) => s.days);
  const now = useNow(30_000);
  const groups = useModelGroups();
  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center justify-between gap-4">
        <p className="text-muted-foreground text-xs">
          {days === 7 ? "The last 7 days" : "The last 30 days"}
        </p>
        <div className="w-48 shrink-0">
          <Segmented
            label="Range"
            value={String(days) as "7" | "30"}
            options={RANGES}
            onChange={(value) => setActivityDays(Number(value) as ActivityDays)}
            fill
          />
        </div>
      </div>
      <section aria-label="Usage activity" className="flex flex-col gap-10">
        <ProviderSections providers={providers}>
          {(usage) =>
            usage.accounts.map((account) => (
              <AccountActivityCard
                key={account.account.account ?? "own"}
                usage={account}
                activity={activity?.accounts.find(
                  (entry) =>
                    entry.account.provider === account.account.provider &&
                    entry.account.account === account.account.account,
                )}
                since={activity?.sinceMs ?? now - days * 86_400_000}
                days={days}
                groups={groups}
                now={now}
              />
            ))
          }
        </ProviderSections>
      </section>
    </div>
  );
}

function AccountActivityCard({
  usage,
  activity,
  since,
  days,
  groups,
  now,
}: {
  usage: AccountUsage;
  activity: AccountActivity | undefined;
  since: number;
  days: ActivityDays;
  groups: readonly ModelGroup[];
  now: number;
}) {
  const view = useAccountView(usage);
  const missing = missingReason(view, usage);
  const states = (usage.quota?.windows ?? []).toSorted((a, b) => byLength(a.window, b.window));
  const series = (activity?.history ?? []).map((history) => {
    const state = states.find((entry) => entry.window.id === history.windowId);
    return {
      id: history.windowId,
      title: state ? windowTitle(state.window) : history.label,
      samples: history.samples,
    };
  });
  const read = series.some((entry) => entry.samples.length > 0);
  const estimates = states
    .map((state) => ({ state, line: forecastLine(state, now) }))
    .filter((entry) => entry.line !== null);
  return (
    <SettingsCard>
      <AccountHeader usage={usage} now={now} />
      <div className="flex flex-col gap-4 px-4 pt-2 pb-4">
        {!activity ? (
          <p className="text-muted-foreground text-xs">Reading…</p>
        ) : read ? (
          <UsageChart series={series} startMs={since} endMs={now} />
        ) : (
          <p className="text-muted-foreground text-xs">
            {missing ?? "No usage read in this range yet"}
          </p>
        )}
        {estimates.length > 0 && (
          <ul data-slot="usage-forecasts" className="flex flex-col gap-1 text-xs">
            {estimates.map(({ state, line }) => (
              <li key={state.window.id} className="flex gap-1.5">
                <span className="shrink-0">{windowTitle(state.window)}</span>
                <span className="text-muted-foreground">{line}</span>
              </li>
            ))}
          </ul>
        )}
        {activity && <TokensView provider={usage.account.provider} tokens={activity.tokens} days={days} groups={groups} />}
      </div>
    </SettingsCard>
  );
}

/**
 * "Runs out ≈20:40, 30m before reset", "≈72% at reset", at the recent rate. None once the
 * window is at its limit: it has run out already.
 */
export function forecastLine(state: WindowState, now: number): string | null {
  const { forecast, window, heat } = state;
  if (!forecast || heat === "limited") return null;
  if (forecast.runsOutAtMs !== null && window.resetsAtMs !== null) {
    return `Runs out ≈${formatResetAt(forecast.runsOutAtMs, now)}, ${formatCountdown(
      window.resetsAtMs,
      forecast.runsOutAtMs,
    )} before it resets, at the recent rate`;
  }
  return `≈${Math.round(forecast.projectedAtReset)}% used when it resets, at the recent rate`;
}

/** Rows shown per list of Brigadier's use. */
const TOP = 5;

/** Brigadier's own tokens on the account in the range: the total, then by model and by conversation. */
function TokensView({
  provider,
  tokens,
  days,
  groups,
}: {
  provider: AccountUsage["account"]["provider"];
  tokens: AccountTokens;
  days: ActivityDays;
  groups: readonly ModelGroup[];
}) {
  const total = tokens.byModel.reduce((sum, entry) => sum + entry.tokens, 0);
  if (total === 0) {
    return <p className="text-muted-foreground text-xs">Brigadier used none of it in the last {days} days.</p>;
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
    <div data-slot="usage-tokens" className="@container flex flex-col gap-3">
      <p className="text-xs">
        Brigadier used {formatTokens(total)} tokens in the last {days} days
      </p>
      <div className="grid gap-4 @md:grid-cols-2">
        <ShareList title="By model" rows={models} total={total} />
        <ShareList title="By conversation" rows={conversations} total={total} />
      </div>
    </div>
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
              <div aria-hidden className="bg-foreground/8 rounded-capsule h-1 overflow-hidden">
                <div
                  className="bg-chart-2 rounded-capsule h-full"
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
