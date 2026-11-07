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

/** The first import shows one notice; from the saved file, so it stays shown once. */
export const firstImport = (saved: { noticed?: boolean }): boolean => saved.noticed !== true;
