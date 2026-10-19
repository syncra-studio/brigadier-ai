import {
  browserAgent,
  browserAgentCapture,
  browserAgentLoad,
  browserGo,
} from "@/ipc/client";
import type { AppCall, BrowserBounds, BrowserGuard, BrowserOp, BrowserPageRead, BrowserResult, BrowserSensitive } from "@/ipc/generated";
import { isPaused } from "@/state/agentPause";
import { openPage, useBrowsers } from "@/state/browsers";
import {
  type BrowserDriver,
  type BrowserTabState,
  changeSessionTab,
  closeTab,
  isAgentTab,
  openAgentTab,
  sameAddress,
  selectTab,
  sessionTabs,
} from "@/state/sessionTabs";
import { useApp } from "@/state/store";

/**
 * The browser tool's calls (Q74), as the app runs them: an agent's call comes from the daemon,
 * runs on a Browser tab of its session and is answered. `open` uses the agents' tab already on
 * that page, else makes a normal Browser tab, behind the one in front, with the user's sign-ins;
 * acting works only on such a tab. The user's own tabs may be read and shown, never worked.
 */

/** A Browser tab's call, as its app call carries it. */
export type BrowserCall = Omit<AppCall, "op"> & { op: BrowserOp };

/** What the app does on a tab's page; the shell's own commands in the app, a fake in tests. */
export type PageHands = {
  /** Makes the tab's page, showing `url`, without showing it over the tab. */
  open: (tab: string, url: string, bounds: BrowserBounds) => Promise<void>;
  load: (tab: string, url: string) => Promise<void>;
  go: (tab: string, action: "back" | "forward" | "reload") => Promise<void>;
  script: (tab: string, op: Record<string, unknown>) => Promise<unknown>;
  capture: (tab: string) => Promise<ArrayBuffer>;
};

const shell: PageHands = {
  open: (tab, url, bounds) => openPage(tab, url, bounds, true),
  load: browserAgentLoad,
  go: browserGo,
  script: browserAgent,
  capture: browserAgentCapture,
};

/** How long a page load may take before the agent hears the page as it is then. */
const LOAD_WAIT_MS = 20_000;
/** How long after a click or typing a page gets to start loading, if it will. */
const SETTLE_MS = 400;
/** The most of a page's text one read gives. */
const READ_MOST = 20_000;

const failed = (message: string): BrowserResult => ({ result: "failed", message });

function messageOf(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "object" && error && "message" in error) return String(error.message);
  return String(error);
}

const wait = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/** What `call` comes to on its tab. */
export async function runCall(call: BrowserCall, hands: PageHands = shell): Promise<BrowserResult> {
  if (!useApp.getState().settings.browserEnabled) return failed("The built-in browser is off.");
  try {
    return await perform(call, hands);
  } catch (error) {
    return failed(messageOf(error));
  }
}

/** The session's Browser tab `id`, if it is still open. */
function browserTab(conversationId: string, id: string): BrowserTabState | null {
  const tab = sessionTabs(conversationId).tabs.find((each) => each.id === id);
  return tab?.kind === "browser" ? tab : null;
}

/**
 * A tab an agent's `open` of `url` goes on in, not another one of that page (#418, #424): one an
 * agent works on it, or one opened from what an agent gave (a preview's address), unless the
 * user took it over.
 */
function adoptable(conversationId: string, url: string): BrowserTabState | undefined {
  return sessionTabs(conversationId).tabs.find(
    (tab): tab is BrowserTabState => tab.kind === "browser" && (Boolean(tab.driver) || tab.agent === true) && sameAddress(tab.url, url) && !isPaused(tab.id),
  );
}

/** Makes the tab the agent's, for request `request`: one the user clicked open stays for them. */
function adopt(conversationId: string, tab: BrowserTabState, driver: BrowserDriver): void {
  const keep = driver.keep === true || (tab.driver ? tab.driver.keep === true : true);
  changeSessionTab(conversationId, tab.id, (current) => current.kind === "browser" ? { ...current, driver: { ...driver, keep } } : current);
}

