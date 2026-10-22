/**
 * A standalone Vite entry, excluded from the app entry and production build: the whole app
 * (App.tsx) around a project session with a long thread, over IPC answered here. Run live by
 * ShellRender.test.ts, which turns the wheel and resizes the window for it (fixtures/input.ts).
 *
 * Without `drive` it only opens a long page and a New tab beside the chat, for a look at it in
 * the real window (a dev build whose window opens this page).
 *
 * `?drive=layout` opens and closes the right panels (Files, Workers, both) with the sidebar
 * expanded and folded, and in a narrow window, measuring every titlebar control on every frame:
 * none moves, the panels open below the titlebar, the left sidebar keeps its width; and where
 * the divider before the tabs sits. `wide=1` widens the left sidebar.
 *
 * `?drive=scroll` checks that the window itself never scrolls: the titlebar and both sidebars
 * stay where they are whatever scrolls inside them (the thread, a page, the tab strip, Settings),
 * whether a wheel turns past an end or the app brings something into view, and that the panes
 * inside still scroll. It writes what it saw to `<pre id="shell-result">`.
 */
import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";

import { App } from "@/app/App";
import planned from "@/fixtures/boards/thread-plan-2026-10-10.events.json";
import { fixtureInput } from "@/fixtures/input";
import type { Conversation, EventEnvelope, Message, Page, PageContent, Project, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { applyToBoard, emptyBoard, useBoard } from "@/state/board";
import { newSessionTab, openPageTab, selectTab, useSessionTabs } from "@/state/sessionTabs";
import { fromSnapshot, useActivity } from "@/state/activity";
import { emptyThread, useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
// A left sidebar the user widened.
if (query.has("wide")) localStorage.setItem("brigadier.sidebarWidth", "420");
else localStorage.removeItem("brigadier.sidebarWidth");
document.documentElement.dataset.platform = "macos";

// The plan session of 2026-10-10 while phase 1 builds: its workers show over the composer
// (`drive=layout`), or a long thread of its own (`drive=scroll`).
const ID = planned.conversationId;
const layout = query.get("drive") === "layout";
const PAGE = "3".repeat(32);
const start = Date.now() - 86_400_000;

const paragraph = (turn: number, index: number) =>
  `Paragraph ${index + 1} of answer ${turn + 1}. The window stays put while what is inside it scrolls, and only the pane under the pointer moves.`;
const messages: Message[] = [];
for (let turn = 0; turn < 24; turn++) {
  const at = start + turn * 60_000;
  messages.push({
    id: `u${turn}`, conversationId: ID, seq: messages.length + 1, role: "user", text: `Question ${turn + 1}`, blob: null,
    createdAtMs: at, attachments: [], mentions: [], model: null, requestId: `r${turn}`, parentId: messages.at(-1)?.id ?? null,
  } as Message);
  messages.push({
    id: `a${turn}`, conversationId: ID, seq: messages.length + 1, role: "assistant",
    text: Array.from({ length: 4 }, (_, index) => paragraph(turn, index)).join("\n\n"), blob: null,
    createdAtMs: at + 30_000, attachments: [], mentions: [], model: null, requestId: `r${turn}`, parentId: messages.at(-1)?.id ?? null,
  } as Message);
}

const conversation = {
  id: ID, kind: "session", projectId: "shell-project", title: "Tidy the release notes",
  pinnedAtMs: null, createdAtMs: start, updatedAtMs: start, lifecycle: "active",
  forkedFrom: null, sideOf: null, fallback: null, quotaWait: null,
  setup: {
    type: "session", repo: "/tmp/shell-fixture",
    environment: { type: "newWorktree", branch: "brigadier/shell/session", base: "main" },
    permission: "fullAccess", planMode: false, workersSeeUncommitted: null,
    orchestrator: { provider: "claude", model: "opus", effort: "high" },
  },
} as unknown as Conversation;

let board = emptyBoard(ID);
const planMessages: Message[] = [];
for (const envelope of (planned.events as unknown as EventEnvelope[]).filter((event) => event.atMs <= 1791603900000)) {
  board = applyToBoard(board, envelope);
  if (envelope.event.type === "messageAppended") planMessages.push({ ...envelope.event.message, seq: envelope.streamSeq });
}
const items = layout ? planMessages : messages;
useBoard.setState({ board: layout ? { ...board, loaded: true, head: items.at(-1)?.id ?? null } : { ...emptyBoard(ID), loaded: true, head: items.at(-1)?.id ?? null } });
useApp.setState((state) => ({
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
  settings: { ...state.settings, onboarded: true, introSeen: true },
  projects: { "shell-project": { id: "shell-project", name: "release-notes", createdAtMs: start, repos: [{ path: "/tmp/shell-fixture", name: "release-notes" }], prefs: {}, trust: [] } as unknown as Project },
  conversations: { [ID]: conversation },
  threads: { [ID]: { ...emptyThread, items } },
  selection: { type: "conversation", id: ID },
  // The session's summary, pinned: its Workers row opens the Workers panel.
  summaryShown: { [ID]: layout },
}));
useActivity.setState({ byConversation: { [ID]: fromSnapshot({ conversationId: ID, run: "running", tasks: [], approvals: [], questions: [], phases: [] }) } });
useSessionTabs.setState({ sessions: {} });
// Opened as is (a dev window pointed at this page, for a look at the real window): a long page
// and a New tab beside the chat.
if (!query.has("drive")) {
  openPageTab(ID, PAGE, "Long page");
  newSessionTab(ID, "newTab");
  selectTab(ID, "chat");
}

const body = Array.from({ length: 80 }, (_, index) => `Line ${index + 1} of a long page, longer than the window.`).join("\n\n");
const page: Page = { id: PAGE, content: { title: "Long page", emoji: null, body } as PageContent, revision: "r1", createdMs: 0, updatedMs: start } as Page;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const request = (payload as { request: Request }).request;
  switch (request.method) {
    case "pageRead": return { method: "pageRead", page };
    case "pageList": return { method: "pageList", pages: [{ id: PAGE, title: "Long page", emoji: null, updatedMs: start }] };
    case "listMessages": return { method: "listMessages", page: { items: [], hasMore: false } };
    // The board as replayed here stays: the daemon's own view of it never comes.
    case "getConversation": return layout ? new Promise(() => {}) : { method: request.method };
    case "listFiles": return { method: "listFiles", files: Array.from({ length: 60 }, (_, index) => `src/file-${index}.ts`), truncated: false };
    default: return { method: request.method };
  }
}, { shouldMockEvents: true });

