/** OSC 7 can describe an SSH host; only remember unambiguously local folders. */
export function localTerminalFolder(value: string): string | null {
  try {
    const url = new URL(value);
    if (url.protocol !== "file:" || (url.hostname && url.hostname !== "localhost")) return null;
    const path = decodeURIComponent(url.pathname);
    return /^\/[A-Za-z]:\//.test(path) ? path.slice(1) : path;
  } catch { return null; }
}
