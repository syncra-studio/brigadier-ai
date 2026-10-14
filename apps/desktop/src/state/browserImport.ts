import { Channel, invoke } from "@tauri-apps/api/core";

import type { ImportFailure, ImportOutcome, ImportProgress, ImportRecord, ImportSource } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";

/*
 * Settings › Import Browser Data: the shell's commands, and the pure words and choices the page
 * shows for each browser (its row, the toasts after an import, why one can't run).
 */

/** The installed browsers in the page's order, each with where it stands. */
export function importSources(): Promise<ImportSource[]> {
  return invoke<ImportSource[]>("browser_import_sources").then((sources) => {
    listed = { sources, atMs: Date.now() };
    return sources;
  });
}

/** The browsers as last listed, and when: the page shows them at once when it opens again. */
export type ListedSources = { sources: ImportSource[]; atMs: number };
let listed: ListedSources | null = null;
let prefetching: Promise<ImportSource[]> | null = null;

export function lastImportSources(): ListedSources | null {
  return listed;
}

/** Lists the browsers ahead of the page opening (the pointer came to its row). */
export function prefetchImportSources(): void {
  if (prefetching) return;
  prefetching = importSources().finally(() => {
    prefetching = null;
  });
  prefetching.catch(() => {});
}

/** The page's read on opening: the prefetch's, while it is on its way, else a new one. */
export function openingImportSources(): Promise<ImportSource[]> {
  return prefetching ?? importSources();
}

/**
 * Copies the cookies of the profiles `ids` of the browser `key` into Brigadier's browser, and
 * their history when `history` is set; `onProgress` hears how it goes.
 */
export function importBrowser(
  key: string,
  ids: readonly string[],
  history: boolean,
  onProgress: (progress: ImportProgress) => void,
): Promise<ImportOutcome> {
  return invoke<ImportOutcome>("browser_import", {
    key,
    ids,
    history,
    progress: new Channel<ImportProgress>(onProgress),
  });
}

/** Opens System Settings at Full Disk Access. */
export function openFullDiskAccess(): Promise<void> {
  return invoke("browser_import_open_access");
}

const number = new Intl.NumberFormat();

/** "1 cookie", "2,790 cookies". */
export function counted(n: number, one: string, many: string): string {
  return `${number.format(n)} ${n === 1 ? one : many}`;
}

export type RowTone = "ready" | "attention" | "idle";
/** What a row's button does: import, look again, ask for Full Disk Access, or nothing. */
export type RowAction = "import" | "recheck" | "grant" | "none";

export type RowView = {
  status: string;
  tone: RowTone;
  /** Shown after the status, apart from each other. */
  details: string[];
  action: RowAction;
  /** The button's words; null: the row has no button. */
  label: string | null;
};

/** The cookies a browser's profiles keep in all; null when one couldn't be counted. */
export function totalCookies(source: ImportSource): number | null {
  let total = 0;
  for (const profile of source.profiles) {
    if (profile.cookies === null) return null;
    total += profile.cookies;
  }
  return total;
}

/** The skipped domains as a row names them: up to three, then how many more. */
export function domainsText(domains: readonly string[], count: number): string {
  const shown = domains.slice(0, 3);
  if (shown.length === 0) return "";
  if (count > 3) return `${shown.join(", ")} and ${number.format(count - 3)} more`;
  if (shown.length === 1) return shown[0] ?? "";
  return `${shown.slice(0, -1).join(", ")} and ${shown.at(-1)}`;
}

/** What the last import brought, as the row's details. */
export function recordDetails(record: ImportRecord): string[] {
  const details = [`${record.profiles.join(", ")} · ${counted(record.imported, "cookie", "cookies")} imported`];
  if (record.skipped > 0) {
    const domains = domainsText(record.skippedDomains, record.skippedDomainCount);
    details.push(`${number.format(record.skipped)} skipped${domains ? ` (${domains})` : ""}`);
  }
  return details;
}

