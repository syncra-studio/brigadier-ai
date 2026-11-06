// The browser's pages: one fixed layer of <webview> for the whole life of the app's window. It's the
// first child of <body>, a sibling of #root, so nothing React mounts or keys can touch it: switching
// sessions, New Tab becoming a page and closing a panel only change where a page shows.
//
// A page is hidden with visibility:hidden and is never moved or removed until its tab closes, since
// Electron destroys a <webview>'s page when its element leaves the DOM (the same finding as T3 Code's
// and Orca's; no code comes from them). The layer lies under #root: the app cuts
// a hole in itself where a page shows (src/app/conversation/browserLayer.ts), so its own menus and
// floating controls draw over the live page in one compositor.

import { type BrowserBounds, type BrowserEvent, type BrowserFound } from "@/ipc/generated";

declare global {
  interface Window {
    brigadierShell: {
      tabsRead(): Promise<Record<string, TabRecord>>;
      tabsWrite(tabs: Record<string, TabRecord>): Promise<void>;
      profilePartition(profileId: string): Promise<string>;
      guestInfo(id: number): Promise<{ id: number; pid: number; url: string } | null>;
      onGuestPopup(listener: (details: { from: number; url: string }) => void): void;
      devControl(op: string, args?: unknown): Promise<unknown>;
    };
  }
}

/** Electron's <webview> element, as far as the layer uses it. */
type Webview = HTMLElement & {
  src: string;
  getWebContentsId(): number;
  loadURL(url: string): Promise<void>;
  getURL(): string;
  goBack(): void;
  goForward(): void;
  reload(): void;
  reloadIgnoringCache(): void;
  stop(): void;
  isLoading(): boolean;
  setZoomFactor(factor: number): void;
  getZoomFactor(): number;
  findInPage(text: string, options?: { forward?: boolean; findNext?: boolean }): number;
  stopFindInPage(action: "clearSelection" | "keepSelection" | "activateSelection"): void;
  print(): Promise<void>;
};

export type TabRecord = { profileId: string };

type Page = {
  id: string;
  element: Webview;
  profileId: string;
  emit: (event: BrowserEvent) => void;
  ready: boolean;
  /** Set while a find is on its way; `found-in-page` answers it. */
  finding: ((found: BrowserFound) => void) | null;
  shown: boolean;
  /** The element's own methods throw until its first dom-ready, so this follows the events. */
  url: string;
  loading: boolean;
};

export const DEFAULT_PROFILE = "default";

const pages = new Map<string, Page>();
let tabs: Record<string, TabRecord> = {};
let loaded: Promise<void> | null = null;
let layer: HTMLDivElement | null = null;
let profileForNewTabs = DEFAULT_PROFILE;
/** Times the layer lost a child: the oracle for "never moved or removed" holds this at the tabs closed. */
const counters = { created: 0, closed: 0, removedFromLayer: 0, moved: 0 };

export function installTabLayer(): void {
  const style = document.createElement("style");
  style.textContent = `
    #brigadier-tab-layer { position: fixed; inset: 0; z-index: 0; overflow: hidden; contain: strict; }
    #brigadier-tab-layer[data-locked] webview { pointer-events: none; }
    #brigadier-tab-layer webview { position: absolute; display: inline-flex; border: 0; background: #fff; }
    #root { position: relative; z-index: 1; }
    [data-tauri-drag-region] { -webkit-app-region: drag; }
    [data-tauri-drag-region] button, [data-tauri-drag-region] a, [data-tauri-drag-region] input { -webkit-app-region: no-drag; }
  `;
  document.head.append(style);
  layer = document.createElement("div");
  layer.id = "brigadier-tab-layer";
  document.body.prepend(layer);
  new MutationObserver((records) => {
    for (const record of records) counters.removedFromLayer += record.removedNodes.length;
  }).observe(layer, { childList: true });

  // A drag anywhere in the app must not fall into a page it crosses: the divider's or a text
  // selection's pointer stays with the app until it's let go.
  window.addEventListener("pointerdown", (event) => {
    if (event.target instanceof Element && event.target.closest("webview")) return;
    layer?.toggleAttribute("data-locked", true);
  }, { capture: true });
  const release = () => { if (!modal) layer?.toggleAttribute("data-locked", false); };
  window.addEventListener("pointerup", release, { capture: true });
  window.addEventListener("pointercancel", release, { capture: true });
  window.addEventListener("blur", release);

  window.brigadierShell.onGuestPopup(({ from, url }) => {
    for (const page of pages.values()) {
      if (page.ready && page.element.getWebContentsId() === from) {
        page.emit({ type: "popup", id: crypto.randomUUID(), url });
        return;
      }
    }
  });
}

