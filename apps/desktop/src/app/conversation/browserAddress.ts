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
 * What the address bar shows while nobody types in it: the whole URL with Show full URL on,
 * else only the site (the path, query and fragment show once the field is focused).
 */
export function shownAddress(url: string, full: boolean): string {
  if (full || !url) return url;
  try {
    const { protocol, host } = new URL(url);
    return protocol === "http:" || protocol === "https:" ? host : url;
  } catch {
    return url;
  }
}
