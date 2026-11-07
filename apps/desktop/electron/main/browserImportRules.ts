// What an import asks of a session, with nothing of Electron in it.
import type { CookiesSetDetails } from "electron";

/** A cookie as the addon read it (crates/browser-data). */
export type ReadCookie = {
  name: string;
  value: string;
  domain: string;
  host_only: boolean;
  path: string;
  expires_unix: number | null;
  secure: boolean;
  http_only: boolean;
  same_site: "None" | "Lax" | "Strict" | null;
  source_insecure: boolean;
};

/** The longest Chromium lets a cookie live, and what a session cookie gets: see sessionCookies.ts. */
export const SESSION_COOKIE_SECONDS = 400 * 24 * 60 * 60;

const SAME_SITE = { None: "no_restriction", Lax: "lax", Strict: "strict" } as const;

/** The arguments that set a read cookie again, in the shape it had: host-only stays host-only. */
export function cookieDetails(cookie: ReadCookie, nowSeconds: number): CookiesSetDetails {
  const host = cookie.domain.replace(/^\./, "");
  // The URL says who set it, for a store that binds cookies to their scheme.
  const scheme = cookie.secure || !cookie.source_insecure ? "https" : "http";
  return {
    url: `${scheme}://${host}${cookie.path || "/"}`,
    name: cookie.name,
    value: cookie.value,
    path: cookie.path || "/",
    secure: cookie.secure,
    httpOnly: cookie.http_only,
    sameSite: cookie.same_site ? SAME_SITE[cookie.same_site] : "unspecified",
    ...(cookie.host_only ? {} : { domain: cookie.domain }),
    // Electron would drop a session cookie at quit; here its copy is never made, so give it the date now.
    expirationDate: cookie.expires_unix ?? nowSeconds + SESSION_COOKIE_SECONDS,
  };
}

/** A cookie of google.com or a subdomain. Orca (MIT) leaves these out of its imports for the same reason. */
export function isGoogleCookie(domain: string): boolean {
  const host = domain.replace(/^\./, "").toLowerCase();
  return host === "google.com" || host.endsWith(".google.com");
}

/** The distinct domains among `domains`, without a leading dot: the first `kept`, and how many in all. */
export function distinctDomains(domains: readonly string[], kept = 20): { shown: string[]; count: number } {
  const seen = new Set(domains.map((domain) => domain.replace(/^\./, "").toLowerCase()).filter(Boolean));
  return { shown: [...seen].slice(0, kept), count: seen.size };
}

/** Where each source profile went last, by its id (`<browser key>/<profile>`). */
export type ImportedSources = Partial<Record<string, { into: string; atMs: number; browser?: string }>>;

/** The browser each Brigadier profile last had an import from, by profile. An entry from before names were kept goes by its key. */
export function importOrigins(sources: ImportedSources): Record<string, string> {
  const latest = new Map<string, { atMs: number; browser: string }>();
  for (const [id, place] of Object.entries(sources)) {
    if (!place || (latest.get(place.into)?.atMs ?? -1) > place.atMs) continue;
    const key = id.split("/")[0] ?? id;
    latest.set(place.into, { atMs: place.atMs, browser: place.browser ?? `${key.charAt(0).toUpperCase()}${key.slice(1)}` });
  }
  return Object.fromEntries([...latest].map(([profile, { browser }]) => [profile, browser]));
}

/** What one source profile's import brought. */
export type ImportedRecord = {
  atMs: number;
  profiles: string[];
  imported: number;
  skipped: number;
  skippedDomains: string[];
  skippedDomainCount: number;
  appBoundDomains: string[];
  history: number;
  bookmarks: number;
  passwords: number;
  passwordsSkipped: number;
  /** Importing over this profile found no usable password, so the ones it had were left alone. */
  passwordsKept: boolean;
  googleLeftOut: number;
};

/** A browser's row: its source profiles' last imports summed up, naming each. Null with none. */
export function mergedRecord(records: readonly ImportedRecord[]): ImportedRecord | null {
  const [first] = records;
  if (!first) return null;
  const sum = (field: (record: ImportedRecord) => number) => records.reduce((total, record) => total + field(record), 0);
  const domains = distinctDomains(records.flatMap((record) => record.skippedDomains));
  return {
    atMs: Math.max(...records.map((record) => record.atMs)),
    profiles: records.flatMap((record) => record.profiles),
    imported: sum((record) => record.imported),
    skipped: sum((record) => record.skipped),
    skippedDomains: domains.shown,
    // Each record keeps only its first domains, so the same site may count once in each: at least this many.
    skippedDomainCount: Math.max(domains.count, ...records.map((record) => record.skippedDomainCount)),
    appBoundDomains: distinctDomains(records.flatMap((record) => record.appBoundDomains)).shown,
    history: sum((record) => record.history),
    bookmarks: sum((record) => record.bookmarks),
    passwords: sum((record) => record.passwords),
    passwordsSkipped: sum((record) => record.passwordsSkipped),
    passwordsKept: records.some((record) => record.passwordsKept),
    googleLeftOut: sum((record) => record.googleLeftOut),
  };
}

/** Importing over a profile keeps its passwords when the source was read and has none usable: replacing would empty it. */
export function passwordsStay<T extends { password: string }>(replacing: boolean, read: boolean, logins: readonly T[], hasSite: (login: T) => boolean): boolean {
  return replacing && read && !logins.some((login) => login.password !== "" && hasSite(login));
}

/** The first import shows one notice; from the saved file, so it stays shown once. */
export const firstImport = (saved: { noticed?: boolean }): boolean => saved.noticed !== true;
