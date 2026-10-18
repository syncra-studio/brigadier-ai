import { BarChart, ChevronRight, ChevronUp, Lightbulb } from "@openai/apps-sdk-ui/components/Icon";
import { useEffect, useId, useRef, useState } from "react";

import { errorText } from "@/app/dialogs/fields";
import { PROVIDER_LABELS } from "@/app/inspector/providers/shared";
import { agentState } from "@/app/onboarding/SetupTerminal";
import { StateDot } from "@/app/providers/parts";
import { Segmented } from "@/app/settings/parts";
import { FootButton, footMenuPlacement } from "@/app/sidebar/nav";
import { ProviderGlyph } from "@/components/glyphs/provider-glyphs";
import { Popover, PopoverContent, PopoverTrigger } from "@/components/ui/popover";
import { useSidebar } from "@/components/ui/sidebar";
import { Switch } from "@/components/ui/switch";
import type { ProviderKind, ProviderOverview, QuotaSnapshot, QuotaWindow } from "@/ipc/generated";
import { formatCountdown } from "@/lib/format";
import { ordered, USAGE_DANGER_FROM, usageLevel, used, windowTitle } from "@/lib/quota";
import { cn } from "@/lib/utils";
import { loadAccounts } from "@/state/accounts";
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
function shareTone(percent: number, limitReached = false): "normal" | "warning" | "danger" {
  if (limitReached || percent >= USAGE_DANGER_FROM) return "danger";
  return percent > USAGE_AMBER_PAST ? "warning" : "normal";
}

/** A provider in the list: one that reports usage, or one that needs signing in. */
type UsageEntry =
  | { provider: ProviderKind; needs: "signIn" }
  | { provider: ProviderKind; needs: null; windows: QuotaWindow[]; top: QuotaWindow; limitReached: boolean };

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
    const windows = ordered(overview.quota);
    const top = tightest(windows);
    if (!top) return [];
    const limitReached = (overview.usage?.limit ?? overview.quota.limit) !== null;
    return [{ provider: overview.provider, needs: null, windows, top, limitReached }];
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

/**
 * Usage at the sidebar's foot, drawn to the user's design: its icon (tinted, with a dot, from 80%
 * used), and on click one row per provider, most used first, each with only its tightest limit;
 * a provider that needs signing in says so with Sign in. Clicking a provider opens every limit
 * it has under it, one provider at a time. Providers › opens Settings › Providers.
 */
