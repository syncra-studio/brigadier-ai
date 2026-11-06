// A tab's favicon. The page says which icon it has; the icon comes from outside the page, as in a browser
// (main/guestFavicon.ts: no credentials, the guest's own session), since most sites' icons carry no CORS header
// and a page can't read them itself. It's decoded here, in the sandboxed app page, which also reads .ico: main's
// nativeImage doesn't. The app's page may show only data: images (main/pagePolicy.ts), hence a small PNG.

/** What may be shown: the canvas's own output, within what a 32px icon takes. */
const fits = (value: unknown): value is string =>
  typeof value === "string" && value.length <= 16 * 1024 && value.startsWith("data:image/png;base64,iVBORw0KGgo");

const toPng = (source: string) => new Promise<string | null>((done) => {
  const image = new Image();
  image.onerror = () => done(null);
  image.onload = () => {
    try {
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 32;
      canvas.getContext("2d")?.drawImage(image, 0, 0, 32, 32);
      done(canvas.toDataURL("image/png"));
    } catch { done(null); }
  };
  image.src = source;
});

/** The document's own icon, or the site's default: what a page with no icon link still gets. */
const ICON_HREF = `document.querySelector('link[rel~="icon"]')?.href || new URL("/favicon.ico", location.href).href`;

export async function readFavicon(run: (code: string) => Promise<unknown>, fetchIcon: (href: string) => Promise<string | null>): Promise<string | null> {
  const href = await run(ICON_HREF).catch(() => null);
  if (typeof href !== "string") return null;
  const source = /^https?:/i.test(href) ? await fetchIcon(href).catch(() => null) : /^data:image\//i.test(href) ? href : null;
  if (!source) return null;
  const value = await toPng(source);
  return fits(value) ? value : null;
}