mockWindows("main");

const wait = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));
const frame = () => new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));
const until = async (check: () => boolean, ms = 5000) => {
  for (const end = Date.now() + ms; Date.now() < end && !check();) await wait(25);
  return check();
};
const one = <T extends Element = HTMLElement>(selector: string) => document.querySelector<T>(selector);
const rect = (element: Element | null) => {
  const box = element?.getBoundingClientRect();
  return box ? { x: Math.round(box.x), y: Math.round(box.y), w: Math.round(box.width), h: Math.round(box.height) } : null;
};
const centre = (element: Element) => {
  const box = element.getBoundingClientRect();
  return { x: Math.round(box.x + box.width / 2), y: Math.round(box.y + box.height / 2) };
};

/** Whether an element scrolls by design: its own overflow lets it. */
const scroller = (element: Element) => {
  const style = getComputedStyle(element);
  return /auto|scroll/.test(style.overflowX) || /auto|scroll/.test(style.overflowY);
};
const describe = (element: Element) => {
  const slot = element.getAttribute("data-slot");
  return `${element.tagName.toLowerCase()}${element.id ? `#${element.id}` : ""}${slot ? `[${slot}]` : ""}.${String(element.getAttribute("class") ?? "").split(" ").slice(0, 4).join(".")}`;
};

