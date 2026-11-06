// A page that couldn't load. Chromium has already put an empty error document at the failed address, so the
// address bar, Back and Reload stay right; this fills that document in. Nothing retries by itself (L5): the
// load runs again only when the user presses Try again.
import type { WebContents } from "electron";

import { note } from "./events";

/** The headline and the sentence for Chromium's net error codes that a person can act on. */
function words(code: number, host: string): { title: string; body: string } {
  const site = host || "This site";
  if (code === -105 || code === -137) return { title: "Can't find this site", body: `No server was found for ${site}. Check the address for a typo.` };
  if (code === -102) return { title: "This site refused to connect", body: `${site} isn't accepting connections right now. It may be down, or on another port.` };
  if (code === -106) return { title: "You're offline", body: "This Mac isn't connected to the internet." };
  if (code === -7 || code === -118) return { title: "This site took too long to answer", body: `${site} didn't respond in time.` };
  if (code === -100 || code === -101 || code === -21) return { title: "The connection was lost", body: `The connection to ${site} ended before the page arrived.` };
  if (code === -109 || code === -104) return { title: "Can't reach this site", body: `The way to ${site} couldn't be found from this network.` };
  if (code <= -200 && code > -300) return { title: "This connection isn't secure", body: `${site} couldn't prove who it is, so the page wasn't opened.` };
  return { title: "This page couldn't open", body: `Something went wrong while loading ${site}.` };
}

// The app's dark surfaces and control (src/styles/tokens.css): --background, --foreground, --muted-foreground,
// --primary, --radius-control. A guest can't read the app's stylesheet.
const STYLE = `
  html, body { height: 100%; margin: 0; }
  body { display: flex; align-items: center; justify-content: center; background: #181818; color: #fafafa; color-scheme: dark;
    font: 13px/1.5 -apple-system, BlinkMacSystemFont, "Segoe UI Variable Text", "Segoe UI", system-ui, sans-serif; }
  main { width: min(26rem, calc(100% - 3rem)); }
  h1 { margin: 0 0 .5rem; font-size: 18px; font-weight: 600; letter-spacing: -.01em; }
  p { margin: 0; color: #a1a1a1; overflow-wrap: anywhere; }
  code { display: block; margin-top: .75rem; color: #a1a1a1; font: 11px/1.5 ui-monospace, "SF Mono", Menlo, Consolas, monospace; overflow-wrap: anywhere; }
  button { margin-top: 1.25rem; height: 2rem; padding: 0 .75rem; border: 0; border-radius: .5rem; background: #e5e5e5; color: #262626; font: inherit; font-weight: 500; cursor: default; }
  button:hover { background: #d4d4d4; }
  button:focus-visible { outline: 2px solid #a1a1a1; outline-offset: 2px; }
`;

/** Runs in the error document. Everything from outside goes in as text, never as markup. */
function fill(url: string, code: number, description: string): string {
  let host = "";
  try {
    host = new URL(url).host;
  } catch { /* the words manage without */ }
  const { title, body } = words(code, host);
  return `(() => {
    const text = ${JSON.stringify({ url, title, body, host, detail: description || `Error ${code}`, style: STYLE })};
    document.title = text.host || text.title;
    const style = document.createElement("style");
    style.textContent = text.style;
    document.head.append(style);
    const main = document.createElement("main");
    main.dataset.failedLoad = text.detail;
    const part = (tag, words) => { const made = document.createElement(tag); made.textContent = words; main.append(made); return made; };
    part("h1", text.title);
    part("p", text.body);
    part("code", text.url + " · " + text.detail);
    const again = part("button", "Try again");
    again.id = "try-again";
    again.addEventListener("click", () => location.reload());
    document.body.replaceChildren(main);
  })()`;
}

export function installFailedLoad(contents: WebContents): void {
  contents.on("did-fail-load", (_event, code, description, url, isMainFrame) => {
    // -3 is an aborted load: a newer navigation or Stop. A scheme that isn't the web's has the tab's own banner.
    if (!isMainFrame || code === -3 || !/^https?:/.test(url)) return;
    note("guest-load-failed", { webContents: contents.id, code, description, url });
    contents.executeJavaScript(fill(url, code, description)).catch((error: unknown) => {
      note("guest-failed-page-not-shown", { webContents: contents.id, code, url, error: String(error) });
    });
  });
}
