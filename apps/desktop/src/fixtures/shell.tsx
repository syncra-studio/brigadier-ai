/**
 * A standalone Vite entry, excluded from the app entry and production build: the whole app
 * (App.tsx) around a project session with a long thread, over IPC answered here. Run live by
 * ShellRender.test.ts, which turns the wheel and resizes the window for it (fixtures/input.ts).
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
import { fixtureInput } from "@/fixtures/input";
import type { Conversation, Message, Page, PageContent, Project, Request } from "@/ipc/generated";
import { select } from "@/state/actions";
import { emptyBoard, useBoard } from "@/state/board";
import { newSessionTab, openPageTab, selectTab, useSessionTabs } from "@/state/sessionTabs";
import { emptyThread, useApp } from "@/state/store";

const query = new URLSearchParams(location.search);
if (query.get("density") === "compact") document.documentElement.dataset.density = "compact";
document.documentElement.dataset.platform = "macos";

const ID = "shell-session";
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

useBoard.setState({ board: { ...emptyBoard(ID), loaded: true, head: messages.at(-1)?.id ?? null } });
useApp.setState((state) => ({
  info: { platform: "macos", version: "fixture", processStartMs: 0, smoke: false, firstLaunch: false, budgetTolerance: 1 },
  connection: { status: "connected", daemon: null, reason: null },
  settings: { ...state.settings, onboarded: true, introSeen: true },
  projects: { "shell-project": { id: "shell-project", name: "release-notes", createdAtMs: start, repos: [{ path: "/tmp/shell-fixture", name: "release-notes" }], prefs: {}, trust: [] } as unknown as Project },
  conversations: { [ID]: conversation },
  threads: { [ID]: { ...emptyThread, items: messages } },
  selection: { type: "conversation", id: ID },
}));
useSessionTabs.setState({ sessions: {} });

const body = Array.from({ length: 80 }, (_, index) => `Line ${index + 1} of a long page, longer than the window.`).join("\n\n");
const page: Page = { id: PAGE, content: { title: "Long page", emoji: null, body } as PageContent, revision: "r1", createdMs: 0, updatedMs: start } as Page;

mockIPC((command, payload) => {
  if (command !== "ipc_request") return null;
  const request = (payload as { request: Request }).request;
  switch (request.method) {
    case "pageRead": return { method: "pageRead", page };
    case "pageList": return { method: "pageList", pages: [{ id: PAGE, title: "Long page", emoji: null, updatedMs: start }] };
    case "listMessages": return { method: "listMessages", page: { items: [], hasMore: false } };
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

function titlebarHeight() {
  return Math.round(Number.parseFloat(getComputedStyle(document.documentElement).getPropertyValue("--spacing-titlebar")) * 16);
}

function Fixture() {
  const [result, setResult] = useState("");
  useEffect(() => {
    if (query.get("drive") !== "scroll") return;
    void driveScroll()
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