/** What moved that shouldn't: the window's own scroll, and any element scrolled that doesn't scroll by design. */
function shellMoved(): string[] {
  const moved: string[] = [];
  if (window.scrollX || window.scrollY) moved.push(`window ${window.scrollX},${window.scrollY}`);
  const root = document.scrollingElement;
  if (root && (root.scrollTop || root.scrollLeft)) moved.push(`scrollingElement ${root.scrollLeft},${root.scrollTop}`);
  for (const element of document.querySelectorAll("*")) {
    if ((element.scrollTop || element.scrollLeft) && !scroller(element)) moved.push(`${describe(element)} ${element.scrollLeft},${element.scrollTop}`);
  }
  return moved;
}

/** Where the window's fixed parts are: the titlebar's controls and both sidebars. */
function fixedParts() {
  return {
    nav: rect(one('[data-slot="titlebar-nav"]')),
    header: rect(one('[data-slot="session-tabs"]')),
    sidebar: rect(one('[data-slot="sidebar-panel"]')),
    workspace: rect(one('[data-slot="workspace-pane"]')),
    root: rect(one("#root")),
  };
}

async function wheel(element: Element | null, deltaY: number, times = 4, deltaX = 0) {
  if (!element) return;
  const { x, y } = centre(element);
  for (let index = 0; index < times; index++) {
    await fixtureInput({ type: "wheel", x, y, deltaY, deltaX });
    await wait(60);
  }
  await wait(250);
}

/** Every ancestor of `element` up to the document, asked to scroll as a script can. */
function pushAncestors(element: Element | null) {
  for (let node = element?.parentElement; node; node = node.parentElement) {
    if (scroller(node)) continue;
    node.scrollTop = 400;
    node.scrollLeft = 400;
  }
  window.scrollTo(400, 400);
  document.scrollingElement?.scrollTo(400, 400);
}

async function driveScroll(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  const viewport = () => one('[data-slot="aui_thread-viewport"]');
  await until(() => Boolean(viewport()?.querySelector('[data-message-id], [data-role="assistant"], p')));
  await wait(500);
  const before = fixedParts();
  seen.before = before;
  // Every box up to the window clips, and the root holds what is positioned against no box of its own.
  seen.rootStyles = ["html", "body", "#root"].map((selector) => {
    const style = getComputedStyle(one(selector)!);
    return `${selector} ${style.overflowX}/${style.overflowY} ${style.position}`;
  });
  seen.documentFits = document.documentElement.scrollHeight <= innerHeight && document.documentElement.scrollWidth <= innerWidth;
  const check = async (name: string) => {
    await frame();
    seen[name] = { moved: shellMoved(), still: JSON.stringify(fixedParts()) === JSON.stringify(before) };
  };

  // The thread, scrolled past both its ends by the wheel: it moves, the window doesn't.
  const thread = viewport()!;
  const threadBottom = thread.scrollTop;
  await wheel(thread, -900, 6);
  seen.threadScrolled = thread.scrollTop < threadBottom;
  await wheel(thread, -2000, 30);
  await check("threadTop");
  await wheel(thread, 2000, 30);
  await check("threadBottom");
  // Over the titlebar and the sidebar, both ways.
  await wheel(one('[data-slot="session-tabs"]'), 600, 4);
  await wheel(one('[data-slot="session-tabs"]'), -600, 4);
  await wheel(one('[data-slot="sidebar-panel"]'), 600, 4);
  await wheel(one('[data-slot="sidebar-panel"]'), -600, 4, 300);
  await check("chrome");
  // The app bringing things into view: the thread's first message, its last.
  thread.querySelector("p")?.scrollIntoView({ block: "start" });
  [...thread.querySelectorAll("p")].at(-1)?.scrollIntoView({ block: "end", inline: "end" });
  await check("threadIntoView");

  // Tabs enough to scroll the tab strip, the front one brought into view each time.
  for (let index = 0; index < 10; index++) newSessionTab(ID, index % 2 ? "terminal" : "newTab");
  await wait(600);
  const tabs = useSessionTabs.getState().sessions[ID]!.tabs;
  selectTab(ID, tabs[0]!.id);
  await wait(300);
  selectTab(ID, tabs.at(-1)!.id);
  await wait(300);
  seen.stripScrolls = (one('[data-slot="session-tabs"] .overflow-x-auto')?.scrollLeft ?? 0) > 0;
  await check("tabs");

  // A long page: scrolled with the wheel past its ends, and its last line brought into view.
  openPageTab(ID, PAGE, "Long page");
  const pageScroll = () => one(".page-scroll");
  await until(() => (pageScroll()?.scrollHeight ?? 0) > (pageScroll()?.clientHeight ?? 0) + 200);
  await wait(300);
  const scroll = pageScroll()!;
  seen.pageBelowTitlebar = rect(scroll)!.y >= rect(one('[data-slot="session-tabs"]'))!.y + rect(one('[data-slot="session-tabs"]'))!.h;
  await wheel(scroll, 2000, 20);
  seen.pageScrolled = scroll.scrollTop > 0;
  await check("pageBottom");
  await wheel(scroll, -2000, 20);
  await check("pageTop");
  [...scroll.querySelectorAll("p")].at(-1)?.scrollIntoView({ block: "end" });
  await check("pageIntoView");

  // Scripts asking every container of the page and the thread to scroll: none that don't by design can.
  pushAncestors(scroll);
  selectTab(ID, "chat");
  await wait(300);
  pushAncestors(viewport()?.querySelector("p") ?? null);
  await check("pushed");

  // Settings: its page scrolls under the titlebar's bottom edge, never past it.
  select({ type: "settings", page: "general" });
  const settings = () => one('[data-slot="settings-page"] .overflow-y-auto');
  await until(() => Boolean(settings()), 8000);
  await wait(500);
  const titlebarBottom = rect(one('[data-slot="titlebar-nav"]'))!;
  const page2 = settings()!;
  seen.settingsTop = { scroller: rect(page2)!.y, titlebar: Math.round(document.documentElement.getBoundingClientRect().y) + titlebarHeight() };
  await wheel(page2, 2000, 20);
  seen.settingsScrolled = page2.scrollTop > 0;
  seen.settingsNav = rect(one('[data-slot="titlebar-nav"]'));
  seen.settingsNavStill = JSON.stringify(seen.settingsNav) === JSON.stringify(titlebarBottom);
  seen.settingsMoved = shellMoved();
  pushAncestors(page2);
  await frame();
  seen.settingsPushed = shellMoved();
  return seen;
}