function loadTabs(): Promise<void> {
  loaded ??= window.brigadierShell.tabsRead().then((saved) => { tabs = saved; });
  return loaded;
}

function need(id: string): Page {
  const page = pages.get(id);
  if (!page) throw new Error(`No page for tab ${id}`);
  return page;
}

function place(page: Page, bounds: BrowserBounds | null): void {
  const { style } = page.element;
  if (bounds === null) {
    style.visibility = "hidden";
    style.pointerEvents = "none";
    page.shown = false;
    return;
  }
  style.left = `${bounds.x}px`;
  style.top = `${bounds.y}px`;
  style.width = `${bounds.width}px`;
  style.height = `${bounds.height}px`;
  style.visibility = "visible";
  style.pointerEvents = "";
  page.shown = true;
}

function listen(page: Page): void {
  const { element } = page;
  const emit = (event: BrowserEvent) => page.emit(event);
  const on = <T>(name: string, listener: (event: T) => void) =>
    element.addEventListener(name, listener as unknown as EventListener);
  const load = () => emit({ type: "load", url: page.url, loading: page.loading });
  on("did-start-loading", () => { page.loading = true; load(); });
  on("did-stop-loading", () => { page.loading = false; load(); });
  on<{ url: string }>("did-navigate", ({ url }) => { page.url = url; load(); });
  on<{ url: string; isMainFrame: boolean }>("did-navigate-in-page", ({ url, isMainFrame }) => {
    if (!isMainFrame) return;
    page.url = url;
    load();
  });
  on<{ title: string }>("page-title-updated", ({ title }) => emit({ type: "title", title }));
  on<{ favicons: string[] }>("page-favicon-updated", ({ favicons }) => {
    if (favicons[0]) emit({ type: "favicon", url: page.url, dataUrl: favicons[0] });
  });
  // Electron has no error page: without this a page that can't load is just white, like a blank tab.
  on<{ errorCode: number; validatedURL: string; isMainFrame: boolean }>("did-fail-load", ({ errorCode, validatedURL, isMainFrame }) => {
    // -3 is an aborted load, which a newer navigation or the Stop button causes.
    if (isMainFrame && errorCode !== -3) emit({ type: "blocked", url: validatedURL });
  });
  on("close", () => emit({ type: "closed" }));
  on<{ result: { activeMatchOrdinal: number; matches: number; finalUpdate: boolean } }>("found-in-page", ({ result }) => {
    if (result.finalUpdate && page.finding) {
      page.finding({ found: result.matches > 0, current: result.activeMatchOrdinal, total: result.matches });
      page.finding = null;
    }
  });
}

/** Opens in flight: React's strict effects ask twice, and there must be one page per tab. */
const opening = new Map<string, Promise<void>>();

export function openPage(id: string, url: string, bounds: BrowserBounds, emit: (event: BrowserEvent) => void): Promise<void> {
  const inFlight = opening.get(id);
  if (inFlight) return inFlight;
  const existing = pages.get(id);
  if (existing) {
    existing.emit = emit;
    return existing.element.loadURL(url);
  }
  const open = makePage(id, url, bounds, emit).finally(() => opening.delete(id));
  opening.set(id, open);
  return open;
}

