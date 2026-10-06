import { Lightbulb, Usage } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useId, useState } from "react";

import { errorText } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { Segmented } from "@/app/settings/parts";
import { FootRow, footMenuPlacement } from "@/app/sidebar/nav";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useSidebar } from "@/components/ui/sidebar";
import { Switch } from "@/components/ui/switch";
import type {
  ProviderOverview,
  QuotaSnapshot,
  QuotaWindow,
} from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { HEAT_LABELS } from "@/lib/routing";
import { cn } from "@/lib/utils";
import { loadProviders, openSettings, refreshProviders } from "@/state/actions";
import {
  keepAwakeOptions,
  keepAwakeState,
  lidClosedHint,
  loadKeepAwake,
  setKeepAwake,
  setKeepAwakeLidClosed,
  useKeepAwake,
} from "@/state/keepAwake";
import { useApp } from "@/state/store";

/** Usage is checked again this often while Brigadier is in front… */
const USAGE_EVERY_MS = 15 * 60_000;
/** …and on coming to the front, when what it shows is older than this. */
const USAGE_STALE_MS = 5 * 60_000;
/** How often keeping awake is checked (an agent starting or finishing changes it). */
const AWAKE_EVERY_MS = 15_000;

let lastUsageRefreshMs = Date.now();

function refreshUsage(): void {
  lastUsageRefreshMs = Date.now();
  refreshProviders().catch((error: unknown) => console.error("refreshing usage failed", error));
}

/*
 * The status rows at the sidebar's foot: keeping the computer awake, and each agent's usage
 * windows. Their menus open upwards, or beside the strip while the sidebar is collapsed.
 */

// ----- usage ---------------------------------------------------------------------------

/** Re-renders every half minute, for the countdowns. */
function useNow(): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  return now;
}

function used(window: QuotaWindow): number {
  return Math.round(Math.min(100, Math.max(0, window.usedPercent)));
}

/** Shortest window first (the session, then the week). */
function ordered(quota: QuotaSnapshot): QuotaWindow[] {
  return quota.windows.toSorted(
    (a, b) =>
      (a.windowMinutes ?? Number.MAX_SAFE_INTEGER) - (b.windowMinutes ?? Number.MAX_SAFE_INTEGER),
  );
}

/** A window worth showing in the menu however long it is. */
const GLANCE_FROM_PERCENT = 50;

/**
 * What the menu shows: the fixed-length windows (the session, the week) and any other window
 * filling up; the rest is on the Usage page.
 */
function glance(quota: QuotaSnapshot): QuotaWindow[] {
  const windows = ordered(quota);
  const shown = windows.filter(
    (window) => window.windowMinutes !== null || used(window) >= GLANCE_FROM_PERCENT,
  );
  return shown.length > 0 ? shown : windows.slice(0, 1);
}

function tone(percent: number): { text: string; fill: string } {
  if (percent >= 80) return { text: "text-destructive", fill: "bg-destructive" };
  if (percent >= 60) return { text: "text-warning", fill: "bg-warning" };
  return { text: "text-foreground", fill: "bg-muted-foreground/60" };
}

function withQuota(
  overview: ProviderOverview,
): overview is ProviderOverview & { quota: QuotaSnapshot } {
  return overview.quota !== null && overview.quota.windows.length > 0;
}

