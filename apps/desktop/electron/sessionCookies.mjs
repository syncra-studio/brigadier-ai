// Electron hard-codes session cookies not to persist (shell/browser/net/network_context_service.cc,
// persist_session_cookies = false, with no API or switch), so every "stay signed in until I close the
// browser" sign-in would end at quit. Each session cookie is rewritten with an expiry as it's set.
// The site sees the same cookie in requests and document.cookie, and clearing or overwriting it works
// as before. It's what Chrome's "continue where you left off" gives users, by a different route.
const KEEP_SECONDS = 400 * 24 * 60 * 60;

/** The arguments that set `cookie` again with an expiry; null if it can't be set from outside. */
export function persistentCopy(cookie, nowSeconds) {
  if (!cookie.session || !cookie.domain) return null;
  const host = cookie.domain.replace(/^\./, "");
  const url = `${cookie.secure ? "https" : "http"}://${host}${cookie.path || "/"}`;
  return {
    url,
    name: cookie.name,
    value: cookie.value,
    path: cookie.path,
    secure: cookie.secure,
    httpOnly: cookie.httpOnly,
    sameSite: cookie.sameSite,
    // A host-only cookie has no Domain attribute, and mustn't gain one.
    ...(cookie.hostOnly ? {} : { domain: cookie.domain }),
    expirationDate: nowSeconds + KEEP_SECONDS,
  };
}

export function keepSessionCookies(session, { note }) {
  let flush = null;
  session.cookies.on("changed", (_event, cookie, _cause, removed) => {
    if (removed || !cookie.session) return;
    const copy = persistentCopy(cookie, Math.floor(Date.now() / 1000));
    if (!copy) return;
    session.cookies.set(copy).then(
      () => {
        // SQLite commits about every 30 s: flush so that a crash soon after sign-in keeps it.
        if (flush === null) {
          flush = setTimeout(() => {
            flush = null;
            void session.cookies.flushStore();
          }, 1000);
        }
      },
      (error) => note("session-cookie-keep-failed", { name: cookie.name, domain: cookie.domain, error: String(error) }),
    );
  });
}
