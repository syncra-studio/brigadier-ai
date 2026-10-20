/** Only web URLs belong in the embedded browser. Local servers default to HTTP. */
export function webAddress(typed: string): string | null {
  const text = typed.trim();
  if (!text || /\s/.test(text)) return null;
  const scheme = /^([a-z][a-z0-9+.-]*):\/\//i.exec(text)?.[1]?.toLowerCase();
  if (scheme) return scheme === "http" || scheme === "https" ? text : null;
  const host = text.split(/[/?#]/)[0] ?? "";
  if (
    /^(localhost|127\.\d+\.\d+\.\d+|0\.0\.0\.0|\[::1\])(:\d+)?$/i.test(host)
  ) {
    return `http://${text}`;
  }
  if (host.includes(".")) return `https://${text}`;
  // A single-label host (`devbox:3000`, `myserver/path`) is an internal machine, not a query;
  // a plain word without a port or path still searches.
  const single = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(:\d+)?$/i.exec(host);
  return single && (single[2] || text[host.length] === "/") ? `http://${text}` : null;
}

/** Blank input stays put; everything other than a web address is a Google query. */
export function browserAddress(typed: string): string | null {
  const text = typed.trim();
  return text ? webAddress(text) ?? `https://www.google.com/search?q=${encodeURIComponent(text)}` : null;
}

/**
 * What the address bar shows while nobody types in it: the site without `https://` or `www.`,
 * and with Show full URL on its path, query and fragment too (a bare `/` path dropped). The
 * whole URL shows once the field is focused. Other addresses (about:blank) show as they are.
 */
export function shownAddress(url: string, full: boolean): string {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return url;
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") return url;
  const site = parsed.host.replace(/^www\./i, "");
  if (!full) return site;
  const rest = `${parsed.pathname}${parsed.search}${parsed.hash}`;
  return rest === "/" ? site : site + rest;
}

/** Local development hosts: http there is the norm, not a warning. */
const LOCAL_HOST = /^(localhost|.+\.localhost|127\.\d+\.\d+\.\d+|0\.0\.0\.0|\[::1\])$/i;

/**
 * How the site information button reads a page: https is secure; http is not, unless the page
 * is on this computer; anything else (the New tab page, about:blank) has no site to describe.
 */
export function siteSecurity(url: string): "secure" | "insecure" | "local" | null {
  try {
    const { protocol, hostname } = new URL(url);
    if (protocol === "https:") return "secure";
    if (protocol === "http:") return LOCAL_HOST.test(hostname) ? "local" : "insecure";
    return null;
  } catch {
    return null;
  }
}