/** Loads usage, then keeps it fresh while the window is in front. */
function useUsageRefresh(): void {
  const connected = useApp((s) => s.connection.status === "connected");
  useEffect(() => {
    if (!connected) return;
    if (!useApp.getState().providers.view) {
      loadProviders().catch((error: unknown) => console.error("loading usage failed", error));
    }
    const inFront = () => document.hasFocus() && useApp.getState().windowVisible;
    const timer = window.setInterval(() => {
      if (inFront()) refreshUsage();
    }, USAGE_EVERY_MS);
    const onFocus = () => {
      const now = Date.now();
      if (now - lastUsageRefreshMs < USAGE_STALE_MS) return;
      const quotas = (useApp.getState().providers.view?.providers ?? []).flatMap((p) =>
        p.quota ? [p.quota] : [],
      );
      if (
        quotas.length === 0 ||
        quotas.some((quota) => now - quota.observedAtMs > USAGE_STALE_MS)
      ) {
        refreshUsage();
      }
    };
    window.addEventListener("focus", onFocus);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [connected]);
}

/**
 * An agent's tone: the quota monitor's heat when it has one (hot or limited show), else how
 * full its tightest window is.
 */
function heatTone(overview: ProviderOverview, tightest: number): { text: string; fill: string } {
  const heat = overview.usage?.heat;
  if (heat === "limited") return { text: "text-destructive", fill: "bg-destructive" };
  if (heat === "hot") return { text: "text-warning", fill: "bg-warning" };
  return tone(tightest);
}

/** How the tightest agent stands, for the Usage row: what is left, and whether to warn. */
function barStatus(shown: (ProviderOverview & { quota: QuotaSnapshot })[]): {
  label: string;
  alert: "text-destructive" | "text-warning" | null;
} {
  if (shown.length === 0) return { label: "Usage", alert: null };
  if (shown.some((overview) => (overview.usage?.limit ?? overview.quota.limit) !== null)) {
    return { label: "Usage · limit reached", alert: "text-destructive" };
  }
  let tightest = 0;
  let alert: "text-destructive" | "text-warning" | null = null;
  for (const overview of shown) {
    const percent = Math.max(...glance(overview.quota).map(used));
    tightest = Math.max(tightest, percent);
    const { text } = heatTone(overview, percent);
    if (text === "text-destructive") alert = text;
    else if (text === "text-warning" && alert === null) alert = text;
  }
  return { label: `Usage · ${100 - tightest}% left`, alert };
}

/**
 * Usage at the sidebar's foot: its icon (tinted, with a dot, while an agent runs hot), and on
 * click each agent's windows, with the way to the Usage page.
 */
export function UsageMenu() {
  useUsageRefresh();
  const providers = useApp((s) => s.providers.view?.providers);
  const onPage = useApp(
    (s) => s.selection.type === "settings" && s.selection.page === "usage",
  );
  const now = useNow();
  const shown = (providers ?? []).filter(withQuota);
  const { label, alert } = barStatus(shown);
  const { open: expanded } = useSidebar();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <FootRow
          label="Usage"
          tip={label}
          selected={onPage}
          icon={<Usage className={alert ?? undefined} />}
          dot={alert && (alert === "text-destructive" ? "bg-destructive" : "bg-warning")}
        />
      </DropdownMenuTrigger>
      <DropdownMenuContent
        {...footMenuPlacement(expanded)}
        className="w-xs"
      >
        <DropdownMenuLabel>Usage</DropdownMenuLabel>
        {shown.length === 0 ? (
          <p className="text-muted-foreground px-2 pb-1.5 text-sm">No agent has reported usage yet.</p>
        ) : (
          shown.map((overview) => (
            <ProviderUsage key={overview.provider} overview={overview} now={now} />
          ))
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => openSettings("usage")}>
          <Usage />
          Open the Usage page
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** One agent in the menu: its name and heat, then a bar for each window worth a glance. */
function ProviderUsage({
  overview,
  now,
}: {
  overview: ProviderOverview & { quota: QuotaSnapshot };
  now: number;
}) {
  const windows = glance(overview.quota);
  const tightest = Math.max(...windows.map(used));
  const limit = overview.usage?.limit ?? overview.quota.limit;
  const chip = heatTone(overview, tightest);
  const heat = overview.usage?.heat;
  return (
    <div className="flex flex-col gap-1.5 px-2 py-1.5">
      <div className="flex items-center gap-2 text-sm">
        <ProviderGlyph provider={overview.provider} className="size-icon-sm shrink-0" />
        <span className="min-w-0 flex-1 truncate">{PROVIDER_LABELS[overview.provider]}</span>
        {limit ? (
          <span className="text-destructive text-xs">
            Limit reached
            {limit.resetsAtMs !== null && ` · ${formatCountdown(limit.resetsAtMs, now)}`}
          </span>
        ) : (
          (heat === "hot" || heat === "limited") && (
            <span className={cn("text-xs", chip.text)}>{HEAT_LABELS[heat]}</span>
          )
        )}
      </div>
      {windows.map((window) => {
        const percent = used(window);
        return (
          <div key={window.id} className="flex flex-col gap-1 text-xs">
            <div className="flex items-center gap-2 tabular-nums">
              <span className="text-muted-foreground min-w-0 flex-1 truncate">{window.label}</span>
              <span className={tone(percent).text}>{percent}% used</span>
              {window.resetsAtMs !== null && (
                <span className="text-muted-foreground">
                  {formatCountdown(window.resetsAtMs, now)}
                </span>
              )}
            </div>
            <span aria-hidden className="bg-muted rounded-capsule h-1 overflow-hidden">
              <span
                className={cn("block h-full", tone(percent).fill)}
                style={{ width: `${percent}%` }}
              />
            </span>
          </div>
        );
      })}
    </div>
  );
}

// ----- keep awake ----------------------------------------------------------------------

/** Keeps the status fresh while the window can be seen. */
function useKeepAwakeStatus(): void {
  const connected = useApp((s) => s.connection.status === "connected");
  useEffect(() => {
    if (!connected) return;
    const load = () => {
      loadKeepAwake().catch((error: unknown) => console.error("checking keep awake failed", error));
    };
    load();
    const timer = window.setInterval(() => {
      if (useApp.getState().windowVisible) load();
    }, AWAKE_EVERY_MS);
    return () => window.clearInterval(timer);
  }, [connected]);
}

/**
 * Keeping awake at the sidebar's foot: a light bulb, lit (with a dot) while the computer is kept
 * awake.
 * On click, a small panel: how it stands now, when to stay awake (off, while agents work,
 * always), and whether that holds with the lid closed.
 */
export function KeepAwakeMenu() {
  useKeepAwakeStatus();
  const keepAwake = useApp((s) => s.settings.keepAwake);
  const lidClosed = useApp((s) => s.settings.keepAwakeLidClosed);
  const status = useKeepAwake((s) => s.status);
  const settingUp = useKeepAwake((s) => s.settingUp);
  const [error, setError] = useState<string | null>(null);
  const options = keepAwakeOptions(status?.screenOn ?? false);
  const option = options.find((entry) => entry.value === keepAwake);
  const state = keepAwakeState(keepAwake, status);
  const shownError = error ?? status?.error ?? null;
  const lidId = useId();
  const { open: expanded } = useSidebar();

  const run = (action: () => Promise<void>) => {
    setError(null);
    action().catch((cause: unknown) => setError(errorText(cause)));
  };

  return (
    <Popover
      onOpenChange={(open) => {
        if (open) run(loadKeepAwake);
      }}
    >
      <PopoverTrigger asChild>
        <FootRow
          label="Keep awake"
          tip={`Keep awake: ${option?.label ?? ""} · ${state.text}`}
          icon={
            <Lightbulb
              className={cn(state.awake && "text-foreground", shownError && "text-warning")}
            />
          }
          dot={shownError ? "bg-warning" : state.awake ? "bg-foreground" : null}
        />
      </PopoverTrigger>
      <PopoverContent
        {...footMenuPlacement(expanded)}
        className="flex w-xs flex-col gap-3 p-3"
      >
        <div className="flex items-start gap-2.5">
          <Lightbulb
            aria-hidden
            className={cn(
              "size-icon-md mt-0.5 shrink-0",
              state.awake ? "text-foreground" : "text-muted-foreground",
            )}
          />
          <div className="flex min-w-0 flex-col gap-0.5">
            <span className="text-sm font-medium">Keep awake</span>
            <span
              className={cn(
                "flex items-center gap-1.5 text-xs",
                state.awake ? "text-foreground" : "text-muted-foreground",
              )}
            >
              <span
                aria-hidden
                className={cn(
                  "size-1.5 shrink-0 rounded-full",
                  state.awake ? "bg-foreground" : "bg-muted-foreground/40",
                )}
              />
              {state.text}
            </span>
          </div>
        </div>

        <div className="flex flex-col gap-1.5">
          <Segmented
            label="When to keep the computer awake"
            value={keepAwake}
            options={options}
            fill
            onChange={(value) => run(() => setKeepAwake(value))}
          />
          <p className="text-muted-foreground text-xs">{option?.hint}.</p>
        </div>

        {status && status.lidClosed !== "unsupported" && (
          <div className="border-divider flex items-start gap-3 border-t pt-3">
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span id={`${lidId}-label`} className="text-sm">
                With the lid closed too
              </span>
              <span id={`${lidId}-hint`} className="text-muted-foreground text-xs">
                {keepAwake === "off" && !status.forRun
                  ? "Takes effect once keeping awake is on."
                  : lidClosedHint(status, settingUp)}
              </span>
            </span>
            <Switch
              className="mt-0.5"
              aria-labelledby={`${lidId}-label`}
              aria-describedby={`${lidId}-hint`}
              checked={lidClosed}
              disabled={settingUp}
              onCheckedChange={(on) => run(() => setKeepAwakeLidClosed(on))}
            />
          </div>
        )}

        {shownError && (
          <p role="alert" className="text-warning text-xs whitespace-normal">
            {shownError}
          </p>
        )}
      </PopoverContent>
    </Popover>
  );
}