/** Every control in the titlebar row, by name, at its x: where it starts and ends. */
function titlebar(): Record<string, [number, number]> {
  const header = one('[data-slot="session-tabs"]')!;
  const controls = [...document.querySelectorAll<HTMLElement>('[data-slot="titlebar-nav"] button'), ...header.querySelectorAll<HTMLElement>("button")]
    .filter((button) => !button.closest("[data-session-tab]"));
  const placed: Record<string, [number, number]> = {};
  for (const control of controls) {
    const box = control.getBoundingClientRect();
    if (!box.width) continue;
    placed[control.getAttribute("aria-label") ?? control.textContent?.trim() ?? "?"] = [Math.round(box.left), Math.round(box.right)];
  }
  placed.divider = [divider(), divider()];
  return placed;
}

/** Where the divider before the tabs is drawn. */
function divider() {
  const header = one('[data-slot="session-tabs"]')!;
  return Math.round(header.getBoundingClientRect().left + Number.parseFloat(getComputedStyle(header, "::before").left));
}

const stateOf = (slot: string) => one(`[data-slot="${slot}"]`)?.getAttribute("data-state");

/** Runs `act`, then reads every frame for `ms`, and on until the panel `slot` names has opened or
 * closed (for at most fifteen seconds more) and come to rest (five), as a busy machine can be that
 * slow: whether a titlebar control moved, the window scrolled, or the left sidebar's width changed. */
