import { invoke } from "@/ipc/native";

import type {
  BrowserClear,
  BrowserDownload,
  BrowserSite,
  BrowserVisit,
  HistoryFilter,
  SitePermission,
} from "@/ipc/generated";

/*
 * What the built-in browser remembers (its history, downloads and site choices, kept by the
 * shell), and the pure helpers Settings › Browser lists them with.
 */

/** How many pages the history view asks for at a time. */
export const HISTORY_PAGE = 200;

/** History newest first; `after` (the last visit shown) asks for the next page. */
export function browserHistory(
  profile: string,
  query: string,
  filter: HistoryFilter,
  after: BrowserVisit | null,
  limit = HISTORY_PAGE,
): Promise<BrowserVisit[]> {
  return invoke<BrowserVisit[]>("browser_history", {
    profile,
    query,
    filter,
    beforeMs: after?.atMs ?? null,
    beforeId: after?.id ?? null,
    limit,
  });
}

export function removeHistory(profile: string, ids: number[]): Promise<void> {
  return invoke("browser_history_remove", { profile, ids });
}

/** A download as the shell lists it: while running, its bytes so far and in all (0: not known). */
export type DownloadRecord = Omit<BrowserDownload, "state"> & {
  state: BrowserDownload["state"] | "cancelled";
  receivedBytes?: number;
  totalBytes?: number;
};

export function browserDownloads(profile: string, query: string): Promise<DownloadRecord[]> {
  return invoke<DownloadRecord[]>("browser_downloads", { profile, query });
}

/** Opens a finished download with the system's app for its type. */
export function openDownload(profile: string, id: number): Promise<void> {
  return invoke("browser_download_open", { profile, id });
}

/** Forgets downloads (the files stay): `ids`, or every finished one. */
export function removeDownloads(profile: string, ids: number[] | null): Promise<void> {
  return invoke("browser_downloads_remove", { profile, ids });
}

/** Stops a running download; its partial file goes. */
export function cancelDownload(profile: string, id: number): Promise<void> {
  return invoke("browser_download_cancel", { profile, id });
}

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"];

function bytesText(bytes: number): string {
  const unit = Math.min(Math.floor(Math.log(Math.max(bytes, 1)) / Math.log(1000)), BYTE_UNITS.length - 1);
  const value = bytes / 1000 ** unit;
  return `${unit === 0 || value >= 100 ? Math.round(value) : value.toFixed(1)} ${BYTE_UNITS[unit]}`;
}

/** How far a running download is, for its row: "45% · 2.3 MB of 5.1 MB". */
export function downloadProgress({ receivedBytes = 0, totalBytes = 0 }: DownloadRecord): string {
  if (totalBytes <= 0) return bytesText(receivedBytes);
  return `${Math.floor((receivedBytes / totalBytes) * 100)}% · ${bytesText(receivedBytes)} of ${bytesText(totalBytes)}`;
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
export function clearBrowsingData(profile: string, clear: BrowserClear): Promise<void> {
  return invoke("browser_clear", { profile, clear });
}

/** How many sites the browser keeps cookies or other data for; null where it can't tell. */
export function siteDataCount(profile: string): Promise<number | null> {
  return invoke<number | null>("browser_site_count", { profile });
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

/** The sites that asked in `profile`: permissions are each profile's own. */
export function browserSites(profile: string): Promise<BrowserSite[]> {
  return invoke<BrowserSite[]>("browser_sites", { profile });
}

/** Sets what a site may use without asking: its camera, its microphone, its location. */
export function setSite(
  profile: string,
  origin: string,
  choice: { camera?: SitePermission; microphone?: SitePermission; location?: SitePermission },
): Promise<void> {
  return invoke("browser_site_set", { profile, origin, camera: choice.camera ?? null, microphone: choice.microphone ?? null, location: choice.location ?? null });
}
