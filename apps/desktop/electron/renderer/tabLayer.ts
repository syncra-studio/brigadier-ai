// The browser's pages: one fixed layer of <webview> for the whole life of the app's window. It's the
// first child of <body>, a sibling of #root, so nothing React mounts or keys can touch it: switching
// sessions, New Tab becoming a page and closing a panel only change where a page shows.
//
// A page is hidden with visibility:hidden and is never moved or removed until its tab closes, since
// Electron destroys a <webview>'s page when its element leaves the DOM (the same finding as T3 Code's
// and Orca's; no code comes from them). The layer lies under #root: the app cuts
// a hole in itself where a page shows (src/app/conversation/browserLayer.ts), so its own menus and
// floating controls draw over the live page in one compositor.

import { ZOOM_LEAST, ZOOM_MOST } from "@/app/conversation/browserKeys";
import { type BrowserBounds, type BrowserEvent, type BrowserFound } from "@/ipc/generated";

import { devToolsChanged, userUsed } from "@/state/agentPause";

import { dropOverlay, installAgentOverlays, placeOverlay } from "./agentOverlay";
import { watchUserComposition } from "./agentFocusGuard";
import { readFavicon } from "./pageFavicon";

declare global {
  interface Window {
    brigadierShell: {
      profiles(op: string, args?: unknown): Promise<unknown>;
      permissionAnswer(request: number, answer: string): Promise<void>;
      flushStorage(): void;
      profilePartition(profileId: string): Promise<string>;
      guestInfo(id: number): Promise<{ id: number; pid: number; url: string } | null>;
      guestFavicon(id: number, href: string): Promise<string | null>;
      onGuestPasskey(listener: (details: { from: number }) => void): void;
      onGuestPopup(listener: (details: { from: number; url: string }) => void): void;
      onGuestPermission(listener: (details: { from: number; request: number; origin: string; camera: boolean; microphone: boolean }) => void): void;
      onGuestEscape(listener: (details: { from: number }) => void): void;
      onGuestKey(listener: () => void): void;
      devControl(op: string, args?: unknown): Promise<unknown>;
      agent(call: Record<string, unknown>): Promise<unknown>;
      onAgentEvent(listener: (details: { from: number; event: "devtools-opened" | "devtools-closed" | "detached" }) => void): void;
    };
  }
}

/** Electron's <webview> element, as far as the layer uses it. */
export type Webview = HTMLElement & {
  src: string;
  getWebContentsId(): number;
  loadURL(url: string): Promise<void>;
  getURL(): string;
  goBack(): void;
  goForward(): void;
  canGoBack(): boolean;
  canGoForward(): boolean;
  reload(): void;
  reloadIgnoringCache(): void;
  stop(): void;
  isLoading(): boolean;
  setZoomFactor(factor: number): void;
  getZoomFactor(): number;
  findInPage(text: string, options?: { forward?: boolean; findNext?: boolean }): number;
  stopFindInPage(action: "clearSelection" | "keepSelection" | "activateSelection"): void;
  print(): Promise<void>;
  executeJavaScript(code: string): Promise<unknown>;
};

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

const pages = new Map<string, Page>();
let layer: HTMLDivElement | null = null;
/** Section 1's interaction check puts its tabs on a profile by this door (tools/browser-check/interactions.mjs). */
let profileForNewTabs: string | null = null;
/** Times the layer lost a child: the oracle for "never moved or removed" holds this at the tabs closed. */
const counters = { created: 0, closed: 0, removedFromLayer: 0, moved: 0 };

/**
 * A page holding the keyboard keeps it through a host element's focus(): Chromium moves it back only when a frame
 * of the host says it took focus (WebContentsImpl::SetFocusedFrame), a frame says so only when Blink's focused frame
 * changes, and the app's page may already be that frame. hostWebContents.focus() does nothing here on macOS, since
 * the host's view is first responder all along (RenderWidgetHostViewMac::Focus), and window.focus() asks for a user
 * gesture that a key taken in the page never gave the host. So a hidden frame takes focus, then the app's page does.
 */
let keyboardFrame: HTMLIFrameElement | null = null;
let keyboardAnchor: HTMLElement | null = null;
/** A page's shortcut just came through it, whatever document.hasFocus() says. */
let pageHadKey = false;
/** The tab whose page gets the keyboard once it shows: its page may not exist or show yet. */
let wantsKeyboard: string | null = null;
const nativeFocus = HTMLElement.prototype.focus;

function keyboardInPage(): boolean {
  return pageHadKey || !document.hasFocus();
}

function takeKeyboard(): void {
  if (!keyboardFrame || !keyboardAnchor) return;
  pageHadKey = false;
  const had = document.activeElement;
  nativeFocus.call(keyboardFrame, { preventScroll: true });
  nativeFocus.call(keyboardAnchor, { preventScroll: true });
  if (had instanceof HTMLElement && had !== document.body && had !== keyboardFrame && had !== keyboardAnchor && had.tagName !== "WEBVIEW") {
    nativeFocus.call(had, { preventScroll: true });
  } else {
    keyboardAnchor.blur();
  }
}

