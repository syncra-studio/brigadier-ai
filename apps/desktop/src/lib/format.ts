const MB = 1024 * 1024;

export function formatMb(bytes: number): string {
  return `${(bytes / MB).toFixed(1)} MB`;
}

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < MB) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * MB) return formatMb(bytes);
  return `${(bytes / (1024 * MB)).toFixed(1)} GB`;
}

const compactNumber = new Intl.NumberFormat(undefined, {
  notation: "compact",
  maximumFractionDigits: 1,
});

/** A token count, compactly ("12.4K", "1.2M"). */
export function formatTokens(tokens: number): string {
  return compactNumber.format(tokens);
}

export function formatMs(ms: number): string {
  if (ms >= 10_000) return `${(ms / 1000).toFixed(1)} s`;
  if (ms >= 100) return `${Math.round(ms)} ms`;
  return `${ms.toFixed(1)} ms`;
}

export function formatDuration(ms: number): string {
  const seconds = Math.floor(ms / 1000);
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = seconds % 60;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s}s`;
  return `${s}s`;
}

const timeFormat = new Intl.DateTimeFormat(undefined, {
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  fractionalSecondDigits: 3,
  hour12: false,
});

export function formatClock(epochMs: number): string {
  return timeFormat.format(epochMs);
}

const dateTimeFormat = new Intl.DateTimeFormat(undefined, {
  dateStyle: "medium",
  timeStyle: "short",
});

export function formatDateTime(epochMs: number): string {
  return dateTimeFormat.format(epochMs);
}

const shortTimeFormat = new Intl.DateTimeFormat(undefined, { timeStyle: "short" });

/** A time of day as the thread shows it ("4:08 PM"). */
export function formatTime(epochMs: number): string {
  return shortTimeFormat.format(epochMs);
}

/** How long ago, coarsely ("now", "3m ago", "2h ago", "4d ago"). */
export function formatAgo(epochMs: number, nowMs: number): string {
  const minutes = Math.floor(Math.max(0, nowMs - epochMs) / 60_000);
  if (minutes < 1) return "now";
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  return hours < 24 ? `${hours}h ago` : `${Math.floor(hours / 24)}d ago`;
}

/** "now", "42m", "2h 5m", "3d 4h": the time left until `ms`. */
export function formatCountdown(ms: number, nowMs: number): string {
  const minutes = Math.ceil((ms - nowMs) / 60_000);
  if (minutes <= 0) return "now";
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return minutes % 60 === 0 ? `${hours}h` : `${hours}h ${minutes % 60}m`;
  const days = Math.floor(hours / 24);
  return hours % 24 === 0 ? `${days}d` : `${days}d ${hours % 24}h`;
}

const DAY_MS = 86_400_000;
const weekdayFormat = new Intl.DateTimeFormat(undefined, { weekday: "long" });
const monthDayFormat = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });

/** Midnight of the day `epochMs` falls on. */
function startOfDay(epochMs: number): number {
  return new Date(epochMs).setHours(0, 0, 0, 0);
}

/** Whole days between the calendar days of `epochMs` and `nowMs` (0: the same day). */
function daysAgo(epochMs: number, nowMs: number): number {
  return Math.round((startOfDay(nowMs) - startOfDay(epochMs)) / DAY_MS);
}

/**
 * When a message was sent, as its hover label: "3:09 AM", "Yesterday 3:09 AM", "Thursday
 * 3:09 AM", "Sep 18 3:09 AM" (the same words as the day's separator above it).
 */
export function formatSentAt(epochMs: number, nowMs: number): string {
  const days = daysAgo(epochMs, nowMs);
  const time = formatTime(epochMs);
  if (days === 0) return time;
  if (days === 1) return `Yesterday ${time}`;
  if (days < 7) return `${weekdayFormat.format(epochMs)} ${time}`;
  return `${monthDayFormat.format(epochMs)} ${time}`;
}

/** The separator above the first message of a day: "Yesterday 3:09 AM". */
export function formatDaySeparator(epochMs: number, nowMs: number): string {
  const days = daysAgo(epochMs, nowMs);
  if (days === 0) return `Today ${formatTime(epochMs)}`;
  return formatSentAt(epochMs, nowMs);
}

/** Whether two instants fall on the same calendar day. */
export function sameDay(a: number, b: number): boolean {
  return daysAgo(a, b) === 0;
}

/** A home-relative path for display (`~/Development/app`). */
export function shortPath(path: string): string {
  return path.replace(/^(\/Users|\/home)\/[^/]+(?=\/|$)/, "~");
}