/** A browser's row: its status, details and its one button. */
export function rowView(source: ImportSource): RowView {
  switch (source.state) {
    case "running":
      return { status: `Running · quit ${source.name} to import`, tone: "attention", details: [], action: "recheck", label: "Recheck" };
    case "needsFullDiskAccess":
      return { status: "Needs Full Disk Access", tone: "attention", details: [], action: "grant", label: "Grant access…" };
    case "noKeychainKey":
      return { status: "No encryption key in Keychain", tone: "idle", details: [], action: "recheck", label: "Recheck" };
    case "unavailable":
      return { status: "Unavailable", tone: "idle", details: [], action: "none", label: null };
    case "needsKeychainAccess":
      return { status: "Needs Keychain access", tone: "attention", details: [], action: "import", label: "Import…" };
    case "ready":
      break;
  }
  const profiles = source.profiles.length;
  const total = totalCookies(source);
  const profileCount = counted(profiles, "profile", "profiles");
  if (profiles === 0 || total === 0) {
    return { status: "No cookies yet", tone: "idle", details: profiles === 0 ? [] : [profileCount], action: "none", label: "Import…" };
  }
  const details = source.record
    ? recordDetails(source.record)
    : [profileCount, ...(total === null ? [] : [counted(total, "cookie", "cookies")])];
  return { status: "Ready", tone: "ready", details, action: "import", label: "Import…" };
}

/** The badge after a browser's name once something was imported from it. */
export function importedBadge(record: ImportRecord, nowMs: number): string {
  const ago = formatAgo(record.atMs, nowMs);
  return `Imported ${ago === "now" ? "just now" : ago}`;
}

/** The toast after an import that ran. */
export function doneToast(name: string, record: ImportRecord): string {
  const history = record.history > 0 ? counted(record.history, "page", "pages") + " of history" : null;
  if (record.imported > 0) {
    const cookies = counted(record.imported, "cookie", "cookies");
    return `Imported ${history ? `${cookies} and ${history}` : cookies} from ${name}`;
  }
  if (history) return `Imported ${history} from ${name}`;
  return `No cookies were imported from ${name}`;
}

/** That toast's tone: plain when nothing came over, as nothing went wrong but nothing was done. */
export function doneTone(record: ImportRecord): "success" | "neutral" {
  return record.imported > 0 || record.history > 0 ? "success" : "neutral";
}

/** Failures that open a dialog to put right (the rest end in an error toast). */
export function opensDialog(reason: ImportFailure): boolean {
  return reason === "running" || reason === "needsFullDiskAccess" || reason === "needsKeychainAccess" || reason === "noKeychainKey";
}

/** Whether trying again can help. */
export function retryable(reason: ImportFailure): boolean {
  return reason !== "gone";
}

/** Why a browser can't be imported from, in plain words. */
export function reasonText(reason: ImportFailure, name: string): string {
  switch (reason) {
    case "running":
      return `Quit ${name} first, so its cookies can be read.`;
    case "needsFullDiskAccess":
      return "Give Brigadier Full Disk Access in System Settings › Privacy & Security, then try again.";
    case "needsKeychainAccess":
      return `Brigadier needs Keychain access to read ${name}'s cookie encryption key. Choose Allow when macOS asks, then try again.`;
    case "noKeychainKey":
      return `No matching encryption key was found in your Keychain. Open ${name} and sign in to a site, then try again.`;
    case "gone":
      return "That browser or profile is no longer here.";
    case "readFailed":
      return `${name}'s cookies couldn't be read.`;
  }
}

/** The import's progress line. */
export function progressText(progress: ImportProgress | null): string {
  if (!progress || progress.type === "reading") return "Reading and decrypting cookies. macOS may ask for Keychain access.";
  if (progress.type === "cookies") {
    return `Adding cookies… ${number.format(progress.done)} of ${number.format(progress.total)}`;
  }
  return "Adding history…";
}
