// A tab's favicon, read inside its own page. The app's page may show only data: images (main/pagePolicy.ts), and
// fetching for a page from the app would bypass that page's CSP and credentials. As in the Tauri build
// (src-tauri/src/browser.rs): a site's CSP or CORS refusal leaves the globe.

/** What a page may hand back: its value comes from the page's own world, so the shape is checked here. */
const fits = (value: unknown): value is string =>
  typeof value === "string" && value.length <= 16 * 1024 && value.startsWith("data:image/png;base64,iVBORw0KGgo");

const raster = (href: string) => `new Promise((done) => {
  const image = new Image();
  image.crossOrigin = "anonymous";
  image.onerror = () => done(null);
  image.onload = () => {
    try {
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 32;
      canvas.getContext("2d").drawImage(image, 0, 0, 32, 32);
      done(canvas.toDataURL("image/png"));
    } catch { done(null); }
  };
  image.src = ${JSON.stringify(href)};
})`;

/** The document's own icon, or the site's default: what a page with no icon link still gets. */
const ICON_HREF = `document.querySelector('link[rel~="icon"]')?.href || new URL("/favicon.ico", location.href).href`;

export async function readFavicon(run: (code: string) => Promise<unknown>): Promise<string | null> {
  const href = await run(ICON_HREF).catch(() => null);
  if (typeof href !== "string" || !/^(https?:|data:image\/)/i.test(href)) return null;
  const value = await run(raster(href)).catch(() => null);
  return fits(value) ? value : null;
}
