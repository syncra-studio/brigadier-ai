import { Channel, invoke } from "@tauri-apps/api/core";

import type {
  BrowserClear,
  BrowserDownload,
  BrowserSite,
  BrowserVisit,
  HistoryFilter,
  ImportProfile,
  ImportProgress,
  ImportResult,
  SitePermission,
} from "@/ipc/generated";

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

export function browserDownloads(query: string): Promise<BrowserDownload[]> {
  return invoke<BrowserDownload[]>("browser_downloads", { query });
}

/** Forgets downloads (the files stay): `ids`, or every finished one. */
export function removeDownloads(ids: number[] | null): Promise<void> {
  return invoke("browser_downloads_remove", { ids });
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

/** Removes what Clear browsing data picked. */
export function clearBrowsingData(clear: BrowserClear): Promise<void> {
  return invoke("browser_clear", { clear });
}

/** How many sites the browser keeps cookies or other data for; null where it can't tell. */
export function siteDataCount(): Promise<number | null> {
  return invoke<number | null>("browser_site_count");
}

export type ClearRange = "hour" | "day" | "week" | "month" | "all";

const RANGE_MS: Record<Exclude<ClearRange, "all">, number> = {
  hour: 3_600_000,
  day: 86_400_000,
  week: 7 * 86_400_000,
  month: 28 * 86_400_000,
};

/** Where a cleared time range starts; null: all time. */
export function rangeStart(range: ClearRange, nowMs: number): number | null {
  return range === "all" ? null : nowMs - RANGE_MS[range];
}

export function browserSites(): Promise<BrowserSite[]> {
  return invoke<BrowserSite[]>("browser_sites");
}

/** Sets what a site may use without asking: its camera, its microphone, or both. */
export function setSite(
  origin: string,
  choice: { camera?: SitePermission; microphone?: SitePermission },
): Promise<void> {
  return invoke("browser_site_set", { origin, camera: choice.camera ?? null, microphone: choice.microphone ?? null });
}

export function importProfiles(): Promise<ImportProfile[]> {
  return invoke<ImportProfile[]>("browser_import_profiles");
}

/** Copies a profile's cookies and/or history into Brigadier's browser; `onProgress` hears how it goes. */
export function importBrowser(
  id: string,
  kinds: { cookies: boolean; history: boolean },
  onProgress: (progress: ImportProgress) => void,
): Promise<ImportResult> {
  return invoke<ImportResult>("browser_import", { id, ...kinds, progress: new Channel<ImportProgress>(onProgress) });
}

/** Opens System Settings at Full Disk Access (Safari's files need it). */
export function openFullDiskAccess(): Promise<void> {
  return invoke("browser_import_open_access");
}

/** The profile an import starts on: the default of a browser that isn't open, else the first. */
export function firstProfile(profiles: readonly ImportProfile[]): ImportProfile | undefined {
  const usable = profiles.filter((p) => p.readable && (p.hasCookies || p.hasHistory));
  return (
    usable.find((p) => p.isDefault && !p.running) ??
    usable.find((p) => !p.running) ??
    usable[0] ??
    profiles[0]
  );
}

const number = new Intl.NumberFormat();

function counted(n: number, one: string, many: string): string {
  return `${number.format(n)} ${n === 1 ? one : many}`;
}

/** An import's result in a few plain sentences. */
export function importSummary(browser: string, result: ImportResult): string[] {
  const lines: string[] = [];
  const brought = [
    result.cookies && !result.cookies.error ? counted(result.cookies.imported, "cookie", "cookies") : null,
    result.history && !result.history.error ? counted(result.history.imported, "page", "pages") + " of history" : null,
  ].filter(Boolean);
  if (brought.length) lines.push(`Imported ${brought.join(" and ")} from ${browser}.`);
  const cookies = result.cookies;
  if (cookies?.error) lines.push(cookies.error);
  else if (cookies) {
    const left = cookies.rejected + cookies.skipped;
    if (left > 0) lines.push(`${counted(left, "cookie", "cookies")} couldn't come across (expired, or a kind Brigadier's browser can't keep).`);
  }
  if (result.history?.error) lines.push(result.history.error);
  return lines;
}
