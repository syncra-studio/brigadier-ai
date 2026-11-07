// Settings › Import Browser Data: reads a profile of another browser through the addon
// (crates/shell-native/src/browser_import.rs) and writes it into one Brigadier profile: cookies and
// site storage into its session, and history, bookmarks and passwords through their own stores.
// A source's data, passwords above all, stays in main: the page hears counts.
import fs from "node:fs";
import path from "node:path";

import { app, nativeImage, session, shell, webContents, type Session } from "electron";

import { addCommands, channelId, channelSender, type Args } from "./commands";
import { importBookmarks } from "./browserBookmarks";
import { importHistory } from "./browserHistory";
import { cookieDetails, distinctDomains, firstImport, type ImportedRecord, type ImportedSources, importOrigins, isGoogleCookie, mergedRecord, passwordsStay, type ReadCookie } from "./browserImportRules";
import { applyStagedStorage } from "./browserImportStaged";
import { importLogins, loginSite, replaceLogins } from "./browserVault";
import { replaceFile } from "./durableStorage";
import { note } from "./events";
import { dev } from "./identity";
import { native } from "./native";
import { profileFolderName, profileOfStorage, profilePartition } from "./profiles";

/** How long a read waits to be written before its passwords are let go of. */
const READ_KEPT_MS = 2 * 60 * 1000;
const COOKIES_AT_ONCE = 50;

type Parts = { signIns: boolean; history: boolean; bookmarks: boolean; passwords: boolean };
type ReadProfile = {
  type: "read";
  profileName: string;
  signInsRead: boolean;
  cookies: ReadCookie[];
  leftOutDomains: string[];
  appBoundDomains: string[];
  siteStorageFrom: string | null;
  history: { url: string; title: string; visited_at_ms: number }[];
  historyError: string | null;
  bookmarks: { url: string; title: string; root: string; folders: string[]; added_at_ms: number | null }[];
  bookmarksError: string | null;
  passwordsRead: boolean;
  logins: Parameters<typeof importLogins>[1][number][];
  loginsLeftOut: number;
  passwordsError: string | null;
};
type Failed = { type: "failed"; reason: string; message: string };
type Listed = { key: string; name: string; appPath: string; state: string; profiles: unknown[]; emptyProfiles: string[]; signIns: boolean; passwords: boolean };

/** What's kept of imports: each browser's last, each source profile's place here, and the notice seen. */
type Saved = {
  /** Each source profile's last import, by its id, which the browser's row sums up. */
  browsers?: Partial<Record<string, { records?: Partial<Record<string, ImportedRecord>>; denied?: boolean }>>;
  sources?: ImportedSources;
  noticed?: boolean;
};

const inData = (name: string) => path.join(app.getPath("userData"), name);
const savedFile = () => inData("browser-imports.json");
const stagedRoot = () => inData("browser-import-staged");
const partitionFolder = (profileId: string) => path.join(app.getPath("userData"), "Partitions", profileFolderName(profileId));

/** Where to look for browsers. A dev run looks in a scratch home, never at the user's real browsers. */
const home = () => (dev && process.env.BRIGADIER_IMPORT_HOME) || app.getPath("home");

function readSaved(): Saved {
  try {
    return JSON.parse(fs.readFileSync(savedFile(), "utf8")) as Saved;
  } catch {
    return {};
  }
}

function changeSaved(change: (saved: Saved) => void): void {
  const saved = readSaved();
  change(saved);
  replaceFile(savedFile(), JSON.stringify(saved));
}

/** Profiles whose session Chromium has open this run; Electron can't close one, so its files can't change. */
const openSessions = new Set<string>();
const icons = new Map<string, string | null>();

async function iconOf(file: string): Promise<string | null> {
  if (!icons.has(file)) {
    // app.getFileIcon ends the whole app on macOS (SIGTRAP on Electron's build here), even for Safari.
    const icon = process.platform === "darwin" ? nativeImage.createThumbnailFromPath(file, { width: 64, height: 64 }) : app.getFileIcon(file, { size: "large" });
    icons.set(file, await icon.then((image) => image.toDataURL(), () => null));
  }
  return icons.get(file) ?? null;
}

