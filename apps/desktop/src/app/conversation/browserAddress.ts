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
  return host.includes(".") ? `https://${text}` : null;
}

/** Blank input stays put; everything other than a web address is a Google query. */
export function browserAddress(typed: string): string | null {
  const text = typed.trim();
  return text ? webAddress(text) ?? `https://www.google.com/search?q=${encodeURIComponent(text)}` : null;
}
