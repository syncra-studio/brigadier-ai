import {
  type BrowserActivity,
  browserAgent,
  browserAgentCapture,
  browserAgentDrive,
  browserAgentEvents,
  browserAgentLoad,
  browserGo,
} from "@/ipc/client";
import type { AppCall, BrowserBounds, BrowserGuard, BrowserOp, BrowserPageRead, BrowserResult, BrowserSensitive } from "@/ipc/generated";
import { devToolsOpen, driverOf, isPaused, useAgentPause } from "@/state/agentPause";
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
  /** An agent begins acting in the tab, or stops: its hold of the page starts and ends with this. */
  drive: (tab: string, on: boolean) => Promise<void>;
  events: (tab: string, since: number, kinds: readonly string[]) => Promise<BrowserActivity>;
};

const shell: PageHands = {
  open: (tab, url, bounds) => openPage(tab, url, bounds, true),
  load: browserAgentLoad,
  go: browserGo,
  script: browserAgent,
  capture: browserAgentCapture,
  drive: browserAgentDrive,
  events: browserAgentEvents,
};

/**
 * The user took a tab over: the agent's hold of its page goes at once, so what they do is their own. Continue gives it
 * back as it was, except that refs from before the user's turn are void.
 */
export function startHandingBack(): () => void {
  const failed = (error: unknown) => console.error("changing an agent's hold of a tab failed", error);
  return useAgentPause.subscribe(({ paused }, before) => {
    for (const tab of Object.keys(paused)) {
      if (!(tab in before.paused)) browserAgentDrive(tab, false).catch(failed);
    }
    for (const tab of Object.keys(before.paused)) {
      if (!(tab in paused) && driverOf(tab)) browserAgentDrive(tab, true, true).catch(failed);
    }
  });
}

const PROFILE_CHANGED = "profile_changed: this tab's profile was deleted, so it moved to the user's default profile. Your refs are void and what it's signed in to may differ. Nothing was done: read it again before going on, and don't assume you are still the same user on the site.";

/** The calls under way on a tab, cancelled when its profile goes. */
const underWay = new Map<string, Set<(result: BrowserResult) => void>>();
/** Tabs whose agent has yet to hear that the profile changed, which it hears once. */
const moved = new Set<string>();

/**
 * A delete moves tab `tab` to another profile, about to close its page (section 4's onProfileChanged says so). Calls under way end now, the hold goes, and
 * the agent's next call hears it once if none was under way to hear it: it mustn't carry on as someone else.
 */
export function profileChanged(tab: string): void {
  if (!driverOf(tab)) return;
  browserAgentDrive(tab, false).catch(() => {});
  const calls = underWay.get(tab);
  if (!calls?.size) {
    moved.add(tab);
    return;
  }
  for (const end of calls) end(failed(PROFILE_CHANGED));
  calls.clear();
}

/** An in-flight call the user's take-over or DevTools overtook. */
function pausedAnswer(answer: unknown, tab: string): BrowserResult | null {
  if (typeof answer !== "object" || !answer || !("paused" in answer)) return null;
  return answer.paused === "devtools" ? failed(PAUSED_DEVTOOLS) : { result: "paused", tab };
}

const PAUSED_DEVTOOLS = "Paused: DevTools open. The user has this tab; nothing was done. Calls on it answer devtools_open until they close DevTools and press Continue; `events` still works. Your refs on it are void then: read it again.";
const DEVTOOLS_OPEN = "devtools_open: DevTools is open on this tab, so nothing was done, and it stays open. `events` still works. It goes on when the user closes DevTools and presses Continue.";

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
  const tab = "tab" in call.op ? call.op.tab : null;
  if (tab !== null && moved.delete(tab)) return failed(PROFILE_CHANGED);
  let end: (result: BrowserResult) => void = () => {};
  const cancelled = new Promise<BrowserResult>((resolve) => { end = resolve; });
  if (tab !== null) underWay.set(tab, (underWay.get(tab) ?? new Set()).add(end));
  try {
    return await Promise.race([perform(call, hands), cancelled]);
  } catch (error) {
    return failed(messageOf(error));
  } finally {
    if (tab !== null) underWay.get(tab)?.delete(end);
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
  // Its agent is done: what stays open is the user's to use.
  for (const tab of current.tabs) {
    if (tab.kind === "browser" && tab.driver?.request === request && !gone.includes(tab.id)) shell.drive(tab.id, false).catch(() => {});
  }
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
      await hands.drive(tab, true);
      await loaded(tab);
    } else {
      await hands.drive(tab, true);
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
  // Worked only with a driver, which the user's input in the tab pauses: one opened just to show
  // the user (open_in_app) is read until browser_open goes on in it.
  const users = !tab.driver;
  if (users && op.op !== "read" && op.op !== "screenshot" && op.op !== "events")
    return failed(isAgentTab(tab)
      ? `Tab ${tab.id} was opened for the user to see: you may read it. To work in it, browser_open its address; that goes on in this tab.`
      : `Tab ${tab.id} is the user's: you may read it, not click, type, navigate or close it. Open your own with browser_open.`);
  if (devToolsOpen(tab.id) && op.op !== "events") return failed(DEVTOOLS_OPEN);
  // The user took it over: nothing is done until they press Continue.
  if (!users && !devToolsOpen(tab.id) && isPaused(tab.id)) return { result: "paused", tab: tab.id };
  // Its page goes with a restart: it loads again where it was. The user may take it over meanwhile.
  if (!useBrowsers.getState().pages[tab.id]) {
    await hands.open(tab.id, tab.url || "about:blank", backgroundBounds());
    await loaded(tab.id);
    if (!users && isPaused(tab.id)) return { result: "paused", tab: tab.id };
  }
  if (!users && !devToolsOpen(tab.id)) await hands.drive(tab.id, true);
  const result = await act(tab.id, op, conversationId, hands, call.label);
  return users && result.result === "page" ? { ...result, user: true } : result;
}

async function act(tab: string, op: Exclude<BrowserOp, { op: "open" | "show" }>, conversationId: string, hands: PageHands, label: string): Promise<BrowserResult> {
  switch (op.op) {
    case "events": {
      const { lines, next, dropped } = await hands.events(tab, Number(op.since), op.kinds);
      return { result: "events", tab, lines, next, dropped };
    }
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
        label,
        target: op.target,
        ...(op.op === "type" ? { text: op.text, submit: op.submit } : {}),
        declared: op.guard.declared,
        approved: approvedHere(tab, op.guard),
      });
      const overtaken = pausedAnswer(answer, tab);
      if (overtaken) return overtaken;
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
  if (devToolsOpen(tab)) return failed(PAUSED_DEVTOOLS);
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
