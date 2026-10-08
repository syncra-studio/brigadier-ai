import type { QuotaSnapshot, QuotaWindow } from "@/ipc/generated";

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

/** How full a window looks: its text and bar colours. */
export function tone(percent: number): { text: string; fill: string } {
  if (percent >= 80) return { text: "text-destructive", fill: "bg-destructive" };
  if (percent >= 60) return { text: "text-warning", fill: "bg-warning" };
  return { text: "text-foreground", fill: "bg-muted-foreground/60" };
}

/** Share left of the fullest window worth a glance, or null without a reading. */
export function percentLeft(quota: QuotaSnapshot | null): number | null {
  if (!quota || quota.windows.length === 0) return null;
  return 100 - Math.max(...glance(quota).map(used));
}