async function listSources(): Promise<unknown[]> {
  const listed = JSON.parse(await native().importSources(home())) as Listed[];
  const saved = readSaved();
  return Promise.all(
    listed.map(async ({ appPath, ...source }) => {
      const known = saved.browsers?.[source.key];
      return {
        ...source,
        state: source.state === "ready" && known?.denied ? "needsKeychainAccess" : source.state,
        icon: await iconOf(appPath),
        record: mergedRecord(Object.values(known?.records ?? {}).filter((one) => one !== undefined)),
        imports: saved.sources ?? {},
        firstImport: firstImport(saved),
      };
    }),
  );
}

/** Reads waiting for their profile to be made, each holding what it read, by token. */
const reads = new Map<number, { key: string; name: string; id: string; read: ReadProfile; timer: ReturnType<typeof setTimeout> }>();
let nextToken = 1;

async function readProfile({ key, name, id, what }: Args): Promise<Failed | { type: "ready"; token: number }> {
  const browser = key as string;
  const read = JSON.parse(await native().importRead(home(), browser, id as string, JSON.stringify(what))) as ReadProfile | Failed;
  if (read.type === "failed") {
    if (read.reason === "needsKeychainAccess") {
      changeSaved((saved) => {
        saved.browsers = { ...saved.browsers, [browser]: { ...saved.browsers?.[browser], denied: true } };
      });
    }
    return read;
  }
  const nothing = read.cookies.length + read.leftOutDomains.length + read.appBoundDomains.length + read.history.length + read.bookmarks.length + read.logins.length + read.loginsLeftOut === 0;
  if (nothing && !read.siteStorageFrom && !read.historyError && !read.bookmarksError && !read.passwordsError) {
    return { type: "failed", reason: "readFailed", message: `There's nothing to bring from ${read.profileName}.` };
  }
  const token = nextToken++;
  reads.set(token, { key: browser, name: name as string, id: id as string, read, timer: setTimeout(() => reads.delete(token), READ_KEPT_MS) });
  return { type: "ready", token };
}

/** Adds cookies a few at a time, telling `progress`. Returns the domains the store refused. */
async function addCookies(target: Session, cookies: readonly ReadCookie[], progress: (done: number) => void): Promise<string[]> {
  const refused: string[] = [];
  const now = Math.floor(Date.now() / 1000);
  for (let at = 0; at < cookies.length; at += COOKIES_AT_ONCE) {
    const batch = cookies.slice(at, at + COOKIES_AT_ONCE);
    await Promise.all(
      batch.map((cookie) =>
        target.cookies.set(cookieDetails(cookie, now)).catch((error: unknown) => {
          refused.push(cookie.domain);
          note("import-cookie-refused", { domain: cookie.domain, error: String(error) });
        }),
      ),
    );
    progress(Math.min(at + COOKIES_AT_ONCE, cookies.length));
  }
  await target.cookies.flushStore();
  return refused;
}

/** Site storage goes in now if the session isn't open, and otherwise when the app next starts. */
async function bringSiteStorage(profileId: string, from: string): Promise<"copied" | "nextStart"> {
  if (!openSessions.has(profileId)) {
    await native().importSiteStorage(from, partitionFolder(profileId));
    return "copied";
  }
  await native().importSiteStorage(from, path.join(stagedRoot(), profileFolderName(profileId)));
  return "nextStart";
}

