// What the production page's origin serves. No Electron here, so it can be tested on its own.
import path from "node:path";

export const PAGE_ORIGIN = "app://brigadier";

/**
 * The Tauri build's policy (src-tauri/tauri.conf.json), with its IPC origins gone: the page reaches the shell
 * through the preload, not over a scheme. A <webview>'s guest is its own page and isn't under this.
 */
export const PAGE_CSP = [
  "default-src 'self'",
  "script-src 'self' 'wasm-unsafe-eval'",
  "style-src 'self' 'unsafe-inline'",
  "img-src 'self' data: blob:",
  "media-src 'self' data: blob:",
  "font-src 'self' data:",
  "frame-src brigadier-ui:",
  "connect-src 'self' data: blob:",
].join("; ");

const TYPES: Record<string, string> = {
  ".html": "text/html; charset=utf-8",
  ".js": "text/javascript; charset=utf-8",
  ".mjs": "text/javascript; charset=utf-8",
  ".css": "text/css; charset=utf-8",
  ".json": "application/json",
  ".svg": "image/svg+xml",
  ".png": "image/png",
  ".jpg": "image/jpeg",
  ".webp": "image/webp",
  ".ico": "image/x-icon",
  ".woff2": "font/woff2",
  ".woff": "font/woff",
  ".ttf": "font/ttf",
  ".wasm": "application/wasm",
  ".mp3": "audio/mpeg",
  ".wav": "audio/wav",
};

export const contentType = (file: string): string => TYPES[path.extname(file).toLowerCase()] ?? "application/octet-stream";

/** The file a request of the page's origin means, inside `root`; null for anything else. */
export function pageFile(root: string, url: string): string | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return null;
  }
  if (parsed.protocol !== "app:" || parsed.host !== "brigadier") return null;
  let decoded: string;
  try {
    decoded = decodeURIComponent(parsed.pathname);
  } catch {
    return null;
  }
  const relative = decoded === "/" ? "index.html" : decoded.slice(1);
  const full = path.resolve(root, relative);
  const inside = path.relative(root, full);
  if (inside === "" || inside.startsWith("..") || path.isAbsolute(inside)) return null;
  return full;
}