/**
 * Request `request` of the session ended: the Browser tabs agents opened for it close, except
 * those kept for the user, the one in front and those the user took over. Returns those closed.
 */
export function closeRequestTabs(conversationId: string, request: string): string[] {
  const current = sessionTabs(conversationId);
  const gone = current.tabs
    .filter((tab): tab is BrowserTabState => tab.kind === "browser" && tab.driver?.request === request && !tab.driver.keep)
    .filter((tab) => tab.id !== current.active && !isPaused(tab.id))
    .map((tab) => tab.id);
  for (const tab of gone) closeTab(conversationId, tab);
  return gone;
}

/** Where a background tab's page lies: over the session's main area, which it would show in. */
export function backgroundBounds(): BrowserBounds {
  const area = typeof document === "undefined" ? null : document.querySelector('[data-slot="pane-workspace"]')?.getBoundingClientRect();
  if (area && area.width > 200 && area.height > 200) return { x: area.left, y: area.top, width: area.width, height: area.height };
  const width = typeof window === "undefined" ? 1280 : window.innerWidth;
  const height = typeof window === "undefined" ? 800 : window.innerHeight;
  return { x: 0, y: 0, width, height };
}

async function perform(call: BrowserCall, hands: PageHands): Promise<BrowserResult> {
  const { op, conversationId } = call;
  if (op.op === "open") {
    const driver: BrowserDriver = { label: call.label, taskId: call.taskId, request: op.request, keep: op.keep };
    const there = adoptable(conversationId, op.url);
    if (there) adopt(conversationId, there, driver);
    const tab = there?.id ?? openAgentTab(conversationId, op.url, driver);
    if (!there || !useBrowsers.getState().pages[tab]) {
      await hands.open(tab, op.url, backgroundBounds());
      await loaded(tab);
    }
    return page(tab, hands);
  }
  const tab = browserTab(conversationId, op.tab);
  if (!tab) return failed(`There is no tab ${op.tab} in this session now: open one with browser_open.`);
  // Showing never moves the window or the session: only which tab the session has in front.
  if (op.op === "show") {
    selectTab(conversationId, tab.id);
    return { result: "shown", tab: tab.id };
  }
  const users = !isAgentTab(tab);
  if (users && op.op !== "read" && op.op !== "screenshot")
    return failed(`Tab ${tab.id} is the user's: you may read it, not click, type, navigate or close it. Open your own with browser_open.`);
  // The user took it over: nothing is done until they press Continue.
  if (!users && isPaused(tab.id)) return { result: "paused", tab: tab.id };
  // Its page goes with a restart: it loads again where it was.
  if (!useBrowsers.getState().pages[tab.id]) {
    await hands.open(tab.id, tab.url || "about:blank", backgroundBounds());
    await loaded(tab.id);
  }
  const result = await act(tab.id, op, conversationId, hands);
  return users && result.result === "page" ? { ...result, user: true } : result;
}

async function act(tab: string, op: Exclude<BrowserOp, { op: "open" | "show" }>, conversationId: string, hands: PageHands): Promise<BrowserResult> {
  switch (op.op) {
    case "navigate":
      starting(tab);
      await hands.load(tab, op.url);
      await loaded(tab);
      return page(tab, hands);
    case "go":
      await hands.go(tab, op.action);
      await settled(tab);
      return page(tab, hands);
    case "read":
      return page(tab, hands);
    case "click":
    case "type": {
      const before = new Set(sessionTabs(conversationId).tabs.map((each) => each.id));
      const answer = await hands.script(tab, {
        kind: op.op,
        target: op.target,
        ...(op.op === "type" ? { text: op.text, submit: op.submit } : {}),
        declared: op.guard.declared,
        approved: approvedHere(tab, op.guard),
      });
      const asked = askOf(answer);
      if (asked) {
        const shown = useBrowsers.getState().pages[tab];
        return {
          result: "needsApproval",
          tab,
          url: urlOf(answer) ?? shown?.url ?? "",
          generation: shown?.generation ?? 0,
          sensitive: asked.sensitive,
          what: asked.what,
        };
      }
      const error = errorOf(answer);
      if (error) return failed(error);
      await settled(tab);
      return withPopups(await page(tab, hands), conversationId, tab, before);
    }
    case "screenshot": {
      const png = await hands.capture(tab);
      return { result: "shot", tab, url: useBrowsers.getState().pages[tab]?.url ?? "", png: base64(new Uint8Array(png)) };
    }
    case "close":
      closeTab(conversationId, tab);
      return { result: "closed", tab };
  }
}

