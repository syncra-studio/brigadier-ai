// What happens in a tab, for `brigadier browser events`: every guest's pages, network and console from Electron's own
// events, so a tab needs no debugger for it. Metadata only, kept in memory, capped per tab: no request or response
// bodies and no headers are ever read, so no cookie or Authorization value can get here.
import type { Session, WebContents } from "electron";

import { sweepNow, sweepSoon } from "./browserAgentSecrets";

type Kind = "navigation" | "network" | "console" | "error";
type Line = { at: number; kind: Kind; text: string };
type Pending = { guest: number; method: string; url: string; started: number };
type Tab = { lines: Line[]; next: number };

/** What one tab keeps, and the most one answer gives. */
const KEPT = 500;
const ANSWER_MOST = 200;
const TEXT_MOST = 1000;
const URL_MOST = 300;

const tabs = new Map<number, Tab>();
/** Requests on their way, by Chromium's id. A page's own load ends with no webContentsId, so the guest is kept here. */
const pending = new Map<number, Pending>();

function tabOf(guest: number): Tab {
  let tab = tabs.get(guest);
  if (!tab) {
    tab = { lines: [], next: 1 };
    tabs.set(guest, tab);
  }
  return tab;
}

function add(guest: number, kind: Kind, text: string): void {
  const tab = tabOf(guest);
  tab.lines.push({ at: tab.next++, kind, text: `${new Date().toTimeString().slice(0, 8)} ${text.slice(0, TEXT_MOST)}` });
  if (tab.lines.length > KEPT) tab.lines.shift();
}

const SECRET_NAME = /pass|pwd|token|secret|auth|session|sid|code|key|otp/i;

/** An address as a line shows it: what a query's sensitive names hold stays out. */
export function shownUrl(url: string): string {
  let shown = url;
  try {
    const parsed = new URL(url);
    if (parsed.username || parsed.password) {
      parsed.username = "";
      parsed.password = "";
    }
    for (const name of [...parsed.searchParams.keys()]) {
      if (SECRET_NAME.test(name)) parsed.searchParams.set(name, "(hidden)");
    }
    shown = parsed.toString();
  } catch {
    // Not an address to take apart: shown as it came, cut.
  }
  return shown.length > URL_MOST ? `${shown.slice(0, URL_MOST)}…` : shown;
}

/** The session's requests, by the guest each came from. Observers only: they can't hold up or change a request. */
export function watchNetwork(session: Session): void {
  const { webRequest } = session;
  webRequest.onSendHeaders(({ id, webContentsId, method, url }) => {
    if (webContentsId) pending.set(id, { guest: webContentsId, method, url, started: Date.now() });
  });
  webRequest.onCompleted(({ id, webContentsId, method, url, statusCode, resourceType, fromCache }) => {
    const sent = pending.get(id);
    pending.delete(id);
    const guest = webContentsId ?? sent?.guest;
    if (!guest) return;
    const took = sent ? `, ${Date.now() - sent.started} ms` : "";
    add(guest, "network", `${statusCode} ${method} ${shownUrl(url)} (${resourceType}${fromCache ? ", cached" : ""}${took})`);
  });
  webRequest.onErrorOccurred(({ id, webContentsId, method, url, error }) => {
    const sent = pending.get(id);
    pending.delete(id);
    const guest = webContentsId ?? sent?.guest;
    if (guest) add(guest, "network", `failed ${method} ${shownUrl(url)}: ${error}`);
  });
}

/** A tab's pages and console. */
export function watchActivity(guest: WebContents): void {
  const id = guest.id;
  // A password field's value is looked for when the page may just have logged it, or is about to leave it.
  guest.on("did-start-navigation", (details) => {
    if (details.isMainFrame && !details.isSameDocument) sweepNow(guest);
  });
  guest.on("did-navigate", (_event, url, status) => add(id, "navigation", `page ${shownUrl(url)} (${status})`));
  guest.on("did-navigate-in-page", (_event, url, isMainFrame) => {
    if (isMainFrame) add(id, "navigation", `address ${shownUrl(url)}`);
  });
  guest.on("did-finish-load", () => add(id, "navigation", `loaded ${shownUrl(guest.getURL())}`));
  guest.on("did-fail-load", (_event, code, description, url, isMainFrame) => {
    if (code !== -3) add(id, "error", `load failed${isMainFrame ? "" : " in a frame"}: ${description} (${code}) ${shownUrl(url)}`);
  });
  // A popup that keeps its opener (a sign-in, say) is a real window of the user's, not a tab: an agent has no verbs for
  // it and sees nothing inside it, only that it opened and closed. The page's own result comes back in this tab.
  guest.on("did-create-window", (window, { url }) => {
    add(id, "navigation", `opened a separate window at ${shownUrl(url)}. Only the user can see and use it, not you: ask them if it needs them.`);
    window.once("closed", () => { if (!guest.isDestroyed()) add(id, "navigation", "that separate window closed"); });
  });
  guest.on("render-process-gone", (_event, { reason }) => add(id, "error", `the page's process ended: ${reason}`));
  guest.on("console-message", ({ level, message, sourceId, lineNumber }) => {
    sweepSoon(guest);
    add(id, "console", `console ${level}: ${message} (${shownUrl(sourceId)}:${lineNumber})`);
  });
}

export function forgetActivity(guest: number): void {
  tabs.delete(guest);
  for (const [id, sent] of pending) if (sent.guest === guest) pending.delete(id);
}

/** What happened after line `since`, oldest first; `dropped` counts what no longer fits, and says so to the agent. */
export function activityOf(guest: number, since: number, kinds: readonly string[]): { lines: string[]; next: number; dropped: number } {
  const tab = tabOf(guest);
  const wanted = (kind: string) => kinds.length === 0 || kinds.includes(kind);
  const found = tab.lines.filter((line) => line.at > since && wanted(line.kind));
  const firstKept = tab.lines[0]?.at ?? tab.next;
  const gone = Math.max(0, firstKept - since - 1);
  const shown = found.slice(-ANSWER_MOST);
  const lines = shown.map((line) => line.text);
  if (wanted("network")) {
    for (const sent of pending.values()) if (sent.guest === guest) lines.push(`${sent.method} ${shownUrl(sent.url)} (no answer yet)`);
  }
  return { lines, next: tab.next - 1, dropped: gone + found.length - shown.length };
}