async function makePage(id: string, url: string, bounds: BrowserBounds, emit: (event: BrowserEvent) => void): Promise<void> {
  await loadTabs();
  const profileId = tabs[id]?.profileId ?? profileForNewTabs;
  if (!tabs[id]) {
    tabs[id] = { profileId };
    void window.brigadierShell.tabsWrite(tabs);
  }
  const element = document.createElement("webview") as Webview;
  element.setAttribute("partition", await window.brigadierShell.profilePartition(profileId));
  element.setAttribute("allowpopups", "");
  element.dataset.tab = id;
  const page: Page = { id, element, profileId, emit, ready: false, finding: null, shown: false, url, loading: true };
  pages.set(id, page);
  listen(page);
  place(page, bounds);
  element.style.visibility = "hidden";
  counters.created += 1;
  await new Promise<void>((resolve) => {
    element.addEventListener("dom-ready", () => { page.ready = true; resolve(); }, { once: true });
    element.setAttribute("src", url);
    (layer as HTMLDivElement).append(element);
  });
}

export async function navigatePage(id: string, url: string, bounds: BrowserBounds): Promise<void> {
  const page = need(id);
  place(page, page.shown ? bounds : null);
  await page.element.loadURL(url);
}

export function placePage(id: string, bounds: BrowserBounds | null): void {
  const page = pages.get(id);
  if (page) place(page, bounds);
}

let modal = false;
/** A menu or dialog of the app is open: a click on a page must reach the app, which closes it. */
export function setModal(open: boolean): void {
  modal = open;
  layer?.toggleAttribute("data-locked", open);
}

export function go(id: string, action: string): void {
  const { element } = need(id);
  switch (action) {
    case "back":
    case "back-unless-typing":
      element.goBack();
      break;
    case "forward":
    case "forward-unless-typing":
      element.goForward();
      break;
    case "reload":
      element.reload();
      break;
    case "force-reload":
      element.reloadIgnoringCache();
      break;
    case "stop":
      element.stop();
      break;
    default:
      throw new Error(`Unknown action ${action}`);
  }
}

export function zoom(id: string, factor: number | null): number {
  const { element } = need(id);
  if (factor !== null) element.setZoomFactor(factor);
  return factor ?? element.getZoomFactor();
}

export function find(id: string, query: string, step: "type" | "next" | "previous"): Promise<BrowserFound> {
  const page = need(id);
  if (!query) {
    page.element.stopFindInPage("clearSelection");
    return Promise.resolve({ found: false, current: 0, total: 0 });
  }
  return new Promise((resolve) => {
    page.finding = resolve;
    page.element.findInPage(query, { forward: step !== "previous", findNext: step === "type" });
  });
}

export function printPage(id: string): Promise<void> {
  return need(id).element.print();
}

export function closePage(id: string): void {
  const page = pages.get(id);
  if (!page) return;
  pages.delete(id);
  delete tabs[id];
  void window.brigadierShell.tabsWrite(tabs);
  counters.closed += 1;
  page.element.remove();
}

/** The profile a new tab opens in. The UI for choosing one comes with section 3. */
export function setProfileForNewTabs(profileId: string): void {
  profileForNewTabs = profileId;
}

/** What the checks compare before and after: identity, not looks. */
export function layerState() {
  return {
    counters: { ...counters },
    children: layer?.children.length ?? 0,
    pages: [...pages.values()].map((page) => {
      const style = getComputedStyle(page.element);
      return {
        id: page.id,
        wc: page.ready ? page.element.getWebContentsId() : null,
        profileId: page.profileId,
        partition: page.element.getAttribute("partition"),
        shown: page.shown,
        visibility: style.visibility,
        rect: { x: page.element.offsetLeft, y: page.element.offsetTop, width: page.element.offsetWidth, height: page.element.offsetHeight },
        indexInLayer: [...(layer?.children ?? [])].indexOf(page.element),
        url: page.url,
      };
    }),
  };
}

// The checks read this object; a dev server's import of the module could be a second copy of it.
Object.assign(window, { __brigadierTabLayer: { layerState, setProfileForNewTabs } });
