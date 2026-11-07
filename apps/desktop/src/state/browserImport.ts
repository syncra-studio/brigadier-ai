import { invoke, newChannel } from "@/ipc/native";

import { create } from "zustand";

import type { ImportFailure, ImportOutcome, ImportProgress, ImportRecord, ImportSource } from "@/ipc/generated";
import { formatAgo } from "@/lib/format";
import { openSettings, select } from "@/state/actions";
import { allProfiles, type BrowserProfile, createProfile, nextHue, placeSession, placeTab } from "@/state/browserProfiles";
import { newSessionTab, openBrowserTab } from "@/state/sessionTabs";
import { useApp } from "@/state/store";
import { toast } from "@/state/toasts";

/*
 * Settings › Import Browser Data: the shell's commands, and the pure words and choices the page
 * shows for each browser (its row, the toasts after an import, why one can't run).
 */

/** A browser as the Electron shell lists it. */
export type ListedSource = ImportSource & {
  /** Its profiles with nothing to bring, by name: they get no profile here. */
  emptyProfiles: string[];
  /** Whether its cookies may be read: not where "Chromium Safe Storage" locks them. */
  signIns: boolean;
  /** Whether its passwords may be read: never on Linux, nor under that key. */
  passwords: boolean;
  /** Where each source profile (any browser's) went last, by its id. */
  imports: Partial<Record<string, { into: string; atMs: number }>>;
  /** No import has run yet, so the page shows its one notice about agents. */
  firstImport: boolean;
};

/** The installed browsers in the page's order, each with where it stands. */
export function importSources(): Promise<ListedSource[]> {
  return invoke<ListedSource[]>("browser_import_sources").then((sources) => {
    listed = { sources, atMs: Date.now() };
    return sources;
  });
}

/** The browser each profile last had an import from, by profile; none where the shell keeps no imports. */
export function importOrigins(): Promise<Partial<Record<string, string>>> {
  return invoke<Partial<Record<string, string>> | null>("browser_import_origins").then((origins) => origins ?? {}, () => ({}));
}

/** The browsers as last listed, and when: the page shows them at once when it opens again. */
export type ListedSources = { sources: ListedSource[]; atMs: number };
let listed: ListedSources | null = null;
let prefetching: Promise<ListedSource[]> | null = null;

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
export function openingImportSources(): Promise<ListedSource[]> {
  return prefetching ?? importSources();
}

/** Import Browser Data opens with this profile as what the browser's first profile replaces. */
export const useImportInto = create<{ into: string | null }>(() => ({ into: null }));

export function importInto(profile: string): void {
  useImportInto.setState({ into: profile });
  openSettings("browserImport");
}

/** The record the shell keeps of an import; the fields past `ImportRecord` are the Electron shell's. */
export type ImportedRecord = ImportRecord & {
  passwords?: number;
  passwordsSkipped?: number;
  passwordsKept?: boolean;
  bookmarks?: number;
  googleLeftOut?: number;
  appBoundDomains?: string[];
};
export type ImportedOutcome =
  | { type: "done"; record: ImportedRecord; historyError: string | null; passwordsError?: string; bookmarksError?: string; siteStorage?: "copied" | "nextStart" | null }
  | Extract<ImportOutcome, { type: "failed" }>;

/**
 * Whether Try again can help a password import that failed. Only a Chromium browser's key can
 * come on asking again: Safari's iCloud Keychain and Firefox's Primary Password stay shut.
 */
export const passwordsRetryable = (family: ImportSource["family"]) => family === "chromium";

/** What a tick in the import brings. */
export type ImportWhat = { signIns: boolean; history: boolean; bookmarks: boolean; passwords: boolean };

/** One ticked profile of the other browser, and where it goes: "new", or the profile it replaces. */
export type ImportChoice = { id: string; name: string; into: string };

export type ProfileImportResult = {
  choice: ImportChoice;
  /** The Brigadier profile it went into. */
  profile: BrowserProfile;
  replaced: boolean;
  what: ImportWhat;
  outcome: ImportedOutcome;
};

/** A new profile's name: the source's own, with the browser's where it wouldn't say which it is. */
export function importedName(browser: string, name: string, taken: readonly string[]): string {
  const plain = !/^(default|default-release|person \d+)$/i.test(name) && !taken.includes(name);
  return plain ? name : `${browser}: ${name}`;
}

/**
 * Imports one profile. It is read first, so a browser that can't be read makes no profile; a new
 * Brigadier profile is made for what was read, and it is written there.
 */
export async function importProfile(
  source: Pick<ImportSource, "key" | "name">,
  choice: ImportChoice,
  what: ImportWhat,
  onProgress: (progress: ImportProgress) => void,
): Promise<ProfileImportResult> {
  const replaced = choice.into !== "new";
  const profiles = allProfiles();
  const existing = profiles.find((profile) => profile.id === choice.into);
  onProgress({ type: "reading" });
  const read = await invoke<{ type: "ready"; token: number } | Extract<ImportOutcome, { type: "failed" }>>("browser_import_read", {
    key: source.key,
    name: source.name,
    id: choice.id,
    what,
  });
  if (read.type === "failed") {
    const profile = existing ?? { id: "", name: choice.name, hue: nextHue(profiles) };
    return { choice, profile, replaced, what, outcome: read };
  }
  const name = importedName(source.name, choice.name, profiles.map((profile) => profile.name));
  const target = existing?.id ?? createProfile(name, nextHue(profiles));
  const outcome = await invoke<ImportedOutcome>("browser_import_write", {
    token: read.token,
    target,
    replace: replaced,
    what,
    progress: newChannel<ImportProgress>(onProgress),
  });
  const profile = allProfiles().find((each) => each.id === target) ?? { id: target, name, hue: nextHue(profiles) };
  return { choice, profile, replaced, what, outcome };
}

/** The session most lately active, where a site or a profile opens from Settings. */
function latestSession(): string | null {
  const open = Object.values(useApp.getState().conversations).filter((conversation) => conversation.lifecycle !== "archived");
  return open.toSorted((a, b) => b.updatedAtMs - a.updatedAtMs)[0]?.id ?? null;
}

/** Opens `site` to sign in again, on `profile`, in the latest session. */
export function signInOn(profile: string, site: string): void {
  const session = latestSession();
  if (!session) {
    toast("Open a session first: sites open in its browser tabs.");
    return;
  }
  const tabId = openBrowserTab(session, `https://${site}/`);
  placeTab(tabId, profile);
  void select({ type: "conversation", id: session });
}

/** Puts that session on `profile`, in a New tab, in front. */
export function openProfile(profile: string): void {
  const session = latestSession();
  if (!session) return;
  placeSession(session, profile);
  newSessionTab(session, "newTab");
  void select({ type: "conversation", id: session });
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
  if (profiles === 0) return { status: "Nothing to bring", tone: "idle", details: [], action: "none", label: "Import…" };
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

/** Failures that open a dialog to put right (the rest show in the results). */
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
