// A tab's icon, fetched outside its page as a browser does, in the guest's own session with no credentials: most
// sites' icons carry no CORS header, so a page can't read its own (Google's, for one). What comes back goes only to
// the app's page, which decodes it (renderer/pageFavicon.ts); the site's page never sees it.
import { ipcMain, webContents, type Session } from "electron";

const MAX_BYTES = 512 * 1024;
const TIMEOUT_MS = 10_000;

const web = (url: string): boolean => {
  try {
    return /^https?:$/.test(new URL(url).protocol);
  } catch {
    return false;
  }
};

async function bytesOf(response: Response): Promise<Buffer | null> {
  const reader = response.body?.getReader();
  if (!reader) return null;
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) return Buffer.concat(chunks);
    total += value.length;
    if (total > MAX_BYTES) {
      void reader.cancel();
      return null;
    }
    chunks.push(value);
  }
}

/** The icon as a data: URL. The scheme is checked here: session.fetch would take a file: URL. */
export async function fetchIcon(session: Session, href: string): Promise<string | null> {
  if (!web(href)) return null;
  // Not "manual": Electron's fetch throws on the first redirect. The network stack refuses one to file:.
  const response = await session.fetch(href, { credentials: "omit", signal: AbortSignal.timeout(TIMEOUT_MS) });
  if (!response.ok) return null;
  const bytes = await bytesOf(response);
  if (!bytes?.length) return null;
  // Chromium sniffs raster formats whatever this says, and a non-image simply fails to decode.
  const said = response.headers.get("content-type")?.split(";")[0]?.trim() ?? "";
  return `data:${/^image\/[\w.+-]+$/.test(said) ? said : "image/x-icon"};base64,${bytes.toString("base64")}`;
}

export function installGuestFavicon(): void {
  ipcMain.handle("brigadier:guest-favicon", async (_event, id: number, href: string) => {
    const guest = webContents.fromId(id);
    if (!guest || guest.getType() !== "webview") return null;
    return fetchIcon(guest.session, href).catch(() => null);
  });
}
