import type { QuotaSample, QuotaSnapshot, QuotaWindow } from "@/ipc/generated";

/** Share of a window used, as a whole percent from 0 to 100. */
export function used(window: QuotaWindow): number {
  return Math.round(Math.min(100, Math.max(0, window.usedPercent)));
}

/** Shortest window first (the session, then the week). */
export function ordered(quota: QuotaSnapshot): QuotaWindow[] {
  return quota.windows.toSorted(
    (a, b) =>
      (a.windowMinutes ?? Number.MAX_SAFE_INTEGER) - (b.windowMinutes ?? Number.MAX_SAFE_INTEGER),
  );
}

/** A window worth a glance however long it is. */
const GLANCE_FROM_PERCENT = 50;

/**
 * The windows worth a glance: the fixed-length ones (the session, the week) and any other window
 * filling up; the rest is on the Usage page.
 */
export function glance(quota: QuotaSnapshot): QuotaWindow[] {
  const windows = ordered(quota);
  const shown = windows.filter(
    (window) => window.windowMinutes !== null || used(window) >= GLANCE_FROM_PERCENT,
  );
  return shown.length > 0 ? shown : windows.slice(0, 1);
}

/** How close usage is to running out: normal below 80% used, warning from 80%, danger from 90%. */
export type UsageLevel = "normal" | "warning" | "danger";

/** Used % from which usage shows in the warning colour. */
export const USAGE_WARNING_FROM = 80;
/** Used % from which usage shows in the danger colour. */
export const USAGE_DANGER_FROM = 90;

/**
 * How full usage is, by the share used alone (a forecast never colours it); a reached limit is
 * danger whatever the share. Where usage shows as "% left", pass 100 minus it.
 */
export function usageLevel(percentUsed: number, limitReached = false): UsageLevel {
  if (limitReached || percentUsed >= USAGE_DANGER_FROM) return "danger";
  if (percentUsed >= USAGE_WARNING_FROM) return "warning";
  return "normal";
}

/** Colours for text, a bar's fill, a chart's line and a chart's mark. */
export type UsageTone = { text: string; fill: string; stroke: string; mark: string };

const USAGE_TONES: Record<Exclude<UsageLevel, "normal">, UsageTone> = {
  warning: { text: "text-warning", fill: "bg-warning", stroke: "stroke-warning", mark: "fill-warning" },
  danger: { text: "text-destructive", fill: "bg-destructive", stroke: "stroke-destructive", mark: "fill-destructive" },
};

/** The normal look, for places that don't say their own. */
const NORMAL_TONE: UsageTone = {
  text: "text-foreground",
  fill: "bg-muted-foreground/60",
  stroke: "stroke-chart-2",
  mark: "fill-chart-2",
};

/**
 * Usage's text, bar and line colours at `percentUsed` (see `usageLevel`); below 80% the
 * place's own `normal` look, else the shared one.
 */
export function usageTone(
  percentUsed: number,
  { limitReached = false, normal = {} }: { limitReached?: boolean; normal?: Partial<UsageTone> } = {},
): UsageTone & { level: UsageLevel } {
  const level = usageLevel(percentUsed, limitReached);
  return { level, ...levelTone(level, normal) };
}

/** A level's colours, for a place that sets its own levels; `normal` as `usageTone`'s. */
export function levelTone(level: UsageLevel, normal: Partial<UsageTone> = {}): UsageTone {
  return level === "normal" ? { ...NORMAL_TONE, ...normal } : USAGE_TONES[level];
}

/**
 * A window's name with what it is limited to folded in: "Weekly · Fable" for Claude's
 * "Weekly (Fable)", "Weekly · gpt-6-luna" for a Codex bucket's window. A provider-wide window
 * (no model) is just its name, even when it sits in a named bucket like Codex's "codex".
 */
export function windowTitle(window: QuotaWindow): string {
  const named = /^(.*) \((.+)\)$/.exec(window.label);
  if (named) return `${named[1]} · ${named[2]}`;
  return window.model ? `${window.label} · ${window.model}` : window.label;
}

/** Share of a window left, as a whole percent from 0 to 100. */
export function left(window: QuotaWindow): number {
  return 100 - used(window);
}

/**
 * Usage's colours by how much is left: normal above 20% left, warning at 20% or less, danger
 * at 10% or less (the same thresholds as 80% and 90% used) or once its limit is reached.
 */
export function leftTone(
  share: number,
  options: { limitReached?: boolean; normal?: Partial<UsageTone> } = {},
): UsageTone & { level: UsageLevel } {
  return usageTone(100 - share, options);
}

/** Share left of the fullest window worth a glance, or null without a reading. */
export function percentLeft(quota: QuotaSnapshot | null): number | null {
  if (!quota || quota.windows.length === 0) return null;
  return 100 - Math.max(...glance(quota).map(used));
}

/** A window shorter than this resets several times a day, too often to draw over a month. */
const DAY_MINUTES = 24 * 60;

/** Whether a range of `days` draws `window` as each day's peak rather than every reading. */
export function drawsDailyPeak(window: QuotaWindow, days: number): boolean {
  return days > 7 && window.windowMinutes !== null && window.windowMinutes < DAY_MINUTES;
}

/**
 * Samples in `startMs`..`endMs` as each local day's highest reading, held flat across that
 * day: over a month, a window that resets every few hours reads as how close each day came to
 * its limit instead of a dense sawtooth. Days with no reading are left out.
 */
export function dailyPeaks(samples: readonly QuotaSample[], startMs: number, endMs: number): QuotaSample[] {
  const days = new Map<number, number>();
  for (const sample of samples) {
    if (sample.atMs < startMs || sample.atMs > endMs) continue;
    const day = new Date(sample.atMs).setHours(0, 0, 0, 0);
    days.set(day, Math.max(days.get(day) ?? 0, sample.usedPercent));
  }
  return [...days.entries()]
    .toSorted(([a], [b]) => a - b)
    .flatMap(([day, peak]) => {
      const next = new Date(day);
      next.setDate(next.getDate() + 1);
      return [
        { atMs: Math.max(day, startMs), usedPercent: peak },
        { atMs: Math.min(next.getTime() - 1, endMs), usedPercent: peak },
      ];
    });
}
