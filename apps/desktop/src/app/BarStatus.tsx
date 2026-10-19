import { BarChart, ChevronRight, Lightbulb } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useId, useRef, useState } from "react";

import { errorText } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { agentState } from "@/app/onboarding/SetupTerminal";
import { StateDot } from "@/app/providers/parts";
import { Segmented } from "@/app/settings/parts";
import { FootButton, footMenuPlacement } from "@/app/sidebar/nav";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Button } from "@/components/ui/button";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useSidebar } from "@/components/ui/sidebar";
import { Switch } from "@/components/ui/switch";
import type { AccountView, ProviderKind, ProviderOverview, QuotaSnapshot, QuotaWindow } from "@/ipc/generated";
import { maskEmailsIn } from "@/lib/email";
import { formatCountdown } from "@/lib/format";
import { levelTone, ordered, USAGE_DANGER_FROM, type UsageLevel, usageLevel, used, windowTitle } from "@/lib/quota";
import { cn } from "@/lib/utils";
import { accountLabel, accountsOf, loadAccounts, oneRowPerAccount } from "@/state/accounts";
import { loadProviders, openSettings, refreshProviders } from "@/state/actions";
import { openAgentPage } from "@/state/agentPage";
import {
  keepAwakeOptions,
  keepAwakeState,
  lidClosedHint,
  loadKeepAwake,
  setKeepAwake,
  setKeepAwakeLidClosed,
  useKeepAwake,
} from "@/state/keepAwake";
import { type Availability, providerOn } from "@/state/providers";
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
    // The chat's account pill needs them too.
    if (!useApp.getState().accounts) {
      loadAccounts().catch((error: unknown) => console.error("reading the accounts failed", error));
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

/** Used % past which a provider's usage turns amber in the list (the user's design). */
const USAGE_AMBER_PAST = 75;

/** The window nearest its limit. */
function tightest(windows: readonly QuotaWindow[]): QuotaWindow | null {
  return windows.reduce<QuotaWindow | null>((top, window) => (top && used(top) >= used(window) ? top : window), null);
}

/** A share's colour in the list: amber past 75% used, red at a reached limit or from 90%. */
function shareTone(percent: number, limitReached = false): UsageLevel {
  if (limitReached || percent >= USAGE_DANGER_FROM) return "danger";
  return percent > USAGE_AMBER_PAST ? "warning" : "normal";
}

/** A provider in the list: one that reports usage (its account in use's), or one that needs signing in. */
type UsageEntry =
  | { provider: ProviderKind; needs: "signIn" }
  | { provider: ProviderKind; needs: null; top: QuotaWindow; limitReached: boolean };

/** The providers on and set up: those needing the user first, then the most used first. */
function usageEntries(
  providers: readonly ProviderOverview[],
  settings: Availability,
): UsageEntry[] {
  const entries = providers.flatMap((overview): UsageEntry[] => {
    if (!providerOn(settings, overview.provider)) return [];
    const state = agentState(overview);
    if (state === "signIn") return [{ provider: overview.provider, needs: "signIn" }];
    if (state !== "ready" || !withQuota(overview)) return [];
    // The overview's quota is the account in use's: the one new work starts on.
    const top = tightest(ordered(overview.quota));
    if (!top) return [];
    const limitReached = (overview.usage?.limit ?? overview.quota.limit) !== null;
    return [{ provider: overview.provider, needs: null, top, limitReached }];
  });
  const rank = (entry: UsageEntry) => (entry.needs ? Number.POSITIVE_INFINITY : used(entry.top));
  return entries.toSorted((a, b) => rank(b) - rank(a));
}

/** How the tightest provider stands, for the Usage icon: its name, and whether to warn. */
function barStatus(entries: readonly UsageEntry[]): {
  label: string;
  alert: "text-destructive" | "text-warning" | null;
} {
  const shown = entries.flatMap((entry) => (entry.needs ? [] : [entry]));
  if (shown.length === 0) return { label: "Usage", alert: null };
  if (shown.some((entry) => entry.limitReached)) return { label: "Usage · limit reached", alert: "text-destructive" };
  // Coloured by how much is used, like the agents' own CLIs: from 80% and from 90%.
  const most = Math.max(...shown.map((entry) => used(entry.top)));
  const level = usageLevel(most);
  const alert = level === "danger" ? "text-destructive" : level === "warning" ? "text-warning" : null;
  return { label: `Usage · ${100 - most}% left`, alert };
}

/** Who `provider`'s account in use is, masked, for its row's tip; `null` with only one account. */
function inUseLabel(views: readonly AccountView[] | undefined, provider: ProviderKind): string | null {
  const rows = oneRowPerAccount(accountsOf(views, provider));
  const inUse = rows.find(({ view }) => view.inUse)?.view;
  return rows.length > 1 && inUse ? maskEmailsIn(accountLabel(inUse)) : null;
}

/**
 * Usage at the sidebar's foot, drawn to the user's design: its icon (tinted, with a dot, from 80%
 * used), and on click one row per provider, most used first, each with only its tightest limit
 * of the account in use (the one new work starts on); a provider that needs signing in says so
 * with Sign in. Clicking a provider opens its page in Settings › Providers; Providers › opens
 * Settings › Providers.
 */
export function UsageMenu() {
  useUsageRefresh();
  const providers = useApp((s) => s.providers.view?.providers);
  const settings = useApp((s) => s.settings);
  const accounts = useApp((s) => s.accounts?.accounts);
  const onPage = useApp((s) => s.selection.type === "settings" && s.selection.page === "providers");
  const now = useNow();
  const [menuOpen, setMenuOpen] = useState(false);
  // In the row, the list lines up with the row's start rather than with its icon.
  const trigger = useRef<HTMLButtonElement>(null);
  const [rowStart, setRowStart] = useState(0);
  const entries = usageEntries(providers ?? [], settings);
  const { label, alert } = barStatus(entries);
  const { open: expanded } = useSidebar();
  const go = (action: () => void) => {
    setMenuOpen(false);
    action();
  };
  return (
    <Popover
      open={menuOpen}
      onOpenChange={(next) => {
        setMenuOpen(next);
        if (next) setRowStart(offsetInRow(trigger.current));
      }}
    >
      <PopoverTrigger ref={trigger} asChild>
        <FootButton
          tip={label}
          selected={onPage}
          icon={<BarChart className={alert ?? undefined} />}
          dot={alert && (alert === "text-destructive" ? "bg-destructive" : "bg-warning")}
        />
      </PopoverTrigger>
      <PopoverContent
        data-slot="usage-popover"
        {...footMenuPlacement(expanded)}
        alignOffset={expanded ? -rowStart : 0}
        className="w-90"
      >
        <div className="flex items-center justify-between px-2 py-1.5">
          <span className="text-muted-foreground font-mono text-2xs font-medium tracking-wide uppercase">Usage</span>
          <button
            type="button"
            onClick={() => go(() => openSettings("providers"))}
            className="text-link flex items-center gap-1 text-xs font-medium hover:underline"
          >
            Providers
            <ChevronRight className="size-icon-xs" />
          </button>
        </div>
        <div className="flex flex-col">
          {entries.length === 0 ? (
            <p className="text-muted-foreground px-2 py-1.5 text-xs">No agent has reported usage yet.</p>
          ) : (
            entries.map((entry) => (
              <ProviderUsage
                key={entry.provider}
                entry={entry}
                now={now}
                account={inUseLabel(accounts, entry.provider)}
                onOpen={() => go(() => openAgentPage(entry.provider))}
                onSignIn={() => go(() => openAgentPage(entry.provider, true))}
              />
            ))
          )}
        </div>
      </PopoverContent>
    </Popover>
  );
}

/** How far a foot icon sits from the start of the foot's row (inside its padding). */
function offsetInRow(button: HTMLElement | null): number {
  const row = button?.parentElement?.parentElement;
  if (!button || !row) return 0;
  const start = row.getBoundingClientRect().left + Number.parseFloat(getComputedStyle(row).paddingLeft);
  return Math.max(0, button.getBoundingClientRect().left - start);
}

/** A limit's thin bar and its share, amber past 75% used, in usage's shared colours. */
function Share({ percent, limitReached = false }: { percent: number; limitReached?: boolean }) {
  const level = shareTone(percent, limitReached);
  const look = levelTone(level, percent === 0 ? { text: "text-muted-foreground" } : {});
  return (
    <>
      <span aria-hidden className="bg-muted rounded-capsule h-1 w-11 shrink-0 overflow-hidden">
        <span className={cn("rounded-capsule block h-full", look.fill)} style={{ width: `${percent}%` }} />
      </span>
      <span
        data-slot="usage-value"
        data-level={level}
        className={cn("w-8.5 shrink-0 text-end text-xs font-medium tabular-nums", look.text)}
      >
        {percent}%
      </span>
    </>
  );
}

/**
 * One provider: its mark and name, then its account in use's tightest limit ("Weekly · 1d 8h",
 * a bar and the share), saying so when its limit is reached; clicking it opens the provider's
 * page. Needing a sign-in, that and Sign in.
 */
function ProviderUsage({
  entry,
  now,
  account,
  onOpen,
  onSignIn,
}: {
  entry: UsageEntry;
  now: number;
  /** Who its account in use is, masked, when it has several. */
  account: string | null;
  onOpen: () => void;
  onSignIn: () => void;
}) {
  const name = PROVIDER_LABELS[entry.provider];
  const mark = (
    <ProviderGlyph
      provider={entry.provider}
      className="size-icon-md shrink-0"
    />
  );
  const row = "h-control-sm rounded-capsule flex w-full items-center gap-2 px-2 text-sm";
  if (entry.needs) {
    return (
      <div data-slot="usage-row" className={row}>
        {mark}
        <span className="truncate">{name}</span>
        <span className="flex min-w-0 flex-1 items-center gap-1.5">
          <StateDot tone="needs" />
          <span className="text-warning truncate text-xs">Not signed in</span>
        </span>
        <Button type="button" variant="outline" size="xs" onClick={onSignIn} className="shrink-0">
          Sign in
        </Button>
      </div>
    );
  }
  const { top, limitReached } = entry;
  const resets = top.resetsAtMs === null ? null : formatCountdown(top.resetsAtMs, now);
  return (
    <button
      type="button"
      data-slot="usage-row"
      title={account ? `In use: ${account}` : undefined}
      onClick={onOpen}
      className={cn(row, "hover:bg-foreground/8 text-start")}
    >
      {mark}
      <span className="truncate">{name}</span>
      <span className="text-muted-foreground min-w-0 flex-1 truncate text-xs">
        {[limitReached ? "At limit" : windowTitle(top), resets].filter(Boolean).join(" · ")}
      </span>
      <Share percent={used(top)} limitReached={limitReached} />
    </button>
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
        <FootButton
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
          <p className="text-muted-foreground text-xs">{option?.hint}</p>
        </div>

        {status && status.lidClosed !== "unsupported" && (
          <div className="border-divider flex items-start gap-3 border-t pt-3">
            <span className="flex min-w-0 flex-1 flex-col gap-0.5">
              <span id={`${lidId}-label`} className="text-sm">
                With the lid closed too
              </span>
              <span id={`${lidId}-hint`} className="text-muted-foreground text-xs">
                {keepAwake === "off" && !status.forRun
                  ? "Takes effect once keeping awake is on"
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