function installKeyboardHandover(): void {
  keyboardAnchor = document.createElement("div");
  keyboardAnchor.tabIndex = -1;
  keyboardAnchor.setAttribute("aria-hidden", "true");
  keyboardAnchor.style.cssText = "position: fixed; left: 0; top: 0; width: 1px; height: 1px; opacity: 0; pointer-events: none; outline: none;";
  keyboardFrame = document.createElement("iframe");
  keyboardFrame.tabIndex = -1;
  keyboardFrame.style.cssText = "display: block; width: 1px; height: 1px; border: 0;";
  keyboardAnchor.append(keyboardFrame);
  document.body.append(keyboardAnchor);
  // Every field of the app, by whatever path it takes focus: autoFocus, a menu key, a shortcut.
  HTMLElement.prototype.focus = function focus(this: HTMLElement, options?: FocusOptions) {
    if (this.tagName !== "WEBVIEW" && keyboardInPage()) takeKeyboard();
    nativeFocus.call(this, options);
  };
  window.brigadierShell.onGuestKey(() => { pageHadKey = true; });
}

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

  installKeyboardHandover();
  installAgentOverlays();
  watchUserComposition();

  // A drag anywhere in the app must not fall into a page it crosses: the divider's or a text
  // selection's pointer stays with the app until it's let go.
  window.addEventListener("pointerdown", (event) => {
    // The shield over a driven page goes at this press, so its release lands in the page, unheard here.
    if (event.target instanceof Element && event.target.closest("webview, .brigadier-agent-frame")) return;
    pageHadKey = false;
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
  window.brigadierShell.onGuestPermission(({ from, ...question }) => {
    for (const page of pages.values()) {
      if (page.ready && page.element.getWebContentsId() === from) page.emit({ type: "permission", ...question });
    }
  });
  window.brigadierShell.onGuestPasskey(({ from }) => {
    for (const page of pages.values()) {
      if (page.ready && page.element.getWebContentsId() === from) page.emit({ type: "passkeyRefused" });
    }
  });
  window.brigadierShell.onGuestEscape(({ from }) => {
    for (const page of pages.values()) {
      if (page.ready && page.element.getWebContentsId() === from) page.emit({ type: "escape" });
    }
  });
  window.brigadierShell.onAgentEvent(({ from, event }) => {
    for (const page of pages.values()) {
      if (!page.ready || page.element.getWebContentsId() !== from) continue;
      // Nothing resumes by itself: DevTools closing, or a debugger ended from outside, leaves the tab with the user.
      if (event === "detached") userUsed(page.id);
      else devToolsChanged(page.id, event === "devtools-opened");
    }
  });
}

function need(id: string): Page {
  const page = pages.get(id);
  if (!page) throw new Error(`No page for tab ${id}`);
  return page;
}

function place(page: Page, bounds: BrowserBounds | null): void {
  const { style } = page.element;
  placeOverlay(page.id, bounds);
  if (bounds === null) {
    style.visibility = "hidden";
    style.pointerEvents = "none";
    if (wantsKeyboard === page.id) wantsKeyboard = null;
    // A hidden page must not hold the keyboard: typing would go to it unseen.
    if (page.shown && keyboardInPage()) takeKeyboard();
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
  if (wantsKeyboard === page.id && page.element.isConnected) {
    wantsKeyboard = null;
    // Unless the person went on to a field of the app while the page was coming.
    if (document.activeElement === null || document.activeElement === document.body) page.element.focus();
  }
}

const NOT_FOUND: BrowserFound = { found: false, current: 0, total: 0 };

function answerFind(page: Page, found: BrowserFound): void {
  page.finding?.(found);
  page.finding = null;
}

function listen(page: Page): void {
  const { element } = page;
  const emit = (event: BrowserEvent) => page.emit(event);
  const on = <T>(name: string, listener: (event: T) => void) =>
    element.addEventListener(name, listener as unknown as EventListener);
  const history = () => emit({
    type: "history",
    canGoBack: page.ready && element.canGoBack(),
    canGoForward: page.ready && element.canGoForward(),
  });
  const load = () => { emit({ type: "load", url: page.url, loading: page.loading }); history(); };
  // A load drops a find in flight, with no answer from the old document: the bar looks again when it stops.
  on("did-start-loading", () => { answerFind(page, NOT_FOUND); page.loading = true; load(); });
  on("did-stop-loading", () => { page.loading = false; load(); });
  on<{ url: string }>("did-navigate", ({ url }) => { page.url = url; load(); });
  on<{ url: string; isMainFrame: boolean }>("did-navigate-in-page", ({ url, isMainFrame }) => {
    // A frame's own navigation adds to the tab's history too.
    if (!isMainFrame) { history(); return; }
    page.url = url;
    load();
  });
  on<{ title: string }>("page-title-updated", ({ title }) => emit({ type: "title", title }));
  const favicon = () => {
    const url = page.url;
    void readFavicon((code) => element.executeJavaScript(code), (href) => window.brigadierShell.guestFavicon(element.getWebContentsId(), href)).then((dataUrl) => {
      if (dataUrl) emit({ type: "favicon", url, dataUrl });
    });
  };
  // Each document, whatever Chromium says: it sent nothing for the second page of a site with the same icon.
  on("dom-ready", favicon);
  // And a page that changes its icon later.
  on("page-favicon-updated", favicon);
  // A web page that can't load gets its own page (main/guestFailedLoad.ts); any other scheme can't open here.
  on<{ errorCode: number; validatedURL: string; isMainFrame: boolean }>("did-fail-load", ({ errorCode, validatedURL, isMainFrame }) => {
    // -3 is an aborted load, which a newer navigation or the Stop button causes.
    if (isMainFrame && errorCode !== -3 && !/^https?:/.test(validatedURL)) emit({ type: "blocked", url: validatedURL });
  });
  on("close", () => emit({ type: "closed" }));
  on<{ result: { activeMatchOrdinal: number; matches: number; finalUpdate: boolean } }>("found-in-page", ({ result }) => {
    if (result.finalUpdate) answerFind(page, { found: result.matches > 0, current: result.activeMatchOrdinal, total: result.matches });
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
    load(existing, url);
    return Promise.resolve();
  }
  const open = makePage(id, url, bounds, emit).finally(() => opening.delete(id));
  opening.set(id, open);
  return open;
}

async function makePage(id: string, url: string, bounds: BrowserBounds, emit: (event: BrowserEvent) => void): Promise<void> {
  // By import, not at the top: this module runs before the app's state may.
  const { pinTabToItsProfile, profilesLoaded } = await import("@/state/browserProfilePlaces");
  await profilesLoaded();
  const profileId = profileForNewTabs ?? pinTabToItsProfile(id);
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
    // Main destroys a guest with any other first address, and dom-ready would never come (a tab restored at a blob:).
    element.setAttribute("src", /^https?:/.test(url) ? url : "about:blank");
    (layer as HTMLDivElement).append(element);
  });
}

/**
 * Starts a load and leaves its outcome to the page's events: did-fail-load tells a real failure from an abort.
 * loadURL's promise also rejects when Stop or a newer address cuts the load short, which is no error.
 */
function load(page: Page, url: string): void {
  page.element.loadURL(url).catch(() => {});
}

/** Loads `url` in the tab's page where it is, showing nothing new. */
export function loadPage(id: string, url: string): void {
  load(need(id), url);
}

/** The tab's guest, for an agent's call; the element's methods work only from its first dom-ready. */
export function agentPage(id: string): { guest: number; element: Webview } {
  const page = need(id);
  if (!page.ready) throw new Error(`The page of tab ${id} isn't ready yet.`);
  return { guest: page.element.getWebContentsId(), element: page.element };
}

/** Where the tab's page shows now; null behind its tab. */
export function pageBounds(id: string): BrowserBounds | null {
  const { style } = need(id).element;
  if (style.visibility !== "visible") return null;
  return { x: parseFloat(style.left), y: parseFloat(style.top), width: parseFloat(style.width), height: parseFloat(style.height) };
}

export function navigatePage(id: string, url: string, bounds: BrowserBounds): void {
  const page = need(id);
  place(page, page.shown ? bounds : null);
  load(page, url);
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
  if (factor !== null) element.setZoomFactor(Math.min(ZOOM_MOST, Math.max(ZOOM_LEAST, factor)));
  return element.getZoomFactor();
}

/** The keyboard to the tab's page, or back to the app's own (null). */
export function focusPage(id: string | null): void {
  wantsKeyboard = null;
  if (id === null) {
    if (keyboardInPage()) takeKeyboard();
    return;
  }
  const page = pages.get(id);
  // A hidden element can't take focus, and a first address has no page yet: place() gives it when it shows.
  if (page?.element.isConnected && page.element.style.visibility === "visible") page.element.focus();
  else wantsKeyboard = id;
}

export function find(id: string, query: string, step: "type" | "next" | "previous"): Promise<BrowserFound> {
  const page = need(id);
  if (!query) {
    answerFind(page, NOT_FOUND);
    page.element.stopFindInPage("clearSelection");
    return Promise.resolve(NOT_FOUND);
  }
  return new Promise((resolve) => {
    // Chromium answers only the latest of two quick searches; the earlier one mustn't wait for ever.
    answerFind(page, NOT_FOUND);
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
  answerFind(page, NOT_FOUND);
  dropOverlay(id);
  pages.delete(id);
  counters.closed += 1;
  page.element.remove();
}

/** Puts the next tabs on `profileId` whatever their session is on; "default" lets go again. Checks only. */
export function setProfileForNewTabs(profileId: string): void {
  profileForNewTabs = profileId === "default" ? null : profileId;
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
