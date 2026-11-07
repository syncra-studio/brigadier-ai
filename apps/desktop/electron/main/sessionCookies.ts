// Electron hard-codes session cookies not to persist (shell/browser/net/network_context_service.cc,
// persist_session_cookies = false, with no API or switch), so every "stay signed in until I close the
// browser" sign-in would end at quit. Each session cookie is rewritten with an expiry as it's set: that does
// change the page's own expiry, from none to 400 days, and only for a cookie that had none. Requests and
// document.cookie are the same; cookieStore.get() shows the date. It's what Chrome's "continue where you left
// off" gives users, by a different route. Electron's Cookie doesn't say whether one is partitioned, so a
// partitioned session cookie gains an unpartitioned copy.
import type { Cookie, CookiesSetDetails, Session } from "electron";

import type { Note } from "./events";

const KEEP_SECONDS = 400 * 24 * 60 * 60;

/** The arguments that set `cookie` again with an expiry; null if it can't be set from outside. */
export function persistentCopy(cookie: Cookie, nowSeconds: number): CookiesSetDetails | null {
  if (!cookie.session || !cookie.domain) return null;
  const host = cookie.domain.replace(/^\./, "");
  const url = `${cookie.secure ? "https" : "http"}://${host}${cookie.path || "/"}`;
  return {
    url,
    name: cookie.name,
    value: cookie.value,
    ...(cookie.path === undefined ? {} : { path: cookie.path }),
    ...(cookie.secure === undefined ? {} : { secure: cookie.secure }),
    ...(cookie.httpOnly === undefined ? {} : { httpOnly: cookie.httpOnly }),
    sameSite: cookie.sameSite,
    // A host-only cookie has no Domain attribute, and mustn't gain one.
    ...(cookie.hostOnly ? {} : { domain: cookie.domain }),
    expirationDate: nowSeconds + KEEP_SECONDS,
  };
}

/** A copy asked for and not yet seen landing. `gone`: the page dropped the cookie since it was asked for. */
type CopyOnItsWay = { value: string; expires: number; gone: boolean };

export function keepSessionCookies(session: Session, { note }: { note: Note }): void {
  let flush: ReturnType<typeof setTimeout> | null = null;
  // SQLite commits about every 30 s: flush so that a crash soon after a sign-in or a sign-out keeps it.
  const flushSoon = () => {
    if (flush !== null) return;
    flush = setTimeout(() => {
      flush = null;
      void session.cookies.flushStore();
    }, 1000);
  };
  // A copy can land after the page has deleted the cookie, and would bring it back: a sign-out that doesn't
  // hold. The change events come in the store's order, so they say which happened first.
  const onTheirWay = new Map<string, CopyOnItsWay[]>();
  session.cookies.on("changed", (_event, cookie, cause, removed) => {
    flushSoon();
    const key = `${cookie.domain}|${cookie.path}|${cookie.name}`;
    const copies = onTheirWay.get(key) ?? [];
    if (removed) {
      if (cause !== "overwrite") for (const copy of copies) copy.gone = true;
      return;
    }
    if (!cookie.session) {
      const landed = copies[0];
      if (!landed || landed.value !== cookie.value || landed.expires !== cookie.expirationDate) return;
      copies.shift();
      if (copies.length === 0) onTheirWay.delete(key);
      // With a later copy still coming, that one overwrites this.
      if (landed.gone && copies.length === 0) {
        const url = persistentCopy({ ...cookie, session: true }, 0)?.url;
        if (url) void session.cookies.remove(url, cookie.name);
      }
      return;
    }
    const copy = persistentCopy(cookie, Math.floor(Date.now() / 1000));
    if (!copy) return;
    const waiting: CopyOnItsWay = { value: cookie.value, expires: copy.expirationDate as number, gone: false };
    onTheirWay.set(key, [...copies, waiting]);
    session.cookies.set(copy).catch((error) => {
      const left = (onTheirWay.get(key) ?? []).filter((each) => each !== waiting);
      if (left.length > 0) onTheirWay.set(key, left);
      else onTheirWay.delete(key);
      note("session-cookie-keep-failed", { name: cookie.name, domain: cookie.domain, error: String(error) });
    });
  });
}