async function writeProfile({ token, target, replace, progress }: Args, sender: Parameters<typeof channelSender>[0]) {
  const waiting = reads.get(token as number);
  if (!waiting) throw new Error("That read has expired. Import again.");
  clearTimeout(waiting.timer);
  reads.delete(token as number);
  const { key, id, read } = waiting;
  const profileId = target as string;
  const partition = profilePartition(profileId);
  const tell = channelSender(sender, channelId(progress));
  const replacing = replace === true;
  const signIns = read.signInsRead;
  // Google binds these to the browser that made them, so they can't keep anyone signed in here.
  const cookies = read.cookies.filter((cookie) => !isGoogleCookie(cookie.domain));
  const googleLeftOut = read.cookies.length - cookies.length;

  let refused: string[] = [];
  let siteStorage: "copied" | "nextStart" | null = null;
  if (signIns) {
    // Before the session opens: it holds these folders from then on.
    if (read.siteStorageFrom) siteStorage = await bringSiteStorage(profileId, read.siteStorageFrom);
    const target = session.fromPartition(partition);
    if (replacing) {
      const storages = ["cookies", "cachestorage", "serviceworkers", ...(siteStorage === "copied" ? [] : ["localstorage", "indexdb"])];
      await target.clearStorageData({ storages: storages as never });
      target.flushStorageData();
    }
    const total = cookies.length;
    tell(JSON.stringify({ type: "cookies", done: 0, total }));
    refused = await addCookies(target, cookies, (done) => tell(JSON.stringify({ type: "cookies", done, total })));
    if (replacing) {
      // Their pages were signed in as someone else: the page said they would reload.
      for (const contents of webContents.getAllWebContents()) {
        if (contents.session === target && contents.getType() === "webview") contents.reload();
      }
    }
  }
  tell(JSON.stringify({ type: "history" }));
  let historyError = read.historyError;
  let history = 0;
  if (read.history.length > 0) {
    try {
      history = importHistory(profileId, read.history.map((visit) => ({ url: visit.url, title: visit.title, atMs: visit.visited_at_ms })));
    } catch (error) {
      historyError = `History couldn't be added (${String(error)}).`;
    }
  }
  let bookmarksError = read.bookmarksError;
  let bookmarks = 0;
  if (read.bookmarks.length > 0) {
    try {
      const added = await importBookmarks(profileId, read.bookmarks.map((bookmark) => ({ url: bookmark.url, title: bookmark.title, folders: bookmark.folders, ...(bookmark.added_at_ms === null ? {} : { addedMs: bookmark.added_at_ms }) })));
      bookmarks = added.added;
    } catch (error) {
      bookmarksError = `Bookmarks couldn't be added (${String(error)}).`;
    }
  }
  let passwordsError = read.passwordsError;
  let passwords = 0;
  let passwordsSkipped = read.loginsLeftOut;
  // replaceLogins with nothing usable would empty the profile, so that swap doesn't happen.
  const passwordsKept = passwordsStay(replacing, read.passwordsRead, read.logins, (login) => loginSite(login.url) !== null);
  if (passwordsKept) passwordsSkipped += read.logins.length;
  else if (read.logins.length > 0) {
    try {
      const counts = await (replacing ? replaceLogins : importLogins)(profileId, read.logins);
      passwords = counts.added + counts.updated + counts.unchanged;
      passwordsSkipped += counts.skipped;
    } catch {
      // Not the error's text: nothing about a password's store needs repeating.
      passwordsError = "Passwords couldn't be saved.";
    }
  }
  const domains = distinctDomains([...read.leftOutDomains, ...read.appBoundDomains, ...refused]);
  const record: ImportedRecord = {
    atMs: Date.now(),
    profiles: [read.profileName],
    imported: cookies.length - refused.length,
    skipped: read.leftOutDomains.length + read.appBoundDomains.length + refused.length,
    skippedDomains: domains.shown,
    skippedDomainCount: domains.count,
    appBoundDomains: distinctDomains(read.appBoundDomains).shown,
    history,
    bookmarks,
    passwords,
    passwordsSkipped,
    passwordsKept,
    googleLeftOut,
  };
  changeSaved((saved) => {
    saved.noticed = true;
    saved.sources = { ...saved.sources, [id]: { into: profileId, atMs: record.atMs, browser: waiting.name } };
    // A try at another part says nothing of what the import brought.
    if (signIns) saved.browsers = { ...saved.browsers, [key]: { records: { ...saved.browsers?.[key]?.records, [id]: record } } };
  });
  note("import-written", { key, profileId, imported: record.imported, skipped: record.skipped, history, bookmarks, passwords, siteStorage, replacing });
  return {
    type: "done",
    record,
    historyError,
    siteStorage,
    ...(passwordsError ? { passwordsError } : {}),
    ...(bookmarksError ? { bookmarksError } : {}),
  };
}

export function installBrowserImport(): void {
  // Before any session opens.
  applyStagedStorage(stagedRoot(), path.join(app.getPath("userData"), "Partitions"), (folder) => note("import-site-storage-applied", { folder }));
  app.on("session-created", (created) => {
    const profileId = profileOfStorage(created.storagePath);
    if (profileId) openSessions.add(profileId);
  });
  addCommands({
    browser_import_sources: () => listSources(),
    browser_import_origins: () => importOrigins(readSaved().sources ?? {}),
    browser_import_read: (args) => readProfile(args),
    browser_import_write: (args, { sender }) => writeProfile(args, sender),
    browser_import_open_access: () => {
      if (process.platform === "darwin") void shell.openExternal("x-apple.systempreferences:com.apple.preference.security?Privacy_AllFiles");
    },
  });
}