/**
 * The page after an action, naming the tabs it opened (a popup, such as a sign-in window): the
 * agent goes on in them by their ids.
 */
function withPopups(result: BrowserResult, conversationId: string, tab: string, before: Set<string>): BrowserResult {
  if (result.result !== "page") return result;
  const opened = sessionTabs(conversationId).tabs.filter(
    (each): each is BrowserTabState => each.kind === "browser" && each.opener === tab && !before.has(each.id),
  );
  if (!opened.length) return result;
  const lines = opened.map((each) => `(This opened a new tab: ${each.id} at ${each.url}. Use its id with the browser tools.)`);
  return { ...result, page: { ...result.page, text: `${result.page.text}\n\n${lines.join("\n")}` } };
}

/** The address of the user's yes, while the tab is still on the page it was given on. */
function approvedHere(tab: string, guard: BrowserGuard): string | null {
  const generation = useBrowsers.getState().pages[tab]?.generation ?? 0;
  return guard.approved && guard.approved.generation === generation ? guard.approved.url : null;
}

const SENSITIVE: readonly BrowserSensitive[] = ["pay", "send", "signIn", "credentials", "card", "oneTimeCode"];

function askOf(answer: unknown): { sensitive: BrowserSensitive; what: string } | null {
  if (typeof answer !== "object" || !answer || !("ask" in answer)) return null;
  const ask = answer.ask as { sensitive?: unknown; what?: unknown };
  const sensitive = SENSITIVE.find((each) => each === ask.sensitive);
  return sensitive ? { sensitive, what: String(ask.what ?? "") } : null;
}

function errorOf(answer: unknown): string | null {
  if (typeof answer !== "object" || !answer) return "The page didn't answer.";
  return "error" in answer ? String(answer.error) : null;
}

function urlOf(answer: unknown): string | null {
  return typeof answer === "object" && answer && "url" in answer ? String(answer.url) : null;
}

/** A load starts now (the shell says so too, a moment later). */
function starting(tab: string): void {
  useBrowsers.setState(({ pages }) => {
    const shown = pages[tab];
    return shown ? { pages: { ...pages, [tab]: { ...shown, loading: true } } } : { pages };
  });
}

/** Waits for the tab's page to finish loading, `LOAD_WAIT_MS` at most. */
async function loaded(tab: string): Promise<void> {
  const deadline = Date.now() + LOAD_WAIT_MS;
  while (Date.now() < deadline) {
    const shown = useBrowsers.getState().pages[tab];
    if (!shown) throw new Error("The tab closed.");
    if (shown.ready && !shown.loading) return;
    await wait(100);
  }
}

/** After an action: a moment for a load to start, then the load, if one did. */
async function settled(tab: string): Promise<void> {
  await wait(SETTLE_MS);
  await loaded(tab);
}

/** The tab's page, read for the agent. */
async function page(tab: string, hands: PageHands): Promise<BrowserResult> {
  const answer = await hands.script(tab, { kind: "read", max: READ_MOST });
  const error = errorOf(answer);
  if (error) return failed(error);
  const read = answer as { url: string; title: string; text: string };
  const shown = useBrowsers.getState().pages[tab];
  const result: BrowserPageRead = {
    url: read.url,
    title: read.title,
    generation: shown?.generation ?? 0,
    loading: shown?.loading ?? false,
    text: read.text,
  };
  return { result: "page", tab, page: result, user: false };
}

/** `bytes` as base64. */
export function base64(bytes: Uint8Array): string {
  let text = "";
  for (let at = 0; at < bytes.length; at += 0x8000) {
    text += String.fromCharCode(...bytes.subarray(at, at + 0x8000));
  }
  return btoa(text);
}