async function watch(act: () => void, ms = 600, slot?: string) {
  const titles = JSON.stringify(titlebar());
  const left = rect(one('[data-slot="sidebar-panel"]'))!.w;
  const moved = new Set<string>();
  const before = slot && stateOf(slot);
  const box = () => JSON.stringify(slot && rect(one(`[data-slot="${slot}"]`)));
  act();
  const end = performance.now() + ms;
  let [last, since, still] = [box(), performance.now(), 0];
  // When the panel's state changed: its five seconds to come to rest count from then.
  let changed = slot && stateOf(slot) !== before ? performance.now() : null;
  const busy = () => {
    if (!slot) return false;
    if (changed === null) return performance.now() < end + 15000;
    return (performance.now() - since < 250 || still < 5) && performance.now() < Math.max(end, changed) + 5000;
  };
  while (performance.now() < end || busy()) {
    await new Promise((resolve) => requestAnimationFrame(resolve));
    if (changed === null && slot && stateOf(slot) !== before) changed = performance.now();
    if (box() === last) still++;
    else [last, since, still] = [box(), performance.now(), 0];
    const now = titlebar();
    for (const [name, at] of Object.entries(now)) if (JSON.stringify(at) !== JSON.stringify(JSON.parse(titles)[name])) moved.add(`${name} ${JSON.stringify(JSON.parse(titles)[name])}→${JSON.stringify(at)}`);
    for (const name of Object.keys(JSON.parse(titles))) if (!(name in now)) moved.add(`${name} gone`);
    for (const line of shellMoved()) moved.add(line);
    const width = rect(one('[data-slot="sidebar-panel"]'))!.w;
    if (width !== left) moved.add(`left sidebar ${left}→${width}`);
  }
  return [...moved];
}

const headerBottom = () => {
  const header = rect(one('[data-slot="session-tabs"]'))!;
  return header.y + header.h;
};
const click = (selector: string) => one<HTMLButtonElement>(selector)?.click();
const toggle = '[data-slot="session-tabs"] button[aria-label="Right sidebar"]';

/** Where the titlebar's controls are and how wide the left sidebar is. */
const shape = () => JSON.stringify([titlebar(), rect(one('[data-slot="sidebar-panel"]'))?.w]);

/** Waits until no titlebar control or sidebar width has changed for a quarter second and five
 * frames, so a fold still easing in on a slow machine isn't counted as moved by what follows. */
async function settle(ms = 4000) {
  let [last, since, still] = [shape(), performance.now(), 0];
  for (const end = performance.now() + ms; performance.now() < end && (performance.now() - since < 250 || still < 5);) {
    await frame();
    const now = shape();
    if (now === last) still++;
    else [last, since, still] = [now, performance.now(), 0];
  }
}

/** The right panels opened and closed, one and then both: what moved, and where they opened. */
async function panels() {
  const seen: Record<string, unknown> = {};
  const header = one('[data-slot="session-tabs"]')!;
  const last = [...header.querySelectorAll("button")].at(-1)!;
  seen.toggleLast = last.getAttribute("aria-label");
  seen.toggleToEdge = Math.round(innerWidth - last.getBoundingClientRect().right);
  seen.filesOpening = await watch(() => click(toggle), 600, "workspace-pane");
  const pane = rect(one('[data-slot="workspace-pane"]'))!;
  seen.filesOpen = { top: pane.y - headerBottom(), width: pane.w, row: one('[data-slot="workspace-pane"] [role="tablist"]')?.textContent !== undefined && rect(one('[data-slot="workspace-pane"] [role="tablist"]'))!.y - headerBottom() };
  // The session's context, where its Workers row (or a worker's line) opens the right panel.
  if (!one(WORKERS_ROW)) {
    click('[data-slot="session-tabs"] button[aria-label="Context"]');
    await until(() => Boolean(one(WORKERS_ROW)));
    await wait(400);
  }
  seen.workersOpening = await watch(() => click(WORKERS_ROW), 600, "right-sidebar");
  const workers = rect(one('[data-slot="right-sidebar"]'))!;
  seen.workersOpen = { top: workers.y - headerBottom(), width: workers.w, state: one('[data-slot="right-sidebar"]')?.getAttribute("data-state"),
    files: one('[data-slot="workspace-pane"]')?.getAttribute("data-state"), row: rect(one('[data-slot="right-sidebar"] [role="tablist"], [data-slot="right-sidebar"] [data-slot="worker-header"]'))!.y - headerBottom() };
  seen.filesClosing = await watch(() => click(toggle), 600, "workspace-pane");
  seen.filesAgain = await watch(() => click(toggle), 600, "workspace-pane");
  seen.workersClosing = await watch(() => click('[data-slot="right-sidebar"] button[aria-label="Close right panel"]'), 600, "right-sidebar");
  seen.filesClosingLast = await watch(() => one('[data-slot="workspace-pane"]')?.getAttribute("data-state") === "expanded" && click(toggle));
  return seen;
}

