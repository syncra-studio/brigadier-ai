import { invoke } from "@tauri-apps/api/core";

import type { BrowserVisit, HistoryFilter } from "@/ipc/generated";

/*
 * What the built-in browser remembers (its history, downloads and site choices, kept by the
 * shell), and the pure helpers Settings › Browser lists them with.
 */

/** How many pages the history view asks for at a time. */
export const HISTORY_PAGE = 200;

export function browserHistory(
  query: string,
  filter: HistoryFilter,
  beforeMs: number | null,
  limit = HISTORY_PAGE,
): Promise<BrowserVisit[]> {
  return invoke<BrowserVisit[]>("browser_history", { query, filter, beforeMs, limit });
}

export function removeHistory(ids: number[]): Promise<void> {
  return invoke("browser_history_remove", { ids });
}

/** A page's site as the lists show it: its host without "www.". */
export function siteOf(url: string): string {
  try {
    return new URL(url).host.replace(/^www\./, "");
  } catch {
    return url;
  }
}

const dayFormat = new Intl.DateTimeFormat(undefined, { dateStyle: "medium" });
const weekdayFormat = new Intl.DateTimeFormat(undefined, { weekday: "long" });

function startOfDay(epochMs: number): number {
  return new Date(epochMs).setHours(0, 0, 0, 0);
}

/** A day's heading: "Today", "Yesterday", a weekday this week, else the date. */
export function dayLabel(epochMs: number, nowMs: number): string {
  const days = Math.round((startOfDay(nowMs) - startOfDay(epochMs)) / 86_400_000);
  if (days === 0) return "Today";
  if (days === 1) return "Yesterday";
  if (days > 1 && days < 7) return weekdayFormat.format(epochMs);
  return dayFormat.format(epochMs);
}

export type Day<T> = { key: number; label: string; items: T[] };

/** Items (newest first) in one group per calendar day, newest day first. */
export function byDay<T extends { atMs: number }>(items: readonly T[], nowMs: number): Day<T>[] {
  const days: Day<T>[] = [];
  for (const item of items) {
    const key = startOfDay(item.atMs);
    const last = days.at(-1);
    if (last?.key === key) last.items.push(item);
    else days.push({ key, label: dayLabel(item.atMs, nowMs), items: [item] });
  }
  return days;
}