export function UsageMenu() {
  useUsageRefresh();
  const providers = useApp((s) => s.providers.view?.providers);
  const settings = useApp((s) => s.settings);
  const onPage = useApp((s) => s.selection.type === "settings" && s.selection.page === "providers");
  const now = useNow();
  const [menuOpen, setMenuOpen] = useState(false);
  const [opened, setOpened] = useState<ProviderKind | null>(null);
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
        if (!next) setOpened(null);
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
        className="bg-settings-raised border-settings-ring shadow-foot-popover w-90 border p-0 ring-0"
      >
        <div className="flex items-center justify-between px-5 pt-3.5 pb-1.5">
          <span className="text-settings-body text-settings-text-secondary">Usage</span>
          <button
            type="button"
            onClick={() => go(() => openSettings("providers"))}
            className="text-settings-small text-settings-link flex items-center gap-1 font-medium hover:underline"
          >
            Providers
            <ChevronRight className="size-3.25" />
          </button>
        </div>
        <div className="flex flex-col px-1.5 pb-2">
          {entries.length === 0 ? (
            <p className="text-settings-small text-settings-text-tertiary px-3.5 py-2">No agent has reported usage yet.</p>
          ) : (
            entries.map((entry) => (
              <ProviderUsage
                key={entry.provider}
                entry={entry}
                now={now}
                open={opened === entry.provider}
                onToggle={() => setOpened((current) => (current === entry.provider ? null : entry.provider))}
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

/** A limit's thin bar and its share, amber past 75% used. */
function Share({ percent, limitReached = false }: { percent: number; limitReached?: boolean }) {
  const tone = shareTone(percent, limitReached);
  return (
    <>
      <span aria-hidden className="bg-settings-ring rounded-providers-bar h-0.75 w-11 shrink-0 overflow-hidden">
        <span
          className={cn(
            "rounded-providers-bar block h-full",
            tone === "danger" ? "bg-destructive" : tone === "warning" ? "bg-settings-warning" : "bg-usage-bar",
          )}
          style={{ width: `${percent}%` }}
        />
      </span>
      <span
        data-slot="usage-value"
        className={cn(
          "text-settings-small w-8.5 shrink-0 text-end font-medium",
          tone === "danger"
            ? "text-destructive"
            : tone === "warning"
              ? "text-settings-warning"
              : percent === 0
                ? "text-settings-text-tertiary"
                : "text-settings-text",
        )}
      >
        {percent}%
      </span>
    </>
  );
}

/**
 * One provider: its mark and name, then its tightest limit ("Weekly · 1d 8h", a bar and the
 * share), or, open, how many limits it has over a row for each; or, needing a sign-in, that and
 * Sign in.
 */
function ProviderUsage({
  entry,
  now,
  open,
  onToggle,
  onSignIn,
}: {
  entry: UsageEntry;
  now: number;
  open: boolean;
  onToggle: () => void;
  onSignIn: () => void;
}) {
  const name = PROVIDER_LABELS[entry.provider];
  const mark = (
    <ProviderGlyph
      provider={entry.provider}
      className={cn("size-3.75 shrink-0", entry.provider === "codex" && "text-settings-glyph")}
    />
  );
  const row = "flex h-8.5 w-full items-center gap-2.5 rounded-full px-3.5";
  if (entry.needs) {
    return (
      <div data-slot="usage-row" className={row}>
        {mark}
        <span className="text-settings-body text-settings-text truncate">{name}</span>
        <span className="flex min-w-0 flex-1 items-center gap-1.5">
          <StateDot tone="needs" />
          <span className="text-settings-small text-settings-warning truncate">Not signed in</span>
        </span>
        <button
          type="button"
          onClick={onSignIn}
          className="bg-settings-raised border-settings-ring text-settings-meta text-settings-text hover:bg-settings-ring h-6 shrink-0 rounded-full border px-2.5 font-medium transition-colors"
        >
          Sign in
        </button>
      </div>
    );
  }
  const { windows, top, limitReached } = entry;
  const resets = (window: QuotaWindow) =>
    window.resetsAtMs === null ? null : formatCountdown(window.resetsAtMs, now);
  const topResets = resets(top);
  return (
    <>
      <button
        type="button"
        data-slot="usage-row"
        aria-expanded={open}
        onClick={onToggle}
        className={cn(row, "hover:bg-settings-ring text-start transition-colors")}
      >
        {mark}
        <span className="text-settings-body text-settings-text truncate">{name}</span>
        {open ? (
          <>
            <span className="text-settings-small text-settings-text-tertiary min-w-0 flex-1 truncate">
              {windows.length} {windows.length === 1 ? "limit" : "limits"}
            </span>
            <ChevronUp className="text-settings-text-secondary size-3.5 shrink-0" />
          </>
        ) : (
          <>
            <span className="text-settings-small text-settings-text-tertiary min-w-0 flex-1 truncate">
              {[windowTitle(top), topResets].filter(Boolean).join(" · ")}
            </span>
            <Share percent={used(top)} limitReached={limitReached} />
          </>
        )}
      </button>
      {open &&
        windows.map((window) => (
          <div
            key={window.id}
            data-slot="usage-limit"
            className="flex h-7.5 items-center gap-2.5 rounded-full ps-9.75 pe-3.5"
          >
            <span className="text-settings-small text-settings-text-secondary truncate">{windowTitle(window)}</span>
            <span className="text-settings-small text-settings-text-tertiary min-w-0 flex-1 truncate">
              {resets(window)}
            </span>
            <Share percent={used(window)} limitReached={limitReached && used(window) >= 100} />
          </div>
        ))}
    </>
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