/** The context card's Workers row, or, while a few work, a worker's line. */
const WORKERS_ROW = '[data-slot="workers-summary"], [data-slot="workers-summary-worker"]';

/** The divider before the tabs and what is beside it, the sidebar expanded or folded. */
function tabsStart() {
  const sidebar = rect(one('[data-slot="sidebar-panel"]'))!;
  const nav = [...document.querySelectorAll<HTMLElement>('[data-slot="titlebar-nav"] button')].map((button) => button.getAttribute("aria-label"));
  const sidebarToggle = rect(one('[data-slot="sidebar-toggle"]'))!;
  const strip = [...document.querySelectorAll<HTMLElement>('[data-slot="sidebar-panel"] button[aria-label]')]
    .filter((button) => !button.closest("[inert]"));
  const firstTab = rect(one('[data-slot="session-tabs"] [data-tab-id="chat"]'))!;
  const rule = getComputedStyle(one('[data-slot="session-tabs"]')!, "::before");
  return {
    nav, divider: divider(), sidebarEnd: sidebar.x + sidebar.w, toggleEnd: sidebarToggle.x + sidebarToggle.w,
    strip: strip.map((button) => button.getAttribute("aria-label")), firstTab: firstTab.x,
    rule: { width: Number.parseFloat(rule.width), height: Math.round(Number.parseFloat(rule.height)), shown: rule.content !== "none" && rule.display !== "none" },
  };
}

async function driveLayout(): Promise<Record<string, unknown>> {
  const seen: Record<string, unknown> = {};
  await until(() => Boolean(one('[data-slot="session-tabs"]')), 8000);
  await wait(800);
  await settle();
  const tab = one('[data-slot="session-tabs"] [data-tab-id="chat"] [role="tab"]')!;
  const spinner = tab.parentElement!.querySelector<HTMLElement>('[data-slot="status-spinner"]');
  const title = tab.querySelector(".session-tab-title")?.textContent ?? "";
  seen.sessionStatus = { words: tab.textContent?.replace(title, "").trim(), tip: tab.title, spinner: !!spinner, width: spinner?.parentElement?.getBoundingClientRect().width ?? 0 };
  seen.projectHeader = one('[data-slot="sidebar-panel"] li > div > button[aria-expanded]')?.textContent?.trim();
  seen.expandedStart = tabsStart();
  seen.expanded = await panels();
  // The sidebar folded: New chat lives in the strip; the divider follows the sidebar toggle.
  seen.folding = await watch(() => click('[data-slot="sidebar-toggle"]'), 700)
    // Only the tabs move over: the toolbar at the end stays, and the window never scrolls.
    .then((moved) => moved.filter((line) => /^(Context|Terminal|Right sidebar) |^(window|scrollingElement) /.test(line)));
  await settle();
  seen.foldedStart = tabsStart();
  seen.folded = await panels();
  click('[data-slot="sidebar-toggle"]');
  await wait(700);
  // A narrow window.
  await fixtureInput({ type: "viewport", width: 1000, height: 760 });
  await wait(800);
  await settle();
  seen.narrowStart = tabsStart();
  seen.narrow = await panels();
  return seen;
}

function titlebarHeight() {
  return Math.round(Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--spacing-titlebar")) * 16);
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    const drive = query.get("drive") === "scroll" ? driveScroll : query.get("drive") === "layout" ? driveLayout : null;
    if (!drive) return;
    void drive()
      .then((seen) => setResult(JSON.stringify(seen)))
      .catch((error: unknown) => setResult(JSON.stringify({ error: String(error instanceof Error ? error.stack : error) })));
  }, []);
  return (
    <>
      <App />
      {result && <pre id="shell-result" hidden>{result}</pre>}
    </>
  );
}

createRoot(document.getElementById("root")!).render(<Fixture />);
